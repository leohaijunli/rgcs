# MagGCS (rgcs-main) Issue 清单

来源：对 rgcs-main 的静态代码评审（未编译、未运行）。
标注"待核实"的条目表示我没有用抓包/源码确认，请先复现再修。

优先级：**P0** 飞行安全或功能不可用 · **P1** 数据正确性/互操作 · **P2** 架构与工程质量 · **P3** 打磨

建议里程碑：M0 = Phase 1 开始前必须完成（P0 全部）；M1 = Phase 1 内完成（P1）；M2 = Phase 2 前（P2）。

状态标记：✅ 已完成 · 🔄 进行中 · ⬜ 未开始。每条状态行以实际提交/验证为准，更新状态时同步改这一行。

### 进度总览（P0）

| # | 主题 | 状态 |
| --- | --- | --- |
| #1 | 心跳超时被任意流量重置 | ✅ 已完成（`f3147fd`） |
| #2 | `Connected` 事件丢失 | ✅ 已完成（`f3147fd`） |
| #3 | GCS 不发 HEARTBEAT | ✅ 已完成（`f3147fd`） |
| #4 | `connect` 绑定失败仍返回成功 | ✅ 已完成（`f3147fd`） |
| #5 | 命令 fire-and-forget，无 COMMAND_ACK | ✅ 已完成（core `d5ec93f`，app/frontend 后续提交） |
| #6 | 遥测聚合无来源过滤 | ✅ 已完成（核心过滤 + 单测） |
| #7 | 任务协议重传定时器未接线 | ✅ 已完成（`on_tick` + 服务 tick + 总超时 + 单测） |
| #8 | 上传时重复 MISSION_REQUEST 被当作失败 | ✅ 已完成（重复请求重发 + 单测） |
| #9 | 下载未过滤发往其他 GCS 的 item | ✅ 已完成（协议单源 + 来源过滤 + 单测） |

---

## P0 · 连接层与命令通道

### #1 [P0][core] Heartbeat timeout is reset by any inbound traffic
**状态**：✅ 已完成（`f3147fd`）
- 位置：`crates/core/src/mavlink/connection.rs`，`run_worker` 的 `select!`
- 现象：`tokio::time::sleep(heartbeat_timeout)` 在每次循环里重新创建，只要有任何帧到达该分支就不会触发，FC 心跳停止后 `HeartbeatLost` 不会发出（除非链路完全静默）。
- 修复：用循环外创建的 `tokio::time::interval`（≤500 ms）检查 `HeartbeatMonitor::is_alive`。
- 验收：单测持续注入非目标帧，FC 心跳停止后 ≤ timeout + 500 ms 触发 `HeartbeatLost`。
- 关联：计划 0.2。

### #2 [P0][core/app] Connected event lost: hub subscribes after the receiver is dropped
**状态**：✅ 已完成（`f3147fd`）
- 位置：`app-tauri/src/commands.rs::connect`（`let (handle, _rx) = ...`）、`core/src/telemetry/hub.rs::spawn`
- 现象：`spawn_connection` 返回的接收器被丢弃，hub 之后才 `subscribe()`，`Connected` 事件可能已发出，link 状态停在 `Disconnected`。
- 修复：把接收器传入 `TelemetryHub::spawn`，或在 spawn 前先订阅；`Connected` 状态同时写入可查询的共享状态。
- 验收：绑定后 100 ms 内 hub 收到 `Connected`；无包时 UI 显示"监听中"而非"未连接"。
- 关联：计划 0.1。

### #3 [P0][core] GCS never sends HEARTBEAT
**状态**：✅ 已完成（`f3147fd`）
- 位置：全仓库无 `MAV_TYPE_GCS` 心跳发送（仅测试里有）
- 影响：PX4 的数据链路丢失判断、mavlink 路由、`udpin` 模式下命令回复地址都依赖对方先收到我们的包。
- 修复：连接成功后以 1 Hz 发送 `HEARTBEAT`（`MAV_TYPE_GCS`，sysid 250，ID 对照 PX4 文档确认）。
- 验收：SITL 抓包看到 1 Hz 心跳；`udpout:` 端点也能收到 FC 数据。
- 关联：计划 0.3。

### #4 [P0][app] `connect` returns success even when binding fails
**状态**：✅ 已完成（`f3147fd`）
- 位置：`commands.rs::connect`、`connection.rs::spawn_connection`、`frontend/src/desktop/bridge.ts`（`.catch(() => undefined)`）
- 现象：真正的 `connect_async` 在后台任务里执行，失败只变成 `LinkError` 事件；前端自动连接还吞掉异常。
- 修复：`connect` 等待首次绑定成功/失败再返回，错误原样透传；自动连接失败要在状态条可见。
- 验收：占用 14550 后点击连接，≤1 s 内界面显示 "Address in use"。
- 关联：计划 0.4。

### #5 [P0][core/app/frontend] Flight commands are fire-and-forget (no COMMAND_ACK)
**状态**：✅ 已完成（core `d5ec93f`；app-tauri 适配器与前端见后续提交）
- 位置：`commands.rs::send_command`、`frontend/src/components/FlightCommands.tsx`
- 现象：只发一个 `COMMAND_LONG`，不等 ACK、不重试，前端 `catch { /* best-effort */ }`。RTL 丢包时用户无任何反馈。
- 修复：在 `core` 实现 `CommandSession`：发送 → 等 `COMMAND_ACK` → 超时重试（`confirmation` 递增）→ 结果上报到 UI；RTL 加二次确认（或长按）；失败要明确显示。
- 额外：待核实 `DO_PAUSE_CONTINUE` 在 PX4 v1.17 的实际行为，不确定则先不在 UI 暴露 pause/resume。
- 验收：模拟丢 ACK / 丢命令 / 被拒（`MAV_RESULT_DENIED`）三种情况，UI 分别给出明确结果。

### #6 [P0][core] Telemetry aggregation has no source filtering
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`core/src/telemetry/mod.rs::TelemetryUpdate::try_from_envelope`、`TelemetryAggregator::apply`
- 现象：不检查 `system_id/component_id`。链路上有 QGC 心跳、相机/gimbal/伴飞计算机心跳时，`heartbeat` 字段被最后到达的覆盖；多机时位置/姿态混写。
- 修复：按目标 `(sysid, compid)` 过滤；预留 `VehicleId` 维度。
- 验收：混入其他节点的 HEARTBEAT/ATTITUDE，快照仍只反映目标 FC。
- 实现：新增 `telemetry::VehicleId`（`system_id`/`component_id`，含 ts-rs 导出）；`TelemetryAggregator` 增加 `target: Option<VehicleId>` 与 `for_vehicle(target)`，`apply(source, update, now_ms)` 丢弃非目标来源；`TelemetryHub::spawn(events, hz, target)` 由 `app-tauri::commands::connect` 用 `config.target_system_id/target_component_id` 接线。`new()` 保留"全接收"语义供单测使用。
- 验证：新增 `aggregator_rejects_foreign_sources`、`hub_drops_foreign_node_telemetry`，并更新既有 `aggregator_folds_updates`。`cargo test -p maggcs-core --lib` 81 passed；`cargo clippy --all-targets -- -D warnings` 通过。前端 `npm run typecheck/build` 通过（新增 `VehicleId.ts` 绑定已同步）。
- 关联：handoff 中"FC 灯红但界面仍显示 connected / 模式显示 Acro"指向同一根因（外来心跳覆盖 `snapshot.heartbeat`）。修好后仍需在 SITL/app 构建上复验。

