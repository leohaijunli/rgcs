# MagGCS 综合实现方案 v2(基于最新代码)

2026-10-11 · 基线:`rgcs-main__3_.zip`(对照上一版 `rgcs-main__2_.zip`)。本文取代上一版综合方案,把五条工作线放在同一张路线图里,并新增第六条:**飞控参数窗口**。

细节文档:

- `parameters-window-plan.md`:参数窗口(新)
- `motor-test-plan.md`、`ui-layout-optimization-plan.md`:早期方案,**大部分已实现并并入 `docs/FEATURES.md` / `ARCHITECTURE.md`**,其中的剩余项在本文汇总;文件顶部已加状态说明

依据:`docs/ARCHITECTURE.md`、`docs/FEATURES.md`、`issues.md`,以及对 `core/{motor_test,calib,signals,rtk,commands,ulog,mavlink}`、`app-tauri`、`frontend/src/{App,components,stores,actuator,design-system}` 的阅读。没有编译或运行过(环境里没有 Rust 工具链)。标 *verify* 的需要实测。规模标记:S < M < L。

---

## 1. 最新代码相对上一版的变化

| 方面 | 变化 |
| --- | --- |
| 文档结构 | `DEVELOPMENT_PLAN.md`、`improve_plan.md`、`HANDOFF.md`、两份 signal-inspector 计划已删除,内容并入 `ARCHITECTURE.md`(架构、PX4 接口事实、ADR 索引)、`FEATURES.md`(功能说明)和 `issues.md`(未完成项);`docs/README.md` 记录了这次整理 |
| 布局 | 左栏(`IconRail`、`Drawer`)、Data 视图、`TelemetryDialog`、`PfdCluster`、`VerticalTape`、`QcDialog` 已删除;Plan 面板拆成 `panels/plan/{Area,Pattern,Waypoints,Sync}Step`;新增 `LinkPopover`;TopBar 有 `Tools ▾`;地图有左上工具条与 `Layers ▸`/`View ▸` |
| 电机测试 | 新增 `core/motor_test`、`core/calib`、`signals/derived.rs`、`actuator_service.rs`、`motor_test_state.rs`、`frontend/src/actuator/`,有集成测试;含手动 / 预设波形 / **SITL 实时转发**三种来源、风险分级安全门、磁场报警 |
| Fly 命令 | 现为 Takeoff / Land / RTL(两步确认);Pause / Continue 已移除——实测 PX4 v1.17 对 `DO_PAUSE_CONTINUE` 回 NACK |
| 未变化 | RTK(只有 RTCM 解析与分片,无服务/UI)、模式切换(无)、参数协议(无)、设计令牌(只多了 Inspector 的曲线调色板) |

---

## 2. 各工作线现状

| 工作线 | 状态 | 剩余 |
| --- | --- | --- |
| **W1 布局重构** | 基本完成 | Dock 现在 `TABS` 为空、不渲染(`profile`/`log` 常量留给 Phase 3b/5,issues O5);基线截图尚未重拍;遥测去重与叠层不重叠的自动化验收需要补 |
| **W2 UI 美化** | 完成(UI1–UI6) | 令牌 v2 ✓、设计系统组件(`components.tsx` Button/IconButton/Popover/Tabs/StatusDot)+gallery ✓、TopBar/MapToolbar/右面板迁移 ✓、底图降噪与 hillshade 融合 ✓、主题选择(跟随系统+夜间红光) ✓、8 张基线截图(深/浅 × 1366/1920 × Plan/Flight)已重拍 |
| **W3 飞行模式切换** | 未开始 | `MODE` pill 只读;前端 `util/px4mode.ts` 已对照 v1.17 的 `px4_custom_mode.h` 核对,可作为迁移依据;没有 `MAV_CMD_DO_SET_MODE` |
| **W4 RTK** | 仅基础件 | 数据源、注入服务、状态/告警、设置页、TopBar 弹窗;`i18n` 里已有"RTK base 可在 Connection 中选择"的提示文案,说明设置页的入口是预留的 |
| **W5 电机测试** | 窗口与核心已完成 | Profile(ULog)与 Function(脚本)来源(代码里写明"A6/A7 待做")、记录导出(A8)、曲线补全(O3)、自定义报警音(O2)、M0 台架实测(D2)、GoPro 监视(D1,已暂缓) |
| **W6 参数窗口** | 新增 | 全部,见 `parameters-window-plan.md` |

