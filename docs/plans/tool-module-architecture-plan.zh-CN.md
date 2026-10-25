# 工具模块与共享飞行器运行时架构

- **语言：** [English](tool-module-architecture-plan.md) | 简体中文
- **日期：** 2026-10-25
- **状态：** 详细架构提案；等待所有者 review，暂停实施。
- **实施顺序：** 以[架构优先总计划](architecture-first-delivery-plan.zh-CN.md)为准，将 TM／SW 范围分配到 AF 工作包；近期为单作业目标，仅在接口层预留未来多机兼容；排期和完整项目估算使用该总计划。
- **相关文档：** [Survey 工作区方案](ugcs-survey-workspace-plan.zh-CN.md)、[当前架构](../ARCHITECTURE.md)、ADR-025／026／028。

## 1. 推荐关系

采用**一个具有设备／飞控会话权威的共享运行时，加可独立开发、独立打开的工具模块**。主程序负责项目和工作流，工具通过类型化 Runtime API 使用共享服务。

明确三种独立性：

1. **模块独立：** 每个工具拥有自己的领域逻辑、界面状态、测试和结果格式；共享服务通过窄接口使用。
2. **工作区独立：** Inspector、Motor Test、Parameters、未来 Configuration／Calibration 可以独立打开、关闭和恢复。
3. **进程独立：** 重型解析／扩展可使用 worker；后续独立 Runtime 进程可让设备会话在主界面故障后继续运行。

现有独立 WebView 已有 UI 隔离价值，但共享 native host 的致命故障仍可能影响所有窗口。独立 Runtime 进程是需要单独交付的隔离能力，不能把“独立窗口”直接记为“进程隔离已完成”。

## 2. 现有基础与迁移约束

| 现有基础 | 来源／含义 |
| --- | --- |
| 主窗口、Inspector、Actuator、Parameters 有独立前端入口和 WebView | `docs/ARCHITECTURE.md`；各 `*_open` 会聚焦已存在窗口。 |
| 已有进程级状态及逐连接服务 | `app-tauri/src/state.rs` 的 `AppState`、`ActiveLink`、`LinkId`、生命周期串行化。 |
| Inspector 使用 tap、批量 Tauri Channel，DSP 在 Rust | `inspector_service.rs`、`core/signals`、`core/inspector`、`core/dsp`。 |
| Motor control 权限默认 OFF，绑定目标／generation，关闭／换目标会撤销 | [ADR-025](../adr/025-motor-control-permission.md)，应直接复用。 |
| Motor Test 可借用主串口连接和参数服务，不拥有关闭主连接的权利 | [ADR-026](../adr/026-main-serial-target-reuse.md)；专用台架目标仍单独标识。 |
| 串口 inventory／lease 正在统一 | [ADR-028](../adr/028-shared-serial-discovery.md)、`core/devices/lease.rs`，迁移需衔接现有分阶段工作。 |
| 无界面运行时复用已有架构方向 | `crates/server` 和架构文档描述的是计划中的 server，并非已交付的独立 daemon。 |
| 工具关闭事件已能停止／取消服务工作 | Inspector 解除 tap；Motor Test 撤销并停止自身会话；Parameters 会取消当前主参数服务操作。增加客户端后，取消必须绑定明确操作所有者。 |

应从这些机制演进，统一复用端口登记、参数缓存、Motor 权限和 MAVLink 栈。

## 3. 主程序、工具与 Runtime

```text
主程序 Shell                      可独立打开的工具
项目／Survey／工作流              Inspector Motor Test Configuration Calibration
菜单／资源／汇总                  各自的界面状态、图表、表单、过程引导
             \                    /       |          |          /
                类型化 Tool SDK／Runtime facade／带版本订阅
                                      |
                         共享 Runtime：唯一权威所有者
       +------------------------------------------------------------+
       | 目标身份／Transport registry／串口 lease                    |
       | 遥测／流速率仲裁／参数服务                                 |
       | Mission／Command／Operation session／资源占用               |
       | 记录器／项目／告警／生命周期／恢复                         |
       +------------------------------------------------------------+
                   |                          |
              飞行器／设备              有界 worker 接口
                                        ULog／DEM／重型分析
```