### #7 [P0][core] Mission protocol retransmission timer is never wired
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`app-tauri/src/mission_service.rs::run`；`core/src/mission/protocol.rs::retransmit_due / take_timeout_failure`
- 现象：服务循环里没有定时器调用这两个函数（仅单测调用）。丢一个包后上传/下载永久挂起，前端 `busy` 永不复位。
- 修复：在服务循环加 tick（如 100–200 ms），发送到期帧；超限时发出 `Failed(RetriesExhausted)` 并回到 Idle；同时给整个操作加总超时。
- 验收：10% 随机丢包下 100 航点上传成功（见 #25 测试夹具）；彻底断链时 UI 在有限时间内回到可操作状态并提示失败。
- 实现：`MissionProtocol::on_tick(now)` 驱动 `retransmit_due`/`take_timeout_failure`，并在超过 `OPERATION_TIMEOUT`（30 s）时 `abort()` 并发 `Failed(Timeout)`；`RETRY_TICK = 100 ms`。`mission_service::run` 的 `select!` 增加 tick 分支并发送到期帧。完成/失败回到 Idle，`on_tick` 在 Idle 直接返回，避免完成后误重传。
- 验证：新增 `on_tick_retransmits_then_exhausts`、`on_tick_aborts_past_operation_timeout`；`cargo test -p maggcs-core --lib mission` 21 passed。
- 待办：10% 丢包 100 航点的整链路验收依赖 #25 的假 FC 夹具（尚未做）。

### #8 [P0][core] Upload: duplicate MISSION_REQUEST treated as failure
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`protocol.rs::on_upload_request`，测试 `upload_seq_mismatch_fails`
- 现象：`req_seq != next_seq` 直接 `SeqMismatch` 失败。FC 没收到 item 时会重复请求同一 seq，GCS 应重发。
- 修复：`req_seq == next_seq - 1` 重发上一项；`== next_seq` 发新项；其他才报错。修改并反转该测试。
- 验收：丢掉任意一个 `MISSION_ITEM_INT` 后上传仍能完成。
- 实现：`on_upload_request` 先处理 `req_seq == next_seq - 1`（重发上一项，不推进 `next_seq`，不发 Failed），再校验 `req_seq == next_seq`，否则才 `SeqMismatch`。附带：下载时收到刚接受项的重复 `MISSION_ITEM_INT` 直接忽略。
- 验证：新增 `upload_duplicate_request_is_resent`、`upload_duplicate_of_last_item_is_resent`、`download_duplicate_item_ignored`；`upload_seq_mismatch_fails` 仍覆盖真正越序。

### #9 [P0][core/app] Download accepts MISSION_ITEM_INT addressed to other GCSs
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`protocol.rs::target_of`（未覆盖 `MISSION_ITEM_INT`、`MISSION_CURRENT`）；`mission_service.rs`（对所有 `MISSION_ITEM_INT` 写入 HashMap，且不看来源）
- 影响：QGC 同时下载任务时，MagGCS 会混入 FC 发给 QGC 的 item。
- 修复：校验发送方是目标 FC、且（若消息带目标字段）目标是本机；`MISSION_ITEM_INT` 本身没有 target 字段的情况下，改为只在自己处于 Download 且 seq 与期望一致时接收；去掉 service 里的第二份 items 副本，以协议状态机为唯一来源。
- 验收：测试夹具中并行注入发往另一 GCS 的任务流，结果不被污染。（待核实：PX4 对多 GCS 同时读取任务的实际行为。）
- 实现：
  - `MissionProtocol::handle` 对 `MISSION_ITEM_INT`/`MISSION_CURRENT`（无可用的目标字段）要求来源头等于目标 FC，其他来源直接丢弃。
  - 下载项改为协议自身持有：新增 `last_downloaded` 字段与 `take_downloaded()`，仅在 Download 状态且 `seq == next_seq` 时写入（`on_download_item`），完成后由 `app-tauri::mission_service` 调用 `take_downloaded()` 发 `mission_plan`。
  - `mission_service.rs` 删除独立的 `HashMap` 副本与 `mission_item_from_mav` 依赖，协议成为唯一来源。
- 验证：新增 `download_ignores_items_from_foreign_source`、`mission_current_from_foreign_source_ignored`、`download_exposes_items_in_seq_order`；`download_round_trip`/`set_current_round_trip` 改用真实 FC 头。`cargo test -p maggcs-core --lib` 84 passed；`cargo clippy --all-targets -- -D warnings` 通过。
- 关联：handoff 报告"Plan 页航点像测绘/标称数据、与实际航线无关"——根因是 service 里不看来源的副本被 QGC 并发下载污染。

---

## P1 · 数据正确性与互操作

### #10 [P1][core] `MissionItem` model is redundant and lossy
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`core/src/mission/types.rs`、`protocol.rs::mission_item_from_mav`
- 现象：`params` 含 7 个值同时又有 `x/y/z`；下载时 `params[4] = x as f32`（1e7 缩放整数转 f32，丢精度，误差可达分米级）；上传时忽略 `params[4..7]`。
- 修复：`params` 只保留 p1–p4，坐标仅用 `x/y/z`；同步更新前端、`planfile.ts`、ts-rs 绑定。
- 验收：对随机经纬度做上传/下载往返，x/y 逐位相等。
- 实现：`MissionItem::params` 只保留 P1–P4；`mission_item_to_mav` 只复制 `params[0..4]`，`mission_item_from_mav` 不再把 `x/y/z` 塞回 `params[4..7]`。前端 `stores/mission.ts` 与 `planfile.ts` 同步（QGC 导出仍在 `params[4..6]` 写坐标以保持兼容）。
- 验证：新增 `coordinates_round_trip_bit_exact`（多组含极端经纬度，x/y 逐位相等），更新 `encode_decode_round_trip`；`cargo test -p maggcs-core --lib` 92 passed。

### #11 [P1][core] `MissionFrame` does not round-trip
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`types.rs::to_mav / from_mav`
- 现象：`Global`→`GLOBAL_INT`、`GlobalTerrainAlt`→`..._INT`，下载回来变成 INT 变体，"逐项一致"必然不成立。
- 修复：只保留 INT 变体（删除冗余非 INT 变体），对未知帧返回错误而不是静默 `_ => Global`。
- 验收：所有支持的 frame 往返相等；未知 frame 报错。
- 实现：删除冗余的非 INT 变体，只保留 `global_int`/`global_relative_alt_int`/`global_terrain_alt_int`/local 系列；`from_mav` 把非 INT 拼写归一化到 INT，未知帧返回新增的 `MissionError::UnsupportedFrame`（不再静默 `=> Global`）；`mission_item_from_mav` 改为 `Result`，下载遇到不支持帧时终止并发 `Failed`。前端 `planfile.ts` 的帧映射同步。
- 验证：新增 `frame_round_trips_and_unknown_errors`、`download_fails_on_unsupported_frame`；`MissionFrame.ts` 绑定已重生成并同步。