### 2.1 需要先对齐的一处差异:发送频率

电机测试现在是 **10 Hz** 的 `ACTUATOR_TEST`,每条带 0.3 s 飞控侧超时(`DEFAULT_TICK_HZ = 10`、`COMMAND_TIMEOUT_S = 0.3`);`ARCHITECTURE.md` §5 说明这些 PX4 事实已按 v1.17 源码核对。你之前提出的是 ≥ 50 Hz。如果仍然需要更高频率,先在 M0 台架上测 PX4 对 `ACTUATOR_TEST` 的处理速率和输出更新节奏;否则就以 10 Hz 为准,并把原因写进 `FEATURES.md`。

---

## 3. 对前几版方案的修订

| # | 修订 |
| --- | --- |
| 1 | 模式切换不再依赖暂停/继续:Hold 用 `DO_SET_MODE` 到 Auto·Loiter 实现(*verify* v1.17 对 `DO_SET_MODE` 的处理);Takeoff / Land / RTL 保持现有 NAV 命令 |
| 2 | Fly 面板的"模式区"= 现有命令(Takeoff/Land/RTL)+ 模式选择器,不再计划把 Pause/Continue 放进去 |
| 3 | 右侧面板宽度沿用令牌 `--mg-panel-w`(288 px) |
| 4 | 模式表以 `px4mode.ts`(已核对 v1.17)为准,迁到 `core` 并用 ts-rs 生成,前端消费生成类型 |
| 5 | 新建的方案文档放 `docs/plans/`,实现后按这个仓库的惯例并入 `FEATURES.md` / `ARCHITECTURE.md` 并删除 |
| 6 | `ARCHITECTURE.md` 的 crates 表与窗口表需要在新窗口(`params`)上线时同步 |

---

## 4. 剩余工作线详述

### 4.1 W3 飞行模式切换

**命令路径**:`MAV_CMD_DO_SET_MODE`(`param1 = 1` 启用自定义模式、`param2 = 主模式`、`param3 = 子模式`),走现有单槽 `CommandService`(低频、要 ACK)。`app-tauri` 的命令名映射增加 `set_mode`,前端 `CommandName` 增加同名类型。

**模式表放 core**:新增 `core/src/telemetry/px4_mode.rs`(编码 / 解码 / `available_modes(state) -> Vec<ModeOption { mode, enabled, reason }>`),`ts-rs` 生成类型;`util/px4mode.ts` 改为消费生成类型,避免两份实现。

| 显示名 | 主 / 子模式 | 可用条件(不满足时灰显并给原因) |
| --- | --- | --- |
| Hold | Auto(4) / Loiter(3) | 链路在线 |
| Mission | Auto(4) / Mission(4) | 飞控上已有任务且与本地同步;GPS 满足要求 |
| Return | Auto(4) / RTL(5) | 已解锁;有 home 点;需二次确认 |
| Land | Auto(4) / Land(6) | 已解锁;需二次确认 |
| Position / Altitude | 3 / 2 | 遥控器可用(提示) |

Manual、Acro、Offboard 默认不提供,放进设置里的"高级模式"。

**交互**:`MODE` pill 改为 `MODE ▾` 弹窗(复用 `LinkPopover` 的弹窗骨架);发出后显示"切换中…",以随后 `HEARTBEAT.custom_mode` 为准回读确认(约 3 s 超时),不做乐观显示;切换期间禁用重复点击;电机测试运行时禁用切换。

| WP | 内容 | 规模 |
| --- | --- | --- |
| MS1 | core:模式表 + `available_modes` + 编码测试;删除前端重复逻辑 | S–M |
| MS2 | 命令映射 + Tauri 命令 + 假飞控集成测试(ACK / 拒绝 / 超时) | M |
| MS3 | 前端 `MODE ▾` 弹窗、回读确认、Fly 面板模式区 | M |

验收:编码与解码互逆;ACK 接受 / 拒绝 / 超时三种路径界面行为正确;Return / Land 必须二次点击;回读与 ACK 不一致时以回读为准并提示。

### 4.2 W4 RTK 模块

已有:`rtk/rtcm.rs`(RTCM3 帧解析 + 重同步)、`rtk/forward.rs`(分片成 `GPS_RTCM_DATA`)、TopBar 的 RTK fix pill。待做:

