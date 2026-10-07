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
| #6 | 遥测聚合无来源过滤 | ⬜ 未开始 |
| #7 | 任务协议重传定时器未接线 | ⬜ 未开始 |
| #8 | 上传时重复 MISSION_REQUEST 被当作失败 | ⬜ 未开始 |
| #9 | 下载未过滤发往其他 GCS 的 item | ⬜ 未开始 |

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
**状态**：⬜ 未开始
- 位置：`core/src/telemetry/mod.rs::TelemetryUpdate::try_from_envelope`、`TelemetryAggregator::apply`
- 现象：不检查 `system_id/component_id`。链路上有 QGC 心跳、相机/gimbal/伴飞计算机心跳时，`heartbeat` 字段被最后到达的覆盖；多机时位置/姿态混写。
- 修复：按目标 `(sysid, compid)` 过滤；预留 `VehicleId` 维度。
- 验收：混入其他节点的 HEARTBEAT/ATTITUDE，快照仍只反映目标 FC。

### #7 [P0][core] Mission protocol retransmission timer is never wired
**状态**：⬜ 未开始
- 位置：`app-tauri/src/mission_service.rs::run`；`core/src/mission/protocol.rs::retransmit_due / take_timeout_failure`
- 现象：服务循环里没有定时器调用这两个函数（仅单测调用）。丢一个包后上传/下载永久挂起，前端 `busy` 永不复位。
- 修复：在服务循环加 tick（如 100–200 ms），发送到期帧；超限时发出 `Failed(RetriesExhausted)` 并回到 Idle；同时给整个操作加总超时。
- 验收：10% 随机丢包下 100 航点上传成功（见 #25 测试夹具）；彻底断链时 UI 在有限时间内回到可操作状态并提示失败。

### #8 [P0][core] Upload: duplicate MISSION_REQUEST treated as failure
**状态**：⬜ 未开始
- 位置：`protocol.rs::on_upload_request`，测试 `upload_seq_mismatch_fails`
- 现象：`req_seq != next_seq` 直接 `SeqMismatch` 失败。FC 没收到 item 时会重复请求同一 seq，GCS 应重发。
- 修复：`req_seq == next_seq - 1` 重发上一项；`== next_seq` 发新项；其他才报错。修改并反转该测试。
- 验收：丢掉任意一个 `MISSION_ITEM_INT` 后上传仍能完成。

### #9 [P0][core/app] Download accepts MISSION_ITEM_INT addressed to other GCSs
**状态**：⬜ 未开始
- 位置：`protocol.rs::target_of`（未覆盖 `MISSION_ITEM_INT`、`MISSION_CURRENT`）；`mission_service.rs`（对所有 `MISSION_ITEM_INT` 写入 HashMap，且不看来源）
- 影响：QGC 同时下载任务时，MagGCS 会混入 FC 发给 QGC 的 item。
- 修复：校验发送方是目标 FC、且（若消息带目标字段）目标是本机；`MISSION_ITEM_INT` 本身没有 target 字段的情况下，改为只在自己处于 Download 且 seq 与期望一致时接收；去掉 service 里的第二份 items 副本，以协议状态机为唯一来源。
- 验收：测试夹具中并行注入发往另一 GCS 的任务流，结果不被污染。（待核实：PX4 对多 GCS 同时读取任务的实际行为。）

---

## P1 · 数据正确性与互操作

### #10 [P1][core] `MissionItem` model is redundant and lossy
**状态**：⬜ 未开始
- 位置：`core/src/mission/types.rs`、`protocol.rs::mission_item_from_mav`
- 现象：`params` 含 7 个值同时又有 `x/y/z`；下载时 `params[4] = x as f32`（1e7 缩放整数转 f32，丢精度，误差可达分米级）；上传时忽略 `params[4..7]`。
- 修复：`params` 只保留 p1–p4，坐标仅用 `x/y/z`；同步更新前端、`planfile.ts`、ts-rs 绑定。
- 验收：对随机经纬度做上传/下载往返，x/y 逐位相等。

### #11 [P1][core] `MissionFrame` does not round-trip
**状态**：⬜ 未开始
- 位置：`types.rs::to_mav / from_mav`
- 现象：`Global`→`GLOBAL_INT`、`GlobalTerrainAlt`→`..._INT`，下载回来变成 INT 变体，"逐项一致"必然不成立。
- 修复：只保留 INT 变体（删除冗余非 INT 变体），对未知帧返回错误而不是静默 `_ => Global`。
- 验收：所有支持的 frame 往返相等；未知 frame 报错。