第一阶段可让 Runtime facade 位于现有 Tauri native 进程。接口和生命周期稳定后，再把组合层迁移到独立 Runtime 进程；两种部署方式共用逻辑 API。

### 3.1 主程序职责

- 项目／任务选择、Survey 工作流、资源、窗口路由、统一外观。
- 汇总显示各工具的状态、目标和活动会话。
- 与工具一样，通过服务 API 请求操作。
- 正常退出通过现有生命周期策略协调。

### 3.2 Runtime 职责

- 拥有物理 I/O、目标验证、连接 generation、协议服务、命令协调、操作清理。
- 提供权威状态和数据年龄；仲裁共享流速率、参数写入等影响设备的操作。
- 记录与活动飞行监控不依赖当前显示哪个页面。
- 按操作类型执行准入规则，保留失败／结果不确定状态。
- 发布有界、带版本事件，保留客户端可重新同步的快照。

### 3.3 工具职责

- 拥有自己的工作区、交互、专属计算与报告。
- 声明需要的服务和操作能力。
- 绑定明确目标，获取快照／订阅或 Operation session。
- 显示目标、连接来源、模式、资源所有者、进度和可恢复错误。
- 按策略释放自己的订阅／操作；借用连接不获得关闭物理连接的所有权，也不取消其他模块工作。

### 3.4 Survey 领域模块

