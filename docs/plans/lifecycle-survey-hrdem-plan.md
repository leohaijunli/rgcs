# MagGCS 实施方案 v3:全生命周期安全 · 架构 · 磁测(含 VIC 日变改正)· HRDEM · UI/UX · 项目管理 · 步行磁测

基线:`rgcs-main__1_.zip`(静态阅读,未编译、未运行)。放置位置:`docs/plans/lifecycle-survey-hrdem-plan.md`;落地后按仓库惯例并入 `FEATURES.md` / `ARCHITECTURE.md` / `issues.md` 并删除本文件。标 *verify* 的事实来自推断或记忆,执行前必须实测或查证,不得当作已知。

## v3 相对 v2 的变化

- 新增 **A11 + U15**:主地图可分离为独立窗口(默认仍内嵌;弹出后主窗口切换为无地图布局;地图窗口崩溃不影响 RTL/Hold 与告警)。
- 新增 **§5A 项目与测量类型(PJ1–PJ5)**:Project → Survey → Session 三层模型;测量类型以 `SurveyProfile` 注册,UAV 磁测与步行磁测是两种 profile。**M1 状态机按此拆分为"通用状态机 + 平台 driver"**,不先做 UAV 专用版再重构。
- 新增 **§5B 步行磁测(W0–W8)** 与 **U16**(步行引导界面)。步行测量**与 UAV 磁测共用同一套传感器与数据流**——同一 PX4、同一 DroneCAN 磁力仪、同一 ZED-F9P RTK、同一 MAVLink 消息集与记录栈,**唯一区别是手持而非飞行**;该 PX4 在步行会话中**不飞行**(仅作仪器中枢:DroneCAN 磁力仪 + RTK GNSS → MAVLink),GCS 不接串口/不解析厂商帧。仪器数据**全部走 MAVLink**,差异仅在会话行为(无 Arm/飞行命令、低速不输出航向、手持磁洁净检查),不在数据通路。GCS 的职责是导航引导、MAVLink 采集、记录、QC,日变改正(M7/M8)复用;步行会话**不含 Arm/RTL/飞行命令**(见 ADR-020)。
- 新增 §9 待核实与待确认项(步行仪器型号与协议是最大的未知项)。

## v2 相对 v1 的变化

- 已确认的 4 项决策并入(见 §1):VIC 观测台数据、HRDEM 按项目镶嵌且先做维多利亚地区、1 m 与 100 m × 100 m 测区、允许强制解锁。
- 新增 **M7–M9**(VIC 日变改正、时间对齐与改正管线、站点高度核对)和 **U1–U14**(UI/UX)。
- HRDEM 部分按已核实的数据入口重写;因测区只有 100 m × 100 m,大面积内存问题降级,**基准链与水平基准精度升为主要风险**。

---

## 0. 给执行 AI 的规则

1. 先读 `AGENTS.md`、`docs/ARCHITECTURE.md`、`docs/FEATURES.md`、`issues.md`、`docs/adr/004,005,006,013,017`。本文不覆盖它们;冲突时以 ADR 为准,并新增 ADR 说明改动。
2. **一次只做一个 WP**。WP 标 ◆ 表示涉及飞行安全、串口/MAVLink 写入、外部数据源或数据格式变更:先输出类型/函数签名、错误类型和失败/风险分析,**停下等确认**再实现。其余可直接实现。
3. 每个 WP 自带可自动化验收(单元测试、`crates/core/tests` 假飞控集成测试或 SITL 脚本);前端 WP 附截图与手动验证步骤(AGENTS 规则 4),并满足 §6 的可度量 UI 验收。完成后写明"如何验证的"。
4. 沿用仓库约束:Rust 生产代码不用 `unwrap`;绝对高度一律 `height::Height`,转换只在 `core::height`;TypeScript strict;常量不写魔法数;用户可见文案与文档为英文(`i18n/en.ts`);跨边界类型走 ts-rs;颜色用设计令牌;提交前跑 `.github/workflows/ci.yml` 的全部命令。
5. 新逻辑放 `crates/core`,不得依赖 Tauri;`app-tauri` 只做适配(ADR-001)。
6. **范围之外**:传感器校准、机型/遥控设置、固件刷写(归 QGC);磁力仪与 GNSS 接收机本身的出厂校准、固件与参数设置(归厂商工具);机载地形跟随(ADR-005);闭环控制;从 UI 发送 21196 强制码;PX4 固件改动。
7. 新 ADR 从 018 起编号。
8. **外部数据合规**:不得把 NRCan 地磁数据放进仓库、测试夹具或安装包;测试用合成数据(见 M7)。
9. **步行磁测的仪器协议、输出格式与交付格式**在 W0 由操作员确认前,不得实现 W1 及之后的 WP;不得猜测厂商私有协议,不得把厂商文档或固件放入仓库。(W0 已于 2026-10-15 确认:DroneCAN/MAVLink + CSV,见 ADR-020;W1 已解锁。)
10. **新增测量类型走 profile**(PJ2),不得在通用代码里按类型硬编码分支。

---

## 1. 已确认的决策

| # | 决策 | 对计划的影响 |
|---|---|---|
| 1 | 接入 **VIC(Victoria)地磁观测台**数据作为日变基准 | 新增 M7–M9;飞行前检查与归档纳入日变状态 |
| 2 | HRDEM 取**按项目镶嵌**,先限定 **BC 温哥华岛维多利亚地区** | 数据入口用 `hrdem-lidar` 集合;H0 先确定覆盖该地区的项目;AOI 跨项目时报警而不是拼接 |
| 3 | 精度取**可提供的最高**;**单个测区 100 m × 100 m** | 默认 1 m,不做降采样;AOI 硬上限为 100 m × 100 m 加缓冲;大面积相关工作(H5)降为 P2;亚米级只能走点云路径(H11,可选) |
| 4 | **允许强制解锁** | L5 采用"带理由的强制解锁",仅绕过 GCS 侧检查清单,不绕过 PX4,记入日志与归档 |

---

## 2. 优先级总览

| 线 | 目标 | WP |
|---|---|---|
| **L** 生命周期安全 | 启动 → 连接 → 飞行前 → 飞行中 → 异常 → 断开/关闭 | L1–L10 |
| **A** 架构 | 统一清理、告警、配置、记录;去掉重复实现 | A1–A11 |
| **M** 磁测 | 航线级记录与 QC、VIC 日变改正、可复现归档、面向磁测的 drape | M1–M9 |
| **H** HRDEM | 按项目镶嵌的 1 m DTM/DSM、基准链、离线流程 | H0–H11 |
| **PJ** 项目与类型 | Project → Survey → Session;测量类型 profile;项目界面与归档 | PJ1–PJ5 |
| **W** 步行磁测 | 仪器接入、导航引导、步行会话、QC、测前检查 | W0–W8 |
| **U** UI/UX | 工作流导航、告警、命令、飞行前与测量运行界面、现场可用性、地图可分离、步行引导 | U1–U16 |

| 级别 | WP |
|---|---|
| P0 | L1 L2 L3 L4 A1 A3 H0 W0 U2 U3 U4 |
| P1 | L5 L6 L7 L8 A2 A4 A5 A6 A7 M1 M2 M4 M5 M7 M8 H1–H4 H7 U1 U5 U6 U7 U8 U9 PJ1 PJ2 PJ3 W1 W2 W3 W4 W5 W6 U16 |
| P2 | L9 L10 A8 A9 A10 M3 M6 M9 H5 H6 H8 H9 H10 H11 U10 U11 U12 U13 U14 U15 A11 PJ4 PJ5 W7 W8 |

---

## 3. 架构优化(A)

### 现状问题(来自代码阅读)

- 清理逻辑分散:`disconnect`(清理 RTK、日志、检查器)、`shutdown_app`(只关主链路)、三个子窗口各自的 `on_window_event`;主窗口没有 `CloseRequested` 处理。
- 链路相关服务(rtk/logs/params/motor_test)挂在 `AppState` 上,靠 `detach()` 手动停(issues O4 同源)。
- 告警来源零散,没有 STATUSTEXT;无飞行记录(tlog/事件日志),无 `tracing` 文件日志。
- 运行配置以 localStorage 为源(`desktop/prefs.ts`),core/headless 读不到。
- `CommandService` 单槽:在途命令会让其他命令被挡住,急停类命令没有优先通道。
- drape 逻辑有 Rust(`core::terrain::drape`)与 TS(`mission/meshDrape.ts`)两份。
- `CogDem::open` 整幅读入;像元角度用中纬度常数换算(源码注释自述漂移约 0.064%)。

### WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **A1** ◆ P0 | **生命周期监督器**:core 新增 `lifecycle`。`trait ManagedService { fn name(); async fn stop(&self, deadline: Duration) -> StopOutcome }`;`Lifecycle::shutdown(reason, policy)` 按固定顺序停止(电机测试 → RTK 注入 → 日志下载 → 参数 → 任务/命令服务 → 记录器 flush → 链路 → 地形服务),每项带截止时间并汇总结果;`policy` 含"已解锁/飞行中是否拒绝"。`disconnect`、`shutdown_app`、主窗口 `CloseRequested` 全部走它。 | 新 `core/src/lifecycle/`;`app-tauri/{lib,commands,state}.rs` | 假服务单测:顺序正确、超时不阻塞其余、已解锁时返回 `Refused{reason}`;集成测:电机测试运行中退出,先收到 Stop 再退出 | M | — |
| **A2** P1 | **服务归属链路**:rtk/logs/params/motor_test 句柄并入 `ActiveLink`,链路析构即停止;SITL 副链路上提到通用层(O4);删除手动 `detach()`。 | `state.rs`、各 `*_service.rs` | 断开后无残留任务(任务计数/句柄 drop 测试);现有集成测试不回退 | M | A1 |
| **A3** P0 | **AlertCenter**:core 新增 `alerts`。`Alert{id, severity(Info/Warn/Critical), source, text, first_ts, last_ts, count, latched, acked}`;去重/节流/锁存/确认;来源:STATUSTEXT、链路、电池、GPS/RTK、mag 告警、命令失败、DEM、VIC 数据。Tauri 侧统一 `alert` 事件;前端一个 store;`ErrorBanner`、事件日志抽屉、TopBar 角标都是它的视图。 | 新 `core/src/alerts/`;`app-tauri`;`frontend/src/stores/` | 单测:去重计数、Critical 锁存到 ack、迟滞;前端三处视图一致 | M | — |
| **A4** ◆ P1 | **飞行记录器(已完成 2026-10-15,ADR-021)**:core 新增 `recorder`;并给 `mavlink::connection` 加**原始帧旁路(方案 B,引用计数门控,不录制时零开销)**。tlog(带时间戳原始 MAVLink 帧,格式对齐 QGC:每帧前置 8 字节**小端**微秒时间戳,见 ADR-021)+ 事件日志 JSONL(告警、命令、模式、航线开始/结束、任务同步、强制解锁)+ `session.json`。专用写线程 + 有界通道,**绝不阻塞 hub**;满则丢最旧并计数告警;停止时 flush+fsync。命令/模式/任务/航线事件源随各自 WP(M1/W4/L5)接入。 | 新 `core/src/recorder/` | 回放 tlog 与源帧逐字节一致;写盘卡顿时 hub 延迟不增加;异常退出后文件可解析到最后一次 flush | L | A1 |
| **A5** ◆ P1 | **安全/飞行前引擎**:core 新增 `safety`。纯函数 `evaluate(&TelemetrySnapshot, &PlanState, &SafetyConfig) -> Checklist`,每项 `{id, group, status(Pass/Warn/Fail/Unknown), detail, fix_hint}`;阈值集中在 `SafetyConfig`;无 IO,可无头复用。 | 新 `core/src/safety/` | 表驱动单测覆盖每项四种状态;`Unknown`(缺数据/过期)不得等于 Pass | M | L6 |
| **A6** P1 | **Rust 侧配置存储**:`app_config_dir` 下版本化 JSON(端点、autoConnect、告警阈值、磁校准路径、DEM 目录、drape 参数、VIC 数据源设置、UI 现场模式)。前端经命令读写;首次启动迁移 localStorage;`SafetyConfig` 从此读取。 | `desktop/prefs.ts`、新 `app-tauri/config_service.rs`、core `config` | 迁移测试;损坏文件回退默认并发 Warn,不崩溃 | M | — |
| **A7** ◆ P1 | **命令优先通道**:`CommandService` 增加 `Priority::{Normal, Emergency}`。Emergency(RTL/Land/Disarm)可顶替在途 Normal 命令,被顶替者上报 `Superseded`;Emergency 不被顶替;仍保持同一时刻最多一条在途命令。 | `core/src/commands/{mod,service}.rs`、`command_service.rs` | 假飞控:在途 `goto` 时发 RTL → RTL 立即发出并得到 ACK,`goto` 收到 `Superseded`;丢 ACK 重传不乱序 | M | — |
| **A8** P2 | **前端隔离**:地图(Cesium)包 `ErrorBoundary` 并处理 WebGL context lost;地图崩溃/卡死时 TopBar、Flight 菜单、告警仍可用。 | `App.tsx`、`MapView.tsx`、`useCesiumViewer.ts` | 手动注入地图异常:控件仍可操作;截图 | S | — |
| **A9** ◆ P2 | **单一 drape 引擎**:Rust 为唯一实现;网格仅用于显示;AGL 参考统一来自 core `ElevationSource`(HRDEM 瓦片)。Rust 与 TS 两版先做黄金测试,一致后删除 `meshDrape.ts`,更新 ADR-005 addendum。 | `core/src/terrain/drape.rs`、`mission/meshDrape.ts`、`stores/terrain.ts` | 逐点误差 < 1e-6 m;删除后 `scripts/check-agl.mjs`、`check-coords.mjs` 通过 | M | H4 |
| **A10** P2 | **可观测性**:`tracing` + 滚动文件日志 + panic hook;"导出诊断包"(日志、配置、清单、最近告警,不含密钥)。 | `app-tauri/lib.rs`、新 `diagnostics` | 触发测试 panic 后文件存在且含版本 | S | A6 |
| **A11** ◆ P2 | **跨窗口单一事实源**(U15 的前置):任务航点、选中项、图层开关、相机视角、drape 结果等"地图与规划共享状态"下沉到 core(或明确的 Rust 侧同步层),各 webview 只持有投影;所有写操作带版本号,冲突时以 core 为准并回告;地图交互(加点、拖动、框选、goto)经命令回传,不直接改前端 store。撤销/重做(U8)基于同一事实源。 | 新 `core/src/shared_state/`;`app-tauri` 事件桥;`stores/*` | 两个 webview 同时订阅:任一侧操作后另一侧状态哈希一致;乱序/重复事件不造成漂移;窗口重开后从 core 全量恢复 | L | A1, A3 |

---

## 4. 生命周期安全(L)

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **L8** P1 | **启动自检 + 连接确认**:启动状态卡(上次端点、磁校准是否加载、DEM 版本与就绪、VIC 数据可达性、地形服务、串口权限、系统时钟/时区)。自动连接前显示"将连接到 X"并留 3 s 取消窗口;连接后显示目标 sysid/机型,与上次不同则要求确认。磁校准路径持久化(现在只在内存里的 `CalibrationStore`,重启即丢)。 | `desktop/bridge.ts`、`SettingsDialog.tsx`、`state.rs`、A6 | 手动:改端点后自动连接前出现确认;重启后校准自动恢复并显示哈希前 8 位 | M | A6 |
| **L6** P1 | **补充遥测**:`HOME_POSITION`、`EXTENDED_SYS_STATE`(着陆状态)、`ESTIMATOR_STATUS`、`VIBRATION`、`RC_CHANNELS`/`RADIO_STATUS`;连接后用现有 `set_message_interval` 请求速率;`TelemetrySnapshot`/`TelemetryFieldAges` 同步扩展。 | `core/src/telemetry/{mod,hub}.rs`、`commands.rs` | 假飞控帧单测解码正确;缺失/过期字段为 `None`,不是 0 | M | — |
| **L5** ◆ P1 | **起飞前检查 + 强制解锁**(决策 4):基于 A5 的清单。全部 Pass 才一键 Arm/Takeoff;Warn 需逐项确认;Fail 阻止,但允许**强制解锁**:输入 `FORCE ARM`、从固定列表选理由(可附备注)、再点确认;强制记录写入事件日志、tlog 标记和 M4 清单(`forced_arm:{items, reason, time}`),该次飞行在飞后 QC 中标记。**强制只绕过 GCS 清单**:PX4 自身的 prearm 检查照常生效,不发送 21196;无 FC 心跳等物理上无法发命令的情形不可强制。基础项:链路新鲜;GPS fix 与卫星数;RTK(如启用);HOME;电池;传感器健康掩码;着陆状态 on-ground;任务已上传且回读一致(`itemsHash`);模式正确;电机测试未运行。 | `core/safety`、`FlightMenu.tsx`、新 `PreflightPanel.tsx`、`command.ts` | 假飞控场景表:每项 Fail 时 Arm 被阻止且给出原因;强制路径需要全部步骤才发命令,且产生日志条目与清单字段;PX4 拒绝时 UI 仍显示 PX4 原因 | L | A5, L6, U5 |
| **L1** P0 | **STATUSTEXT**:core 解码(含分片 `chunk_seq`),进入 AlertCenter(MAV_SEVERITY 映射),落事件日志与记录器;命令被拒(`temporarily_rejected`)时把紧随其后的 STATUSTEXT 关联到该命令展示原因。 | `core/src/telemetry`、A3、`FlightMenu.tsx`、`EventLogDrawer.tsx` | 单测:分片拼接;SITL 脚本:Arm 被拒后 UI 显示 prearm 原因(*verify* 原因文本确实随 STATUSTEXT 到达) | M | A3 |
| **L2** ◆ P0 | **常驻 RTL / Hold**:地图或 TopBar 上始终可见 RTL 与 Hold 按钮(不进下拉菜单);确认条显示当前模式、相对高度、电量;走 A7 Emergency 通道。命令状态条持续显示"已发送/等待 ACK/结果/耗时",拒绝时保留到操作员确认。交互细节见 U4。 | `TopBar.tsx`、`FlightMenu.tsx`、`stores/command.ts` | 从任一视图、任一窗口状态,发出 RTL ≤ 2 次交互;在途 goto 时 RTL 仍立即发出 | M | A7, U4 |
| **L7** P1 | **分级告警**:电池(电压与百分比,双阈值)、GPS(卫星数/fix 降级)、链路质量(丢包/RSSI)的 Warn/Critical,带声音与持续提示,阈值来自 `SafetyConfig`(替换 `TopBar.tsx` 里硬编码的 15/30%);去抖+迟滞同 mag 告警模式。 | `TopBar.tsx`、`core/safety`、已有告警音封装 | 表驱动单测;跨阈值只触发一次,不重复刷屏 | M | A3, A6 |
| **L9** P2 | `DO_REPOSITION`(点击飞到)加最大距离与高度保护;确认条显示距离。 | `commands.rs::send_goto` | 超限被拒并提示 | S | A6 |
| **L3** ◆ P0 | **FC 心跳丢失处理**:持久横幅(丢失时长计时、最后位置/高度/电量、声音);Flight 菜单置灰并说明原因,不再整体消失;遥测数值置灰+时间戳,地图飞机标"旧数据"。重连后自动重读模式、解锁状态、任务同步状态并提示"已恢复"。 | `bridge.ts`、`FlightMenu.tsx`、`TopBar.tsx`、`useDataAge.ts`、`mission` store | 假飞控:断开心跳 3 s 后出现横幅,恢复后状态重读并清除;截图 | M | A3 |
| **L4** ◆ P0 | **断开/退出护栏**:`disconnect` 与退出在已解锁或飞行中时拒绝并说明,需要时输入 `DISCONNECT`/`QUIT` 确认。主窗口 `CloseRequested`(X、Alt-F4、系统关机)与设置里的退出走同一个 `Lifecycle::shutdown`。退出前电机测试 Stop 要**等待发送结果**(现为 `try_send`),超时才强制;提示未上传/未保存的任务。 | `commands.rs`、`lib.rs`、`actuator_service.rs`、`SettingsDialog.tsx`、A1 | 已解锁时三条退出路径都被拒;电机测试中退出 → Stop 先于退出;主窗口先关而子窗口仍开时进程行为明确(*verify* 现状并写入测试) | M | A1 |
| **L10** P2 | **场景测试集**:链路丢失时按 RTL、解锁时退出、电机测试中断线、重连后状态恢复、STATUSTEXT 洪泛、ACK 丢失、强制解锁路径;全用现有假飞控框架。 | `crates/core/tests`、`crates/app-tauri/tests` | 全部进 CI | M | 以上 |