### #12 [P1][frontend] `.plan` import crashes on ComplexItem
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`frontend/src/mission/planfile.ts::qgcToItem`
- 现象：直接解构 `raw.coordinate`，QGC 的 Survey/Corridor Scan 等 `ComplexItem` 没有该字段，抛 TypeError。
- 修复：按 `type` 分支；ComplexItem 要么展开其子项，要么明确提示"不支持"并列出位置，不允许静默丢弃。
- 验收：用包含 Survey 复杂项的真实 `.plan` 样本（放入 `testdata/`）导入不崩溃且有明确提示。
- 实现：`qgcToItem` 拆成面向 `coordinate` 的 `simpleToItem`（缺坐标返回 `null`）；`parsePlan` 处理 `ComplexItem`：有 `simpleItems` 则展开其子项，否则计入 `unsupported`（标签如 `Survey #1`）并在 UI 提示。`importPlanFile` 返回 `PlanImport{items,mode,home,base,unsupported}`。
- 验证：新增 `testdata/qgc-survey.plan`（SimpleItem + 带 `simpleItems` 的 Survey），用 esbuild+Node 跑 `parsePlan`/`buildPlan`——3 航点、无 unsupported；去掉子项后报 `Survey #1` 且不崩溃。
- 升级（improve_plan finding 3）：Survey 不再被压平成普通航点，改为不透明 `PlanBlock`（`raw` 原样写回、子项只读、合成 seq 防冲突）；`orderedMissionItems` 按 `base.mission.items` 还原飞行顺序；上传/地图/导出都用该顺序。`npm run check:planfile` 断言 Survey 往返字节一致、`doJumpId` 不变。

### #13 [P1][frontend] `.plan` export is not faithful
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`planfile.ts::buildPlan / itemToQgc`
- 问题：`plannedHomePosition` 用第一个航点代替；`doJumpId` 恒为 0（QGC 从 1 递增）；`vehicleType/firmwareType/cruiseSpeed/hoverSpeed` 写死；导入时丢弃 `geoFence/rallyPoints/mission` 其他字段。
- 修复：保留导入文件的未知字段并在导出时写回（passthrough）；Home 位置单独建模；`doJumpId` 递增。
- 验收：`.plan` 导入→导出→再导入，结构化比较无差异（忽略格式化）；用 QGC 实际打开验证（待核实）。
- 实现：导入时保存整份 `base` 文档，导出时 `structuredClone` 后写回未知字段（`geoFence`/`rallyPoints`/`cruiseSpeed`/`firmwareType`/`vehicleType`…）；`plannedHomePosition` 单独建模（`home`）；`doJumpId` 从 1 递增。前端 store 保存 `planBase`/`home` 供导出复用。
- 验证：Node 检查确认 geoFence/rallyPoints/cruiseSpeed/firmwareType/home 透传，`doJumpId=[1,2,3]`，导入→导出→再导入 items/home 完全一致。（QGC 实机打开仍待人工核实。）
- 升级（improve_plan findings 3/4/5）：复杂项原样保留后续导出；`null` 参数在导入时归零（Rust `f32` 不再拒绝真实 QGC 文件）；同步哈希去掉 `seq`/`current`，避免下载回读误报不一致。

### #14 [P1][frontend] Add-waypoint button inserts at ~(0°, 0°) when nothing is selected
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`frontend/src/components/panels/PlanningPanel.tsx`
- 现象：`addWaypoint(degFromMavInt(selected?.x ?? 48.6493), ...)`，回退值已是度却再除以 1e7，结果约 4.9e-6°。
- 修复：区分"度"与"1e7 整数"，推荐把 store 层统一为度，仅在边界处转换；去掉硬编码默认坐标，改用地图中心或当前机位。
- 附带：列表显示 `y, x`（经度, 纬度）无标签，易读反，改为带标签的 `lat, lon`。
- 验收：未选中航点时添加，航点落在地图当前中心。
- 实现：`PlanningPanel.addAt` 未选中时用地图中心（`ui` store 新增 `mapCenter`，由 `MapView` 的 `camera.moveEnd` 发布，单位度），选中时用该航点；两者都已是度，去掉重复的 `/1e7`。列表改为带标签的 `Lat … · Lon …`。硬编码默认坐标只保留在导出兜底常量 `FALLBACK_HOME`。
- 验证：`npm run typecheck`/`build` 通过；手动步骤：不选航点、移动地图后点 Add，落点应在地图中心。

### #15 [P1][frontend/app] Mission sync state is unmodelled; no read-back verification
**状态**：✅ 已完成（工作区改动，未提交）
- 现象：没有"已同步/已修改/与 FC 不一致"状态；上传后不回读比对。
- 修复：增加 `dirty` 与 `lastSyncedHash`；上传成功后自动下载并逐项比对，不一致则报错；`busy` 在后端无响应时由总超时复位（见 #7）。
- 验收：上传后手动改 FC 端任务，界面能提示不一致。
- 实现：store 新增 `dirty`/`lastSyncedHash`/`fcMatches`/`verifying`；任何编辑置 `dirty` 并清 `fcMatches`。上传成功（`handleEvent`）后置同步并触发 `verifyFc()` 自动下载回读，`mission_plan` 到达时若 `verifying` 则只比对哈希、不覆盖界面，不一致置 `fcMatches=false`；正常下载仍替换列表。面板显示"未保存/与 FC 不一致"。
- 验证：`typecheck`/`build` 通过；手动步骤：上传后改 FC 端任务再触发比对，界面应提示不一致。

### #16 [P1][core] Per-field data age is not tracked
**状态**：⬜ 未开始
- 位置：`TelemetrySnapshot`、`frontend/src/hooks/useDataAge.ts`
- 现象：只有整份快照的更新时间；GPS 停更而姿态仍更新时 UI 无法判断。
- 修复：每个字段带 `updated_at_ms`（或快照里带 age 表），前端对陈旧字段变灰/告警。
- 验收：停止注入 GPS_RAW_INT 后 ≤ N 秒，GPS 相关指示变为陈旧状态。

### #17 [P1][core] Error events delivered through `watch<Option<String>>` can be lost
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`telemetry/hub.rs`（`error_rx`）
- 现象：`watch` 只保留最新值，两次读取之间的错误会被覆盖；`Lagged(_) => continue` 静默丢帧。
- 修复：错误改走 `broadcast`/`mpsc`，带类别（`LinkErrorKind`）和时间戳；丢帧时累计计数并暴露在链路诊断里；前端保留错误历史（对应计划里的"链路日志"）。
- 实现：新增 `TelemetryError { kind: LinkErrorKind, message, at_ms }`（`core::telemetry`，`LinkErrorKind` 改为 `Serialize/Deserialize/TS`）；`TelemetryHub` 错误通道改为容量 64 的 `broadcast`（`subscribe_error()` 返回 `broadcast::Receiver`，`TelemetryHub` 自身改为持有 `broadcast::Sender` 以保持 `Clone`）；新增 `subscribe_dropped_frames()`（`watch<u64>`），pump 在 `Lagged(n)` 时累计并推送。`app-tauri/telemetry_pump.rs` 改用 `err_rx.recv()`（`Lagged` 静默续读、`Closed` 退出），发出 `"link_error"`（`TelemetryError` 对象）与 `"telemetry_dropped"`（累计计数）。前端 `stores/link.ts` 增加 `errorHistory`（上限 50）、`droppedFrames`、`pushError`/`clearErrors`；`bridge.ts` 监听新事件；`ErrorBanner.tsx` 显示类别+消息、可展开历史、丢帧计数提示。
- 验证：`cargo test -p maggcs-core --lib`（98 passed，新增 `errors_are_delivered_losslessly` 与 `lagging_pump_reports_dropped_frames` 两个用例）；`cargo clippy --all-targets -- -D warnings` 通过；`npm run typecheck && npm run check:colors && npm run check:contrast && npm run build` 通过。ts-rs 新增 `LinkErrorKind.ts`/`TelemetryError.ts` 并同步到 `frontend/src/generated-types/`。

