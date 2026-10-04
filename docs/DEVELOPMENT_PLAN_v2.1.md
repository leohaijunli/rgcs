# MagGCS 开发计划 v2.1

2026-10-03 · 在 v2.0 基础上新增：RTK 基站连接与 RTCM 转发、USB 设备 udev 规则自动配置、科技感 UI 与 UgCS 式布局

## 一、定位与边界

**定位**：与 QGroundControl 并行使用的开源地面站，面向测绘/地球物理类无人机作业。飞控设置、传感器校准、Airframe、Radio 仍交给 QGC。

**三个差异化支柱**

1. Survey-grade 规划：3D 测线、切割线、沿地形 drape、爬升率约束，任务参数与 DEM 版本一并存档。
2. 实时数据与质控：磁场曲线、航迹偏差、AGL 偏差、RTK 状态一眼可见。
3. 开放集成：兼容 QGC `.plan`，提供 headless 模式与 REST/WebSocket API。

**目标机型**：先按多旋翼。**不做**：传感器校准、Airframe、Radio、固件刷写。

## 二、v2.1 变更摘要

- 新增 `core::rtk`：RTK 基站连接、设置、状态显示，并把基站发来的 RTCM 消息经 MAVLink 转发给飞控（第八节）。
- 新增 `core::devices`：USB 设备识别与 udev 规则自动生成/安装（第九节）。
- 新增 UI 设计规范：深色科技感风格，布局参考 UgCS（第六节），设计令牌与界面壳提前到 Phase 0，避免后期返工。
- 阶段重排：Phase 2 为 RTK，地形与 Survey 顺延为 Phase 3。RTK 与地形互相独立，可并行。

## 三、技术决策（每条写成 ADR，放 `docs/adr/`）

| ADR | 决策 |
| --- | --- |
| 001 | 架构：`core` library crate + `app-tauri`（桌面）+ `server`（headless），Tauri 本身是 Rust，不另起后端 |
| 002 | 前端类型由 Rust 经 `ts-rs` 或 `specta` 生成，不手写两份 |
| 003 | 与 QGC 共存：MAVLink 2、独立 system/component ID、UDP 或 mavlink-router 分流（ID 取值对照 PX4 文档确认） |
| 004 | 地形源：本地 GeoTIFF 为默认，Cesium ion 仅可选 |
| 005 | 地形跟随：路径 A（地面站预计算）优先，路径 B（机载）后续对照 |
| 006 | 高度基准：内部统一用椭球高，所有高度类型带基准标签 |
| 007 | 许可证：Apache-2.0 或 GPLv3，Phase 0 内必须定 |
| 008 | RTCM 来源抽象与转发策略（单源注入、分片、带宽统计） |
| 009 | udev 提权策略：polkit + 独立辅助程序，主程序不以 root 运行 |
| 010 | 前端框架与设计系统（建议 React + Zustand + Tailwind + Radix，待确认） |

## 四、仓库结构

```
maggcs/
├── AGENTS.md                 # 定位、不做清单、高度约定、AI 规则
├── crates/
│   ├── core/                 # mavlink/ mission/ telemetry/ terrain/ survey/ mag/ qc/ rtk/ devices/
│   ├── app-tauri/            # 桌面入口
│   └── server/               # headless：REST + WebSocket
├── helpers/udev-installer/   # 提权辅助程序（只写本应用的规则文件）
├── tools/dem-prep/           # LAS → DTM/DSM → COG + 元数据
├── frontend/                 # cesium/ views/ components/ design-system/ stores/ i18n/ generated-types/
├── testdata/                 # 小型 DEM、ULog、.plan、RTCM 录制样本
├── scripts/sitl/
└── docs/adr/
```

## 五、高度与坐标约定

- 三种高度：CGVD2013 正高（BC LiDAR）、WGS84 椭球高（Cesium）、AMSL（PX4 `MAV_FRAME_GLOBAL`）。转换链：`H + N(CGG2013) = h`，`h − N(EGM96) = AMSL`。
- `core` 内禁止裸 `f64` 表示高度，必须用带基准的类型，转换集中在一个模块。
- AGL 默认相对 DTM，另提供相对 DSM 的最小净空检查。
- **验收**：已知高程点经转换后与 PX4 AMSL 偏差 < 0.5 m（阈值待实测调整）。

