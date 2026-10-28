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

### O6 [P2][frontend] 测试模式:一键静默告警与闪屏
操作员需求(2026-10-20):增加一个**测试模式**按键;该模式下不闪屏、不报警
(抑制告警声音/横幅闪烁/夜航闪灯等视觉与声音效果),用于桌面联调与演示。
设计要点:走 A3 AlertCenter 的统一静音入口(而非各视图散落开关),持久化到
`config.json`,TopBar 常驻 pill + 状态冗余;安全告警(Critical 锁存)是否也
静音需操作员确认——默认建议仅静音 Warn 及以下,Critical 仍横幅但无声。

### O7 [已解决 2026-10-21 代码,待实机复验] [frontend/inspector] Signal Inspector 重联后整段曲线闪动 + 数据稳定等待 2 s
现象(操作员 2026-10-20):Inspector 打开趋势数据时,重联飞控几乎必现一整段
曲线闪动。初步排查(代码阅读,待实测确认)三个叠加原因:
1. 重联使 Rust tap 重建(`attach_inspector`/`inspector_reset_source`),
   `inspector_status.messages` 归零 → 前端 poll 检测到 `messages < lastMsgs`
   即 `buffers.current.clear()` 清空全部采样环(`InspectorApp.tsx` ~L718)——
   曲线先整体消失,随后 `set_message_interval` 重连请求的高速率突发把曲线
   密集回填,每个 rAF 重绘一次,视觉即"整段闪动"。
2. tap 重建 = 新 `TimestampMapper`(signals/tap.rs)基线:重连建立期首包
   `rx − fc` 偏移偏大,新样本 x 轴相对旧环整体平移;旧样本仍持旧基线,
   同一环内两种基线混排(乱序 x),uPlot 绘制异常。
3. trace 重注册使滤波管线重启(fs EMA 重新学习),filtered 曲线跳变。
修复方向(操作员已点名第 3 项):**链接重建后等待 ~2 s(数据稳定)再开始
读入**——在 poll 检测到 stats reset 时清环后进入 "settling" 门闸,丢弃
`Date.now()+2s` 之前的帧(状态栏显示 "settling…");同时门闸结束后再
`inspector_set_traces`,让 mapper 基线与 fs EMA 在门闸内收敛,消除混排。


**实现(2026-10-21,操作员方案)**:检测到 tap 重建(`messages` 计数回落)→ 清空采样环
→ 启动 **2 s settling 门闸**(`SETTLE_MS`,状态栏显示 "settling…")→ 门闸期间
`onMessage` 直接丢弃帧 → 到期后才开始读入。这样突发回填(原因 1)与新旧时间戳
基线混排(原因 2)都落在门闸内,滤波 EMA(原因 3)也在门闸内收敛。代价:每次重联
丢 ≤2 s 数据(重联期间不在测量段)。**实机复验场景**:开滤波 → 重联飞控 → 应看到
短暂 "settling…" 后曲线从稳定数据开始,无晃动段;若极低截止频率滤波仍有短爬坡,
调大 `SETTLE_MS`(单常量)。
### O8 [已解决 2026-10-21] [frontend/harness] Playwright 检查壳自 PJ3 起失效;修复后暴露两类预存在问题
2026-10-20 修复:①强制启动页(PJ3)把所有 Playwright 检查挡在启动面——各检查
脚本现经 `window.__mgProject` 注入一次性项目跳过;②headless SwiftShader 下
Cesium 渲染崩溃页面(A8 已记录的渲染器 artifact)——新增 `?nomap` URL 开关
(App 不挂 MapView,生产不受影响),8 个检查脚本默认带 `?nomap` 运行;
`check-heartbeat` 的 2 条 map-stale 断言在 nomap 下 SKIP(归实机/真 GPU 验证);
`check-teardown` 的 `3 · Waypoints` 选择器是 U17a 前的旧格式(已改 hasText);
`check-safety` 的 `Flight` 标签在 U1 已更名 `Fly`(已改);`check-teardown`
clean-quit 的 confirm 断言与 guard 异步存在竞态(已加 waitUntil)。
**全部解决(2026-10-21)**:
- `IconButton` 加 `shrink-0` + `minWidth`——顶栏窄屏下图标按钮不再被压到 24/28 px;
- 顶栏改为「左侧可收缩包装(`min-w-0 flex-1 overflow-hidden`)+ 右侧 `shrink-0`
  集群」:长项目名先截断(≤1366 时 `max-w-[120px]`),**左侧内容超出时在自身盒内
  裁剪,绝不滑到右侧集群之下**——此前状态药丸会盖住最后一个阶段标签(Fly),
  Playwright 点击被拦截(check-safety/preflight/command-ux 间歇超时的真凶);