---

## P2 · 架构与工程质量

### #18 [P2][arch] Move `MissionService` from app-tauri into `core`
**状态**：✅ 已完成（工作区改动，未提交）
- 违反 ADR-001：服务循环、下载收集、事件序列化都在 Tauri 层，headless server 将无法复用。
- 做法：`core::mission::service` 输入命令、输出事件流；Tauri 层只做 `app.emit` 适配。
- 实现：新增 `core::mission::service`（`MissionIds` / `MissionCommand` / `MissionServiceError` / `MissionServiceEvent` / `MissionService::spawn`），把协议状态机、重传 tick、总超时、下载条目组装全部搬入 core；`MissionServiceEvent::{Protocol(MissionEvent), PlanDownloaded(Vec<MissionItem>)}` 通过 `mpsc` 输出。app-tauri 的 `mission_service.rs` 瘦身为纯适配器（`payload()`/`op_name()` 映射 + `app.emit("mission"/"mission_plan")`），`state.rs` 直接持有 `maggcs_core::mission::service::MissionService`，`commands.rs` 用 `mission_service::spawn(app, handle, MissionIds{..})` 并 `.map_err(|e| e.to_string())`。
- 验证：新增 `crates/core/tests/mission_service_integration.rs`（真实 UDP 假 FC：上传走 `MISSION_COUNT→MISSION_REQUEST_INT→MISSION_ITEM_INT→MISSION_ACK` 收到 `Completed(Upload)`；下载走 `MISSION_REQUEST_LIST→MISSION_COUNT→…` 收到 `PlanDownloaded` 两条 item），2 passed；`cargo test -p maggcs-core --lib` 98 passed；`cargo clippy --all-targets -- -D warnings` 通过。前端 wire contract 未变。

### #19 [P2][arch] Replace polling pump with event-driven hub
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`telemetry/hub.rs::pump`（`try_recv` 排空 + `sleep(50 ms)`）
- 影响：每个事件最多额外延迟 50 ms，占用"位置延迟 < 200 ms"的预算。
- 做法：`select!` 同时等待事件与节流 ticker；丢帧计数入诊断。
- 实现：`pump` 改为单个 `tokio::select!`，同时等待 `events.recv()`、`tokio::time::interval(period)`（`MissedTickBehavior::Delay`）与 `shutdown_rx.changed()`；事件到达即消费，快照按 `hz` 节流推送，`dirty` 标志避免空推。`Lagged(n)` 累计到 `dropped_frames` 并通过 `dropped_tx` 暴露（诊断）、`Closed` 退出，删除 `try_recv` 排空与 `sleep` 轮询。
- 验证：新增 `snapshot_latency_is_bounded` 单测（20 Hz 下心跳快照 < 500 ms 且值正确）；`cargo test -p maggcs-core --lib` 99 passed；`cargo clippy --all-targets -- -D warnings` 通过。

### #20 [P2][arch] Introduce message routing in the connection layer
**状态**：✅ 已完成（工作区改动，未提交）
- 现象：hub、mission service 各自订阅全量 broadcast 再各自过滤；RTK、参数、日志模块都会重复这一模式。
- 做法：连接层统一做 sysid/compid 过滤与按消息类型分发。
- 实现：新增 `core::mavlink::router`：`MessageRoute`（按 `message_ids` 与来源 `system_id`/`component_id` 组合的谓词，builder 风格 `messages()`/`from_node()`/`only()`/`from()` + `matches()`）；`RoutedEvents` 包装 `broadcast::Receiver<ConnectionEvent>`，跳过不匹配的 `Message`、始终放行生命周期事件（`Connected`/心跳/`LinkError`/`Failed`），并把 `RecvError::Lagged` 透传给调用方以便计数。新增 `ConnectionHandle::subscribe_route(route)`。消费者改造：hub 用 `MessageRoute::from_node(target)`；mission service 用 `MessageRoute::messages(MISSION_MESSAGE_IDS)`（协议里新增该 const，列出 40/41/42/43/44/45/47/51/73）；command service 用 `COMMAND_ACK` 的 message id。
- 验证：`MessageRoute` 4 个单测（all/node/message-id/组合）；`cargo test -p maggcs-core`（lib 103 passed，command/mission/connection 集成测试全绿）；`cargo clippy --all-targets -- -D warnings` 通过。

### #21 [P2][arch] Make app state multi-vehicle ready and `connect` atomic
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`app-tauri/src/state.rs`、`commands.rs::connect`
- 现象：全局单例 `connection/hub/mission`；先 take 再 set 非原子，并发 `connect` 有竞态。
- 做法：引入 `LinkId/VehicleId`，`connect/disconnect` 串行化（单一 actor 或持锁到完成）。
- 实现：`state.rs` 引入 `LinkId`（进程内单调分配）与 `ActiveLink { id, connection, hub, mission, command }`，`AppInner` 改为 `{ links: BTreeMap<LinkId, ActiveLink>, primary: Option<LinkId>, link }`，提供 `set_primary_link`（返回旧主链路）/`take_primary_link`/`link_count`/`hub`/`mission`/`command`；新增 `AppState::ops()`（`tokio::sync::Mutex<()>`）。`connect`/`disconnect`/`shutdown_app` 全流程持有该锁，`connect` 成功后一次性换入新链路再关闭旧链路（失败的 reconnect 不再杀掉已有链路）；旧链路经 `old.hub.shutdown()` + `old.connection.shutdown()` 回收，mission/command 句柄 drop 即停。
- 验证：`state.rs` 单测（`LinkId` 唯一且单调、`publish_link`/`link_status` 往返、空状态 `take_primary_link` 返回 None）；`cargo test -p maggcs-app` 6 passed；`cargo clippy --all-targets -- -D warnings` 通过。

### #22 [P2][security] Tighten Tauri configuration
**状态**：✅ 已完成（工作区改动，未提交）
- `tauri.conf.json`：`csp: null`；`capabilities/default.json`：`fs:default` + 读写权限无 scope；`Cargo.toml`：release 仍含 `devtools`。
- 做法：设置严格 CSP（Cesium worker/blob 单独放行）；fs 权限限定到用户通过对话框选择的路径；release 关闭 devtools。
- 实现：`tauri.conf.json` 设置 CSP：`default-src 'self'`、`object-src 'none'`、`base-uri/form-action/frame-ancestors` 收紧；`script-src 'self'`（Tauri 自动注入 nonce）；`style-src 'self' 'unsafe-inline'`（Cesium/Radix 动态内联样式）；`img-src`/`connect-src` 放行 `data:`、`blob:` 与 OSM 瓦片域 `https://tile.openstreetmap.org`/`https://*.tile.openstreetmap.org`；`worker-src`/`child-src` 放行 `'self' blob:`（Cesium worker）；`connect-src` 追加 `ipc: http://ipc.localhost`（Tauri IPC）。`capabilities/default.json` 删除 `fs:default`/`fs:read-files`/`fs:write-files`，只保留 `fs:allow-read-text-file`/`fs:allow-write-text-file`——实际访问范围由 dialog 插件在运行时把用户选中的文件加入 fs scope（`allow_file`）。`Cargo.toml` 去掉 `tauri` 的 `devtools` feature（该 feature 只在 release 保留 devtools；debug 构建默认仍可用）。
- 验证：`cargo build -p maggcs-app` 通过（`tauri-build` 校验 CSP 与 capability schema）；前端未新增远程依赖，Cesium 资产仍走本地 static copy。