| WP | 内容 | 规模 |
| --- | --- | --- |
| R0 | 查 PX4 对 `GPS_RTCM_DATA` 分片的接收行为,尤其"恰好 360/540 字节"的边界(当前 `fragment()` 不补零长度结束包);确认机上 GPS 类型与链路速率 | S |
| R1 | `RtcmSource` trait + `SerialSource`(复用现有 `serialport` 与 `devices/`)+ 测试替身 | M |
| R2 | `RtkService`:组帧 → 分片 → `ConnectionHandle::send()`;单源互斥;发送队列有上限,满则丢旧并计数;序号递增;B/s 统计 | M |
| R3 | 告警(中断 > 5 s)、各 RTCM 类型速率、数据新鲜度、带宽提示 | S |
| R4 | Tauri 命令 + 设置页 RTK 页(设置里已为"RTK base 串口"预留文案)+ TopBar `GPS/RTK ▾` 弹窗 | M |
| R5 | 实机:F9P 基站 → GCS → PX4,记录达到 RTK Fixed 的时间 | S |
| R6 | F9P 基站配置(survey-in / 固定坐标、消息集、保存) | M |
| R7 | 可选:TCP/UDP 与 NTRIP 数据源(TLS 依赖先对照 `deny.toml`) | M |

要点:与 QGC 共存时提供注入开关并在检测到多源时警告;低速数传下建议 MSM4;fix 状态颜色同时配图标与文字。基站数据源是 F9P 串口还是 NTRIP 仍待你确认。

### 4.3 W2 UI 美化

目标与原则:数据优先、低噪声、状态不单靠颜色、户外可读、同类控件一种长相、颜色/间距/圆角/阴影全部来自令牌(`check:colors` 继续强制)。

**令牌 v2**(在现有 `--mg-*` 上扩展):表面层级(`--mg-surface-0/1/2`)、状态淡底(`--mg-ok-soft` 等)、焦点环、圆角(4/8/12)、阴影(1/2)、动效(120/200 ms)、层级(`--mg-z-*`)、等宽数字、`--mg-pill-h`、`--mg-touch`。色值以 `check:contrast` 通过为准。

**组件**(从现有代码里抽取并统一,而不是重写):`StatusPill`(现为 `TopBar` 内部组件)、`Button` / `IconButton`(现有 `ToolButton`)、`Segmented`、`Section`(Plan 面板的分步卡片可直接复用)、`PopoverMenu`(`LinkPopover`、`ToolsPopover`、地图的两个弹出菜单目前各自实现)、`Banner` / `Toast`(`MockBanner` / `ErrorBanner` 合并)、`EmptyState`、`Dialog`、`Slider`(电机测试窗口里已有一份)。交付一个仅开发可见的组件陈列页,深浅主题下一次看到全部状态。

**主题**:深色(默认)、浅色(户外高对比)之外,增加"跟随系统",可选"夜间红光"(保护夜视);现在的 `Theme` 类型只有 `dark | light`,设置页的 Appearance 页放选择。

**地图叠层与 HUD**:叠层统一玻璃面板令牌 + 阴影 + 圆角 + 固定边距;航点与航线样式统一;底图网格降噪(旧截图里青色网格线很亮,*verify* 当前版本);磁场图层使用感知均匀、色盲友好的色带,不用彩虹色带。

**其余**:1366×768 紧凑密度;所有交互元素有可见焦点环;过渡 120–200 ms 并尊重"减少动效";可选增加 `zh-CN` 语言包(`check:i18n` 校验键一致)。

| WP | 内容 | 规模 |
| --- | --- | --- |
| UI1 | 令牌 v2;`check:colors` / `check:contrast` 全绿 | M |
| UI2 | 基础组件抽取 + 组件陈列页 | L |
| UI3 | TopBar / 右侧面板 / 工具条迁移到新组件;`MODE ▾`、`GPS/RTK ▾` 随之落地 | M |
| UI4 | 地图叠层与 HUD 视觉统一、底图降噪、航点/航线样式 | M |
| UI5 | 主题选择(跟随系统、可选夜间红光) | S |
| UI6 | 重拍截图基线:深/浅 × 1366×768 / 1920×1080 × Plan / Fly(同时解决 issues O5) | S |

### 4.4 W5 电机测试剩余项