- `devices` store 对 `enumerate_devices` 返回 null 做守卫(RtkDrawer 常驻顶栏,
  null → `devices.length` 崩页,整个窗口空白)——连带修了 `RtkDrawer` 的防御读取;
- field-mode 的 44 px 下限改 `!important`(胜过 IconButton 的行内 `minWidth`)。
`check-topbar` 28/28,全部 10 个 Playwright 脚本绿。

### O9 [P2][data] H10 对比验证暂缓(操作员 2026-10-21)
HRDEM×BC LiDAR 重叠区对比(RMSE/偏差/最大差/空洞率)与 WCS 一次性交叉对照
(计划 §6.4 H10)暂不实施——前置依赖 H7 控制点现场验收(实机),且当前 BC-SW
单一来源下对比价值有限。解冻条件:操作员提供第二数据源或完成 H7 现场验收。

### O10 [P3][data] H11 亚米级点云路径暂缓(操作员 2026-10-21)
计划 §6.4 H11(可选):LiDAR 点云自行生成优于 1 m 的 DEM(`dem-prep process`
LAS 路径,点云来源与许可另议)。默认本就不启用;暂缓期间不投入。解冻条件:
操作员明确需要优于 1 m 的分辨率并确认点云密度与许可。

### O11 [P3][terrain] H5 惰性分块读取暂缓(操作员 2026-10-21)
计划 §6.4 H5:COG 按需读 tile + LRU——原为"将来放宽测区面积上限"的前置条件。
当前测区上限 100 m×100 m(HRDEM 总窗口 ≤2 km)下整幅读取无压力,**暂不实施**。
解冻条件:操作员决定放宽测区/AOI 上限。