### #23 [P2][ci] ts-rs binding sync check can never fail
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`.github/workflows/ci.yml`，`git diff --exit-code ... || echo ...`
- 修复：去掉 `|| echo`，改为失败时输出修复提示并以非零退出。
- 实现：改为 `if ! git diff --exit-code -- crates/core/bindings; then echo "::error::..."; exit 1; fi`，绑定过期时以非零退出并给出 `cargo test -p maggcs-core` 修复提示。
- 验证：YAML 解析通过；逻辑等价于本地 `git diff --exit-code`（当前工作区绑定与生成一致）。

### #24 [P2][ci] Workflow inconsistencies
**状态**：✅ 已完成（工作区改动，未提交）
- SITL 固定 PX4 v1.14.3 + jmavsim，而联调环境是 v1.17.0；注释说"短 smoke 每次 push 跑"但 `if` 只允许 schedule/手动。
- `windows-build` 每次 push 都往固定 `v0.1.0` 创建草稿 release，改为仅在打 tag 时触发。
- 增加依赖/许可证检查（`cargo deny`、`npm audit`），兑现 ADR-007 里"CI 检查 license"的承诺。
- 实现：`on.push` 增加 `tags: ["v*"]`；`windows-build` 改为 `startsWith(github.ref, 'refs/tags/v') || workflow_dispatch`，`tagName`/`releaseName` 用 `github.ref_name`（非 tag 时回退 v0.1.0）；SITL 注释改为"仅 schedule/手动"，PX4 分支由 `v1.14.3` 对齐到 `v1.17.0`；新增 `deny` job（`taiki-e/install-action` 固定 `cargo-deny@0.20.2` + `cargo deny check`）与根 `deny.toml`；frontend job 增加 `npm audit --omit=dev --audit-level=high`（只审运行时依赖）。
- deny.toml：`[advisories]` `yanked="deny"`/`unmaintained="workspace"`，忽略 `RUSTSEC-2026-0194/0195`（仅 `mavlink-bindgen` 构建期解析可信 dialect XML 的 quick-xml 0.39，被 mavlink 0.17.1 锁死无法升级，已注明理由）；`[licenses]` 允许 MIT/Apache-2.0/BSD/Unicode-3.0/Zlib/0BSD/CC0-1.0/MPL-2.0，工作区 crate 标 `publish = false` + `private = { ignore = true }` 跳过；`[sources]` 告警未知 registry/git。为匹配 0.20 schema 同时给 `crates/core`/`crates/app-tauri` 增加 `publish = false`。
- 验证：`cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`（exit 0）；`npm audit --omit=dev --audit-level=high` → `found 0 vulnerabilities`；`ci.yml` YAML 解析通过。

### #25 [P2][test] Add a lossy fake-FC test harness for the mission protocol
**状态**：✅ 已完成（工作区改动，未提交）
- 在 `core/tests/` 里实现可控丢包/重复/乱序/延迟的假 FC，覆盖上传、下载、清空、设置当前航点。
- 用它自动验证 Phase 1 验收（100 航点上传下载一致；10% 丢包仍可完成）以及 #7 #8 #9 的回归。
- 实现：新增 `crates/core/tests/mission_fake_fc.rs`：`FakeFc` 实现上传（MISSION_COUNT→逐条 REQUEST_INT→ACK）、下载（REQUEST_LIST→COUNT→逐条 item→ACK）、清空、设置当前航点，并在超时后重发最后一个控制帧；`Sim` 用有向双队列 + `Faults { drop_period, duplicate_period }` 注入丢包/重复，并用虚拟时钟（无进展时前进一个 `RETRY_TIMEOUT` 再 `on_tick`）按 `retransmit_due` 驱动重传，因此完全确定、瞬时完成。MAVLink mission 是 stop-and-wait，乱序/延迟在该模型中等价于丢包+重复，已由重传路径覆盖。
- 验证：6 个测试全绿——`upload/download_100_waypoints_round_trips_exactly`（100 条逐字段相等）、`upload/download_completes_with_10_percent_loss`（drop_period=10 + duplicate_period=17，约 8/23 次重传后完成）、`clear_removes_the_fc_mission`、`set_current_reports_the_active_waypoint`；#7 重传回归由丢包用例覆盖，#8/#9 由 `protocol.rs` 既有单测（`upload_duplicate_request_is_resent`、`download_ignores_items_from_foreign_source` 等）覆盖。`cargo test -p maggcs-core` 全绿。

### #26 [P2][docs] Documentation and ADR consistency
**状态**：✅ 已完成（工作区改动，未提交）
- AGENTS.md 要求全英文，但 `DEVELOPMENT_PLAN.md`（v2.2）为中文，v2.1 为英文，两份并存。
- ADR-006（工作基准 AMSL）与计划 v2.2 ADR 表（"内部统一用椭球高"）矛盾，需统一并改状态。
- 所有 ADR 仍为 Draft；ADR-010 已实现应转 Accepted；ADR-011（链路状态模型与 GCS 心跳）在计划里被引用但 `docs/adr/` 中不存在。
- AGENTS.md 中 `crates/server`、`frontend` 的"尚未搭建"描述已过期；`LICENSE` 仍未决定（ADR-007）。
- 实现：把 `docs/DEVELOPMENT_PLAN.md` 全文译为英文 v2.2（修正 ADR 表中 006 的高度基准矛盾、010 改为 Accepted、007 改为已定、011 已存在），删除已被取代的 `DEVELOPMENT_PLAN_v2.1.md`，并把分散在各 README/源码里对 `_v2.1` 的引用改为 `DEVELOPMENT_PLAN.md`；`docs/README.md` 同步。新增 `docs/adr/011-link-state-model.md`（四级链路模型 + 1 Hz GCS HEARTBEAT + 独立超时定时器），ADR-001/002/006/010 由 Draft 转 Accepted（附日期与依据），ADR-007 定为 Apache-2.0 并落地 `LICENSE`（官方文本）与 workspace `license = "Apache-2.0"`。`AGENTS.md` 仓库布局改为反映现状（`core` 已含 mavlink/telemetry/mission/commands/devices/height，app-tauri 与 frontend 已搭建，server/udev-installer/dem-prep 仍为占位）。
- 验证：`rg "[\x{4e00}-\x{9fff}]" docs AGENTS.md` 无输出（docs 全英文）；`rg "DEVELOPMENT_PLAN_v2.1"` 无引用；`cargo deny check` → `licenses ok`（工作区 crate 现为 Apache-2.0）；`cargo check -p maggcs-core` 通过。

---

## P3 · 前端与打磨