| WP | 内容 | 规模 | 备注 |
| --- | --- | --- | --- |
| A6 | Profile 来源:扩展 `ulog/` 读取器展开 `esc_status` 的嵌套数组(当前不展开)、读取 `actuator_motors`;重采样、电机映射、播放 / seek / 倍速;`SessionState` 增加 `ProfileRunning/Paused` | L | 现有"SITL 实时转发"覆盖了一部分需求,但不能回放实飞记录 |
| A7 | Function 来源:内置波形已有预设,补用户脚本(建议 Rhai,依赖前对照 `deny.toml`) | M | |
| A8 | 记录与导出:CSV + 元数据 JSON(负载、模式、校准文件哈希、基准值、PX4 版本、联锁阈值) | S–M | 现有代码里没有记录功能 |
| O3 | 曲线补全:指令 + 姿态组、磁场基准偏移组、单条曲线显示/隐藏 | M | issues O3 |
| O2 | 自定义报警音文件 | S | issues O2 |
| M0 | 台架实测(`COM_MOT_TEST_EN=1`、`ACTUATOR_TEST` 语义、`ESC_STATUS` 速率、SITL 转发端到端、报警参数标定) | S | issues D2,需要硬件 |
| — | GoPro 监视 | — | issues D1,操作员指示暂缓,不排期 |

联动:电机测试窗口通过参数缓存读取 `COM_MOT_TEST_EN`,不满足时提示并跳转参数页(`parameters-window-plan.md` PM9)。

### 4.5 W6 参数窗口(摘要)

独立窗口 `params`,沿用 `actuator` 窗口的全部范式(Vite 入口、capabilities、`*_service.rs`、`Destroyed` 清理、`Tools ▾` 入口)。核心在 `core/src/params/`:`codec`(两种值编码)、`session`(拉取位图 + 补读、设置 + 回显确认的纯状态机)、`service`、`cache`、`file`(QGC `.params` / JSON)、`diff`。界面:分组树 + 虚拟化参数表 + 详情面板 + 暂存区;编辑后批量应用并自动快照,失败可回滚;导入先看差异;解锁状态下默认禁止写入,高风险参数输入式确认;重启后提示重新拉取。元数据先用随应用打包的 PX4 v1.17.0 `parameters.json`(由 `tools/param-meta/` 从你的 PX4 checkout 生成)。界面相对 QGroundControl 做了优化:三栏常驻(分组树 / 虚拟化表 / 详情)、内联编辑而非模态框、行内校验、状态符号、边拉边填、变更托盘、窗口壳与另外两个窗口统一;不增加新功能范围。完整设计、协议要点、工作包 PM0–PM10 与验收见 `parameters-window-plan.md`。

---

## 5. 共用基础设施

| 项 | 说明 |
| --- | --- |
| `Tools ▾` | `Signal inspector`、`Motor test`、新增 `Parameters` |
| 状态弹窗骨架 | `LinkPopover` 是现成样板;`MODE ▾`、`GPS/RTK ▾` 复用,统一抽成 `PopoverMenu` |
| 两步确认 | `FlightCommands` 的确认逻辑、电机测试的风险分级安全门、参数窗口的高风险确认——共用一个确认交互模式 |
| 消息路由 | `MessageRoute` 增加 `PARAM_VALUE`(参数)与 RTK 相关订阅;其余服务不受影响 |
| 假飞控测试 | 沿用可丢包的假飞控(`mission_fake_fc.rs`)与 `motor_test_service_integration.rs`,给模式切换与参数各加一份 |
| 类型与 i18n | 所有新类型走 `ts-rs`;所有新文案走 `i18n` 并通过 `check:i18n` |
| 可选共享件 | `core/mavftp` 客户端:将来同时服务参数元数据获取与 Logs 页(issues O1);当前不排期 |

---

## 6. 综合路线图

### 6.1 依赖

```
UI1 令牌 ─► UI2 组件 ─► UI3 迁移(+ MODE▾ / GPS·RTK▾) ─► UI4 ─► UI5 ─► UI6
MS1 ─► MS2 ─► MS3(依赖 UI2 的弹窗组件)
PM0 ─► PM1 ─► PM2 ─┬─► PM3 ─► PM5 ─► PM6 ─► PM7 ∥ PM8 ─► (PM10)
        PM0 ─► PM4 ┘                       PM2 ─► PM9
R0 ─► R1 ─► R2 ─► R3 ─► R4(依赖 UI2) ─► R5 ─► (R6, R7)
M0 台架 ─► A6 ─► A7 ─► A8 ─► O3 / O2
```

四条后端线(MS1–MS2、PM1–PM2、R1–R3、A6)互相独立,可以并行;前端要等 UI1–UI2 的令牌与基础组件。

### 6.2 阶段