---

## 5. 磁测专项(M)

目标:以**航线**为单位组织飞行、记录、QC 与归档;用 VIC 数据做日变改正;让高程数据服务于离地高度一致性。

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **M1** ◆ P1 | **测量会话状态机**(core `survey_session`):`Planned → Uploaded → Verified → Ready → Surveying(line k) → Complete/Aborted`。绑定任务、S1 的 `PatternLine` 表(survey/tie/cal)与 `MISSION_CURRENT`,发出 `LineStarted/LineEnded{line_id, kind}` 事件并写入记录器(issues N3)。磁数据本体仍由 FC/ULog 记录。 | 新 `core/src/survey_session/`;`MagTrigger::GcsSide` | 给定航线表与 `MISSION_CURRENT` 序列,事件序列正确(含中途改点、重复上报、断线恢复);SITL 脚本整任务跑通 | L | A4 |
| **M2** ◆ P1 | **磁测飞行前检查**(扩展 A5):FC 日志已启用且有存储(参数名按 v1.17 核对 *verify*);磁传感器消息持续到达且速率达标;静止 10 s 噪声指标在阈值内;磁校准 JSON 已加载且哈希已记录;DEM 覆盖 AOI+缓冲且来源/版本已显示;任务与 DEM 版本一致;电量覆盖计划 ETA 加储备(用 `PatternStats`);RTK 要求满足;**VIC 数据可用性与扰动状态**(见 M7/M8);站点高度核对(M9)。 | `core/safety`、`calib`、`terrain`、`PreflightPanel.tsx` | 表驱动单测;缺校准/缺 DEM/磁数据中断/VIC 数据过期时给出对应状态与原因 | M | A5, L5, H4 |
| **M3** P2 | **实时航线 QC**(core,滑窗):横向偏差、AGL 偏差(计划 vs 实际;有雷达时用 `DISTANCE_SENSOR`,即 R1)、地速波动、航向偏差、磁噪声指标(如四阶差分极值,算法与阈值可配);**经 VIC 改正后的残差**(M8)。每条航线汇总成 `LineQc{pass, flags}`;不合格航线进"补飞清单"。**只观测与告警,不进控制回路**。 | 新 `core/src/qc/`、survey 运行面板(U6) | 合成数据单测(注入偏差/尖峰/日变,有检出率与误报率数值);回放 ULog 验证 | L | M1, M8 |
| **M4** ◆ P1 | **任务归档清单**(兑现 ADR-005):`survey_manifest.json` 含计划与航线表、DEM 来源/项目名/STAC item id/采集日期/哈希、大地水准面网格版本、磁校准哈希、FC 参数快照、**VIC 数据源/通道码/时间范围/取回时间/数据哈希/署名**、强制解锁记录、软件版本、操作员备注。与 tlog/事件日志同目录,并写入导出 `.plan` 扩展字段。 | 新 `core/src/archive/`、`mission/planfile.ts` | 往返测试:从清单可重建同一任务;哈希不匹配时 UI 警告 | M | A4, H4, M7 |
| **M5** ◆ P1 | **面向磁测的 drape**:默认**相对 DTM 恒定 AGL**(磁信号随与地面磁性源距离衰减,离地高度一致性优先);相对 **DSM** 做最小净空检查,树冠/障碍留可配余量;沿航线平滑并限制爬升/下降率,上限取自 FC 参数(参数名按 v1.17 核对 *verify*);不可行处报告而不是静默截断;输出每条航线 AGL 统计(均值/标准差/最小净空)、最大坡度与所需爬升率;使用 M9 的站点高度偏差(若已核对)。 | `core/src/terrain/drape.rs`、`survey/compile.rs`、`PatternStep.tsx` | 合成地形单测:坡度超限报不可行并指出航点;DSM 净空违反时抬升且计数;报告数值可复现 | L | H4, M9 |
| **M6** P2 | **飞后衔接**:导出航线表、事件 JSONL、改正后总场序列(含 UTC 时间)的文档化格式,供外部 QC 工具按同一张表切分;R2(ULog 展开 `DISTANCE_SENSOR`,与 drape 计划 AGL 对比)沿用现有 ULog 读取器。 | `core/ulog`、`docs/FEATURES.md` | 样例 ULog 产出对比曲线与偏差带 | M | M3, A4 |
| **M7** ◆ P1 | **VIC 日变数据源**(core 新增 `diurnal`):`trait DiurnalSource` + `DiurnalSeries{utc, f_nt, quality}`;实现 ① **NRCan FDSN 在线源**(通道 `LFF` 秒级总场、`LFX/LFY/LFZ` 秒级矢量;`UFF` 等分钟级;位置码 `R0` 经互联网 *verify* 实际用哪个)② **IAGA-2002 文件导入**(离线/事后)③ 可选 INTERMAGNET 回退(分钟级,是否提供 VIC 秒级 *verify*)。本地缓存(按时间窗口),飞前预取过去 24 h 以评估当日磁扰,飞行中按可配周期轮询,飞后取完整时间窗。**合规**:数据不入库、不随安装包分发;设置页显示 NRCan 使用条款与署名(条款要点:仅供申请者及合作者使用,未经书面许可不得出售/向第三方分发;请向 NRCan 寄送报告副本;商业用户需联系 NRCan);商业使用前由操作员确认(见 §9)。测试一律用**合成数据**生成器。 | 新 `core/src/diurnal/`;`app-tauri` 适配;设置页 | 解码器对 IAGA-2002 样本(合成)与 FDSN 响应样本(合成,按公开格式构造)正确;断网/超时/无数据返回明确错误并发告警;缓存命中不重复请求 | L | A3, A6 |
| **M8** ◆ P1 | **时间对齐与改正管线**:(a) 机上磁数据与 VIC 数据统一到 UTC:用 GPS 时间/`SYSTEM_TIME` 建立 boot 时间到 UTC 的映射并写入记录器,漂移/跳变检测;(b) 日变 `ΔF(t) = F_VIC(t) − F_ref`,参考值可选"测量开始均值/当日中位数/操作员指定",并记录;(c) 改正后总场 = 校准总场 − ΔF(插值到机上时间戳);(d) **扰动门限**:基于 VIC 的弦长(chord)与变化率,阈值可配(默认值由操作员按合同/规范设定,不得内置未经确认的数值),超限给 Warn/Critical 并标记受影响航线;(e) 记录 VIC 与测区的距离,距离大时提示空间相关性风险。VIC 约在维多利亚以北 16 km 的小萨尼奇山,测区离其越远,日变空间差异越大。 | `core/src/diurnal/correct.rs`、`recorder`、`qc` | 合成日变(已知幅度/周期)+ 合成机上数据:改正残差 RMS 低于阈值(给出数值);时间偏移注入 ≥ 2 s 时被检出;插值边界(数据缺口)显式标记不外推 | L | M7, A4 |
| **M9** P2 | **站点高度核对**:起飞前在同一地点比较"DEM 推出的地面 AMSL"与"飞控着陆状态下的 AMSL"(需 RTK 固定、`EXTENDED_SYS_STATE` on-ground),给出偏差与建议;操作员可采纳为本次任务的高度偏差并记入清单。在 1 m 精度下,基准链误差与机体离地高度会盖过 DEM 本身误差,此项让误差可见。偏差含机体支撑高度,仅作指示,阈值可配。 | `core/height`、`safety`、`PreflightPanel.tsx` | 合成场景:注入已知偏差,检出值与注入值一致;未满足 RTK/on-ground 条件时状态为 `Unknown` | M | L6, H7 |