### #27 [P3][frontend] Show a clear "MOCK DATA" indicator when `isMock` is true
**状态**：✅ 已完成（工作区改动，未提交）
- 防止模拟遥测被误认为真实数据。
- 实现：新增 `frontend/src/components/MockBanner.tsx`，`isMock` 为真时在地图区顶部居中显示醒目的 "MOCK DATA · Simulated telemetry — not a live link" 提示条（`role="status"`、warn 配色、带图标）；`App.tsx` 挂载；移除 HUD 里 10px 的 "MOCK" 小标签。i18n 新增 `mock.banner`/`mock.detail`。
- 验证：`npm run typecheck`、`npm run check:colors` 通过；`isMock` 由 `startMockFeed()` 设置，浏览器开发模式下可见。

### #28 [P3][frontend] Telemetry channel will not scale to Phase 4 mag data
**状态**：✅ 已完成（设计定稿，实现留待 Phase 4）
- 当前 20 Hz 整份 JSON 快照适合现有 6 类消息；磁数据高频曲线需单独的批量/二进制通道（Tauri `Channel`）与 uPlot 环形缓冲。
- 设计在 Phase 4 前定稿，不要沿用 `telemetry` 事件。
- 实现：新增 `docs/design/telemetry-channels.md`，冻结双通道方案——`telemetry` 事件保持 20 Hz JSON 状态快照（HUD/地图/离散状态的事实来源），高频序列另开 Tauri `Channel<T>` 二进制批量帧（每 ~50–100 ms 一批，小端 f32/i32，带 series id/起始时间戳/采样率/通道掩码/样本数头部），前端写入定长环形缓冲（容量/窗口为 config）供 uPlot 直接绘制；背压采用丢最旧 + 计数上报，与 #17 的丢帧诊断一致。Phase 4 直接照此实现。

### #29 [P3][frontend] Split `MapView.tsx` (~17 KB) before adding survey layers
**状态**：✅ 已完成（工作区改动，未提交）
- 先拆成场景初始化 / 图层 / 航点交互 / 相机跟随几个模块。
- 实现：`MapView.tsx` 从 538 行降到 46 行，只剩组合（viewer 生命周期 + store wiring + 渲染）。新增 `frontend/src/cesium/`：`constants.ts`（home/trail/预测/跟随常量与模型 URI）、`uav.ts`（纯 Cesium 数学：`groundSpeedMps` / `uavQuaternion` / `uavOrientation` / `projectAhead`）、`scene.ts`（WebGL 探测 + 网格/OSM 影像 + 初始视角，返回 `Viewer`）、`entities.ts`（drone/trail/predict/home 图层与航点折线+编号点，含 `disposeWaypointLayer`）、`waypoints.ts`（屏幕空间拖拽处理器）、`follow.ts`（`createFollowController`：dead-reckon 推进 + 手写跟随相机）；新增 `frontend/src/hooks/useCesiumViewer.ts`（React 生命周期绑定，返回 `{ initError, setSnapshot, setMission, goHome }`）。行为不变（纯搬移 + import）。
- 验证：`npm run typecheck`、`npm run check:colors`、`npm run check:contrast`、`npm run build` 全通过；用 Playwright 无头 Chromium 加载构建产物，控制台无错误、无 404。拆分路线记录在 `docs/design/map-view-modules.md`。

### #30 [P3][build] Trim Cesium assets for packaging
**状态**：✅ 已完成（工作区改动，未提交）
- `vite.config.ts` 复制整个 `Workers/ThirdParty/Assets/Widgets`，`chunkSizeWarningLimit` 调到 2 MB；Phase 6 前裁剪默认影像等不用的资源。
- 实现：`vite.config.ts` 不再整目录复制 `Assets`，改为按需列出子项——保留 `approximateTerrainHeights.json`、`IAU2006_XYS`、`Images`、`Textures/{SkyBox,LensFlare,moonSmall.jpg}`；剔除 `Textures/{NaturalEarthII,maki,waterNormals*.jpg}`（默认影像、PinBuilder 图标、海面法线，本应用从不请求）。`Workers`/`ThirdParty`/`Widgets` 仍整体复制（懒加载、按名引用）。
- 验证：`vite build` 后 `dist/cesium/Assets` 4.6 MB → 3.3 MB、`dist/cesium` 7.7 MB → 6.2 MB、`dist` 23 MB → 19 MB。用 Playwright 无头 Chromium 抓取全部网络请求核对：被剔除的资源均未被请求；过程中发现默认场景会请求 `IAU2006_XYS/IAU2006_XYS_18.json`（太阳/月亮 ICRF），已保留，最终无 404、无控制台错误。截图基线与本机环境本身已有 ~38–47% 差异（同一构建连续两次运行 `dark-planning-1920x1080` 即相差 25%，OSM 瓦片/模拟数据导致），故与本次裁剪无关。

### #31 [P3][core] Device hotplug watcher polls every 500 ms and fails hard
**状态**：✅ 已完成（工作区改动，未提交）
- 位置：`devices/manager.rs::spawn_watcher`（`?` 提前返回、轮询枚举）。
- 做法：枚举失败不应中止监听；Linux 上后续换 udev 事件；`helpers/udev-installer` 只有 README，实现前先按 ADR-009 写威胁模型和拒绝路径的测试。
- 实现：`spawn_watcher` 取消 `Result` 返回，改为返回 `DeviceManagerHandle`（含 `stop`/`spawn_watcher` 关闭路径）；枚举失败只记录、不再提前返回中止监听，并新增 `primed` 标志在首轮成功后标记；把轮询枚举抽成 `hotplug_events()` 便于测试。纯枚举（无特权操作、无写设备），故按 ADR-009 只需非致命化 + 测试，不需要新的威胁模型。
- 验证：`devices/manager.rs` 新增单测覆盖 `hotplug_events()` 的加入/移除/无变化与"枚举失败不致命"；`cargo test -p maggcs-core --lib` 通过；`cargo clippy --all-targets -- -D warnings` 通过。

---

### #32 [P3][frontend] UAV marker: 3D model, MAVLink-driven attitude, forward-only prediction
**状态**：✅ 已完成
- 位置：`frontend/src/components/MapView.tsx`、`frontend/scripts/build-uav-model.mjs`、`model/`
- 现象（修复前）：蓝色三角锥朝向与航向不符；图标会随相机环绕改变姿态，与实际姿态偏差很大（应水平时图标向下倾斜）；落地时图标悬在空中；预测轨迹飞过后不消失。
- 修复：
  - 用真实 glTF 无人机模型替换程序化锥体；高度改用 `relative_alt_m`，落地时图标贴地。
  - 姿态直接来自 MAVLink `ATTITUDE`（`snapshot.attitude` 的 roll/pitch/yaw），不再受相机影响；yaw 按罗盘角映射到机头方向。
  - 前向匀速预测轨迹（120 s），每一帧新 fix 重建，已飞过段自动消失。
  - 相机环绕只改变视角，不改变图标姿态。
- 已知限制：Cesium 不会光栅化该资产的 skinned mesh，因此由 `scripts/build-uav-model.mjs` 生成去蒙皮静态模型（8.4 MB → 2.8 MB）供运行时加载；源资产保留在 `model/`（CC-BY-4.0，署名见 `model/README.md`）。
- 验收：注入已知 roll/pitch/yaw 的 MAVLink 快照后截图核对——roll=pitch=0 时图标水平；机头指向 = HDG；+pitch 机头上仰；+roll 右翼下压。

