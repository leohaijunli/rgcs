# MagGCS (rgcs-main) Issue 清单

遗留问题与待办（2026-10-10 重组）：已完成的问题压缩为索引（git 历史含全文），**未完成项在下方全文列出**。


## Resolved issues（已完成，共 46 条 — 全文见 git 历史）

### 进度总览（P0）
### #1 [P0][core] Heartbeat timeout is reset by any inbound traffic
### #2 [P0][core/app] Connected event lost: hub subscribes after the receiver is dropped
### #3 [P0][core] GCS never sends HEARTBEAT
### #4 [P0][app] `connect` returns success even when binding fails
### #5 [P0][core/app/frontend] Flight commands are fire-and-forget (no COMMAND_ACK)
### #6 [P0][core] Telemetry aggregation has no source filtering
### #7 [P0][core] Mission protocol retransmission timer is never wired
### #8 [P0][core] Upload: duplicate MISSION_REQUEST treated as failure
### #9 [P0][core/app] Download accepts MISSION_ITEM_INT addressed to other GCSs
### #10 [P1][core] `MissionItem` model is redundant and lossy
### #11 [P1][core] `MissionFrame` does not round-trip
### #12 [P1][frontend] `.plan` import crashes on ComplexItem
### #13 [P1][frontend] `.plan` export is not faithful
### #14 [P1][frontend] Add-waypoint button inserts at ~(0°, 0°) when nothing is selected
### #15 [P1][frontend/app] Mission sync state is unmodelled; no read-back verification
### #16 [P1][core] Per-field data age is not tracked
### #17 [P1][core] Error events delivered through `watch<Option<String>>` can be lost
### #18 [P2][arch] Move `MissionService` from app-tauri into `core`
### #19 [P2][arch] Replace polling pump with event-driven hub
### #20 [P2][arch] Introduce message routing in the connection layer
### #21 [P2][arch] Make app state multi-vehicle ready and `connect` atomic
### #22 [P2][security] Tighten Tauri configuration
### #23 [P2][ci] ts-rs binding sync check can never fail
### #24 [P2][ci] Workflow inconsistencies
### #25 [P2][test] Add a lossy fake-FC test harness for the mission protocol
### #26 [P2][docs] Documentation and ADR consistency
### #27 [P3][frontend] Show a clear "MOCK DATA" indicator when `isMock` is true
### #28 [P3][frontend] Telemetry channel will not scale to Phase 4 mag data
### #29 [P3][frontend] Split `MapView.tsx` (~17 KB) before adding survey layers
### #30 [P3][build] Trim Cesium assets for packaging
### #31 [P3][core] Device hotplug watcher polls every 500 ms and fails hard
### #32 [P3][frontend] UAV marker: 3D model, MAVLink-driven attitude, forward-only prediction
### #33 [P1][frontend] Upload keeps saying "Unsaved changes — not the FC plan"
### #34 [P0][core/frontend] Upload fails with `mission ack denied (type 3)` on a plan with a DO item
### #35 [P2][frontend] Tie lines look like spacing lines; the Layers panel is decorative
### #36 [P2][frontend] No height visualization per waypoint
### #37 [P1][frontend] The waypoint list says nothing while flying
### #38 [P1][frontend] Upload reports "FC mission differs from the uploaded plan" for a plan the FC holds
### #39 [P2][frontend] Selecting a waypoint does not show it on the map; height sticks float
### #40 [P2][app/devices] 串口列表不自动刷新
### #41 [P2][app/devices] 串口列表应只显示 ttyACM* / ttyUSB*
### #42 [P1][helper] udev 规则/权限自动安装助手未实现
### #43 [P2][frontend] 设置菜单应提供 UAV 3D 模型朝向的设置（pitch/roll/yaw 偏移）
### O1 [P2][frontend] Logs 页是占位
### O5 [P3][frontend] Data 视图移除后的遗留键
### N1 [P2][frontend] Drape 前的地形数据发现

## Open issues（未完成）

### O2 [P3][actuator] 报警音可选自定义 wav/mp3 文件路径
当前为 WebAudio 合成蜂鸣（内置，无依赖）。plan §9.1 里的"可选本机音频文件"
设置尚未做。

### O3 [P3][actuator] 曲线扩展
Live 曲线目前为 总场(Δmag) + ESC 电流/RPM。plan §6.2 的另外两组（指令 +
姿态；磁场偏移基准曲线组）与显示/隐藏单条曲线的开关未做。

### O4 [P2][app] 双链路转发仅限 Motor 测试窗口
SITL 副链路（`actuator_connect_sitl`）挂在 MotorTestState 上；若未来
Headless/主界面也要用副链路，需要把 SITL 连接提升到 AppState 通用层。