---

## 5A. 项目与测量类型(PJ)

### 概念模型

| 层级 | 含义 | 例子 |
|---|---|---|
| **Project** | 一个现场/合同的容器:坐标系与基准、测区、共享资源、备注 | "Saanich 2026-10" |
| **Survey** | 项目内的一种测量:**类型** + 测区 + 航线表 + 配置 | `UavMag` 100×100 m @ 40 m AGL;`WalkMagContinuous` 同一测区 |
| **Session** | 一次实际的飞行或步行:原始记录、事件日志、清单、QC 结果 | `2026-10-15T1432Z` |

- **同一项目可含多个 Survey**(例如先 UAV 后步行验证同一测区),各自独立执行,共享项目级资源(DEM 缓存、磁校准、参数预设、VIC 设置)。
- **类型由 `SurveyProfile` 决定**,不是在各处写 `if walk`:profile 声明所需设备、阶段导航、飞行前/测前清单模板、QC 模板、航线跟随方式、输出格式。新增类型 = 新增 profile + 平台 driver。
- 项目目录(本地优先,无云依赖;日变原始数据不进归档,见规则 8):

```
<project>/
  project.json                 # schema_version、名称、坐标系、测区、备注
  resources/                   # dem/(按 AOI 哈希)、calibration/、presets/
  surveys/<survey-id>/
    survey.json                # kind、aoi、航线表、配置
    sessions/<ts>/
      session.json             # 状态、起止时间、操作员
      manifest.json            # M4 清单(含各类型扩展字段)
      events.jsonl             # 事件日志
      raw/                     # tlog(UAV/步行,均为原始 MAVLink 帧)+ 步行提取的磁/GNSS 样本
      qc.json
```

### WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **PJ1** ◆ P1 | **项目/测量/会话模型与存储(ADR-019)**:core `project`:`Project`、`Survey`、`Session`;目录布局如上;`schema_version` + 迁移;原子写(临时文件 + rename);项目目录锁文件防双开;文件损坏回退并发 Warn。A6 的应用配置只保留应用级设置,**项目级设置**(DEM 目录、VIC 源设置、预设、校准路径)迁入项目。 | 新 `core/src/project/`;`app-tauri/config_service.rs` | 往返测试;写入中途被杀后可恢复;旧版本迁移;同一项目双开被拒 | M | A6 |
| **PJ2** ◆ P1 | **测量类型 profile 与平台无关的会话状态机**:`SurveyKind{UavMag, WalkMagContinuous, WalkMagStation, BaseStation}`;`SurveyProfile{required_devices, phases, safety_template, qc_template, line_following, outputs}`。**M1 实现时直接拆为"通用状态机 + 平台 driver"**:UAV driver 绑定 `MISSION_CURRENT`,Walk driver 见 W4。A5 的 `evaluate` 接收 profile 给出的清单模板;U1 阶段标签由 profile 决定(UAV:Prepare/Plan/Preflight/Fly/Review;步行:Prepare/Plan/Pre-survey/Walk/Review)。 | 新 `core/src/survey_profile/`;M1、A5 | UAV 既有测试不回退;注册一个**假的第三种类型**,不改通用代码即可跑通状态机与清单 | M | PJ1, M1, A5 |
| **PJ3** P1 | **项目界面**:启动页(最近项目/新建/打开);新建项目向导(名称、坐标系、测区、备注);项目内测量列表(类型徽标、状态、最近会话);会话历史(日期、QC 结果、**强制解锁/强制开始**标记);TopBar 显示当前项目与测量类型。**切换/关闭项目在 Surveying 或已解锁时拒绝**(走 A1 policy)。U14 首次向导并入"新建项目"。 | 新 `ProjectHome.tsx`、`TopBar.tsx`、`stores/project.ts` | 截图;会话进行中切项目被拒并说明原因;重启回到上次项目 | M | PJ1, PJ2, U1 |
| **PJ4** P2 | **项目级共享资源与归档**:DEM 缓存、磁校准、预设、VIC 设置挂在项目下,多个测量复用;项目导出/导入 zip,含清单与事件日志,**排除 NRCan 日变原始数据**,仅保留取回参数与数据哈希;导入时校验哈希。 | `core/project`、`core/archive` | 导出包扫描不含受限数据(测试);导入后可重建同一任务/航线表 | M | PJ1, M4, H9 |
| **PJ5** P2 | **跨 Session 比较**:同一项目内多个 Session(UAV vs 步行、重复测量)在重叠区比较改正后总场(差值、交叉点残差、统计),离线复盘用。 | 新 `core/src/compare/` | 合成数据:注入已知偏置/高度差,检出值与注入值一致 | M | PJ1, M6, W5 |

---

## 5B. 步行磁测(W)

### 与 UAV 磁测的本质差异

**步行磁测与 UAV 磁测使用同一套硬件与数据流**(同一 PX4、同一 DroneCAN 磁力仪、同一 ZED-F9P RTK、同一 MAVLink 消息与记录栈),**唯一区别是手持而非飞行**;下表的差异都是会话/运行层面的,不是数据通路层面的。步行会话中该 PX4 **不飞行**,只作仪器中枢。

| 方面 | UAV 磁测(现有) | 步行磁测 |
|---|---|---|
| 平台与控制 | PX4 + MAVLink,GCS 可发命令 | **PX4 只是仪器中枢,不飞行**;GCS 不发飞行命令,只做导航引导、采集、记录、QC |
| 位置来源 | FC 的 EKF/GNSS | 同一 PX4 上的 **RTK GNSS**(ZED-F9P),经 MAVLink;树冠下易丢星、RTK 掉到浮点 |
| 磁数据来源 | FC / ULog | 同一 PX4 上的 **DroneCAN 磁力仪**(RM3100 或 Bartington UAS-Mag,单传感器),经 MAVLink;主记录仍在 PX4 SD/ULog |
| 高度 | drape 保持 AGL | **固定杆高**,不 drape;DEM 仅用于显示、起终点与可达性提示;杆高需记录 |
| 航线执行 | 自动飞 | 人沿线走,需要**实时引导**:横向偏差、速度、航线进度 |
| 采样间距 | 速度稳定 | 由行走速度决定,波动大,**间距必须 QC** |
| 安全清单 | Arm / RTL / 电池 / GPS | 设备在线与速率、GNSS、时间同步、**磁洁净**、存储、电量;无 Arm/RTL |
| 磁干扰源 | 机体磁场(靠校准) | **操作员身上的铁磁物**(靴子、手机、钥匙、杆),需测前检查 |
| 日变 | VIC | 同 VIC(M7/M8 复用),可另加本地基站(W8) |
| 记录 | tlog + ULog | 原始 MAVLink 帧(tlog)+ 提取的磁/GNSS 样本 + 事件日志 + CSV,崩溃安全;全速率主记录仍在 PX4 SD/ULog |

### 特殊需求清单 → WP 对应

1. **MAVLink 仪器接入**(磁力仪 + RTK GNSS 样本,流速率与时效监控,复用遥测/告警)→ W1
2. **时间同步**:GNSS 时间、PX4 `SYSTEM_TIME`、主机时钟对齐,检测漂移/跳变 → W1、W7(沿用 M8 的 UTC 映射思想)
3. **实时导航引导**(横向偏差、速度、进度、偏离告警)→ W3、U16
4. **连续与站点两种采集方式**(连续行走记录 vs 到点停留平均)→ W4
5. **航线起止与中断处理**(自动/手动触发、暂停、续测、重测、marker/fiducial)→ W4
6. **GNSS 质量管理**(fix 类型、HDOP、卫星数、位置间隙、插值标记,不外推)→ W5
7. **采样间距与速度 QC**,交叉点/tie 线残差 → W5
8. **测前检查**(磁洁净、静止噪声、时间偏差、存储)→ W6
9. **传感器与 GNSS 的时延/航向差标定**(往返测线)→ W7
10. **本地基站作为日变源**(可选)→ W8
11. **野外可用性**:日光、手套、单手、声音/震动提示、离线 → U16、U12

### 复用

S1 `PatternLine`(survey/tie/cal)、M7/M8 日变管线、A3 告警、A4 记录器(扩展为多来源)、A5 清单引擎(换模板)、M4 清单(增加扩展字段:仪器型号与序列号、杆高、磁洁净检查结果、速度/间距统计、GNSS 质量统计)、H 系列 DEM(仅显示)。

### WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **W0** ◆ P0 | **调研与决策(ADR-020,已完成 2026-10-15)**:操作员确认——步行**携带 PX4,仅作仪器中枢,不飞行**;**仪器数据全部走 MAVLink**;磁力仪为 **DroneCAN 节点**(RM3100 或 Bartington UAS-Mag,按项目二选一,**单传感器,非梯度**);GNSS 为 **Holybro H-RTK ZED-F9P**(接在 PX4 上,**用 RTK**);以连续步行为主;主记录在 PX4 SD/ULog,GCS 消费实时磁+位置;交付 **CSV**(字段表见 ADR-020)。GCS 只做导航引导、采集、记录、QC、日变与归档;**不接串口、不解析厂商帧、不配置 DroneCAN 节点、不发飞行命令**。 | `docs/adr/020-walk-survey.md` | 接口/消息表与交付 CSV 字段表写入 ADR;**原 §5B"无飞控/无 MAVLink、GCS 解析仪器帧"假设作废** | S–M | 操作员 |
| **W1** ◆ P1 | **MAVLink 仪器接入层**(与 UAV 磁测**共用同一套传感器与数据流**,不新增步行专用采集通路):core `instruments`:`trait InstrumentSource`;从**现有** `core::telemetry`(`GPS_RAW_INT`/`GLOBAL_POSITION_INT`)与 `core::signals`(磁消息 → nT)提取类型化磁/GNSS 样本;**不写 NMEA 解析、不写串口/TCP/蓝牙传输**;流速率与时效监控(复用 A3/L3);时间戳 = GNSS/PX4 时间 + 主机单调时钟(游走/跳变检测)。**含 MAVLink 合成数据模拟器**(磁 + GPS 消息),CI 不依赖硬件。 | `core/src/instruments/`(薄适配);`app-tauri` 适配 | 模拟器 MAVLink 流端到端提取正确;磁流中断/速率偏离/GPS 降级有告警;时间跳变被检出 | M | W0, A3 |
| **W2** ◆ P1 | **步行记录器**(**扩展现有 A4 记录器**,不新建独立管线):除 tlog(原始 MAVLink 帧)外,把提取的**磁样本与 GNSS 样本**带时间戳落盘 + 事件 JSONL(航线开始/结束、marker、暂停、质量事件)+ `session.json`;沿用"专用写线程 + 有界通道 + 不阻塞采集";导出 **CSV**(字段表见 ADR-020:UTC、纬经高/AMSL、F、X/Y/Z、fix/HDOP/卫星、line_id、quality)。 | `core/src/recorder/` | 回放原始文件与采集样本一致;写盘卡顿时采集不丢帧;异常退出后可解析到最后一次 flush | M | A4, W1 |
| **W3** P1 | **步行导航引导**(core 计算):横向偏差、沿线进度、到下一点距离、航线方向、建议速度带;偏离/超速/过慢告警;**输入为 MAVLink(RTK)GNSS 位置**(带质量与时效)。**低速时不输出航向**(不用磁罗盘,不用磁力仪读数);位置过期标 `Stale` 并停止引导,不显示为有效。小范围内用局部切平面计算。 | 新 `core/src/walk_nav/` | 合成轨迹(含噪声/漂移/暂停):横向偏差与解析值差 < 1 mm;低速不输出航向;位置过期置 `Stale` | M | W1, PJ2 |
| **W4** ◆ P1 | **步行测量 driver**(接入 PJ2/M1 状态机):`Planned → Ready → AtStart → Walking(line k) → Paused → LineDone → Complete/Aborted`;航线起止触发可选:手动按钮、进入起点缓冲区 + 速度阈值自动、marker;暂停/续测/重测;**站点式**:到点后平均 N 秒、样本数不足不得通过;事件写入记录器。 | 新 `core/src/survey_session/walk.rs` | 表驱动事件序列测试:重复触发、中途暂停、GNSS 丢失、重测、站点样本不足 | L | PJ2, W2, W3 |
| **W5** ◆ P1 | **步行 QC**(M3 的步行版):采样间距分布、速度带、GNSS 质量(fix/HDOP/浮点占比)、位置间隙(插值段显式标记,不外推)、磁数据中断、尖峰/噪声、**经 VIC 改正后残差**(M8)、tie 线交叉点残差;每条航线 `LineQc{pass, flags}`,不合格进补测清单。**所有阈值可配,不内置未经确认的数值**(沿用 M8 原则)。只观测与告警。 | 新 `core/src/qc/walk.rs` | 合成数据:注入间距超限/位置缺口/尖峰/日变,有检出率与误报率数值 | L | W4, M8 |
| **W6** ◆ P1 | **测前检查 profile**(A5 模板):**MAVLink 磁流在线与速率达标**、GNSS fix 与卫星数、**RTK 状态**、**主机时钟与 GNSS/PX4 时间偏差**、存储空间、电量、杆高已填写、VIC 数据可用与扰动状态、DEM/测区(仅显示用)就绪;**磁洁净检查**:操作员静止噪声指标 + "携带物品前后"读数变化,在阈值内才 Pass。**无 Arm/RTL/飞行命令**;强制开始沿用 L5 的关键字 + 理由 + 记录机制(仅绕过 GCS 清单)。 | `core/safety`、`PreflightPanel.tsx` | 表驱动单测,每项四种状态;缺数据为 `Unknown`,不等于 Pass | M | A5, PJ2, W1 |
| **W7** P2 | **时延与航向差标定**:往返测线(line reversal)估计传感器-GNSS 时延与 heading error;结果写入清单,**应用于改正需操作员确认**,不自动应用。 | `core/src/qc/`、`PreflightPanel.tsx` | 合成往返数据注入已知时延,估计值与注入值一致 | M | W5 |
| **W8** P2 | **本地基站日变源**:第二台磁力仪作为 `DiurnalSource` 实现,与 VIC 并存、可对比;基站与测区同址时优先,切换与来源记入清单。 | `core/src/diurnal/` | 合成日变:基站与 VIC 对比输出差异统计;切换源可复现 | M | M7, W1 |

---

## 6. HRDEM 引入(H)

### 6.1 已核实的事实

- 来源:NRCan CanElevation。许可 **Open Government Licence – Canada**,引用与署名按其要求。
- 数据入口:AWS 开放数据桶 `s3://canelevation-dem/`(区域 `ca-central-1`,**匿名访问**)。按项目镶嵌在前缀 **`hrdem-lidar/`**;STAC API:`https://datacube.services.geo.ca/stac/api/search?collections=hrdem-lidar`。1 m/2 m 全国镶嵌是 `hrdem-mosaic-1m/`、`hrdem-mosaic-2m/`,本计划不用。
- 产品:DTM + DSM,基准 **CGVD2013**,重投影到 **EPSG:3979(Canada Atlas Lambert)**,以 **COG** 提供;项目之间**没有拼边处理**;南部由机载 LiDAR 生成 DTM 与 DSM,北部多为仅 DSM。
- 项目 item 以"省/机构 + 项目名 + 分辨率"命名(例如 `MB The Pas 1m`),带采集时间范围。
- **已由 H0 核实(ADR-018)**:覆盖维多利亚地区的项目 = `BC-Vancouver_Island_Sunshine_Coast_2018-1m`(采集 2019-10-01);资产 `-dtm.tif`/`-dsm.tif`(S3 HTTPS 免认证);nodata **−32767**;COG 512×512 分块、Float32、LZW、8 级 overview;垂直精度按产品规格"一般优于 1 m"(项目级数值未随 STAC 提供)。

### 6.2 与现有代码的差距

- `core::terrain::cog` 只读地理坐标(WGS84 度)栅格;`dem-prep` 能读 UTM 与地理 GeoTIFF,**不支持 Lambert**,也没有联网获取步骤。
- 像元角度由中纬度常数换算:4 m 可接受;1 m 下漂移不可忽略(H4 修正)。
- DSM 降采样若用双线性会削平尖峰,净空检查偏乐观(本决策默认不降采样,但重投影仍需保守处理)。
- 默认处理分辨率为 4 m(`dem-prep process --resolution 4`)。

### 6.3 精度的真实瓶颈(100 m × 100 m,1 m 分辨率)

测区只有 1 万个栅格,内存与性能不是问题。限制精度的是:

1. **水平基准**:HRDEM 为 NAD83(CSRS),PX4 GNSS 为 WGS84;两者水平差在加拿大约 1–2 m,约等于 1–2 个像元。**ADR-018 §D 已决定取空变换**,残留约 1 m 记入风险,由 M9/H7 控制点核对;必要时回退 EPSG:1946。
2. **垂直基准链**:`H(CGVD2013) + N(CGG2013) = h`,`h − N(EGM96) = AMSL`;缺真实 CGG2013 格网时 AMSL 精度无法保证(issues N2)。
3. **DEM 自身的垂直精度与采集年代**(树冠、地表变化)。
4. **机体离地高度与 GNSS 垂直精度**(M9 让其可见)。