## 六、UI 设计规范

**视觉风格（科技感）**

- 默认深色主题：深蓝灰底 + 青色（cyan）主强调色，细线框、半透明毛玻璃面板、克制的发光。
- 状态色（绿/黄/红）只表示状态，不做装饰；磁场色标与状态色分开。
- 遥测数字用等宽（tabular）字体，字体本地打包，不依赖在线字体（外场可能无网）。
- 动效只用于状态变化（面板展开、告警）。毛玻璃面板的数量和面积设上限，避免拖慢 Cesium 帧率。
- 提供高对比度浅色主题（户外强光），触控目标不小于 44 px，中英文 i18n 从第一天开始。

**布局（参考 UgCS 的布局思路，不复制其图标、配色和资源）**

- 中央：全屏 3D/2D 地图。
- 顶部：模式切换（规划 / 飞行 / 数据）+ 链路状态条（飞控、RTK、设备）。
- 左侧：任务/测线列表、机体列表、图层管理。
- 右侧：所选对象的属性面板（航点、测线、区域参数）。
- 底部（可折叠）：沿测线的地形/AGL 剖面图，飞行时切换为实时 QC 曲线。
- 悬浮 HUD：姿态、速度、高度、电池、GPS/RTK 状态。
- 我没有逐像素核对 UgCS 当前界面，布局细节请对照其截图确认。

**实现建议（ADR-010 确认）**：React + TypeScript 严格模式、Zustand、Tailwind + Radix（无样式组件）、uPlot（高频实时曲线）。颜色/间距/字号全部定义为设计令牌（CSS 变量），主题切换只改令牌。

**UI 验收**

- 1920×1080 与 1366×768 下关键信息不被遮挡，每个视图有截图基线（Playwright 视觉回归）。
- 代码中无硬编码颜色（lint 规则检查），两套主题均通过对比度检查。
- 打开侧边面板时地图保持 ≥ 30 fps（指定参考硬件）。

## 七、分阶段计划

原则：差异化功能前移，通用控制后移。每个验收标准必须是可自动化测试或带明确数值（下列数值均为初值，需实测校准）。

### Phase 0：骨架、设备枚举与 UI 壳

**任务**

1. workspace、`AGENTS.md`、ADR 草稿、许可证。
2. `core::mavlink`：UDP/串口/TCP 连接、心跳监测、重连；解析 HEARTBEAT、GLOBAL_POSITION_INT、ATTITUDE、SYS_STATUS、BATTERY_STATUS、GPS_RAW_INT。
3. `core::devices` 基础版：枚举串口及 VID/PID/序列号，热插拔事件。
4. 前端 UI 壳：设计令牌、两套主题、布局框架（顶栏/左右面板/底栏）、i18n 骨架；Cesium 基础场景 + 实时位置。
5. GitHub Actions 跑 PX4 SITL 集成测试。

**补充（Phase 0 收尾）**：设置菜单合并为单一 tab 弹窗（连接 / 主题·语言 / 日志 / 设备·udev / 关于），替换原主题 Popover，为后续阶段设置预留 tab 位（RTK、QC）。

**验收**：SITL 连续 30 分钟心跳丢失 0 次；位置延迟 < 200 ms（本地测）；与 QGC 同时连接同一 SITL 正常；UI 验收见第六节。

### Phase 1：Mission Protocol + 规划视图

**任务**：Upload/Download/Clear/Set Current 状态机（超时、重传、断线恢复，MISSION_ITEM_INT）；兼容 MISSION_CURRENT 扩展；规划视图（左侧列表、右侧属性、航点点击与拖拽、三种高度模式）；QGC `.plan` 导入导出；Pause/Continue、RTL。

**验收**：100 航点上传后下载逐项一致；10% 丢包仍能完成上传；`.plan` 往返无信息丢失。

### Phase 2：RTK 基站与 RTCM 转发

任务、验收见第八节。与地形互相独立，可并行开发。

### Phase 3：地形 + Survey 规划器（MVP，可发 alpha）

**任务**：`ElevationSource` trait（GeoTIFF 为默认实现）、高度转换链、多边形 → 平行测线 + 切割线（方位角、间距、外延、转弯半径显式）、沿 DEM 加密并平滑（爬升率/坡度约束）、AGL 剖面图、DEM 元数据显示、任务存档（参数 + DEM 版本 + 航点）。