## 待核实项（先验证再决定是否开 issue）

- `mavlink` crate 0.17 的 UDP 监听是否设置 `SO_REUSEADDR`；`udpin` 的回复地址行为（抓包确认）。
- `MISSION_COUNT` 的 `mission_type` 等扩展字段默认值，与 QGC 上传的帧对比。
- PX4 对多个 GCS 同时读取任务时的行为（影响 #9 的具体修法）。
- `DO_PAUSE_CONTINUE` 在 PX4 v1.17 不支持（已实测 NACK）——已用 NAV_TAKEOFF/LAND 替代，关闭。
- PX4 v1.17 `MAV_FRAME_GLOBAL_TERRAIN_ALT` + `terrain_estimation` 的确切行为：
  无雷达时 mission 是否拒飞、雷达融合参数链（`SENS_EN_RANGEFINDER`、
  `EKF2_RNG_CTRL`）的实际效果（SITL 需模拟 DISTANCE_SENSOR）。决定是否
  值得评估飞控侧跟踪；GCS 预计算路线（ADR-005/013）不依赖此项。

## New open issues（2026-10-11 综合方案执行后新增）

### N2 [P2][height] CGG2013 (HT2) 网格加载
`core::height::load` 已支持任意 .mggeoid；**dem-prep terrain --geoid** 已实现
`H + N(CGG2013) = h` 换算（含 fixture `cgg2013_bc_sw.mggeoid` 与网格覆盖校验，
ADR-004/006）。剩余只差真实数据：等 BC 省发布 CGG2013 网格提取后替换 fixture，
完成 AGENTS.md 的 <0.5 m 控制点两段验收（`H + N(CGG2013) = h`、
`h − N(EGM96) = AMSL`）。

### N3 [P3][frontend] 磁测记录的 GCS 侧分段落地
survey 编译器已输出 PatternLine 表（S1），`MagTrigger::GcsSide` 是默认；
飞行时按 MISSION_CURRENT + 线表自动开关磁数据正式段的 UI/存储标记未做
（数据链与 QC 工具按同一张表切分）。

### N4 [P3][core/rtk] 多注入源检测 — 已完成（39d8fff，2026-10-11）

### R1 [P2][frontend/core] 雷达测距（DISTANCE_SENSOR）实时 AGL 显示 + 低空告警
飞控带测距仪时机载实测离地高度（`DISTANCE_SENSOR` 消息）：TopBar pill 或
飞行 HUD 显示实际 AGL，与计划 AGL 偏差大/低于阈值时告警（同 mag 报警的
阈值/去抖/迟滞模式）。**只做观测与告警，不进控制回路**（ADR-005）。
无硬件可开发：SITL 用模拟 DISTANCE_SENSOR 验证；PX4 terrain frame 行为
另见待核实项。

### R2 [P2][qc] 飞行后雷达 AGL vs drape 计划 AGL 对比曲线
磁测 QC 延伸：ULog 展开 `DISTANCE_SENSOR`，与 drape 计划的 40 m AGL 线逐点
对比（偏差带 + 报告），量化 drape 质量与离地一致性（磁测数据质量直接
相关）。依赖 ULog 读取器扩展（A6 同类）；实机验收需要雷达硬件。