### 6.4 WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **H0** ◆ P0 | **调研与决策(ADR-018)**:(a) 用 STAC API 以维多利亚地区 bbox 查询 `hrdem-lidar`,列出覆盖的项目、采集日期、DTM/DSM 资产、CRS、nodata、COG 分块/overview、垂直精度说明;选定覆盖测区且含 DTM+DSM 的项目(多项目覆盖时取最新并说明);(b) 取数路径:对 S3 的 HTTPS 范围读取 COG 窗口(资产 href 以 STAC 返回为准),确认无需认证;(c) NAD83(CSRS)↔WGS84 水平差对 1 m 数据的影响与处理决定;(d) 许可与署名文本;(e) **100 m × 100 m(加缓冲)测区的 DTM+DSM 裁剪**作为 `testdata/hrdem/` 夹具(OGL 允许,需随附署名;大小 ≤ 2 MB)。**需要操作员提供测区中心坐标**(未提供时用占位 AOI,并在 ADR 中注明)。 | `docs/adr/018-hrdem.md`、`testdata/hrdem/` | 每条事实有来源链接与抓取日期;夹具可被后续测试读取;给出被选项目的名称与采集年代 | S–M | — |
| **H1** ◆ P1 | **`dem-prep hrdem` 子命令**:输入测区中心+尺寸(或多边形/`.plan`)+ 缓冲;STAC 查询;AOI 跨项目时**报警并要求选择单一项目**,不拼接;抓取 DTM/DSM 窗口;记录出处(item id、项目名、采集日期、资产 href、抓取时间、sha256、许可文本)。AOI 硬上限 100 m × 100 m 加缓冲(配置项,超限拒绝并说明原因)。离线明确报错;重复运行命中缓存。 | `tools/dem-prep/src/{main,hrdem.rs}`、`Cargo.toml` | 夹具 AOI 端到端产出原生栅格 + 出处 JSON;超限/跨项目/离线各有明确错误 | M | H0 |
| **H2** ◆ P1 | **EPSG:3979 支持**:Lambert 正反投影(NAD83(CSRS) 椭球);按 H0(c) 决定是否附加 NAD83(CSRS)→WGS84 变换;读取投影 GeoTIFF 的 GeoKey 识别 3979(现有 `read_projected_band` 只认 UTM)。 | `tools/dem-prep/src/{geotiff,projection}.rs` | 已知控制点正反投影往返误差 < 1 cm;与独立参考实现对比(出处写入测试注释) | M | H0 |
| **H3** ◆ P1 | **重采样策略**:默认**保持 1 m**(决策 3),仅做重投影到 WGS84 规则网格;DTM 双线性;**DSM 用足迹内最大值(保守)**;nodata 保留不填补;DTM 与 DSM 严格同网格。预估输出尺寸并显示。 | `tools/dem-prep/src/` | 带尖峰的合成 DSM 重采样后峰值不被削低;DTM/DSM 网格一致检查 | M | H1, H2 |
| **H4** ◆ P1 | **元数据扩展 `dem.json`**:`source:"hrdem-lidar"`、项目名、STAC item id、采集日期范围、精确像元角度(`dlat_deg`、`dlon_deg`,**消除运行时中纬度常数换算**)、原始 CRS、高度基准 `CGVD2013`、`has_dtm`/`has_dsm`、出处哈希。对旧瓦片向后兼容。 | `core/src/terrain/{mod,cog}.rs`、`tools/dem-prep/src/cog.rs`、ts-rs 绑定 | 旧瓦片仍可打开;新瓦片在 1 m 下行列漂移 < 0.1 像元(与解析值对比) | M | H3 |
| **H7** ◆ P1 | **垂直基准链收尾**:issues **N2 升为阻塞项**。接入真实 CGG2013 格网(核对 NRCan 当前可用格网与格式 *verify*);`--geoid` 支持 HRDEM;**缺格网时 UI 明确标注"高度基准未验证",并阻止导出 AMSL 任务**;控制点验收按 AGENTS.md(偏差 < 0.5 m,阈值待实测,不得自行放宽)。 | `core/src/height/`、`dem-prep terrain --geoid` | 控制点两段式测试;缺格网时导出被阻止 | M | H4 |
| **H5** P2 | **惰性分块读取**(降级):测区上限为 100 m × 100 m 时不需要;保留为将来放宽面积上限的前置条件。`ElevationSource` 的 COG 实现改为按需读 tile + LRU + 内存上限;沿用"不外推"。 | `core/src/terrain/cog.rs` | 放宽上限后峰值内存低于配置;采样结果与整幅读取一致 | M | H4 |
| **H6** P2 | **来源选择与新鲜度**:手动选择并记住;UI chip 显示来源、项目、分辨率、采集年代、基准;采集年代过旧(阈值可配)提示植被/地貌变化风险。 | `terrain_service.rs`、`stores/terrain.ts`、`LayersPopover` | 选择可复现;chip 截图 | S–M | H4 |
| **H8** P2 | **3D 网格**:`dem-prep terrain` 以 HRDEM 瓦片生成;1 m、100 m × 100 m 下给出瓦片数/体积估算写入文档;网格仅用于显示(A9)。 | `tools/dem-prep/src/mesh.rs`、ADR-004 addendum | 小 AOI 生成并显示;体积符合估算 | S | H4 |
| **H9** P2 | **"准备测区"一键流程**(Plan ▸ Area,UI 见 U9):画/输入 100 m × 100 m 后联网一键执行 H1→H3→H4→(H8),按 AOI 哈希缓存;完成后飞行前检查显示"DEM 就绪"。外场离线只用缓存。About 与导出文件带 NRCan 署名与 OGL 文本。 | `AreaStep.tsx`、`terrain_service.rs`、`SettingsDialog.tsx` | 联网一次后断网重启,drape 与显示仍可用;署名出现在导出 `.plan` 与 About | M | H1–H4 |
| **H10** P2 | **验证**:HRDEM 与 BC LiDAR 处理瓦片在重叠区对比(RMSE、偏差、最大差、空洞率);控制点验证基准链;同一航线 AGL 差异记录。 | `testdata/`、`docs` | 对比报告含数值;可复跑 | M | H7 |
| **H11** P2 可选 | **亚米级点云路径**:若需要优于 1 m,只能从 LiDAR 点云自行生成(现有 `dem-prep process` 的 LAS 路径,点云来源与许可另议)。默认不启用;启用前由操作员确认点云密度足以支撑所选分辨率。 | `tools/dem-prep` | 密度不足时拒绝生成并说明 | S | 操作员确认 |

### 6.5 磁测使用约定

- **AGL 默认相对 DTM**(AGENTS.md);净空检查相对 DSM。UI 与报告必须分别标注。
- **仅 DSM 区域**:`has_dtm=false` 时 AGL 实为离表面高度,规划面板必须显示警告,QC 报告标注该前提。
- 项目采集年代写入 M4 清单;采集年代与现状差异大时 H6 提示。

---

## 7. UI/UX 优化(U)

### 7.1 现状观察(来自 `frontend/src`)

- 只有两个视图(`planning` / `flight`);`RightInspector` 只在 `planning` 渲染内容(`view === 'planning' ? <PlanSteps/> : null`),**Flight 视图的右栏是空的**;`Dock` 的 `TABS` 为空、不渲染。
- TopBar 高 48 px,控件高度不统一:状态/菜单约 32 px,`Tools` 按钮 44 px,`IconButton` 28–32 px;文字令牌 `xs=11px`、`sm=12px`,HUD 还有硬编码 `text-[10px]`。对手套、日光下的平板或笔记本偏小。
- `RightInspector` 写死 `w-[264px]`,而设计令牌 `--mg-panel-w` 为 288 px。
- 规划默认值:矩形区域 500 × 500 m,默认 AGL 50 m(文档中的 drape 例子是 40 m);与 100 m × 100 m 的实际测区不符。
- 确认方式不统一:飞行命令"5 秒内点两次",电机测试"输入 `START PROPS` + 按住 3 s"。
- 告警分散(见 A3);子窗口(Inspector/Actuator/Params)是否显示链路丢失等告警 *verify*。
- 已有可复用资产:设计令牌与主题(dark/light/system/night-red)、`Pill`/`Popover`/`Tabs`/`StatusDot` 等组件、gallery、基线截图、磁告警声音。

### 7.2 设计原则

1. **按作业阶段组织**,不是按功能堆叠:准备 → 规划 → 飞行前 → 飞行 → 复盘。
2. **关键状态一眼可读**:解锁、模式、链路、电量、GPS/RTK、日变状态永远可见,颜色之外同时有图标/文字。
3. **危险操作的确认方式按风险分级**并全局一致。
4. **每个阻止项都给出原因和下一步**(带"去修复"入口),不让操作员猜。
5. **现场可用**:大目标、高对比、夜间红光、键盘可达、少依赖鼠标悬停。
6. 不做乐观显示:命令只显示"待确认/已确认"(沿用 ADR-017 原则)。