**验收**：同一存档重新生成航点逐点一致；起伏地形 SITL（需 Gazebo）实飞与设定 AGL 偏差 < 2 m；爬升率超限测线能检出并告警。

### Phase 4：磁数据层与实时 QC（飞行视图完整化）

**任务**：实时磁场曲线、cross-track error、AGL 偏差、噪声指标及告警；加载 GeoJSON/GeoTIFF/CSV/瓦片，色标、透明度、图例；飞后导入 ULog，叠加轨迹与 crossover；飞行视图 HUD 与底部 QC 曲线。

**验收**：10 万点以上 30 fps；SITL 注入偏航/高度偏差后 QC 告警 2 秒内触发；ULog crossover 结果与离线脚本一致。

### Phase 5：按需补齐通用功能

只读参数、必要参数修改、Guided Goto、Set Mode、多机基础、日志回放；评估固定翼。

**补充：PX4 日志拉取/保存**。core 实现 MAVLink LOG 协议（`LOG_REQUEST_LIST`、`LOG_REQUEST_DATA`、`LOG_ERASE`），从飞控拉取 ULog；写入本地日志目录；设置页日志 tab 提供"刷新列表 / 拉取 / 保存为 / 擦除"操作。与日志回放共用 MAVLink 端与存储层。

### Phase 6：发布

Tauri 打包（Linux/Windows）、udev 辅助程序打包与 polkit 策略、离线包（示例 DEM + 大地水准面格网）、文档、插件接口稳定化。

## 八、RTK 与 RTCM 转发设计

**数据流**：RTK 基站接收机（USB 串口）→ `rtk::source` 读取 RTCM3 字节流 → `rtk::rtcm` 按帧解析并校验 CRC → `rtk::forward` 封装为 MAVLink `GPS_RTCM_DATA` → 经飞控链路发送 → 飞控 GPS 驱动注入机载 RTK 接收机。

**来源抽象**：`RtcmSource` trait。Phase 2 实现串口基站；NTRIP 客户端、TCP/UDP 转发作为后续可选实现。同一时刻只允许一个来源注入。

**转发要点**

- `GPS_RTCM_DATA` 单包最多 180 字节，更长的 RTCM 帧需按协议分片（最多 4 片，带序号）。长度恰为 180 的整数倍等边界情况以 MAVLink 文档为准，写单元测试。
- 与 QGC 共存时 QGC 也可能注入 RTCM：提供“RTCM 注入”开关，检测到多源时告警，避免双路注入。
- 带宽：RTCM 数据量取决于消息类型和星座数，低速数传电台可能被占满。统计实际 B/s，UI 给占用提示，支持选择消息集（例如优先 MSM4 而非 MSM7）。
- 基站数据中断超过阈值（默认 5 s，可配置）告警。

**基站设置**（首个支持对象为 u-blox ZED-F9P，其余通过 trait 扩展）

- Survey-in（最小时长、目标精度）或固定坐标模式；输出的 RTCM3 消息集；端口输出配置；保存到 RAM/Flash。
- 先预览再应用，应用后读回校验。

**状态显示**

- 基站：模式、survey-in 进度与当前精度、卫星数、各类型 RTCM 消息速率、数据年龄、累计字节。
- 机载：`fix_type`（RTK Float = 5，RTK Fixed = 6）、卫星数、HDOP、进入 RTK Fixed 的计时。
- 顶部状态条及告警：绿 = Fixed，黄 = Float，红 = 无 RTK 或中断。

**测试与验收**

- RTCM 帧解析单测（坐帧、粘包、半包、错误 CRC），用录制样本；分片后重组字节一致；转发路径用模拟 MAVLink 接收端校验。
- SITL 没有真实 GPS 接收机，只能验证发送正确，不能验证 RTK Fixed；真机验证用基站 + 机载接收机，开阔环境下记录进入 RTK Fixed 的时间（阈值实测后定）。
- 基站链路中断 5 s 内触发告警。

## 九、USB 设备与 udev 自动配置

**目标**：插入已知 USB 设备后，自动识别并生成稳定设备名与权限规则，免手写。