### N5 [P3][docs/plans] 综合方案 v3
v3 已落盘为 `docs/plans/lifecycle-survey-hrdem-plan.md`（v2
`comprehensive-implementation-plan.md` 待删除）。执行进度：**A3 AlertCenter
已完成**（core `alerts` + app-tauri 适配 + 前端 store/badge/banner/drawer，
`frontend/scripts/check-alerts.mjs` 为验收脚本）；**A1 生命周期监督器已完成**
（core `lifecycle` + 单测/假飞控集成测，app-tauri `lifecycle_service` 适配，
`disconnect`/`shutdown_app`/主窗口 `CloseRequested` 三条路径统一走它；L4 的
关键字确认 UI 与分级告警留待后续 WP）。**L1 STATUSTEXT 已完成**：core
`telemetry::statustext`（MAV_SEVERITY 映射、分片拼接、`temporarily_rejected`
与随后 STATUSTEXT 的关联）+ hub `subscribe_status_text` 通道 + app-tauri 转
AlertCenter/`statustext` 事件 + 前端事件日志 `fc` 行；顺带开启 mavlink
`emit-extensions`（分片字段 `id`/`chunk_seq` 属 MAVLink 2 扩展，此前被生成
代码丢弃），全仓库 29 处消息字面量补 `..Default::default()`——编码端
`remove_trailing_zeroes` 保证零值扩展不上线，发送字节不变。
**L3 心跳丢失处理已完成**：前端 `HeartbeatBanner`（丢失时长计时、最后位置/
高度/电量、告警音 + 静音开关、`link_state === 'connected' && !fc_alive` 才
亮，正常断开不误报）、`FlightMenu` 置灰并就地说明原因（不再整体消失）、
`link` store 的 lost→alive 边沿时间戳 + `mission.linkRestored()`（重连后重读
任务状态；模式/解锁状态随心跳自动恢复）、TopBar 遥测组置灰并标出样本年龄、
HUD "STALE n s · 最后时刻"、地图飞机标 `UAV · OLD DATA`（Cesium 实体标签 +
右下角角标）。验收脚本 `frontend/scripts/check-heartbeat.mjs`（26 条断言：
3 s 超时前不亮横幅、时长递增、Flight 菜单不可用、遥测置灰、重连提示与
`mission_download` 重读）。
**L4 断开/退出护栏已完成**：core `LifecyclePolicy::refusal`（纯策略，UI 可先问）+ `StopReason::confirm_keyword()`（`DISCONNECT`/`QUIT`）+ 新增 `TeardownGuard::of(policy, reason, vehicle, at_stake)`（拒绝优先，被拒时永不给出关键字；关键字只在"允许但有代价"时出现），`StopReason`/`TeardownGuard` 走 ts-rs 导出；app-tauri 新增只读命令 `teardown_guard(reason, at_stake)`（复用同一个 `lifecycle_service::policy()`，UI 与监督器不会各写一份规则），关窗强制退出（`StopReason::WindowClose`）在被拒情形下补一条 Warn 告警，强制退出不再静默；前端 `stores/teardown.ts` + `dialogs/TeardownDialog.tsx` 把三个入口（LinkPopover 断开、Settings 断开、Settings 退出）统一到一个对话框：已解锁/飞行中显示拒绝原因与出路且无确认按钮，有未上传任务时显示"n waypoints are not on the flight controller yet"并要求逐字输入关键字（`teardown_guard` 收到 `atStake: true`），干净退出只需一次确认。验收脚本 `frontend/scripts/check-teardown.mjs`（20 条断言，含真实在 Plan 面板输入坐标造出未上传任务）。核对（读码，非自动测）：主窗口 `CloseRequested` 走 `Lifecycle::shutdown(WindowClose)` 后 `app.exit(0)`，**子窗口仍开也会退出**（A1 既有行为）；该路径需要 Tauri 运行时，未进自动测试。
**A7 命令优先通道已完成**：core `commands` 新增 `Priority{Normal,Emergency}` 与 `command_priority(MavCmd)`（RTL / LAND / ARM_DISARM 为 Emergency），`CommandSession` 的 AwaitAck 状态记录在途优先级；`begin_with_priority` 只允许 Emergency 抢占 Normal（被抢占者回 `CommandError::Superseded`，且**先**于新的 `Sent` 上报），Emergency 之间仍 `Busy`，**同时最多一条在途命令**的约束不变；`CommandService::send/send_await` 自动按 `command_priority` 取优先级（调用点无需也不该自行判断），另留 `send_with_priority`。app 层把 `Superseded` 作为 verdict 上报（`result: "superseded"`），前端 `FlightMenu` 与 i18n 显示"Superseded by an emergency command"。验收：core 单测 6 条（分类、抢占、不许抢占、Emergency 不被抢占、被抢占者停止重传且迟到 ACK 被忽略）+ 假飞控集成测 `an_emergency_command_preempts_an_in_flight_goto`（在途 goto 时发 RTL：RTL 立即上线并 ACK；goto 收到 Superseded；RTL 丢 ACK 重传带 confirmation=1 且 goto 不会重新上线；迟到的 goto ACK 无副作用）。
**U4 命令交互分级已完成**：设计系统新增 `HoldButton`（按住 `HOLD_MS = 1 s`
时按钮外圈自绘进度环，按住期间按钮文字换成"将要执行的内容"，提前松开/Esc/
pointercancel 一律不发送；**按下超过 1 s 才松开也算完成**——渲染被地图挤住时
`setTimeout` 可能迟到数秒，早于计时器到达的"松手"不应吞掉一次真正的长按）、
`ConfirmBar`（模式 / 相对高度 / 电池三格，可带一条 warn 备注）与
`CommandStatusStrip`（已发送 → 等待 ACK → 结果 + 耗时）；新增
`util/commandRisk.ts` 把风险分级集中成一张表（`takeoff/land/rtl/arm_disarm`
= medium，`set_mode` = low，`modeRisk(option.confirm)` 让 core 的 `confirm`
标记等价于 medium；高风险（强制解锁 L5、退出/断开 L4）仍是关键字层）。
`stores/command.ts` 记录 `sentAtMs`/`lastElapsedMs` 并导出 `useCommandProgress()`
（在途时按 200 ms 刷新耗时，落地后冻结在 ACK 时刻的数值）；`FlightMenu` 用
`HoldButton` 替换"5 秒内点两次"、弹出层顶部加 `ConfirmContextBar`（AGL 走
`cesium/heights.ts` 的同一套地面参考：DEM/3D mesh 优先，其次 HOME 平面）、
导航条上常驻命令状态条；`ModePopover` 把 core 标记 `confirm` 的 Return/Land
改成按住发送，低风险模式仍是单击。验收脚本
`frontend/scripts/check-command-ux.mjs`（30 条断言，假飞控 + `command` 事件
驱动）：四个飞行命令逐个单击都不发送、未满窗口的按住不发送、满窗口按住才
发送且只发一条、状态条三个阶段与耗时、**RTL 从飞行视图 2 次交互发出**、
键盘按住 Enter 同样可发（且自动重复不会发第二条）、模式弹层里 Hold 单击即
切 / Return 单击无效而按住才切。排查途中定位到一个通用陷阱见 N8。
下一步按 P0 顺序推进 L2（依赖 A7/U4）与 U2、U3。全部 WP 落地后再按计划并入
`FEATURES.md`/`ARCHITECTURE.md` 并删除本计划文件。