### 7.3 WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **U1** P1 | **作业阶段导航**:把两个视图扩展为 `Prepare / Plan / Preflight / Fly / Review`。Prepare:连接、校准文件、DEM 准备、VIC 数据;Preflight:检查清单(U5);Fly:测量运行面板(U6);Review:本次航线 QC、补飞清单、导出(M3/M6)。阶段标签上显示完成度徽标(如 `Preflight 9/11`);阶段间可自由切换,不强制线性。保留 `Plan` 现有四步卡片。 | `stores/ui.ts`(`View`、`VIEW_CONFIG`)、`TopBar.tsx`、`RightInspector.tsx` | 五个阶段均可进入;完成度徽标随检查结果变化;截图 | M | A5 |
| **U2** P0 | **TopBar 信息层级与一致性**:分三组——状态(Armed、Mode、Link)、健康(GPS/RTK、Battery、**VIC 日变**、DEM 就绪)、工具(Tools、告警、设置);统一控件高度与最小点击目标;状态 pill 带图标;单一告警角标(数量+最高严重度)打开 AlertCenter;修复 `Tools` 44 px 与其他 32 px 不一致。窄屏(≤ 1366)时低优先级 pill 折叠进"更多",状态组永不折叠。 | `TopBar.tsx`、`design-system/components.tsx` | 1280 与 1920 宽度截图:状态组始终完整可见;点击目标尺寸脚本断言(见 §7.4) | M | A3 |
| **U3** P0 | **告警中心界面**:Critical 以顶部横幅显示并持续到确认(链路丢失横幅含计时与最后位置,L3);Warn 为不遮挡的 toast;角标计数;抽屉按严重度/来源过滤,含 STATUSTEXT 控制台视图;严重度各有**不同的声音模式**,可调音量,静音有时限并自动恢复;颜色之外有图标;`aria-live` 播报。所有窗口(含 Inspector/Actuator/Params)挂同一个告警横幅组件。 | `ErrorBanner.tsx`、`EventLogDrawer.tsx`、新 `AlertCenter.tsx`、各子窗口 `main.tsx` | 触发 Critical 后,在子窗口也出现横幅;确认后消失;重复告警不刷屏;截图 | M | A3 |
| **U4** ◆ P0 | **命令交互分级**(统一规则):**低风险**(切 Hold、改显示)一次点击;**中风险**(Takeoff、Land、Arm)按住 1 s 带进度环,期间显示将要执行的内容;**高风险/不可撤销**(强制解锁、退出、清飞控任务)输入关键字或再点确认。RTL/Hold 常驻按钮(L2),确认条显示模式、相对高度、电量。统一 `HoldButton`、`ConfirmBar` 组件替换现有"5 秒内点两次"。命令状态条持续显示"已发送 → 等待 ACK → 结果 + 耗时"。 | `FlightMenu.tsx`、`ModePopover.tsx`、`design-system/components.tsx`、`stores/command.ts` | 手动+截图;自动测试:误触单击不会发出中/高风险命令;从任一阶段发 RTL ≤ 2 次交互 | M | A7 |
| **U5** ◆ P1 | **飞行前清单界面**:按组显示(链路 / 导航 / 电源 / 任务 / 地形 / 磁测 / 日变),每行"状态图标 + 一句话原因 + 修复入口"(如"加载磁校准…""准备测区""打开 VIC 设置");顶部显示 `9/11 就绪`;Warn 逐项确认;Fail 显示"强制解锁"入口,流程见 L5(关键字 + 理由 + 记录);进入飞行阶段前的总体"可飞/不可飞"徽标。 | 新 `PreflightPanel.tsx`、`FlightMenu.tsx` | 每个 Fail 项都有可点的修复入口;强制路径记入日志;截图 | M | A5, L5 |
| **U6** P1 | **测量运行面板**(填充 Flight 视图的空右栏):当前航线 `Line 7/24 · survey · 63%`、横向偏差条(±m)、AGL 实际 vs 计划与允许带、地速、电量预测(完成时剩余)、下一航线预览、航线 QC 状态胶囊(通过/待定/标记)、日变迷你曲线(VIC ΔF 与当前扰动等级)。地图上航线按 QC 状态着色,当前航线高亮;`Dock` 启用"高度剖面"页:计划 vs 实际 vs DTM/DSM(复用 `AltitudeProfile`)。 | `RightInspector.tsx`、新 `SurveyRunPanel.tsx`、`Dock.tsx`、`entities.ts` | SITL 回放:航线切换时面板与地图同步;截图 | L | M1, M3, M7 |
| **U7** P1 | **HUD 重做**:关键数值加大并用等宽数字字体(`--mg-font-mono`),加入 **AGL(相对 DTM)**,保留 REL/AMSL;陈旧数据时整块置灰并显示"STALE + 秒数",取代当前两个 10 px 小徽标;可拖到四角并记住位置;最小字号不低于 12 px(现场模式 14 px)。 | `Hud.tsx`、`instruments/` | 截图;陈旧注入测试显示正确 | M | L3 |
| **U8** P1 | **小测区规划体验**:新建测区默认 **100 m × 100 m**,默认中心取"飞机位置 → HOME → 上次测区"(按优先级);支持数值输入宽高与方位角并在地图上实时预览;生成前显示航线数、总长度、ETA、预估电量;**参数预设**(保存/载入如"40 m AGL · 5 m/s · 5 m 间距");Generate 按钮不可用时就地显示原因;规划编辑支持撤销/重做(现状 *verify*);默认 AGL 与文档一致。 | `AreaStep.tsx`、`PatternStep.tsx`、`PlanSteps.tsx`、`mission/patterns.ts`、A6 | 打开即为 100 × 100 m;预设保存后重启仍在;撤销/重做测试 | M | A6 |
| **U9** P1 | **"准备测区"卡片**(H9):显示 HRDEM 项目名、采集年份、分辨率、DTM/DSM 是否齐全、基准状态、体积与下载进度(可取消);地图上预览覆盖范围;离线就绪徽标;失败原因(跨项目、无覆盖、离线、格网缺失)都有明确文案与下一步。 | `AreaStep.tsx`、`terrain_service.rs`、`LayersPopover` | 各失败场景截图;联网一次后离线可用 | M | H9 |
| **U10** P2 | **VIC 日变面板**:状态胶囊(`VIC ● 1 s · 8 s 前`)、F(t) 曲线与 ΔF、距测区距离、扰动等级、数据源与署名;事后改正时对比"原始 / 已改正"。 | 新 `DiurnalPanel.tsx`、TopBar pill | 合成数据下曲线与数值正确;断网时状态变灰并给出原因 | M | M7, M8 |
| **U11** P2 | **多窗口协调**:`Tools` 菜单显示各工具窗口的运行状态(如电机测试"发送中"徽标)与"置前";记住窗口位置与大小 *verify 现状*;所有窗口共享告警横幅(U3)与链路状态指示。 | `TopBar.tsx`(Tools)、`actuator_service.rs` 等窗口创建处 | 手动:切换窗口不丢失告警;截图 | S–M | U3 |
| **U12** P2 | **现场模式与无障碍**:设置里的"Field mode":点击目标 ≥ 44 px、基础字号 14 px、高对比;与现有 `night-red` 主题兼容;完整键盘导航与焦点环;颜色之外的状态冗余(图标/形状);可减少动画;全局快捷键(如 `Esc` 关闭、`?` 显示快捷键表),**不为危险命令设单键快捷键**。 | `design-system/{theme,index.css}`、`SettingsDialog.tsx`、各组件 | 脚本断言:现场模式下所有可交互元素 ≥ 44 px;键盘可完成"连接 → 查看清单 → 发 RTL" | M | U2 |
| **U13** P2 | **反馈与长任务**:上传任务、DEM 下载/构建、VIC 取数、日志下载统一显示进度、已用时、可取消;空状态与错误状态有统一样式;单位集中设置(m、m/s、nT、坐标格式)。 | `components/`、各 store | 各长任务有进度与取消;截图 | M | — |
| **U14** P2 | **设计系统整理与首次使用**:修复 `RightInspector` 264 px vs 令牌 288 px、硬编码字号;补 `HoldButton`、`ConfirmBar`、`Toast`、`ChecklistRow`、`Gauge`、`Banner` 到组件库与 gallery;基线截图扩展到新阶段与现场模式(深/浅/夜间红光 × 1366/1920);首次启动向导(连接 → 校准文件 → HOME/车辆 → 准备测区 → VIC 设置)与 `?` 帮助。 | `design-system/`、`gallery/`、`App.tsx` | 基线截图更新并通过;向导可跳过且可重新打开 | M | U1 |
| **U15** ◆ P2 | **地图可分离窗口(detachable)**:默认地图内嵌;"弹出地图"创建独立 webview(沿用 Inspector/Actuator/Params 的窗口模式),弹出后主窗口切换为**无地图布局**(放大 HUD、测量运行面板、清单);关闭地图窗口自动回嵌且不丢状态。**主窗口必须自足**:TopBar、HUD、命令状态条、告警在无地图时完整可用,地图只是增强视图。地图窗口同样挂统一告警横幅与链路状态条(U3/U11),全屏地图时 Critical 仍可见。窗口语义:关闭地图窗口≠退出;主窗口关闭走 `Lifecycle::shutdown`,地图窗口随之关闭(L4)。地图渲染崩溃/WebGL context lost 时仅该窗口受影响(A8),主窗口显示"地图不可用"并可一键重开。窗口位置/大小/所在显示器记忆;目标显示器被拔掉时回落到主屏。 | `TopBar.tsx`、`App.tsx`、`MapView.tsx`、`useCesiumViewer.ts`、窗口创建处(`app-tauri`)、`stores/ui.ts` | 自动测试:注入地图窗口崩溃/强制关闭后,主窗口发 RTL ≤ 2 次交互且命令走 A7 Emergency;弹出/回嵌前后任务、选中、图层、相机状态哈希一致(A11);地图窗口内出现 Critical 横幅并可确认;拔掉第二显示器后窗口回落主屏(手动 + 截图) | L | A11, A1, A3, A8, U3, U11 |
| **U16** P1 | **步行引导与运行界面**(`WalkMag*` 类型的 Fly 阶段):大号"航线 k/N · 进度 %"、**横向偏差条**(±m,居中为绿)、速度与建议速度带、GNSS 质量胶囊(fix/HDOP/卫星)、磁力仪读数与速率、航线 QC 胶囊、下一航线箭头;偏离/超速/GNSS 降级有**不同的声音与震动提示**(设备支持时);站点式显示"采集中 n/N s"大按钮;暂停/续测/重测为大目标按钮,中风险动作走 U4 的 `HoldButton`;适配单手、手套与日光(与 U12 现场模式联动);离线地图可用。 | 新 `WalkGuidancePanel.tsx`、`stores/walk.ts`、`entities.ts` | 合成轨迹回放:偏差条与 W3 数值一致;GNSS 位置过期时整块置灰显示 STALE + 秒数;现场模式下所有可交互元素 ≥ 44 px;截图 | M | W3, W4, U1, U4 |

### 7.4 可度量的 UI 验收(写成脚本,进 CI 或随 PR 提供)