**适用范围**：Linux 原生。Windows/macOS 没有 udev，只做设备识别和端口命名映射。WSL2 中 USB 需先用 usbipd-win 附加到 WSL，且 udev 是否生效取决于 WSL 的 systemd 设置，需单独验证。

**流程**

1. 监听 USB 串口热插拔，读取 VID/PID/序列号/厂商/产品名。
2. 匹配设备库（常见：PX4/Pixhawk 系、u-blox、FTDI、CP210x、CH340），给出角色建议（飞控 / RTK 基站 / 其他），用户确认或手选。
3. 生成规则预览：设置权限（`MODE`/`GROUP` 或 `TAG+="uaccess"`）、稳定符号链接 `/dev/maggcs/<角色>`、ModemManager 忽略标记（`ID_MM_DEVICE_IGNORE`），避免其占用串口。
4. 同 VID/PID 多个设备：优先用序列号区分；无序列号时按物理端口路径区分，并提示规则绑定的是接口。
5. 提权安装：经 polkit（pkexec）调用独立辅助程序，只允许写 `/etc/udev/rules.d/` 下本应用命名的文件并重载规则，主程序不以 root 运行。headless 模式提供 CLI 子命令。
6. 支持列出/卸载/回滚已装规则；失败时给出可手动执行的命令。

**验收**

- 规则生成器黄金文件测试：给定设备输入，规则文本逐字一致。
- 辅助程序拒绝写入其他路径和其他文件名（安全测试）。
- 真机插入后 5 s 内 `/dev/maggcs/<角色>` 出现，且无需 sudo 即可打开；卸载后规则与链接清除。

## 十、任务卡：DEM 预处理（tools/dem-prep）

- 输入：GeoBC LiDAR 点云（LAS 1.4，NAD83(CSRS)/UTM10，CGVD2013，CGG2013）；若已是栅格产品，跳过栅格化，只做元数据与基准核对。
- 输出：DTM（第 2 类点，COG，1–2 m）、DSM（第 1 类点最大值，树冠安全层）、元数据 JSON（项目、日期、密度、标称精度、基准、空洞掩膜）。
- 要求：空洞和水体明确标记，不静默插值。
- 验收：检查点误差在元数据标称精度内；单测覆盖空洞与坐标基准。

## 十一、AI 协作规则

1. 每次只做一个小任务，不一次生成整个 Phase。
2. 先输出 struct/enum/trait/函数签名/错误类型，确认后再写实现。
3. 每个任务附可自动化或有数值的验收标准，完成后自检并说明验证方法。
4. 后端模块必须有单元测试或 SITL 测试脚本；前端任务附截图与手动验证步骤。
5. 涉及提权、串口写入、基站配置写入的任务，必须先给出失败与风险分析（写错配置、误写其他设备）。
6. Rust 生产代码使用 Result，避免 unwrap；TypeScript 严格模式；魔法数字提取为常量或配置。
7. 每个模块完成后同步 README/docs；重要决定写 ADR；不确定时先提问或给方案对比。
8. 执行顺序：MAVLink 与遥测 → UI 壳 → Mission 协议 → 航点编辑 → RTK → DEM 与高度转换 → Survey 规划 → 磁数据层与 QC。

## 十二、启动指令

> 当前从 Phase 0 开始。先创建 workspace 骨架与 `AGENTS.md`，输出 `core` crate 的 struct / enum / 错误类型与函数签名（包括 `mavlink`、`devices` 两个模块），等我确认后再实现 UDP 连接管理与 HEARTBEAT / GLOBAL_POSITION_INT 解析，并附 SITL 集成测试脚本。
>
> 完成后输出：文件树、关键代码、运行验证方法、对照 Phase 0 验收标准的自检结果。

## 十三、待决事项

1. RTK 基站具体型号（是否 u-blox ZED-F9P 一类），决定首个支持的设置协议。**（已确认：u-blox ZED-F9P）**
2. 是否需要 NTRIP 作为 RTCM 来源。
3. 前端框架与组件库（ADR-010）。
4. 许可证：Apache-2.0 还是 GPLv3。
5. GeoBC 数据实际提供点云还是栅格，以及覆盖范围。
6. 各阶段验收数值需在 SITL 与实飞数据上校准。