### O12 [P2][frontend] A11 后续切片暂缓(操作员 2026-10-21)
A11 第一阶段(版本化 KV + 命令 + 广播 + 地图图层开关切片)已实现并 push。
剩余切片(**选中项、相机视角、drape 结果**的跨窗同步)按操作员指示暂不实施。
影响:U15(地图分离)的"选中/相机状态哈希一致"验收项因此待办——U15 目前
只保证图层开关经 shared state 同步;相机/选中在弹出地图后各自独立。
解冻条件:操作员需要 U15 完整跨窗同步时。

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
~~`core::height::load` 已支持任意 .mggeoid；**dem-prep terrain --geoid** 已实现
`H + N(CGG2013) = h` 换算（含 fixture `cgg2013_bc_sw.mggeoid` 与网格覆盖校验，
ADR-004/006）。剩余只差真实数据：等 BC 省发布 CGG2013 网格提取后替换 fixture，
完成 AGENTS.md 的 <0.5 m 控制点两段验收（`H + N(CGG2013) = h`、
`h − N(EGM96) = AMSL`）。~~
**已解决（H7, 2026-10-17）**：真实 CGG2013 格网（NRCan 官方，经 PROJ CDN 镜像
`ca_nrc_CGG2013n83.tif`，OGL-Canada，EPSG:9246 标称 0.03 m）经 `dem-prep geoid`
转换为 `crates/core/testdata/geoid/cgg2013_bc_sw.mggeoid`（出处与许可在文件头），
core 嵌入 `cgg2013_model()`；两段式控制点对 PROJ `cct` 互校通过
（CGG2013 段 ±0.02 m、EGM96 段 ±0.2 m，4 个控制点）；`H≈AMSL` 近似实测偏差
0.52–1.27 m（超出 0.5 m 预算），已替换为真实链条并加 AMSL 导出 fail-closed 门禁。
剩余：AGENTS.md 的 <0.5 m **PX4 实机**验收保留为现场待测项（不得放宽阈值替代）。

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
**L4 断开/退出护栏已完成**：core `LifecyclePolicy::refusal`（纯策略，UI 可先问）+ `StopReason::confirm_keyword()`（`DISCONNECT`/`QUIT`）+ `TeardownGuard::of(policy, reason, vehicle, at_stake)`（拒绝优先，被拒时永不给关键字；关键字只在"允许但有代价"时出现），`StopReason`/`TeardownGuard` 走 ts-rs 导出；app-tauri 的只读命令 `teardown_guard(reason, at_stake)` 与监督器复用同一策略。Disconnect、ShutdownApp、WindowClose 全部拒绝 armed/airborne；native `CloseRequested` prevent-close 后经 `teardown_refused` 通知主界面 banner，解除 close single-flight 锁以便落地解锁后重试；没有自动 RTL/Land/Disarm。未知遥测仍允许退出；OS 强杀不在 GCS 可控范围。前端 in-app 三入口共用确认框，未上传任务需逐字输入关键字。`frontend/scripts/check-teardown.mjs` 覆盖 native refusal banner、未发退出命令与可关闭通知；桌面事件适配需在 Tauri 运行时手动验证。
**L5 强制解锁审计闭环已完成**：强制路径沿用 `evaluate_force_arm`（无链路、已解锁、清单已通过均拒绝），通过后必须先落盘审计才发 Arm：`RecordEvent::ForceArmAudit`（未通过项、固定理由、备注、UTC 微秒）+ `session.json.forced_arm`，两者同一时间戳；recorder 未运行或写盘失败时拒绝发送。tlog 追加同时间戳的 GCS `STATUSTEXT` 标记（`MAGGCS FORCE ARM: <reason> (<n> items)`，系统/组件 ID 取 GCS），回放可见且不会冒充飞控消息；ADR-021 明确这是原始帧规则的唯一合成例外。审计事件/manifest 先于命令且已 fsync，标记随后入队，队列压力下最多丢标记、不丢授权。旧 session 无 `forced_arm` 字段仍可读。验收：core recorder 10 条（含标记可解码与同时间戳、失败不进 manifest、关闭后拒绝）、app 61 条（含无 recorder 时拒绝）、`cargo check`/`fmt`/`clippy` 干净；前端 `check-preflight.mjs` 覆盖关键字、理由与单次命令发送。未做真实 Tauri 窗口/实机验证。
**A6 配置单一事实源已完成**：`AppConfig` 新增 `safety: SafetyConfig`，与端点、自动连接、初始位置、模型姿态、音量同存于版本化 `config.json`；`SafetyConfig::normalized` 修复 stale 500–60000 ms、卫星数 ≤32、电量 0–100 且 critical ≤ warn，坏字段不丢弃其余配置。启动时 `load_at_startup` 加载并缓存到 `AppState`，`config_save` 原子写成功后才更新缓存，`commands::safety_config` 直接读取缓存，不再返回硬编码默认值。ts-rs 绑定与 `frontend/src/generated-types/` 已同步；前端加载时应用、保存时原样带回安全阈值，避免 L7/U5 编辑 UI 上线前被无关设置重置。验收：core config 单测（缺 `safety` 回退、坏值修复）、core 591 通过、app 63 通过（含持久配置阈值测试）、`cargo check --tests` 与 `npm run typecheck` 通过。
**L7 分级健康告警已完成**：core 新增 `safety::health::HealthMonitor`，按每个 telemetry snapshot 评估电池百分比/电压、GNSS fix/卫星数、RSSI、TX buffer 与丢包率；活跃条件只在首次出现或 severity 变化时调用 `AlertCenter::level`，恢复仍走 3 s 迟滞，Unknown/过期字段不告警。丢包率由相邻 `RADIO_STATUS.rxerrors`/`fixed` 差值计算，计数器回绕或电台重启时丢弃一个样本。阈值全部来自 `AppConfig.safety`：`critical_satellites`、`warn/critical_txbuf_percent`、`warn/critical_packet_loss_percent` 有默认值，`warn/critical_battery_voltage_mv` 与 `warn/critical_rssi` 默认 `None`（不同电池/电台不可安全泛化）。`AlertState::apply_health` 一次加锁并批量发布 transition，`telemetry_pump` 在发出 `"telemetry"` 前喂入同一个 monitor；`TopBar.tsx` 电量色阶改为读取同一份 warn/critical 百分比。另修复 `AlertCenter::raise`：Critical 的 `latched` 改为粘性 OR，后续较低 severity 不再解除锁存。验收：health 表驱动单测 8 条（未配置不误报、跨阈值只触发一次、恢复迟滞、Critical 锁存、电压、fix/卫星、RSSI/TX buffer、丢包 delta）+ alerts 锁存回归 1 条 + app 发布链路 1 条；core 599 通过（跳过 1 条需回环端口的 raw-tap 用例）、app 64 通过 + 1 ignored；`clippy -D warnings`/`fmt`、前端 typecheck/build 与全部静态检查通过。未做真实 Tauri 窗口/实机验证；阈值编辑器仍待 U5。
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
**U4 实时高度／速度目标的软件路径已完成（2026-10-27）：** Fly 地图右键菜单
新增 `Change altitude…` 与 `Change speed…`；两者均按 medium 风险使用长按发送。
高度输入为 DTM AGL（1–120 m），core `height::to_px4_amsl` 通过
`H + N(CGG2013) = h`、`h − N(EGM96) = AMSL` 转成 PX4 绝对 AMSL，缺 DEM／
基准覆盖或未解锁时 fail-closed；速度范围 1–15 m/s，实时命令、survey 与 drape
共用 PX4 v1.17 的 `DO_CHANGE_SPEED` 编码（速度在 `param2`）。浏览器验收覆盖真实
Cesium 右键、两个弹框、单击零发送、长按各一条 intent、遥测不覆盖正在编辑的值
及未解锁零写入。**尚未完成：** PX4 SITL／实机飞行验证；不得据此关闭 SW-01
真实航段速度接入或完整 Drape 正确性工作。
**L2 常驻 RTL/Hold 已完成**（commit `891bda3`，本轮补记）：`SafetyCommands.tsx`
把 RTL（medium，`HoldButton`）与 Hold（low，单击）作为常驻按钮放在 TopBar，
位于 L3 置灰分组**之外**——链路可疑时正是要用 RTL，所以它不随遥测置灰；两者
同为唯一不自己在 UI 里判优先级的命令按钮，A7 的 `begin_with_priority` 是唯一
仲裁者（在途 Normal 时 RTL 仍立即上线，被抢占者回 `superseded`）。Hold 用
PX4 `AUTO/LOITER`（main 4 / sub 3），落地后按钮点亮 `data-mode-current`。
验收脚本 `frontend/scripts/check-safety.mjs`（22 条断言，2026-10-15 实测全过）：
任一视图/阶段 RTL 常驻、单击不发、**从 Plan 阶段一次按住即发出（RTL ≤ 2 次
交互，实测 1 次）**、按住时确认条显示 MODE/AGL/BATT、在途命令时下拉被挡而
RTL 仍可发、`superseded` 结果保留到操作员确认、Hold 单击切模式并点亮、心跳
丢失时两者置灰。
**U2 TopBar 信息层级已完成**（本轮）：TopBar 重构为三组——状态（armed / mode /
link）、健康（RTK、battery）、工具（Tools、告警、设置），组间细分隔线；臂章加
`ShieldCheck/ShieldOff`、mode 加 `Compass`、link 加 `Radio`、RTK 加 `Satellite`、
battery 加 `BatteryMedium` 图标（"状态 pill 带图标"）；控件高度统一到 32px
（`CONTROL = 'h-8'`），修掉 `Tools` 的 44px 与其余 32px 的不一致，返回
`IconButton` 由默认 44 改为 `size={32}`；新增 `hooks/useIsNarrow.ts`
（`matchMedia(max-width:1366px)`，可响应 resize）。窄屏（≤1366）时**健康组**
折叠进 `topbar-more` 弹出层，**状态组与 L2 安全对永不折叠**（§7.2 的"关键状态
一眼可读"在宽屏完整满足；窄屏下电量/GNSS 的降级仍由 L7/告警中心兜底，故按
U2 的显式折叠规则处理）。验收脚本 `frontend/scripts/check-topbar.mjs`（28 条
断言，2026-10-15 实测全过）：1920/1280 下状态组完整且在视口内、与安全对不
重叠；1280 下健康组折叠且 More 能展开 RTK/battery；8 个交互控件均 ≥32px。
既有 `check-heartbeat`/`check-alerts`/`check-teardown`/`check-command-ux`
回归全过（`link-stale` data 属性与 `alert-open` 语义不变）。截图
`frontend/screenshots/u2/topbar-{1280,1920}.png`。
下一步 P0 只剩 **U3 告警中心界面**。全部 WP 落地后再按计划并入
`FEATURES.md`/`ARCHITECTURE.md` 并删除本计划文件。
**U3 告警中心界面已完成**（本轮）：新增两个共享组件，每个窗口都挂载——`AlertLayer.tsx`（Critical 顶部锁存横幅 + Warning 不遮挡 toast + **新 Critical 的全屏红色闪光** + 分级告警音 + `aria-live` 播报，并在此处统一挂 `useAlertStream()`）、`AlertCenter.tsx`（抽屉：Alerts/Events/**Console** 三视图，严重度与来源过滤，严重度图标，音量的**限时静音**——默认 5 分钟后自动恢复）。
- **全屏红光**（操作员明确要求）：`stores/alerts` 新增可见告警的"新增检测"（忽略首个快照，所以窗口重载时锁存的 Critical 不会重新闪光），新 Critical 触发 `AlertFlash`——约 1 s 的 3 次脉冲（`index.css` 的 `@keyframes mg-alert-flash`，用 `--mg-error` 令牌而非硬编码红），**尊重 `prefers-reduced-motion`（关闭动画）**；静音只静音声音，不抑制视觉闪光（脚本已验证）。
- 事件流从 `desktop/bridge.ts` 移到 `stores/alerts::useAlertStream()`，因此 Inspector/Actuator/Params 三个子窗口只挂 `AlertLayer` 就能收到 `alert` + `statustext`；`ErrorBanner.tsx` 只保留链路 I/O 错误（Critical 横幅移入 AlertLayer）；`HeartbeatBanner` 在 Critical 横幅存在时下移（`top-16`）避免遮挡；`EventLogDrawer.tsx` 删除（其内容并入 `AlertCenter.tsx`）。
- 告警音：`audio/alarm.ts` 增加 `alertBeep(severity, volume)`（Critical 双高音、Warn 单低音、Info 静音）；`stores/alertSound.ts` 保存音量与 `mutedUntil`。
- 旁路修复：`check-mission-sync`/`check-planfile` 的 tauri stub 补 `listen` 导出（`alerts.ts` 现在静态 import `@tauri-apps/api/event`）。
- 验收：`frontend/scripts/check-alert-ux.mjs`（**24 条断言，2026-10-15 实测全过**）：安静时无闪光、新 Critical 触发闪光且覆盖整个 1440×900 视口、闪光自行结束、Critical 横幅锁存且 `aria-live` 播报、Warn 只出 toast 且不闪光、Info 不出 toast、toast 可关闭不 ack、抽屉按严重度/来源过滤、Console 显示 FC STATUSTEXT、静音/音量可用、静音期间 Critical 仍闪光、`reduced-motion` 下不闪光。既有 6 个 E2E（safety/topbar/command-ux/teardown/alerts/heartbeat）与全部静态检查（typecheck/build/colors/contrast/i18n/planfile/mission-sync/coords/agl/progress/polygon）回归全过。截图 `frontend/screenshots/u3/`。
至此计划 §2 的 **P0 全部完成**（L1–L4、A1、A3、H0/W0 由后续阶段处理，U2/U3/U4）。全部 WP 落地后再按计划并入 `FEATURES.md`/`ARCHITECTURE.md` 并删除本计划文件。
**A6 Rust 侧配置存储已完成**（P1，本轮）：core 新增 `config` 模块（`AppConfig`：`version`/`endpoint`/`auto_connect`/`initial_position`(复用 `GeoPoint`，不裸 `f64`)/`model_orientation`/`alert_volume`，`ModelOrientation` 走 ts-rs 导出为 `crates/core/bindings/AppConfig.ts`+`ModelOrientation.ts`，已复制到 `frontend/src/generated-types/`）。解析策略：容器级 `#[serde(default)]` 让缺字段回落到 `AppConfig::default()`，未知字段忽略；`version` 缺失按 0 迁移、**比当前新则拒绝**（`UnsupportedVersion`）；坏值（越界位置/非有限音量）就地修复而非整份丢弃，`endpoint` 空白回落默认。app-tauri 新增 `config_service.rs`：`config_load`（缺失/损坏文件返回 `None` 并 `eprintln!` Warn，绝不崩溃）、`config_save`（写入前 `normalized()`，临时文件 + `rename` 原子落盘），文件在 `app_config_dir/config.json`，已注册到 `invoke_handler`。前端新增 `desktop/appConfig.ts`：主窗口启动时 `initAppConfig(true)` 先 `config_load`，**首次启动无文件时把 localStorage 引导值写回文件做迁移**，随后订阅 `prefs`/`alertSound` store 防抖 300 ms 写穿 `config_save`；三个子窗口 `initAppConfig()` 只读水合（拿到告警音量）不写文件，避免与主窗口抢写。四个入口（`main.tsx`/`inspector`/`actuator`/`params`）均在首帧前 `await` 水合。非 Tauri（浏览器/dev）下 `initAppConfig` 直接返回，localStorage 行为不变。旁路修复：`check-mission-sync`/`check-planfile` 的 tauri stub 补 `listen`。验收：core `config` 单测 9 条（默认值、往返、缺字段、未知字段、v0 迁移、拒绝新版本、坏 JSON、坏值修复、空 endpoint）+ app `config_service` 单测 4 条（往返、缺文件、损坏文件回退且默认可用、保存时归一化）；`typecheck`/`build` 通过；`check-topbar`/`check-alert-ux`/`check-safety`/`colors`/`i18n` 回归全过。
**H0 HRDEM 调研与决策已完成（ADR-018，本轮）**：新增 `docs/adr/018-hrdem.md`（状态 **Accepted**，操作员已确认 AOI 与 §D 空变换决策）。要点：数据入口 NRCan `hrdem-lidar` STAC 集合（`https://datacube.services.geo.ca/stac/api`，许可 **OGL-Canada-2.0**，垂直基准 **CGVD2013/EPSG:6647**，分发为 **EPSG:3979** NAD83(CSRS)/Canada Atlas Lambert；集合注明项目间**无接边**，故 AOI 跨项目一律报警不拼接）；维多利亚测区仅 1 个 item 覆盖——**`BC-Vancouver_Island_Sunshine_Coast_2018-1m`**（datetime 2019-10-01，DTM+DSM 资产在 S3 上 HTTPS **免认证**，`HEAD`/`Range` 200/206 已验证）；COG 为 BigTIFF、512×512 分块、Float32、LZW、**nodata −32767**、8 级 overview；垂直精度引 HRDEM Product Specification（Ed 1.7, 2026-05-19）"一般优于 1 m"（按项目精度在项目 File Geodatabase，未随 STAC 提供）。**NAD83(CSRS)↔WGS84 水平处理决定（§D）**：取**空变换**（视 NAD83(CSRS)≡WGS84），依据 EPSG:1842（null/copy，"for many purposes … coincident"）、EPSG:4617（≈1 m 精度）、EPSG:1946（7 参数，操作精度亦仅 1.0 m 且基于 1997 历元），残留约 1 m 记录在案，交由 M9/H7 控制点核对（< 0.5 m）暴露系统性偏差，届时回退 EPSG:1946；不手写基准变换。**夹具（§H0(e)）**：`testdata/hrdem/victoria-aoi-{dtm,dsm}.tif`（AOI 中心 48.55606,−123.38136；140×140＝100 m 核心＋20 m 缓冲，EPSG:3979，Float32，nodata −32767，DEFLATE 分块 COG，**与源 COG 逐像元精确一致**，5×5 抽样比对相同），随附 `provenance.json`（item/采集日期/资产 href/窗口/sha256/许可）与 `ATTRIBUTION.md`（OGL 署名）；总 130 317 B ≪ 2 MB。`.gitignore` 加 `!testdata/hrdem/*.tif` 例外以便提交夹具。来源与抓取日期（2026-10-15）均记入 ADR。
**W0/W1 步行磁测仪器接入已完成（ADR-020，本轮）**：新增 `docs/adr/020-walk-survey.md`（**Accepted**）并加补遗——步行磁测**与 UAV 磁测共用同一套传感器与数据流**（同一 PX4、同一 DroneCAN 磁力仪、同一 ZED-F9P RTK、同一 MAVLink 消息集与记录栈），**唯一区别是手持而非飞行**；步行会话该 PX4 不飞行，只作仪器中枢，仪器数据全部走 MAVLink，GCS 不接串口、不解析厂商帧、不配置 DroneCAN 节点、不发飞行命令；主记录仍在 PX4 SD/ULog，GCS 交付 CSV。**W1 core `instruments`**：`sample.rs`（`InstrumentKind`/`MagInstance`(默认 `HIGHRES_IMU` sensor 1)/`MagSample`(nT)/`GnssSample`(带 `height::Height`)/`InstrumentSample`）、`decode.rs`（复用 `signals::extract`+`normalize_mag_fields`：`HIGHRES_IMU` gauss、`RAW_IMU` milligauss → nT；GNSS 复用 `telemetry::GpsRawInt`；`InstrumentError` thiserror）、`source.rs`（`InstrumentSource` + 磁/GNSS 源 + `default_sources`）、`monitor.rs`（`StreamMonitor`/`MonitorConfig`/`StreamHealth`/`StreamEvent`：磁/GNSS 时效、磁速率带、GNSS fix 降级、时钟跳变，边沿触发，`to_alert()` 复用 A3 并带稳定 key）、`sim.rs`（确定性 LCG 合成 MAVLink 流，`HIGHRES_IMU`/`GPS_RAW_INT` 信封，默认维多利亚 AOI 位置但**不内嵌任何区域地磁数据**）、`live.rs`（`route()`=105/27/24/33，`run_instruments` 仿 `run_tap`，`InstrumentEvent`）。**app-tauri 适配**：新增 `instruments_service.rs`（`InstrumentsState`：`attach`/`connect`/`disconnect`/`status`，`forward_loop` 推送事件到窗口 channel；`instruments_connect`/`instruments_disconnect`/`instruments_status` 已注册 `invoke_handler`），连接时按窗口注册情况 attach、断开时 detach。ts-rs 导出 `MagSample`/`GnssSample`/`MagInstance`/`InstrumentKind`/`InstrumentSample`/`StreamEvent`/`StreamHealth` 至 `crates/core/bindings/` 并复制到 `frontend/src/generated-types/`。验收：core `instruments` 单测 31 条（合成 MAVLink 流端到端解码、磁中断/速率偏离/GNSS 降级告警、≥2 s 时钟跳变、gauss/milligauss→nT 与 `signals` 参考一致、同种子可复现、runner 停止/忽略非本类帧）+ app 单测 2 条；`cargo test -p maggcs-core` 全 **518 通过**、`clippy`/`fmt` 干净、`npm run typecheck` 通过。操作员已确认磁数据取 **`HIGHRES_IMU`（PX4 已校准）** sensor 1、**≈100 Hz**（`MonitorConfig` 默认速率带按其半/双倍，新增常量 `EXPECTED_MAG_RATE_HZ=100.0`）；仅剩具体节点型号与杆高待项目/实机输入（ADR-020 Open items）。
**A4 飞行记录器已完成**（◆ P1，本轮，ADR-021）：**原始帧旁路（方案 B）**——`mavlink::connection` 的工作线程由 `recv()` 改为 `recv_raw()` + `<MavMessage as Message>::parse`，从而同时拿到**逐字节线缆帧**与解码结果；新增 `ConnectionHandle::subscribe_raw()` 与 `RawFrame{header, bytes, received_at, utc_us}`，**仅在 `receiver_count() > 0` 时复制/投递**（不录制时零额外开销）；CRC 合法但方言无法表示的帧（PX4 v1.17 越界枚举）仍按"跳过一次"处理，与旧行为一致但路径显式。**core `recorder`**：`tlog.rs`（QGC 对齐格式 `[u64 µs][原始 MAVLink 帧]`，读端按自定界重新分帧，截断尾记录忽略）、`events.rs`（`RecordEvent` schema：session/alerts/command/mode/mission/force-arm/line start-end/marker/quality/note + JSONL，截断尾行忽略）、`session.rs`（`SessionManifest` 临时文件 + rename 原子写）、`service.rs`（`Recorder::spawn`；`Mutex`+`Condvar` 有界队列，**满则丢最旧并计数，push 从不阻塞**；独立 `maggcs-recorder` 线程 + `BufWriter`，每 500 ms flush、停止时 flush+`fsync`；`RecorderStats`；`pump_raw`；`impl ManagedService for Stage::Recorder`）。**app-tauri**：新增 `recorder_service.rs`（`RecorderState`：`start`/`stop`/`retire`/`record_event`/`status`；每个链路一个 session；`pump_raw` 与告警镜像任务共享一个 `watch` 停止开关；链路切换时 `retire` 后台 flush 旧 session，**不阻塞 connect**；录制失败只发一条 Warn 告警、不影响连接），`lifecycle_service.rs` 新增 `RecorderStage` 并注册（顺序在 MissionAndCommand 之后、Link 之前），命令 `recorder_status`/`recorder_log_event` 入 `invoke_handler`。**tlog 格式决定（`docs/adr/021-recorder.md`，并解决 §9 第 6 项）**：QGC 文档格式为"8 字节自 Unix 纪元起微秒时间戳（`uint64_t`，`memcpy` 写入=主机字节序，目标平台恒为小端）+ 原始 MAVLink 包（含起始符）"，故写**小端 u64**；读端重新分帧以兼容 QGC 写出的文件（大端主机需复核）。验收：core `recorder` 单测 7 条（tlog 逐字节往返、截断尾忽略、JSONL 全变体往返/截断尾忽略、队列丢最旧且 push 不阻塞、写盘往返 + manifest 计数）+ 连接层原始旁路集成测 1 条（UDP 回环断言旁路字节与发送字节完全一致）；`cargo test -p maggcs-core --lib` **526 通过**、app 47 通过、`clippy`/`fmt` 干净、`cargo check --workspace --all-targets` 与 `npm run typecheck` 通过。**未做**：命令/模式/任务同步/航线起止事件源随各自 WP（M1/W4/L5）接入，届时调用 `Recorder::record_event`；当前可由 `recorder_log_event` 手动落盘。
**L6 补充遥测已完成**（P1，本轮）：core `telemetry` 新增 6 组类型与解码——`ExtendedSysState`（`LandedState`/`VtolState`，供 L5 的 on-ground 与 M1 的空中判定）、`HomePosition`（复用 `height::Height`，AMSL）、`EstimatorStatus`+`EstimatorStatusFlags`（EKF 1-σ 精度与 12 个有效位，`raw` 位掩码保留未知位）、`Vibration`、`RcChannels`（按 `chancount` 截断，**不补零**）、`RadioStatus`（`0xFF` RSSI/noise 哨兵 → `None`）。`TelemetryUpdate`/`TelemetryAggregator`/`TelemetrySnapshot`/`TelemetryFieldAges` 同步扩展；**未收到的字段一律 `None`（含各自 `*_at_ms`），绝不为 0/false**（验收要求）。**连接后请求速率**：core 导出 `SUPPLEMENTARY_STREAM_RATES`（`StreamRateRequest` 表，消息 id 用命名常量：`EXTENDED_SYS_STATE` 2 Hz、`ESTIMATOR_STATUS` 2 Hz、`VIBRATION` 2 Hz、`RC_CHANNELS` 5 Hz、`HOME_POSITION` 1 Hz、`RADIO_STATUS` 1 Hz）；app-tauri `connect` 在链路就绪后经 `ConnectionHandle::send` **fire-and-forget** 发出 `MAV_CMD_SET_MESSAGE_INTERVAL`（走解析出的 target sys/comp，重启即恢复 PX4 默认，不写持久配置）。ts-rs 导出 `LandedState`/`VtolState`/`ExtendedSysState`/`HomePosition`/`EstimatorStatus`/`EstimatorStatusFlags`/`Vibration`/`RcChannels`/`RadioStatus` 并更新 `TelemetrySnapshot`/`TelemetryFieldAges`，已复制到 `frontend/src/generated-types/`，`frontend/src/telemetry/mock.ts` 补新字段（`in_air`）。验收：core `telemetry` 单测 9 条（六类帧解码、flags 位、`chancount` 截断与 `0xFF`→`None`、缺帧为 `None`、速率表良构）+ hub 端到端 1 条（合成 FC 帧经 pump 落入快照、未到达字段保持 `None`）；`cargo test --all-targets` 全绿、`clippy -D warnings`/`fmt` 干净、`npm run typecheck`+全部静态检查（colors/contrast/i18n/planfile/mission-sync/coords/agl/progress/polygon）通过。

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

### N9 [P1][motor-test/signals] `SCALED_IMU{,2,3}` 磁场曲线隔离 — 已完成（2026-10-28）
全链路以 `(system_id, component_id, message_id)` 区分磁传感器来源：修正 26／116／129
消息常量、mG→nT 换算和逐来源时间戳 mapper；Motor Test 使用
`mag.<highres_imu|scaled_imu1|scaled_imu2|scaled_imu3>.{raw,cal}_total` 独立 ring，且只有
`HIGHRES_IMU` 驱动报警／状态读数；Inspector 的三轴累加器、订阅和派生 trace ID
同样按完整来源隔离。ULog `sensor_mag` 多实例现在显示为 **instance 1／2／3…**，每个
实例有独立合成 message ID、nT 规格化的 `xmag/ymag/zmag` 与各自 `mag_total`，图例／
trace chip 保留实例名称。Rust 与纯 TS 回归均交错输入三组轴并只改变 IMU3，验证
IMU1／2 样本和派生 ring 不变。

### N10 [P1][inspector] ULog 选择信号后仍固定为 30 s，且缺少双轴缩放
载入 ULog 并选择信号后，应立即绘制该信号的完整记录范围，而不是沿用 live
模式的 30 s 滚动窗；增加明确的横向时间缩放与纵向数值缩放，并保留恢复全图
操作及可测的轴范围。

### N11 [P1][runtime/frontend] Reload 必须是窗口局部操作
在一个窗口右键 Reload 只应重载该 webview、释放该窗口自己的订阅；不得重置
另一个 Signal Inspector 的已载入 ULog、选择、缩放或 Runtime 会话。增加双窗口
隔离回归，覆盖 Inspector 活跃时重载主窗口及另一个工具窗口。

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