### #33 [P1][frontend] Upload keeps saying "Unsaved changes — not the FC plan"
**状态**：✅ 已完成（工作区改动，未提交）
- 现象（操作员报告）：编辑好航迹后点 Upload，面板仍显示 "Unsaved changes — not the FC plan"，看不出上传到底成没成功；而且没有清空本地航迹的按钮。
- 根因：同步状态只有一个布尔 `dirty`，"从未上传"和"改过后未同步"、"上传失败"共用同一句红色警告；上传失败（未连 FC、FC 不应答、回读丢失）时 `dirty` 本就该保持 true，但文案让人以为上传成功了。另有 `verifying` 悬挂与链路断开后 `busy` 永不复位两处状态机漏洞。
- 修复：
  - 新增 `frontend/src/mission/sync.ts::planSyncStatus`，把 `itemCount/dirty/fcMatches/lastSyncedHash` 归为 5 态：`empty`（无航迹）/`unsynced`（本地航迹，从未上传）/`dirty`（上传后又改）/`mismatch`（FC 回读不一致，红）/`synced`（与 FC 一致，绿）。面板按状态显示单一文案，上传成功后明确显示 "In sync with the FC"。
  - 新增本地 `clearPlan`（清空航迹/复杂块/home/基线，无需链路，二次点击确认）与 FC 的 `Clear FC` 区分开，按钮文案同步修改；Upload/Clear FC 在未连接时补 `title` 说明。
  - `handleEvent` 收到任意 `failed` 时复位 `verifying`（回读丢失不再吞掉下一次下载）；新增 `linkLost`，`bridge.ts` 在 `fc_alive` 转假时调用（mission service 任务随连接一起退出，永远不会再发终止事件，否则 Upload 会永远停在 "Uploading…" 且按钮全灰）。
  - 重新 Generate 预设轨迹时替换上一次生成的轨迹块而不是叠加（`InsertedPattern.count`；任何手工编辑会解除该块），`applyImport` 也解除，避免导入后误删导入的航点。
- 验收：`npm run check:mission-sync`（新增，esbuild+Node 驱动真实 store 跑 15 个场景：上传成功清警告、失败保留编辑、回读不一致报 mismatch、断链复位、清空航迹、预设轨迹替换）。
- 验证：`npm run typecheck`、`check:colors`、`check:contrast`、`check:i18n`、`check:planfile`、`check:mission-sync`、`npm run build` 全通过；`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -p maggcs-core --lib`（163）、`cargo test -p maggcs-app`、`cargo deny check` 通过；`scripts/desktop-smoke.sh` PASS（56.5% bright）。

---

### #34 [P0][core/frontend] Upload fails with `mission ack denied (type 3)` on a plan with a DO item
**状态**：✅ 已完成（工作区改动，未提交）
- 现象（操作员报告）：生成 sweep 预设后点 Upload，出现 `Mission failed: mission ack denied (type 3)`，整条任务被拒。
- 定位：`type 3 = MAV_MISSION_UNSUPPORTED`。对照本机 PX4 v1.17 源码 `src/modules/mavlink/mavlink_mission.cpp::parse_mavlink_mission_item`：带全局坐标系的帧只接受一个**命令白名单**（NAV_WAYPOINT / LAND / TAKEOFF / LOITER_* / VTOL / FENCE_* / RALLY / ROI / SET_HOME…），其余命令（`DO_*`、`CONDITION_*`、`NAV_RETURN_TO_LAUNCH`、`NAV_DELAY`）必须用 `MAV_FRAME_MISSION`(2) 发送，参数放在 P1..P4；否则走 `default:` 直接返回 `MAV_MISSION_UNSUPPORTED`。我们的 `compile` 把**每个**航点都编译成当前高度模式帧，而 `core::survey` 的 sweep 在 `speed_mps` 存在时会在首位插入 `DO_CHANGE_SPEED`（默认 5 m/s，即每次 Generate 都有），于是 PX4 拒绝整条上传。
- 修复：
  - `core::mission::command_uses_coordinate`（+ 前端镜像 `mission/compile.ts::commandUsesCoordinate`）给出白名单；`MissionFrame` 新增 `Mission`（`MAV_FRAME_MISSION`，`from_mav`/`to_mav` 双向映射，之前下载含 DO 项的 QGC/PX4 任务会因 `UnsupportedFrame` 整条失败）。
  - `PlannedMission::compile` / `compileWaypoints`：非坐标命令一律编译为 `mission` 帧，且 `z` 不做基准换算（命令项的参数不是高度）；坐标项照旧跟随模式帧。
  - `core::survey` 不再把扫描高度复制到 `DO_CHANGE_SPEED` 项。
  - `.plan` 互操作：`frame 2 ↔ mission`；没有 `coordinate` 的 `MAV_FRAME_MISSION` 项不再被误判为 ComplexItem 丢弃；`deriveHome` 跳过命令项（避免 home=(0,0,0)）。
  - 面板把命令项显示为 “Command item · MAV_CMD 178”，不再显示经纬高。
- 验收：`cargo test -p maggcs-core --lib`（167 passed，含 `command_items_compile_to_the_mission_frame`、`coordinate_commands_are_classified_like_px4`、`mission_frame_round_trips`）；`check:planfile` 新增 5 个命令项用例（三种模式帧、模型往返、QGC 导入、导出 frame=2）。
- 待办（同一根因的另一半）：AGL 模式编译到 `GLOBAL_TERRAIN_ALT_INT`(11)，PX4 同样不接受（会回 `type 2 UNSUPPORTED_FRAME`）——按 ADR-005/WS-D 由地面站做地形跟随，等 DEM 落地后改为相对帧 + AGL 偏移。

---

### #35 [P2][frontend] Tie lines look like spacing lines; the Layers panel is decorative
**状态**：✅ 已完成（工作区改动，未提交）
- 现象（操作员报告）：sweep 预设生成后，测线（spacing line）与 tie line 是同一条颜色的折线，无法分辨；左侧 Layers 面板是四个写死的 ON/OFF，点了没有任何效果，"not useful at all"。
- 修复：
  - 新增 `frontend/src/mission/lineKinds.ts`（`kindsBySeq`/`splitRuns`/`heightRuns`，纯函数）：把 `PatternLine` 表摊平成 `seq -> kind`，再把航迹按同色连续段（run）切分。
  - `cesium/entities.ts::renderWaypoints` 改为**每段一条折线**：survey = accent，tie = warn（宽度 3），calibration = mag，未分类 = ok；`MapView` 传入 `kindsBySeq(lastPattern.lines)`，因此 tie line 与相邻测线一定异色。图例见 `plan.pattern.legend.*`。
  - `components/Drawer.tsx::LayersSection` 重写：只列地图真实拥有的两个图层（OSM 影像、离线经纬网）并接线到 `stores/ui.ts::showImagery/showGrid` → `useCesiumViewer::setLayers` → `scene.ts::applyLayerVisibility`；DTM/DSM 归入 "Not loaded yet — arrives with the DEM (Phase 3)"，不再假装是开关。
  - `MissionsSection` 同步重写为真实数据（航迹条数、同步状态、当前航点、pattern 行表），不再是写死的 "Waypoint 1..3"。
- 验收：`check:planfile` 新增 4 个用例覆盖 `kindsBySeq`/`splitRuns`（tie 段不与 survey 段合并、空 kind 段分开）；`npm run typecheck` 通过。