### #12 [P1][frontend] `.plan` import crashes on ComplexItem
**状态**：⬜ 未开始
- 位置：`frontend/src/mission/planfile.ts::qgcToItem`
- 现象：直接解构 `raw.coordinate`，QGC 的 Survey/Corridor Scan 等 `ComplexItem` 没有该字段，抛 TypeError。
- 修复：按 `type` 分支；ComplexItem 要么展开其子项，要么明确提示"不支持"并列出位置，不允许静默丢弃。
- 验收：用包含 Survey 复杂项的真实 `.plan` 样本（放入 `testdata/`）导入不崩溃且有明确提示。

### #13 [P1][frontend] `.plan` export is not faithful
**状态**：⬜ 未开始
- 位置：`planfile.ts::buildPlan / itemToQgc`
- 问题：`plannedHomePosition` 用第一个航点代替；`doJumpId` 恒为 0（QGC 从 1 递增）；`vehicleType/firmwareType/cruiseSpeed/hoverSpeed` 写死；导入时丢弃 `geoFence/rallyPoints/mission` 其他字段。
- 修复：保留导入文件的未知字段并在导出时写回（passthrough）；Home 位置单独建模；`doJumpId` 递增。
- 验收：`.plan` 导入→导出→再导入，结构化比较无差异（忽略格式化）；用 QGC 实际打开验证（待核实）。

### #14 [P1][frontend] Add-waypoint button inserts at ~(0°, 0°) when nothing is selected
**状态**：⬜ 未开始
- 位置：`frontend/src/components/panels/PlanningPanel.tsx`
- 现象：`addWaypoint(degFromMavInt(selected?.x ?? 48.6493), ...)`，回退值已是度却再除以 1e7，结果约 4.9e-6°。
- 修复：区分"度"与"1e7 整数"，推荐把 store 层统一为度，仅在边界处转换；去掉硬编码默认坐标，改用地图中心或当前机位。
- 附带：列表显示 `y, x`（经度, 纬度）无标签，易读反，改为带标签的 `lat, lon`。
- 验收：未选中航点时添加，航点落在地图当前中心。

### #15 [P1][frontend/app] Mission sync state is unmodelled; no read-back verification
**状态**：⬜ 未开始
- 现象：没有"已同步/已修改/与 FC 不一致"状态；上传后不回读比对。
- 修复：增加 `dirty` 与 `lastSyncedHash`；上传成功后自动下载并逐项比对，不一致则报错；`busy` 在后端无响应时由总超时复位（见 #7）。
- 验收：上传后手动改 FC 端任务，界面能提示不一致。

### #16 [P1][core] Per-field data age is not tracked
**状态**：⬜ 未开始
- 位置：`TelemetrySnapshot`、`frontend/src/hooks/useDataAge.ts`
- 现象：只有整份快照的更新时间；GPS 停更而姿态仍更新时 UI 无法判断。
- 修复：每个字段带 `updated_at_ms`（或快照里带 age 表），前端对陈旧字段变灰/告警。
- 验收：停止注入 GPS_RAW_INT 后 ≤ N 秒，GPS 相关指示变为陈旧状态。

### #17 [P1][core] Error events delivered through `watch<Option<String>>` can be lost
**状态**：⬜ 未开始
- 位置：`telemetry/hub.rs`（`error_rx`）
- 现象：`watch` 只保留最新值，两次读取之间的错误会被覆盖；`Lagged(_) => continue` 静默丢帧。
- 修复：错误改走 `broadcast`/`mpsc`，带类别（`LinkErrorKind`）和时间戳；丢帧时累计计数并暴露在链路诊断里；前端保留错误历史（对应计划里的"链路日志"）。

---

## P2 · 架构与工程质量

### #18 [P2][arch] Move `MissionService` from app-tauri into `core`
**状态**：⬜ 未开始
- 违反 ADR-001：服务循环、下载收集、事件序列化都在 Tauri 层，headless server 将无法复用。
- 做法：`core::mission::service` 输入命令、输出事件流；Tauri 层只做 `app.emit` 适配。

### #19 [P2][arch] Replace polling pump with event-driven hub
**状态**：⬜ 未开始
- 位置：`telemetry/hub.rs::pump`（`try_recv` 排空 + `sleep(50 ms)`）
- 影响：每个事件最多额外延迟 50 ms，占用"位置延迟 < 200 ms"的预算。
- 做法：`select!` 同时等待事件与节流 ticker；丢帧计数入诊断。

### #20 [P2][arch] Introduce message routing in the connection layer
**状态**：⬜ 未开始
- 现象：hub、mission service 各自订阅全量 broadcast 再各自过滤；RTK、参数、日志模块都会重复这一模式。
- 做法：连接层统一做 sysid/compid 过滤与按消息类型分发。

### #21 [P2][arch] Make app state multi-vehicle ready and `connect` atomic
**状态**：⬜ 未开始
- 位置：`app-tauri/src/state.rs`、`commands.rs::connect`
- 现象：全局单例 `connection/hub/mission`；先 take 再 set 非原子，并发 `connect` 有竞态。
- 做法：引入 `LinkId/VehicleId`，`connect/disconnect` 串行化（单一 actor 或持锁到完成）。