| 阶段 | 内容 | 出口 |
| --- | --- | --- |
| **1 基础** | UI1 令牌;UI2 组件抽取;MS1 模式表;PM0 实测与确认;PM1 参数 core 基础;R0 查分片行为;M0 台架实测(需要硬件时可并行) | 组件陈列页可用;令牌检查全绿;模式表与参数 codec 有测试 |
| **2 框架与核心** | UI3 迁移;MS2–MS3 模式切换上线;PM2 服务 + 假飞控测试;PM4 元数据工具;PM9 `COM_MOT_TEST_EN` 联动;R1–R3 RTK 后端 | TopBar 有 `MODE ▾`;SITL 下可切换模式;参数在假飞控上可拉取并设置 |
| **3 窗口** | PM3 / PM5 / PM6 参数窗口;R4 RTK 设置页与弹窗;A6 Profile 回放;A7 函数脚本 | 参数窗口可浏览、编辑、应用;RTK 可从界面开关并看状态 |
| **4 实机与收尾** | PM7 / PM8;R5 RTK 实机;A8 记录导出;O3 / O2;UI4–UI6;文档同步;(可选 PM10、R6、R7) | 实机 time-to-Fixed 有记录;基线截图重拍;文档并入 FEATURES / ARCHITECTURE |

### 6.3 排序理由

- 令牌与组件放最前:模式弹窗、RTK 弹窗、参数窗口都要用,先做能避免重复。
- 模式切换后端很小(一个命令 + 一张表),先于前端完成,可以早早在 SITL 里验证。
- 参数窗口的价值很直接(电机测试前置条件、磁力计校准域、快速备份飞控配置),PM2 之后就能做 PM9,把电机测试的前置条件变成可见提示。
- RTK 后端与界面无关,可以并行;实机测试放后面。
- 电机测试剩余的 A6 以 M0 台架实测为前提。

---

## 7. 风险与待确认

| 项 | 说明 |
| --- | --- |
| PX4 对 `DO_SET_MODE` 的拒绝条件 | 不同状态下可能被拒;以 ACK 与心跳回读为准(*verify* v1.17) |
| `GPS_RTCM_DATA` 分片边界 | 见 R0;若 PX4 按片重组,需要在 `fragment()` 补边界规则与测试 |
| 参数写入的风险 | 见参数方案 §7;高风险清单在 PM0 确认 |
| 令牌改动的回归面 | UI1 影响全局颜色;靠 `check:colors` / `check:contrast` 与基线截图兜底 |
| Windows 定时 | 若电机测试将来要 ≥ 50 Hz,发送线程需要独立线程与高精度定时 |
| 范围蔓延 | 闭环转速跟踪、多机、改 PX4 固件、解锁/起飞按钮之外的新操作不在本方案内 |

**需要你确认**:

1. 模式列表(Hold / Mission / Return / Land / Position / Altitude)是否合适?要不要加解锁?
2. RTK 基站是 F9P 串口还是 NTRIP?
3. 电机测试发送频率:保持 10 Hz,还是需要 ≥ 50 Hz(先做 M0 实测)?
4. 是否需要中文界面与"夜间红光"主题?
5. 参数窗口:是否允许飞行中改参数(建议默认关闭)?元数据先用打包的静态文件是否可以?
6. 磁力计 JSON 的校准输入域:现有 `calib/mag.rs` 的模型是 `A·(raw−b)+K·I`,请确认与你的 JSON 一致。

---

## 8. 文档与 issue 同步

- 新方案放 `docs/plans/`(参数窗口、综合路线图),实现后并入 `FEATURES.md` / `ARCHITECTURE.md` 并删除,沿用仓库现有的做法。
- `issues.md`:新增未完成项:
  - 模式切换(MS1–MS3)、RTK(R0–R7)、UI 美化(UI1–UI6)、参数窗口(PM0–PM10)、电机测试剩余(A6–A8)各一条总 issue,WP 作为子项;
  - 与现有的 O2、O3、O5、D2 互相引用。
- ADR:ADR-017 飞行模式切换与确认策略;ADR-018 参数协议与安全策略;RTK 沿用 ADR-008 补充分片边界结论。
- `ARCHITECTURE.md`:crates 表增加 `params/`,窗口表增加 `params`,PX4 接口事实增加 `DO_SET_MODE`、参数协议的核对结论。
- `FEATURES.md`:随各功能上线增加对应小节。
- `docs/README.md`:列出 `docs/plans/`。