Flight Plan、Drape、Mission Execution、Flight Monitor 分为独立领域模块，由 Survey 工作区组合，与独立工具共用 Runtime 基础服务。各自职责、输入／输出及单向依赖见 [Survey 方案第 12 节](ugcs-survey-workspace-plan.zh-CN.md#12-flight-plandrapeexecutionmonitor-模块拆分)。模块边界、窗口边界和进程边界分别设计。

## 4. 各工具的合适边界

| 工具 | 后端契约 | 独立性／耦合规则 |
| --- | --- | --- |
| Signal Inspector | 只读遥测订阅、DSP／分析会话；需要时单独申请改变流速率。 | 关图只解除该订阅；绘图慢不能反压飞行控制。改变消息流速率是共享设备操作，不能按纯只读处理。 |
| Motor Test | 现有 Motor 权限，加具有所有权、deadline／watchdog 的执行器操作会话。 | 借用时共用已验证连接和服务；停止只释放自己的输出；UI 丢失／重连撤销权限，重开默认 monitor-only。 |
| Aircraft Configuration | 参数快照、暂存修改、能力检查、有所有权的写入事务、echo／读回及重启核对。 | 界面独立，写入协调；复用参数服务。部分成功的批量写入应显示部分完成，不伪装成原子事务。 |
| Aircraft Calibration | 固件相关过程 driver、独占操作上下文、进度观察、取消、终态核对。 | Runtime 持有过程状态；向导展示真实进度，关闭页面不等于飞控校准已成功停止。 |
| 日志／DEM／分析 | 带身份的数据 job、进度／取消、带版本输出。 | 重型解析可移到 worker；CPU／内存问题不能阻塞设备释放或任务命令。 |

**范围说明：** 当前 `AGENTS.md` 将机体初始配置和飞控传感器校准交给 QGroundControl。本次用户希望将其纳入后续产品候选，实施时需要先 review 并更新范围。现有地球物理校准数据及 Survey 校准机动，与飞控配置／校准是不同功能。

## 5. 资源所有权、冲突与并发

### 5.1 目标身份

Attachment 和 Operation 绑定现有已验证 TargetRef，以及 `(LinkId／handle identity, transport generation)`。串口名称不代表机体身份，借用连接保留原 I/O 所有者。

即使 transport 未关闭，飞控也可能重启。是否需要新的 boot-epoch 契约，应遵循 ADR-028 的 LINK-05 现场核查，而不是先假设现有 generation 足够或直接引入新体系。身份未确定的新目标不能继承旧写入权限。

未来多 transport 支持，应按已确认目标仲裁控制，而不是只按端口。连向同一飞行器的两条链路，不能在本 Runtime 内形成两个互不知情的执行器／配置所有者；身份模糊时不猜测它们相同或不同。

### 5.2 资源声明

使用 Operation 级资源声明，初期对影响设备的操作采用保守的逐目标互斥：

| 资源 | 共享规则 |
| --- | --- |
| 遥测读取 | 多订阅者，各自有界缓冲和降采样。 |
| 物理串口 | 一个 I/O 所有者；借用 handle 不拥有 shutdown。 |
| 消息流速率配置 | 统一仲裁订阅需求和链路带宽预算。 |
| 参数写入 | 冲突的 set／batch 由统一 writer 协调，读取和缓存继续可用。 |
| 执行器控制 | 复用 Motor Test 所有权／权限，并与其他目标操作协调。 |
| 校准／设置过程 | 对涉及模式、校准状态、相关参数的过程提供适当独占。 |
| 任务替换／启动 | 结合活动目标／会话，与校准和执行器操作检查冲突。 |
| Shell／FC 日志传输 | 复用已有共享协议互斥，取消权属于具体操作所有者。 |

Busy 错误明确给出占用者、操作和下一步。Stop／Release 不排在普通配置、日志下载、DSP 后面。

这些规则只能协调 RGCS 客户端，不能锁住外部 QGC 或遥控器。外部模式／参数变化应使受影响 Operation 按契约重新验证、核对或停止。

### 5.3 生命周期规则

| 事件 | 预期行为 |
| --- | --- |
| 切换 Survey 阶段 | 仅换视图；活动记录／监控保持其冻结飞行基线。 |
| 关闭 Inspector | 只取消自身订阅／分析 job，其他消费者和 transport 保留。 |
| 关闭 Motor Test | 立即撤销自身权限，执行所属取消／释放；失败保留供重试／诊断。 |
| 关闭 Configuration | 草稿作为数据保留；只取消本工具待处理操作；已发出的写入需核对。 |
| 关闭 Calibration | 按过程契约 detach／cancel，其他位置仍可显示实际 Runtime 状态。 |
| 重连／替换目标 | 旧操作上下文失效，旧 generation 写入不能进入新目标。 |
| UI renderer 故障 | 敏感客户端操作 lease 到期；native host 尚在时，无关订阅／服务继续工作。 |
| 独立 Shell 进程故障 | 独立 Runtime 交付后可保留飞行观察／记录；执行器客户端授权仍应到期。 |
| Runtime 故障 | FC 自身配置的超时／failsafe 是最后边界；重启后核对状态和日志，不自动重放电机／启动／校准意图。 |
| 正常 Quit | 按明确生命周期策略及活动操作状态处理，区别于关闭某个工具。 |

## 6. 扩展机制与 API

### 6.1 Tool descriptor

工具注册信息包括：

- 稳定 ID、版本、名称／图标、入口视图、窗口偏好。
- 需要的设备角色及服务。
- 读取／操作能力、支持的目标 profile。
- 状态／报告 schema 版本及迁移。
- 隔离级别：独立工作区、分析 worker、独立启动客户端。

Tools 菜单根据 descriptor 生成。启动保持幂等：按实例／目标聚焦现有工作区。主菜单和工具复用相同命令可用性说明。

初期使用编译期可信模块和稳定服务边界。后续第三方扩展采用带版本的进程 IPC；模块独立不要求通过动态 Rust ABI 加载。

### 6.2 窄 Runtime API

以下是供 review 的接口草图，并未实现：

```rust
pub struct ToolAttachment { /* 工具实例 + 已验证目标 */ }
pub struct OperationRequest { /* 类型 + 目标 + 预期版本 + 资源 */ }
pub struct OperationHandle { /* 操作 ID + generation + 取消权 */ }

pub enum RuntimeError {
    TargetUnavailable,
    StaleTarget,
    UnsupportedCapability,
    ResourceBusy,
    PermissionRequired,
    Cancelled,
    OutcomeUncertain,
    IncompatibleApiVersion,
}

pub trait ToolRuntime {
    // 示意签名；异步和事件传输由具体适配器确定。
    fn attach(&self, request: AttachRequest) -> Result<ToolAttachment, RuntimeError>;
    fn begin(&self, request: OperationRequest) -> Result<OperationHandle, RuntimeError>;
    fn cancel(&self, operation: OperationHandle) -> Result<(), RuntimeError>;
    fn snapshot(&self, attachment: &ToolAttachment) -> Result<ToolSnapshot, RuntimeError>;
}
```

`AttachRequest`、`ToolSnapshot` 同样是拟议类型。生产命令使用类型化 payload 及明确 target／session ID，工具 UI 不直接发送任意串口字节。协议携带 request ID、revision、终态和取消所有权。预期版本用于本地并发校验，不会使飞控多参数写入自动具有原子性。

### 6.3 数据面与命令面

- **命令面：** 有界请求，带身份、所有权、deadline、确认与终态，复用既有控制服务及释放语义。
- **数据面：** 有界列式批量数据或最新快照，带 sequence、timestamp、丢样数量和字段年龄，各消费者独立降采样。
- **重新同步：** 重连先取快照／revision，再接事件，避免用缺失了一部分的事件流猜测当前状态。
- **Worker：** 输入哈希和 job ID 贯穿请求／结果，旧结果不能覆盖当前任务或操作。

初期 facade 下使用 Tauri invoke／Channel。后续进程适配器使用本地 IPC、版本握手及明确的本地客户端授权边界。API 隔离和进程隔离分别测试。

## 7. 配置与校准的具体流程

### 7.1 Configuration

1. 绑定明确目标，读取固件、能力和当前参数。
2. 编辑本地草稿，显示基线／请求值及需要重启的项。
3. 检查批次，申请写入上下文，经 `ParamService` 发送。
4. 区分 confirmed、clamped、rejected、pending、uncertain。
5. 中断后重新读回核对；只有支持且验证成功的恢复才记为 rollback，否则保留明确部分结果。
6. 重启作为显式操作，之后重新验证目标并读回。

已有 Parameters 窗口成为复用的底层编辑器；未来 Configuration 在其上增加机型表单和流程，不再建第二套参数协议／缓存。

### 7.2 Calibration

按支持的固件／过程族提供 driver：

`Prepare → Acquire operation → Start → Observe progress → Verify → Complete`

每个活动状态都有文档化取消／失败分支。进度来自与本操作相关的 FC 反馈，不由 UI 的 Next 按钮推进。固件不能确认取消／完成时，状态保持 Uncertain，直到核对清楚。报告保存过程类型、机体／固件、相关前后参数、时间和结果。

Runtime 在重要步骤前检查新鲜目标状态和资源冲突。涉及执行器的操作复用 Motor Test 权限模型，不增加另一套互不知情的“解锁”。

## 8. 迁移步骤与资源

| 工作包 | 内容 | 工程人周 |
| --- | --- | --- |
| TM-01 | Runtime facade、目标／会话契约、所有权清单 | 1.5–3 |
| TM-02 | Tool descriptor、前端窄 SDK、现有窗口适配 | 1–2 |
| TM-03 | Operation 冲突／取消、权限复用、watchdog 契约 | 2–4 |
| TM-04 | 有界订阅、与导航解耦的生命周期、故障注入测试 | 1.5–3 |
| TM-05 | 独立 Runtime／worker 进程适配、监督、恢复、打包 | 4–8 |
| TM-06 | 基于现有参数服务的 Configuration，先支持一类目标 | 2–4 |
| TM-07 | 明确 PX4 校准过程集合的 driver／UI／报告 | 4–8 |

- **现有工具模块化（TM-01–04）：约 6–12 工程人周。**
- **进程级隔离（TM-05）：另加 4–8 工程人周。**
- **未来配置／校准（TM-06–07）：另加 6–12 工程人周。**
- 全部架构／工具扩展合计约 **16–32 工程人周**，尚未计专门 QA／外场及余量；与 Survey SW-04／SW-05／SW-14B 可能重叠约 2–4 人周，细化排期时只计算一次。

这些是按现有技术栈、一个目标固件族做的早期估算，不是覆盖所有机型／校准过程的报价。TM-07 排期前先列清校准过程。复用 Survey 估算中的 core／前端／QA 角色；TM-05 需要 native IPC／打包经验，TM-06／07 需要飞机领域工程师及台架操作人员。

## 9. 必须获得的鲁棒性证据

1. 其他工具订阅时打开／关闭／重开某工具，transport 和无关操作保持身份与正常运行。
2. 压满 Inspector 数据／FFT 负载，测量内存有界、丢样可见，命令／释放调度不受反压。
3. 杀死／刷新 Motor Test UI，按配置 deadline 验证权限撤销和所属输出释放，再做台架电气验证。
4. 一个客户端取消参数读取／写入，不取消其他客户端操作；部分写入在读回前仍明确为部分／不确定。
5. 在各 Operation 状态重连／换目标，旧请求被拒。
6. 注入外部 QGC／FC 参数或模式变化，过程会核对，而不是假设独占了整个飞行器。
7. 活动监控／记录时切换所有主阶段，服务和冻结基线不随导航被替换。
8. 分析 worker 崩溃，Runtime 返回失败 job 并继续处理飞行／执行器清理；另测 Shell／Runtime 崩溃边界。
9. 带中断操作 journal 重启，恢复证据和当前状态，不自动重放执行器／启动／校准命令。
10. 注册测试工具，只依赖 SDK，不修改 transport、参数、生命周期或其他工具实现。

物理输出释放、FC 重启和 native 窗口／进程失败分别需要台架／打包应用证据。现有测试及 ADR 是基础，本次文档任务没有执行这些实验。

## 10. 待 review 的决策

实现前确认 Runtime 所有权／API 边界、首次进程隔离里程碑、Configuration／Calibration 范围扩展及资源冲突表。之后再将一个工作包拆成具体签名和验收用例。本文件不构成启动工作包的授权。

## 11. 按需加载、效率与鲁棒生命周期

### 11.1 小而常驻的核心，按需启动的功能

启动时常驻轻量 Shell 和必要 Runtime 协调器，仅登记工具元数据；真正需要时才初始化实现、缓冲区、数据文件和 worker。

| 组件 | 启动时机 | 保留／停止条件 |
| --- | --- | --- |
| Shell、工具目录、项目元数据、告警／生命周期协调器 | 应用启动 | 随应用／Runtime 常驻，仅保留小状态。 |
| 设备发现及已配置自动连接 | 启用的 native 连接策略 | 归配置角色所有，不与工具窗口显隐绑定。 |
| 逐连接遥测／命令服务 | 已验证连接建立 | 跟随连接／会话，保留必要心跳和命令能力。 |
| Flight Plan UI／生成器 | 打开 Plan、新建／编辑任务 | 草稿作为数据持久化，空闲 UI／工作缓冲可释放。 |
| Cesium／地图实现 | 打开需要地图的工作区 | 按地图视图／资源 lease 保留；StartPage 不需要完整地图 bundle。 |
| Drape／DEM 工作 | Terrain 模式或明确请求地形／剖面计算 | 带版本、可取消 job；引用的剖面／瓦片进入有界缓存。 |
| Flight Monitor／Recorder | 活动执行策略／已准备执行基线 | 由执行会话 pin，切页面／关面板不停止。 |
| Inspector UI／DSP | 打开工具并请求信号／分析 | 只订阅所选信号；不用时释放自身 tap／pipeline。 |
| Motor Test | 打开／绑定用于观察；显式控制操作才驱动执行器 | 观察与驱动分开，驱动遵守现有权限和 deadline。 |
| Configuration | 打开相关参数／表单页 | 只加载需要的元数据／值；草稿保留，活动写入具有独立生命周期。 |
| Calibration | 打开选定流程／显式开始 | 过程持有 Runtime 状态，直到核对过的终态。 |
| ULog／DEM／大分析 worker | 提交 job | 有界池，按资源策略退出空闲 worker／回收缓存。 |

保留现有独立 RTK／基站和自动连接策略：隐藏 UI 不会撤销操作员已经启用的 Runtime 角色。

### 11.2 前端加载边界

- 保留各工具独立 Vite／WebView 入口，不在启动时预建一批隐藏窗口。
- 对重型工作区／组件使用动态 import，配套 loading／error／retry 边界；descriptor 保存 loader 引用，不急切导入全部工具实现。
- 当前 `App.tsx` 静态导入 MapView，`RightInspector.tsx` 静态导入阶段面板。先分析实际 bundle，再按依赖图拆分，不预先宣称启动已加速。
- 共用设计 token 和小 SDK 保持轻量；Inspector 客户端不应因共享入口而拉入 Cesium 和全部 Survey 编辑器。
- import／渲染失败保留 Shell 和其他工具，并显示可重试模块错误。React ErrorBoundary 不能承诺隔离 native WebView 或共用 host 崩溃，后者由进程隔离里程碑验证。

React 卸载组件可以释放实例／订阅，但缓存的 JavaScript module 未必卸载。编译进同一 Rust 二进制的模块，也不等于服务已经启动或能随面板关闭动态卸载。效率首先来自延后初始化、数据读取、计算、订阅和 worker 创建；真正代码／进程回收由 WebView／worker／进程边界承担。

### 11.3 Runtime 生命周期与单次初始化

明确服务状态：

`Dormant → Starting → Ready／Active → Idle → Stopping → Dormant`

启动可能失败，并返回可重试错误；状态变化携带实例 ID、target generation 和 revision。

- 同一服务／目标的并发请求共用一次初始化。
- 初始化期间取消／关闭后，迟到结果不能复活旧工具或绑定新目标。
- 区分**视图引用、操作所有者、执行 pin**。最后一个视图关闭，不代表可以停止活动校准或飞行监控。
- 回收空闲实例须满足：无活动 owner／pin、清理已完成、资源策略允许。释放失败保留诊断／所有权状态供核对。
- 重开先读 Runtime 快照，再订阅；同一所有者不重复启动协议 reader、参数缓存、stream supervisor 或设备探测。

### 11.4 CPU、I/O 和内存预算

- CPU 重任务进入有界 worker 池，与异步 I/O／控制路径分离。Stop／Release 不等待 FFT、DEM 解析、压缩或长时间持有的分析锁。
- 遥测统一路由，每个消费者使用有界批量／快照。显示降采样与记录／分析所需速率分别控制，丢样／缺口明确报告。
- 流速率需求由已有共享服务合并；关闭消费者时仅撤回其需求，保留其他所有者要求。
- 地形按数据源／版本／区域缓存并规定内存预算，大栅格优先按窗口／瓦片访问。正在执行的基线和必需资源保持 pin，大草稿 job 不能暗中逐出它们。
- Worker／job 队列有上限，显示排队／执行／取消／失败；超额工作明确拒绝或排队，不无限分配。
- 轻量配方／历史引用与大型派生缓冲分开；关图释放历史缓冲，重开按需申请有界回放。

### 11.5 效率验收

1. 自动角色关闭时，启动不创建可选工具窗口、DSP、DEM 读取、校准或电机控制会话；开启某自动角色时，只发生该角色已定义的启动行为。
2. 用启动网络／bundle trace 确认 Inspector／Motor Test／Calibration UI chunk 按需请求；StartPage 能在不请求重型地图 chunk 时显示。
3. 并发打开同工具十次，只创建一个实例和必要 attachment，聚焦／绑定幂等。
4. 初始化尚未结束就取消并换目标，不遗留旧订阅、窗口、权限或设备写入。
5. 重复开关，记录活动订阅／task／buffer 数；清理后回到预热基线，无单调增长。
6. 压满分析输入，队列不超配置上限，命令／释放调度仍满足既有验证 deadline。
7. 活动执行时隐藏／关闭全部监控面板，pinned monitor、recorder、目标身份和冻结基线继续正确运行。
8. 在明确的参考机器上，对比冷／热启动、打开工具耗时、空闲 CPU、RSS／heap、worker 数。先取基线，再定绝对预算，不虚构当前性能数据。

基本懒注册／加载计入 TM-02，生命周期／预算行为计入 TM-04；大 COG 按块访问、进程内存隔离与 SW-07／SW-10、TM-05 重叠，细化时只估算一次。全部仍等待所有者 review 后实施。