### #22 [P2][security] Tighten Tauri configuration
**状态**：⬜ 未开始
- `tauri.conf.json`：`csp: null`；`capabilities/default.json`：`fs:default` + 读写权限无 scope；`Cargo.toml`：release 仍含 `devtools`。
- 做法：设置严格 CSP（Cesium worker/blob 单独放行）；fs 权限限定到用户通过对话框选择的路径；release 关闭 devtools。

### #23 [P2][ci] ts-rs binding sync check can never fail
**状态**：⬜ 未开始
- 位置：`.github/workflows/ci.yml`，`git diff --exit-code ... || echo ...`
- 修复：去掉 `|| echo`，改为失败时输出修复提示并以非零退出。

### #24 [P2][ci] Workflow inconsistencies
**状态**：⬜ 未开始
- SITL 固定 PX4 v1.14.3 + jmavsim，而联调环境是 v1.17.0；注释说"短 smoke 每次 push 跑"但 `if` 只允许 schedule/手动。
- `windows-build` 每次 push 都往固定 `v0.1.0` 创建草稿 release，改为仅在打 tag 时触发。
- 增加依赖/许可证检查（`cargo deny`、`npm audit`），兑现 ADR-007 里"CI 检查 license"的承诺。

### #25 [P2][test] Add a lossy fake-FC test harness for the mission protocol
**状态**：⬜ 未开始
- 在 `core/tests/` 里实现可控丢包/重复/乱序/延迟的假 FC，覆盖上传、下载、清空、设置当前航点。
- 用它自动验证 Phase 1 验收（100 航点上传下载一致；10% 丢包仍可完成）以及 #7 #8 #9 的回归。

### #26 [P2][docs] Documentation and ADR consistency
**状态**：⬜ 未开始
- AGENTS.md 要求全英文，但 `DEVELOPMENT_PLAN.md`（v2.2）为中文，v2.1 为英文，两份并存。
- ADR-006（工作基准 AMSL）与计划 v2.2 ADR 表（"内部统一用椭球高"）矛盾，需统一并改状态。
- 所有 ADR 仍为 Draft；ADR-010 已实现应转 Accepted；ADR-011（链路状态模型与 GCS 心跳）在计划里被引用但 `docs/adr/` 中不存在。
- AGENTS.md 中 `crates/server`、`frontend` 的"尚未搭建"描述已过期；`LICENSE` 仍未决定（ADR-007）。

---

## P3 · 前端与打磨

### #27 [P3][frontend] Show a clear "MOCK DATA" indicator when `isMock` is true
**状态**：⬜ 未开始
- 防止模拟遥测被误认为真实数据。

### #28 [P3][frontend] Telemetry channel will not scale to Phase 4 mag data
**状态**：⬜ 未开始
- 当前 20 Hz 整份 JSON 快照适合现有 6 类消息；磁数据高频曲线需单独的批量/二进制通道（Tauri `Channel`）与 uPlot 环形缓冲。
- 设计在 Phase 4 前定稿，不要沿用 `telemetry` 事件。

### #29 [P3][frontend] Split `MapView.tsx` (~17 KB) before adding survey layers
**状态**：⬜ 未开始
- 先拆成场景初始化 / 图层 / 航点交互 / 相机跟随几个模块。（此文件我只看了结构，未逐行审，拆分方案需你先确认。）

### #30 [P3][build] Trim Cesium assets for packaging
**状态**：⬜ 未开始
- `vite.config.ts` 复制整个 `Workers/ThirdParty/Assets/Widgets`，`chunkSizeWarningLimit` 调到 2 MB；Phase 6 前裁剪默认影像等不用的资源。

### #31 [P3][core] Device hotplug watcher polls every 500 ms and fails hard
**状态**：⬜ 未开始
- 位置：`devices/manager.rs::spawn_watcher`（`?` 提前返回、轮询枚举）。
- 做法：枚举失败不应中止监听；Linux 上后续换 udev 事件；`helpers/udev-installer` 只有 README，实现前先按 ADR-009 写威胁模型和拒绝路径的测试。

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

## 待核实项（先验证再决定是否开 issue）
- `mavlink` crate 0.17 的 UDP 监听是否设置 `SO_REUSEADDR`；`udpin` 的回复地址行为（抓包确认）。
- `DO_PAUSE_CONTINUE` 在 PX4 v1.17 的实际语义。
- `MISSION_COUNT` 的 `mission_type` 等扩展字段默认值，与 QGC 上传的帧对比。
- PX4 对多个 GCS 同时读取任务时的行为（影响 #9 的具体修法）。