### N6 [P1][frontend] `devices` store 信任后端返回，null 会把整个外壳拆掉
`stores/devices.ts` 的 `load()` 直接把 `enumerate_devices` 的返回值塞进
store，`RtkDrawer` 随后读 `devices.length`。任何返回 null 的情形（mock/旧
后端/命令失败）都会抛错，被挂在下方的 `ErrorBoundary` 兜住——本次写
`check-alerts.mjs` 时实测：TopBar/告警角标/横幅一起消失。建议 `load()` 做
`Array.isArray` 兜底并在失败时发一条 Alert（A3）。已用 mock 返回 `[]` 绕过，
未改动业务代码。

### N8 [P1][frontend] 弹层的"点击外部关闭"会被按下瞬间改动的 DOM 骗到
`ModePopover`/`FlightMenu`/`ToolsPopover` 的关闭逻辑都是
`window` 上的 `pointerdown` + `rootRef.current.contains(e.target)`。React 的事件
监听挂在 `#root`（冒泡阶段早于 `window`），所以处理顺序是：目标的
`onPointerDown` 先改 DOM → 再轮到 `window` 上的这个判断。写 U4 的
`HoldButton` 时踩到：按钮在 pointerdown 里把子节点从 Fragment 换成字符串，
React 把原来那两个 `<span>` 摘了下来，`e.target` 于是成了**已脱离文档**的节点，
`contains()` 返回 false，弹层把自己关了——表现为"按住模式条目时弹层消失"，
且被 `hidden` 类切换的写法掩盖（现在 HoldButton 的 label 与 hint 都常驻、只用
`hidden` 切换）。建议把这几个判断改成捕获阶段监听
（`addEventListener('pointerdown', onDown, true)`）或改判
`e.target.isConnected === false ? 忽略 : contains(...)`，否则任何"按下即改变
自身 DOM"的内容都会复现。

### N7 [P2][core/test] `connection_integration::gcs_heartbeat_is_announced_automatically` 在满负载下偶发失败
`cargo test --all-targets` 并行跑全部 target 时该用例 3 s 收不到 GCS
heartbeat 而 panic；单独跑（或重跑）稳定通过。属既有计时/负载敏感用例，
非本次改动引入。

## Deferred（操作员指示暂缓）

### D1 GoPro 视频监视（motor-test-plan §14 G1–G4）
HERO8/9（OpenGoPro 1.0）client 模式，mDNS 发现，HTTP 控制（录制
start/stop），RTP→ffmpeg→HLS 预览，视频卡无流不渲染。**操作员指示：
需要时再启动。**

### D2 M0 台架实测清单（motor-test-plan §11，需要硬件）
- FC 端 `COM_MOT_TEST_EN=1`。
- 验证 `ACTUATOR_TEST` 语义（超时/是否要求未解锁/停转动作）与
  `ESC_STATUS` 推送速率。
- SITL 转发端到端：SITL 飞任务 → Realtime forward → 真机电机随动。
- 报警参数（阈值/去抖/迟滞/冷却/α）按台架数据标定。

### D3 操作员验证清单（软件已就绪，待确认）
- 飞行状态着色（重启应用后确认）。
- 模型朝向设置（Settings → Vehicle）与串口热插拔 + 权限修复按钮。
- 三源图层（已确认 ✓）。