### #36 [P2][frontend] No height visualization per waypoint
**状态**：✅ 已完成（工作区改动，未提交）
- 现象（操作员报告）：三维地图上只能看到航迹的水平形状，看不出哪个航点更高，"the height should have a way to visualize for each waypoint"。
- 修复：
  - `lineKinds.ts::heightRuns`：给出每个航点的竖直段。初版以本航迹最低航点为基线，后按操作员要求改为**画到地面**（HOME 的 AMSL 高度；无 home 时退回最低航点），见 #39。
  - `entities.ts::renderWaypoints` 新增 `{ heights?: boolean }`：每个航点画一根 muted 色竖直杆 + `seq · N m` 高度标签；低于 `HEIGHT_EPSILON_M = 0.25` 的杆不画（整条航迹等高时不产生噪音）。
  - `MapToolbar` 增加高度显示开关（`stores/ui.ts::showHeights`，`map.heights`）。
  - `compile.ts::toDisplayItems` 把 relative/terrain 帧的 `z` 换算成 AMSL 再交给地图，否则相对高度会被画在错误的高度上。
- 验收：`npm run typecheck`、`check:colors`（颜色全部取 CSS token）、`npm run build` 通过；`MapView` 传入 AMSL 后的航迹高度与 `waypoints[].altitude.meters` 一致。

### #37 [P1][frontend] The waypoint list says nothing while flying
**状态**：✅ 已完成（工作区改动，未提交）
- 现象（操作员报告）：飞行时打开 Missions / Properties，看到的是编辑用的航点坐标表，与"现在飞到哪、下一个还有多远"无关；航点列表永远把第 0 个标成 CURRENT。
- 修复：
  - 新增 `frontend/src/mission/geo.ts`（haversine 距离、初始方位）与 `frontend/src/mission/progress.ts`（`missionProgress`、`formatDistance`、`formatDuration`、`progressItemsOf`）：按 FC 的 `MISSION_CURRENT`（目标航点）算出已飞/剩余航点数、到目标距离与方位、目标高度、剩余航程、进度比例，以及按当前地速估算的 ETA。没有 `MISSION_CURRENT` 或没有位置解时降级为航迹自身统计，不显示虚假距离。
  - 新增 `hooks/useMissionProgress.ts` 与 `components/panels/MissionProgressCard.tsx`：飞行视图的 Properties 面板顶部显示进度卡（Active WP n/N、To WP 距离·方位、Target alt、Still to fly、ETA、进度条）；`Drawer.tsx::FlightMissionSection` 在飞行视图改列**航点进度表**（seq、pattern 类别、AMSL 高度，当前航点高亮，点击选中），不再列 pattern 行表。
  - `PlanningPanel` 的航点列表：CURRENT 徽标改用 `currentSeq`（不再恒为 seq 0）；有位置解时每行显示到该航点的直线距离；`Set current` 按钮对任意选中航点可用。
- 验收：`npm run check:progress`（新增，16 个场景：已知球面几何、卡盘方位、无 MISSION_CURRENT/无解/未知 seq/非连续 seq/空航迹、ETA 与格式化）；`frontend` typecheck/colors/contrast/i18n/planfile/mission-sync/coords/build 全通过；`cargo fmt --check`、`clippy -D warnings`、`cargo test -p maggcs-core --lib`（167）、`cargo test -p maggcs-app`、`cargo deny check` 通过。
- 备注：`check:planfile`/`check:mission-sync`/`check:coords`/`check:progress` 之前只在本地跑，已加入 `.github/workflows/ci.yml`。

### #38 [P1][frontend] Upload reports "FC mission differs from the uploaded plan" for a plan the FC holds
**状态**：✅ 已完成（工作区改动，未提交）
- 现象（操作员报告）：上传成功后，面板回读校验仍报红字 "FC mission differs from the uploaded plan"。
- 根因：`MissionItem.z` 与 `params` 在模型/线路上是 **f32**（`core::mission::MissionItem` 是 `Vec<f32>`/`f32`），但前端 `compile.ts` 用 f64 生成这些值。回读比较用 `itemsHash` 做**精确** JSON 相等，于是本地 `80.123456789` 与 FC 原样回传的 `80.12346`（f32 展开）不等，每次上传都误报不一致。触发条件是高度不是 f32 精确值（例如由导入 `.plan` 的 home 高度 + 50 m 生成的预设轨迹）。
- 修复：把比较与生成都收敛到线路精度——`compile.ts::wire()` 用 `Math.fround` 把 `z`/`params` 落到 f32（本地计划即为将要发出的值），`hash.ts::itemsHash` 对 `params`/`z` 同样取 `Math.fround`（覆盖从 `.plan` 导入的 f64 复杂块子项）。
- 验收：`check:mission-sync` 新增用例 "a read-back at wire precision is not a false mismatch"——先断言编译结果的 `z` 已是 f32，再用 f32 回读走完 upload→completed→handlePlan，状态必须是 `synced`；16 个场景全通过。

### #39 [P2][frontend] Selecting a waypoint does not show it on the map; height sticks float
**状态**：✅ 已完成（工作区改动，未提交）
- 现象（操作员报告）：在航点列表里选中一项，地图上对应的航点看不出来；另外高度竖线只画到本航迹最低点，不是画到地面。
- 修复：
  - `entities.ts::renderWaypoints`：选中航点加一圈白色 halo（`pixelSize 26`、透明填充、描边 3）并把点放大到 16，避免整条 survey 航迹同色时"换色也看不出来"。
  - 列表点击改用 `stores/mission.ts::focus(seq)`（= 选中 + 一次性请求地图取景）；`useCesiumViewer` 消费 `focusSeq` 后自动清空，因此**地图点选不会反过来移动相机**，只有列表点击会飞到该航点（`FOCUS_HEIGHT_M = 600`，0.8 s）。
  - 高度竖线改为**从地面画到航点**：`lineKinds.ts::heightRuns(items, groundM)` 的 `groundM` 取 HOME 的 AMSL 高度（没有 DEM 前与 AGL 模式同一假设），无 home 时退回本航迹最低点；判空改用 `Math.abs`，因此低于 home 的航点也会画出向下的杆。
  - 顺带修掉一个回归：折线改成 "每段一色" 后 `layer.line` 不再存在，拖拽预览只移动了点、不更新折线。`WaypointLayer` 改为记录 `runs: {entity, seqs}[]`，`previewWaypoint` 重画包含该 seq 的那一段。
- 验收：`check:planfile` 新增用例覆盖 `heightRuns(items, groundM)`（落地基准 + 各自净空，9 个用例全通过）；`npm run typecheck`、`check:colors`、`check:contrast`、`npm run build` 通过。

## 待核实项（先验证再决定是否开 issue）
- `mavlink` crate 0.17 的 UDP 监听是否设置 `SO_REUSEADDR`；`udpin` 的回复地址行为（抓包确认）。
- `DO_PAUSE_CONTINUE` 在 PX4 v1.17 的实际语义。现状（2026-10-07）：Pause/Continue 已按 MAVLink 规范接入（param1 = 0 暂停 / 1 继续，`send_command("pause"|"continue")`，见 `crates/app-tauri/src/commands.rs::named_command`，含 3 个单测）；PX4 v1.17 是否支持、以及参数语义仍需 SITL 抓包确认——不支持时 FC 回 NACK，UI 会显示 "Not supported by the FC"。
- `MISSION_COUNT` 的 `mission_type` 等扩展字段默认值，与 QGC 上传的帧对比。
- PX4 对多个 GCS 同时读取任务时的行为（影响 #9 的具体修法）。