- 任一阶段、任一视图下,**发出 RTL 不超过 2 次交互**。
- **现场模式**下所有可交互元素的可点击区域 ≥ 44 × 44 px;普通模式 ≥ 32 px;最小字号 ≥ 12 px(现场模式 ≥ 14 px)。
- 状态组(Armed/Mode/Link)在 1280、1366、1920 宽度均不折叠、不被遮挡。
- 每个飞行前 Fail 项均有"原因 + 修复入口"。
- Critical 告警在所有窗口可见,确认后全部消失。
- 对比度:正文与背景 ≥ 4.5:1(各主题);状态不只靠颜色表达。
- 截图基线:5 个阶段 × (深/浅/夜间红光) × (1366/1920);另加"地图已分离"布局(主窗口无地图 + 地图窗口),每个主题至少一张。
- **地图窗口故障隔离**:强制关闭或崩溃地图窗口后,主窗口 RTL/Hold、告警、命令状态条仍可用(自动化断言)。
- 地图弹出/回嵌的往返不改变任务、选中项、图层与相机状态(状态哈希比较)。

---

## 8. 路线图

| 阶段 | 内容 | 退出标准 |
|---|---|---|
| **P0 安全底座** | H0(并行)、A1、A3、L1、L2、L3、L4、U2、U3、U4 | 已解锁时无任何路径能误退出/断开;STATUSTEXT 可见;RTL 常驻;心跳丢失横幅;所有窗口显示告警;命令确认分级统一;相关假飞控测试进 CI |
| **P1 记录与检查** | A7、A6、L6、A5、L5、U5、A4、L7、L8、A2、U1 | 飞行前检查能阻止解锁并可强制;每次飞行都有 tlog + 事件日志 + 清单骨架;五阶段导航可用 |
| **P2 HRDEM** | H1→H2→H3→H4→H7、U8、U9 | 夹具测区从联网获取到 drape 全流程通过;基准链控制点验收通过;规划默认 100 m × 100 m |
| **P2b 项目与测量类型** | PJ1、PJ2、PJ3(与 M1 同步设计,M1 直接按 PJ2 拆分) | 项目可新建/打开/重启恢复;注册假类型无需改通用代码;会话进行中切项目被拒 |
| **P3 磁测与日变** | M7→M8、M1、M5、M2、M4、U6、U7,随后 M3、M9、U10 | 一次 SITL 整任务:航线事件正确、日变改正残差达标(合成数据)、drape 报告可复现、归档清单可重建任务、测量运行面板与地图同步 |
| **P3b 步行磁测** | W0(尽早,并行)→ W1→W2→W3→W4→W5、W6、U16;随后 W7、W8 | 模拟器端到端:GNSS+磁数据 → 引导 → 航线事件 → 记录 → QC(含 VIC 改正残差);测前检查可阻止/强制开始;崩溃后记录可解析;仪器断线重连有告警 |
| **P4 收尾** | A9、A8、A10、A11、H5、H6、H8、H9、H10、H11、M6、L9、L10、U11–U15、PJ4、PJ5 | 删除 TS drape;地图可分离且崩溃隔离通过;现场模式与基线截图完成;文档合并;`issues.md` 清理 |

依赖要点:A3 → L1/L3/L7/U3;A1 → L4/A4/A2;A5+L6 → L5 → U5 → M2;H0 → H1/H2 → H3 → H4 → (H7, M5, A9);M7 → M8 → M3/U10;A4 → M1/M4/M8;U4 → L2;A11+A1+A3+A8+U3+U11 → U15;A6 → PJ1 → PJ2(与 M1 同步)→ PJ3;W0 → W1 → W2/W3 → W4 → W5;PJ2+A5 → W6。

---

## 9. 待核实 / 风险 / 待操作员确认

**待核实**

1. ~~覆盖维多利亚地区的 HRDEM 项目、采集年份、资产命名、nodata、垂直精度(H0)。~~ **已解决(H0/ADR-018)**:项目 `BC-Vancouver_Island_Sunshine_Coast_2018-1m`(2019-10-01),DTM+DSM,EPSG:3979/NAD83(CSRS),CGVD2013,nodata −32767;垂直精度按产品规格"一般优于 1 m"(项目级数值在项目 File Geodatabase,未取)。
2. ~~NAD83(CSRS) 与 WGS84 水平差对 1 m 数据的影响(H0(c))。~~ **已决定(H0/ADR-018 §D)**:取空变换(视 NAD83(CSRS)≡WGS84),残留约 1 m,交由 M9/H7 控制点核对;需要时回退 EPSG:1946。
3. CGG2013 格网的可获得性与格式(H7,issues N2)。
4. VIC 经 NRCan FDSN 服务的实际可用通道(`LFF`/`LFX…` 与位置码)、秒级总场的真实采样率、近实时延迟、是否需要登记邮箱(M7)。
5. VIC 是否在 INTERMAGNET 提供秒级数据(回退路径)。
6. ~~tlog 格式是否与 QGC 完全一致(A4)。~~ **已解决(A4/ADR-021)**:QGC 文档为"8 字节自 Unix 纪元起微秒的 `uint64_t`(`memcpy`,主机字节序)+ 原始 MAVLink 包(含起始符)";目标平台均小端,写端取小端 u64,读端按自定界重新分帧以兼容 QGC 文件。大端主机需另复核。
7. PX4 v1.17 的日志与速度限制参数名(M2、M5)。
8. 解锁被拒时 STATUSTEXT 原因是否稳定随后到达(L1)。
9. 主窗口先关而子窗口仍开时的进程行为(L4);子窗口是否显示链路/飞行告警(U3)。
10. 规划编辑是否已有撤销/重做、窗口位置是否已记忆(U8、U11)。
11. 地图与规划共享的前端 store 范围(任务、选中、图层、相机、drape 结果),决定 A11 的工作量(U15)。
12. Tauri 多 webview/窗口在目标平台(Windows/macOS/Linux)上的行为:Cesium 在独立窗口的 WebGL 初始化、窗口回落、context lost 恢复(U15、A8)。
13. ~~步行磁力仪与 GNSS 的实际型号、接口、输出格式与采样率(W0)。~~ **已确认(W0/ADR-020)**:DroneCAN 磁力仪(RM3100 或 Bartington UAS-Mag,单传感器)+ ZED-F9P RTK,数据全部走 MAVLink(PX4 仅作仪器中枢);磁数据取 **`HIGHRES_IMU`(PX4 已校准)** sensor 1,**≈100 Hz**。**仍待实机确认**:具体节点型号与杆高(杆高为项目/操作员输入)。

**风险**

- **NRCan 地磁数据条款**:仅供申请者及合作者使用,未经书面许可不得向第三方分发,并要求寄送报告副本,商业用户需联系 NRCan。若本项目的测量属商业性质,在上线 M7 前必须先取得许可或确认适用范围。代码层面已按"不分发、仅本地缓存"实现。
- 日变空间相关性:VIC 在维多利亚以北约 16 km,测区距离越大改正越不可靠;M8 记录距离并告警。
- 1 m 数据下,基准链、水平基准和机体离地高度的误差可能超过 DEM 自身误差;M9 与 H7 的目的就是让它们可见,不得用放宽阈值来"通过"验收。
- HRDEM 项目采集年代可能较旧,与现场植被/地表不一致:靠 M4 留痕与 H6 提示。
- **步行仪器细节**:协议已定为 DroneCAN/MAVLink(ADR-020);磁数据取 **`HIGHRES_IMU`(PX4 已校准)**、**≈100 Hz**(已确认)。W1 先用 MAVLink 合成流跑通管线、记录与 QC;仅剩具体节点型号与杆高待实机/项目输入。
- **步行 GNSS 质量**:树冠下浮点/丢星会让位置误差大于测线间距;W5 必须把这类段落显式标记,不得静默通过。
- **操作员磁污染**:身上铁磁物会直接污染数据,W6 的磁洁净检查只能发现明显问题,不能替代操作规程。
- **地图分离的状态漂移**:两个 webview 若各持一份状态会漂移;A11 未完成前不得上线 U15。
- **项目模型迁移成本**:PJ1 引入后,A6 与 M4 的存放位置会变;先定 ADR-019 再动 A6/M1,避免二次迁移。

**需要操作员确认**

1. 测区中心坐标(H0 夹具需要;未提供则用占位 AOI)。
2. 本项目使用 VIC 数据是否属商业性质(决定 M7 上线前是否必须取得 NRCan 许可)。
3. 日变改正是否需要**飞行中实时**显示,还是事后改正即可(决定 M7 的轮询与延迟要求)。
4. 日变扰动门限(弦长与变化率)的取值来源(合同/规范)。
5. 现场使用的设备类型(笔记本 / 平板 / 触屏)与是否戴手套(决定 U12 的默认现场模式与尺寸)。
6. 是否使用多显示器(决定 U15 的优先级;单屏/平板场景下建议保持内嵌默认)。
7. ~~步行磁测采集方式~~ **已确认(W0)**:以连续步行为主(站点式仍留 W4);**非梯度**(单传感器);磁采样率 **≈100 Hz(`HIGHRES_IMU`)**;仍待定:杆高。
8. ~~步行仪器由 GCS 直接记录,还是仪器自带记录~~ **已确认(W0)**:两者并用——PX4 SD/ULog 为全速率主记录,GCS 消费 MAVLink 实时磁+位置并另存。
9. ~~步行测量的交付格式~~ **已部分确认(W0)**:交付 **CSV**(字段表见 ADR-020);坐标系为 WGS84,高度含椭球高与 AMSL;是否需其他格式与基准字段仍待定(影响 PJ1 项目坐标系字段)。
10. 同一项目内是否会对同一测区先后做 UAV 与步行测量并比较(决定 PJ5 的优先级)。
