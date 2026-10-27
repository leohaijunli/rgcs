# 架构优先实施计划

- **语言：** [English](architecture-first-delivery-plan.md) | 简体中文
- **日期：** 2026-10-26
- **状态：** Execution authorized on 2026-10-25; AF-00 baseline recorded.
  Small-slice implementation continues under owner authorization and the existing
  engineering gates. See the [execution inventory](../architecture/af-00-current-system.md),
  [Runtime contracts](../architecture/runtime-contracts.md) and
  [measured baseline](../verification/af-00-baseline-20261025.md).
  AF-01 target/read-facade checkpoint is implemented and verified:
  [delivery evidence](../verification/af-01-facade-20261025.md).
  AF-02a core coordination is verified; AF-02b native integration is active:
  [operation checkpoint](../verification/af-02a-operations-20261025.md).
  AF-02b.2 Parameters ownership and AF-02b.3 cross-client/native-close regression,
  screenshots and documentation are complete:
  [acceptance evidence](../verification/af-02b-parameters-20261025.md).
  AF-02b.4a Command 所属操作和 Parameters 重启接入已实现并通过验收：
  [验收证据](../verification/af-02b-command-reboot-20261025.md)。
  AF-02b.4b general flight Command migration is complete within software scope:
  [acceptance evidence](../verification/af-02b-flight-commands-20261026.md).
  Verified: 16 native flight fixtures, 268 synchronized generated contracts,
  264 browser assertions / 46 screenshots, 1378 workspace tests, strict checks
  and production/release builds. Mission/Shell/Logs/Motor and the complete adapter
  matrix remain pending; G1 stays open. The
  [Mission interface proposal](../architecture/af-02b4-mission-ownership-proposal.md)
  is implemented and accepted in software scope:
  [evidence](../verification/af-02b-mission-20261026.md). Upload/download/clear/
  set-current, exact cancellation, sent-snapshot read-back and stale-target/editor
  isolation are verified. Shell/Logs/Motor and the cross-adapter matrix remain;
  G1 stays open.
  The [Shell/Logs contract](../architecture/af-02b4-shell-logs-ownership.md)
  is implemented and accepted within software scope without another confirmation
  round: [evidence](../verification/af-02b-shell-logs-20261026.md). Exact session/
  cancellation, priority close, durable downloads, delete/directory read-back,
  293 synchronized contracts, 330 browser assertions/68 screenshots, 1463
  workspace tests (12 opt-in ignores) and all 26 delivery gates pass. Dedicated
  Motor ownership and the final cross-adapter/client/fault matrix are next; G1
  stays open.
  Motor contract/data checkpoint (2026-10-27) is verified: owner/target/source/
  revision DTOs and numeric validation, 136 filtered core Motor checks and 310
  complete generated mirrors; see [evidence](../verification/af-02b-motor-contract-20261027.md).
  Native permission/operation admission and the remaining G1 matrix are active.
- **优先级决定：** 先做架构、按需加载、现有工具、执行与监控；**Flight Plan 和 Drape 放到最后的功能阶段。**
- **近期范围：** 单个作业飞行器，保留既有专用台架、SITL 来源和独立基站角色。未来多机兼容仅作为接口设计考虑，不列入近期功能开发包。
- **顺序依据：** 本文替代此前 Survey／工具方案中的实施排序；原功能设计和 BL／SW／TM 编号继续作为参考。
- **相关设计：** [Survey 工作区](ugcs-survey-workspace-plan.zh-CN.md)、[工具／Runtime 架构](tool-module-architecture-plan.zh-CN.md)、[UgCS 操作基线](../ugcs-survey-operations-baseline.zh-CN.md)。

**Review 导读：** §4 看阶段顺序，§5 看架构工作包，§6 看最后的 Flight Plan／Drape，§8 看验收，§11 看资源，§14 看未来多无人机扩展。

## 1. 目标与排序决定

第一批交付是可靠、高效、能够承载相对独立工具的应用平台。应先证明资源所有权、生命周期、加载和故障行为，再替换 Survey 规划器和地形算法。

建议顺序：

```text
契约与当前系统基线
  → 共享 Runtime 所有权与操作协调
  → 按需 Module Host／有界数据服务
  → 迁移 Inspector、Parameters、Motor Test 和菜单
  → 执行快照／与导航解耦的监控
  → Worker 和 Runtime 进程隔离／架构验收
  → 证明未来 Configuration／Calibration 的扩展契约
  → 最后做 FLIGHT PLAN：语义任务、编辑器、测量生成器
  → 最后做 DRAPE：高程设计、完整航程地形处理及测量闭环
  → 集成发布验收
```

前期仅在其他模块确实需要时预留小型 Flight Plan／Drape 数据契约。新编辑器、生成器、地形求解接线、完整路线能力和测线级业务逻辑都在后期交付。

## 2. 架构规则

1. **物理 I/O 只有一个所有者。** 复用现有设备发现／lease 和连接服务，借用连接不能关闭所有者的 transport。
2. **操作必须有所有者。** Start、Cancel、Stop、清理均绑定工具／会话和目标 generation，不操作“碰巧当前”的全局服务。
3. **视图不拥有飞行。** 切阶段、关图表不停止活动飞行监控／记录，也不替换执行基线。
4. **权威在 native Runtime。** 设备写入和准入检查经 Runtime，沿用现有 Motor Test 权限／释放语义。
5. **异步工作都有身份。** 按需要携带 request、operation、instance、target generation 和输入 revision。
6. **资源有界。** 订阅、队列、缓存、worker 有明确上限，分析不阻塞输出释放和命令调度。
7. **按需加载。** 启动仅登记小 descriptor；需要时或活动 Runtime 角色要求时才初始化 UI、数据、DSP 和 worker。
8. **恢复保留不确定性。** 超时／崩溃不等于成功停止或成功写入；重连不恢复电机权限、不自动重放启动／校准意图。
9. **共享契约保持小。** 工具使用受限服务接口，不获取整个可变 `AppState` 或其他工具的 UI store。
10. **迁移可回退。** 替换适配器时，每种有副作用操作只有一条有效路径，不能为了对照而重复发送设备命令。
11. **保留 native 角色策略。** RTK／基站和已配置自动连接独立于窗口显隐。
12. **前期能力边界真实。** 架构验收不代表旧 Flight Plan／Drape 缺陷已经修复。

## 3. 现有基础与抽取边界

| 现有代码／设计 | 复用或抽取方向 |
| --- | --- |
| `app-tauri/src/state.rs`：`AppState`、`ActiveLink`、`LinkId`、生命周期锁 | Runtime 组合层、目标注册表、受限服务 handle |
| `core/devices`、`mavlink`、`telemetry`、`params`、`commands`、`mission` | 继续作为设备／协议权威 |
| ADR-025／ADR-026 | 电机权限、generation 撤销、所属输出释放、主链路借用 |
| ADR-028 及正在进行的串口改造 | 共用 inventory／lease，衔接既有分阶段工作 |
| `inspector_service.rs`、`actuator_service.rs`、`params_service.rs` | 逐个迁移工具适配器 |
| `capture_service`、`run_service`、`stream_service`、logs／RTK 服务 | 复用服务边界，不重建协议／缓存 |
| `TopBar.tsx`、`WorkflowNav.tsx`、独立工具 HTML、`RightInspector.tsx` | 懒加载、工具路由、菜单整理、UI 生命周期独立 |
| 现有任务上传／读回和 drape monitor 类型 | 前期 Execution／Monitor 的兼容输入 |
| `core/plan`、`core/survey`、`core/terrain` | 最终功能阶段前，由适配器继续承接现有功能 |

表中 `core/`、`app-tauri/` 按上下文对应 `crates/.../src/`。工作树有正在进行的用户改动；抽取必须衔接这些改动，不能用旧快照整块替换服务。

## 4. 阶段与验收门槛

| 阶段 | 工作包 | 交付内容 | 退出条件 |
| --- | --- | --- | --- |
| 0：基线与契约 | AF-00 | 所有权／生命周期清单、启动资源实测基线、API 词汇 | G0：契约和验收夹具确认 |
| 1：共享 Runtime 核心 | AF-01–02 | 绑定目标的 facade、操作协调、按所有者取消 | G1：单一所有者，无旧请求写入和跨所有者取消 |
| 2：按需加载与数据面 | AF-03–04 | Lazy host、单次初始化、有界订阅及 worker 接口 | G2：可选功能默认休眠，资源边界可观测 |
| 3：现有工具与导航 | AF-05–08 | 迁移 Inspector／Parameters／Motor Test，整理菜单顶栏 | G3：工具相对独立，原有行为保留 |
| 4：执行与监控 | AF-09–10 | 不可变执行基线，监控与导航解耦 | G4：不依赖新规划器即可观察已验证现有任务 |
| 5：隔离与架构交付 | AF-11–14 | Job／Runtime 隔离、恢复证据、扩展性证明 | **G-ARCH：架构通过后才进入新 Flight Plan／Drape** |
| 6：后期 Flight Plan | AF-15–16 | 语义任务／编辑器、生成器、载荷、交换与估算 | G-PLAN：对应 BL 场景通过 |
| 7：最后 Drape 与测量闭环 | AF-17 | 高度／drape 正确性、完整地形路径及依赖业务 | G-DRAPE：数值／飞控验收及已授权依赖通过 |
| 8：集成验收 | AF-18 | Native／SITL／台架／外场证据与发布文档 | G-RELEASE：批准范围可证明完成 |

这些门槛指工程验收证据。在所有者 review 并授权实施之前，不启动任何阶段。

## 5. 前期工作包详解

### AF-00：清点当前系统与冻结最小契约

**交付：** 服务／所有者／生命周期清单、模块依赖图、事件／命令目录、参考机器及目标夹具定义。

- 跟踪全部连接、借用、分离、换目标、关窗口、退出路径。
- 标出前端 eager import、native task 启动、后台订阅、栅格／文件读取和可变全局缓存。
- 实测冷／热启动、开工具耗时、CPU、内存、worker 数、关闭清理，再确定优化预算。
- 评审最小类型：`VehicleId`、`VehicleSessionId`、`LinkId`、`ClientId`、`ModuleInstanceId`、`TargetRef`、`OperationId`、`RequestId`、`Revision`、订阅和操作结果。
- 记录已配置的控制 deadline，以及软件／台架如何验证。

**验收：** 每个现有设备／服务有明确所有者和停止路径；夹具、测量可复现。通过 G0 不需要先设计完整 Survey schema。

### AF-01：Runtime facade 与目标上下文

**涉及：** `AppState`／`ActiveLink`、主链路／专用链路适配、公共类型。

- 对现有服务建立窄 facade，初期仍可位于同一进程。
- 调用绑定已验证目标及现有 handle／generation。
- 暴露快照／服务能力，不暴露可变全局 store。
- 主程序和工具采用一致的目标解析规则。
- 保留主链路借用，以及独立台架／基站角色。
- 分开逻辑飞行器与 transport 身份；primary／selected vehicle 只作为 UI／默认选择，不能成为隐式全局命令目标。

**验收：** 换目标使旧请求失效；借用工具 detach 后主 transport／参数缓存身份不变；不确定目标不能继承操作权限。

**仅作前瞻性设计：** AF-00–02 保留明确 Vehicle／Target 上下文、会话作用域和带目标的事件／操作。近期 Runtime 服务一个作业飞行器及已有设备角色；多机 registry 的实际实现、机队 UI、共享链路多机复用、并发任务服务、多机集成联调均延后。接口预留纳入 AF-00–02 的普通契约工作。

### AF-02：操作协调与取消所有权

**涉及：** Command／Parameters／Motor／Mission 适配，生命周期和操作记录。

- 每个重要操作绑定 owner、target、资源、deadline、取消 handle、终态。
- 复用现有策略，协调冲突的参数、执行器、校准、任务、Shell／日志操作。
- 将“全局取消当前操作”调用改为取消所属 Operation。
- 保留 Motor Test 默认 OFF、generation 绑定和所属输出释放。
- Stop／Release 优先，失败／不确定清理保留证据。

**验收：** A 客户端不能取消 B 的工作；空闲 STOP 不释放别人的输出；重连撤销授权；旧结果不能完成新操作。仅协调本 Runtime，不宣称可以锁住外部 QGC／RC。

### AF-03：懒加载 Module Host 与 Tool SDK

**涉及：** 前端独立入口、`App.tsx`、`RightInspector.tsx`、工具路由和 descriptor。

- 登记轻量 descriptor：ID、版本、loader、服务需求、机型能力、入口及状态 schema。
- 保留工具独立入口，主工作区重组件／地图采用动态加载。
- 按模块／目标实现单次初始化和 loading／error／retry。
- 区分代码已注册、UI 已加载、服务运行、视图引用、操作所有者、执行 pin。
- 关闭时保留轻量草稿／偏好，释放所属工作缓冲。

**验收：** 没有相关配置角色时，启动不建可选窗口／DSP／DEM job；并发打开十次只有一个实例；取消加载或换目标后无迟到绑定；import 失败不拖垮 Shell。

### AF-04：有界订阅与共享数据服务

**涉及：** Telemetry／router、Inspector 批处理、stream service、Runtime 订阅。

- 提供最新快照和有界列式批量数据，包含 sequence／time／age／gap。
- 显示降采样与记录／分析所需采样分离。
- 流速率需求由带 owner 的共用服务合并。
- 明确 queue／cache／job 预算和超载状态。
- CPU 重任务不进入控制 I/O 或释放关键锁。

**验收：** 慢消费者不能导致无限队列，释放调度不超过已记录 deadline；关消费者仅撤回其流需求；重新订阅从一致快照／revision 开始。

### AF-05：先迁移 Signal Inspector

- Attachment／订阅／分析生命周期经过 SDK／facade。
- 保留现有曲线、过滤、FFT、Channel 批处理和独立窗口。
- 显示目标、数据年龄、丢样；界面偏好由工具自身保存。
- 关窗口只解除自身 tap／pipeline，不受主页面切换影响。

**验收：** 原有 Inspector 验证不回退；重复开关不增加活动订阅；分析过载时共享控制仍能响应。

### AF-06：迁移 Parameters 与配置基础能力

- 复用 `ParamService`、缓存、metadata、staging、echo／read-back。
- Pull／write／cancel 绑定 Operation owner。
- 保留 confirmed／clamped／rejected／uncertain 和外部修改处理。
- 为未来 Configuration 提供这些原语，暂不开发机型专用新表单。

**验收：** 两个客户端可读；冲突写入受协调；关 A 窗口不取消 B 事务；中断批次经核对，不能报告为原子成功。

### AF-07：按现有安全契约迁移 Motor Test

- 采用共用目标上下文，保留专用／借用 Runtime 语义。
- Start／set／stop／tone／configuration 经所属 Operation 和既有权限。
- 分开观察、capture 和驱动生命周期。
- 窗口丢失后保留释放失败诊断及重试状态。
- UI 故障处理与 FC 输出超时分别取证。

**验收：** 默认 OFF 被动；旧 generation 控制不能进入新目标；关闭／故障撤销授权并仅释放所属输出；借用清理保留主 I/O。软件夹具通过后验证 native close 和电气行为。

### AF-08：导航与菜单外壳

**参考：** 既有[导航示意图](ugcs-survey-navigation-concept.svg)。

- 建立全局／状态栏与任务／阶段栏，固定健康状态和 Hold／RTL 位置。
- Tools 菜单由 descriptor 生成，复用聚焦／可用性行为。
- 应用、任务、选中对象的命令归位，并明确作用域。
- 现有规划器由兼容入口继续承接；新 Survey 大纲／编辑器、Drape 控件留到后期。
- 复用 token、键盘操作、Field 密度和既有命令确认。

**验收：** 1920×1080、1366×768、1024×640 无横向溢出；电量／Hold／RTL 可见可达；点 Fly 导航不发飞行命令；菜单共用动作并聚焦现有工具窗口。

### AF-09：Mission Execution 与不可变基线适配

**涉及：** 现有 Mission／Command 服务、传输适配，不开发新规划生成器。

- 建立 `ExecutionBaseline`／快照引用，包含实际协议任务项、目标、传输身份、必要高度参考和可选可信来源测线映射。
- 适配现有明确任务、导入夹具或飞控读回；缺少 Home／基准／LineId 时明确表达。
- 固定实际发送项，按该快照验证读回，不读取可变编辑器重新解释。
- 任务意图经现有机型模式／命令适配器协调。
- 上传／执行结果与当前显示阶段无关。

**验收：** 上传中编辑／换目标不改变已发送身份；读回匹配前不显示已核对；传输失败可恢复；已验证简单任务不需要加载新 Flight Plan／Drape 即可运行。

### AF-10：Flight Monitor 与记录器生命周期

- 从 `RightInspector`／React 视图 effect 中抽离监控所有权。
- 绑定冻结执行基线和带时间戳遥测。
- 复用现有偏差／记录代码，经有界订阅发布。
- 支持不依赖新 Survey 生成器的航点／路径观察。
- 地形／测线身份不可用时明确报告；输入可信且齐备时才推导相应指标。

**验收：** 切换全部阶段／关闭图表不停止 Monitor／Recorder、不替换基线；改规划草稿不影响回放结果；过期数据不显示成零偏差。

AF-09／10 只规范已有协议执行与观察。推迟中的 M1 测量会话状态机、测线级续测及 M3／QC 业务仍放在 AF-17e，并遵守对应范围 review。

### AF-11：Worker 隔离与资源核算

- 重分析／解析使用有界 job／worker，携带输入 hash 和 job ID。
- 按需启动，明确取消、清理、空闲退出和预算报告。
- Pin 活动操作／监控资源，只回收无 owner 的空闲数据。
- 用现有分析／日志夹具证明隔离；新的 Drape 算法、大范围栅格重构留到 AF-17。

**验收：** Worker 崩溃／超额请求返回明确失败；控制／清理可响应；旧结果不覆盖新工作；清理后资源数回到预热基线。

### AF-12：独立 Runtime 进程

- 将 native 服务组合层迁到已验证 facade 后面。
- 交付本地 IPC，包含版本握手、本地客户端授权边界、request ID、有界数据和快照重连。
- 保证唯一 Runtime I/O 权威，多 Shell／工具不能重复打开同一个端口。
- 区分关 Shell、关工具、正常停 Runtime、Runtime 崩溃。
- 重启恢复证据／当前状态，不自动重放电机／启动／校准意图。
- 先验证选定的主要桌面平台，再扩展平台。

**验收：** 终止 Shell／worker 不丢失独立 Runtime；敏感客户端授权正确到期；Runtime 重启后可核对；API 不兼容明确显示；正常退出遵守既有操作策略。

### AF-13：架构级验收

- 执行第 8 节所有权、生命周期、加载、并发、性能和恢复矩阵。
- 三个现有工具组合验证，包括 native 窗口、主链路借用。
- 审计依赖：可选 UI／工具代码按需加载，工具只使用受限 API。
- 仅在调用者和验收全部迁移后删除旧适配路径。
- 将旧 Flight Plan／Drape 的已知限制与平台验收结果分别记录。

**验收：** G-ARCH 有可复现证据，owner／generation、取消、有界加载、关键清理不存在未解决的架构问题。

### AF-14：证明未来扩展独立性

- 通过 descriptor／SDK 注册一个仅测试用的 Configuration／Calibration 类模块。
- 覆盖读取、所属写入模拟、progress、cancel、failure、result。
- 证明它不需要新建 transport、参数缓存、菜单专用 dispatcher，也不修改其他工具。
- 单独 review 真实 Configuration／Calibration 的产品范围。

**验收：** 测试模块在合成目标上通过，移除它不影响 Runtime 中无关模块。

AF-13 最终 G-ARCH 签收包含 AF-14 的扩展性证明。工具迁移中可以持续开展架构验证，但必须等 AF-11／12 隔离和 AF-14 证据通过后才能关闭门槛，不能因编号先后而提前签收。

**可选扩展，获准后可安排在末期 Survey 之前：** AF-14C 实现一类机体 Configuration；AF-14K 实现明确的 PX4 校准过程。当前 `AGENTS.md` 将这些功能交给 QGC，实施前须更新产品范围。G-ARCH 要求的是扩展性证明，不是提前交付所有未来校准功能。

## 6. 最后的功能阶段

### AF-15：Flight Plan 模型与编辑器

G-ARCH 通过后开始。实现持久语义任务／航段、稳定 LineId、配方、方案变体、覆盖值和语义历史；在已建立的 Lazy host／SDK 上建设新大纲、上下文 Inspector 和联动选择。

**验收：** 多航段任务可保存／重开／撤销；插入／复制保留来源身份；普通规划在 Drape 未加载时也可使用。复用 SW-06／SW-08 和原 Flight Plan 设计。

### AF-16：Flight Plan 工具、载荷和交换

增加剩余基线生成器／配方、双网格／倾斜、有序动作、AOI 交换、转换报告、全航程估算输入。相机／LiDAR 参数必须有真实效果和明确目标契约。

**验收：** 对应 BL 场景用确定性几何／动作夹具通过；不支持能力和不完整数据明确显示。复用 SW-09–11。

### AF-17：Drape 与依赖的 Survey 闭环

最后一个功能包拆成以下可 review 的小片段：

1. **AF-17a 高度／命令正确性：** 修复已记录的速度、基准／anchor、来源测线、无坐标命令混入问题，命令编码归统一 Compiler。
2. **AF-17b 纯 Drape 契约：** 带源版本的 DTM／DSM、带基准剖面、数值约束、取消／revision、独立测试。
3. **AF-17c 完整航程：** 接近、转弯、overshoot、返回、降落上下文，已验证简化和完整任务项统计。
4. **AF-17d 真实 UI 结果：** 真实高度带／底线、中间地形采样、整任务／逐线剖面、问题定位。
5. **AF-17e 测量执行闭环：** 测线级换电续测、QC／补飞、可复现清单，在既有推迟依赖获准后实施；复用已交付 Execution／Monitor。

**验收：** 等价基准、山脊／NoData、旧结果、点数上限、实际轨迹、测线续测场景通过。这里才关闭延后的 Flight Plan／Drape 问题，架构验收不代替它。

### AF-18：集成发布证据

对最终模块执行全部批准的 BL 场景和 native／SITL／台架／外场验收；检查架构／懒加载不回退、快照／报告可复现、平台打包和未交付范围有说明。这一阶段补充验收证据，不在 Drape 后再增加另一类新功能。

## 7. 兼容、迁移与回退

- **先 facade 后搬代码：** 先将现有命令接到受限接口，再移动实现；同一片段不同时重写公开 API 和核心算法。
- **副作用只有一条路径：** 使用旧适配器委托 Runtime，或使用新适配器；可做只读对照，不能双发设备写入。
- **旧任务适配：** 前期 Execution／Monitor 接受已知有效协议任务和明确参考；不依赖新航段生成／地形设计；可选来源信息保持真实可选。
- **持久化带版本：** 相应模型阶段到来时再迁移，保留旧项目／配方读取；区分数据 schema 与 Runtime API 版本。
- **回退：** 所属操作核对清楚后才切回兼容适配器／UI；不能清除释放失败记录来制造回退成功。
- **崩溃／重连：** 保留 journal，重新取得 FC 状态，撤销旧客户端权限，为重要写入建立新的有效上下文。
- **移除适配器：** 调用者、类型、测试、用户行为文档迁移完成后才删除旧路径，并保留无关串口／工具的在途工作。

## 8. 架构验收矩阵

| 场景 | 必须得到的结果 |
| --- | --- |
| 同工具／目标并发打开十次 | 一个实例和必要 attachment，聚焦幂等 |
| 其他客户端活动时关闭／刷新 Inspector | 停止自身订阅，共用 transport 和其他客户端继续 |
| B 活动时取消 Parameters 操作 A | 只取消 A，B 状态／echo 正确关联 |
| Motor Test 待启动／运行／释放失败时关窗口 | 权限撤销，仅清理自己，失败／不确定保留 |
| Motor Test 借用主链路后 detach | 主 handle／参数缓存仍正常工作 |
| 初始化／写入期间换目标 | 旧 generation 结果／写入不能影响新目标 |
| 外部 QGC 修改参数／模式 | 重新验证／核对，不假设全局独占 |
| 关闭自动角色后启动应用 | 不创建可选窗口、DSP、DEM 读取、校准／电机会话 |
| 启用某 native 自动角色后启动 | 仅发生该角色已定义行为 |
| 重复预热后开关工具 | 所属订阅／task／buffer 回到基线，不单调增长 |
| FFT／日志／分析压满配置容量 | 内存／队列有界，控制／释放满足已记录 deadline |
| 活动执行期间切换工作流 | Monitor／Recorder 及冻结基线继续运行 |
| 输入／目标替换后旧 job 迟到 | 按身份丢弃或核对，不盲目应用 |
| Worker／Shell／Runtime 故障 | 分别证明声明的隔离边界 |
| 带中断操作记录重开 | 恢复证据／状态，不恢复电机／启动／校准意图 |
| 测试扩展模块 | 不修改无关工具或 transport 协议 |
| 主飞行目标断开时独立基站／来源仍活动 | 保留该角色所属会话，准确显示飞行侧注入／attachment 状态 |

逐项记录夹具、目标、deadline、前后资源数量、结果和证据路径。Unit／PTY／浏览器测试不能替代所声称的 native 进程、FC 重启或电气输出证据。

## 9. 已知问题按新顺序分配

| 原问题 | 新安排 |
| --- | --- |
| F01 Drape 速度参数及固定模型速度 | AF-17a；前期 Runtime 使用明确、已验证任务夹具 |
| F02 Drape 基准／anchor 合并 | 前期只定最小带基准契约，真实 Drape 路径修正放 AF-17a |
| F03 Drape 测线重绑 | 前期定义可选稳定来源 ID，生成器／drape 映射在 AF-15／17 重写 |
| F04 Drape revision／缓存循环 | 通用 job 生命周期在 AF-03／11，Drape 专属修正在 AF-17b |
| F05 上传／读回取可变状态 | AF-09，属于执行 Runtime 职责 |
| F06 剖面常量／端点插值 | AF-17d |
| F07 机型上限和空检查事实 | 目标能力／可用性契约在 AF-01／09，完整路线估算及 Drape 事实在 AF-16／17 |
| F08 非完整航程地形处理 | AF-17c |

后期通过前，旧地形生成任务仍保留其已知验证限制。前期架构验收使用明确有效任务／夹具，不作为这些旧路径的飞行放行依据。

## 10. 依赖与人员组织

关键路径：

`AF-00 → AF-01／02 → AF-03／04 → 现有工具迁移 → AF-09／10 → AF-11–14 → AF-15／16 → AF-17 → AF-18`

契约确认后，前端 loader／菜单可与 native facade 交叉推进；所有权稳定后 Inspector／Parameters 可交叉迁移；Motor Test 在这些基础之后。隔离打包准备可提前，I/O 权威迁出则在服务边界通过验证之后。

建议角色：

- Rust／Runtime 负责人：所有权、协议语义、Operation、进程边界。
- React／Tauri 工程师：懒加载、Tool SDK 消费、菜单、窗口／布局。
- 集成／GIS 工程师：适配、有界数据／worker、夹具，后期 Survey／Drape。
- 兼职 QA／native／台架、交互设计、飞机／地球物理 review。
- 所有者／操作员：范围、目标平台和验收决策。

## 11. 更新后的工作量与资源

1 工程人周为 5 个有效工作日，包含实现相关测试、review、文档；专门 QA／设计／领域支持另列。

| 工作包 | 工程人周 |
| --- | --- |
| AF-00 | 0.5–1 |
| AF-01 | 1–2 |
| AF-02 | 1.5–3 |
| AF-03 | 1–2 |
| AF-04 | 1–2 |
| AF-05 | 0.5–1 |
| AF-06 | 1–2 |
| AF-07 | 1.5–3 |
| AF-08 | 1–2 |
| AF-09 | 1.5–3 |
| AF-10 | 1–2 |
| AF-11 | 1–2 |
| AF-12 | 3–6 |
| AF-13 | 1–2 |
| AF-14 | 0.5–1 |
| **前期架构 AF-00–14 合计** | **17–34** |
| AF-15 | 6–10 |
| AF-16 | 8–15 |
| AF-17 | 13–25 |
| AF-18 | 2–4 |
| **后期功能／发布合计** | **29–54** |
| **完整工程合计** | **46–88** |

### 11.1 优先批准架构阶段的资源范围

AF-00–14 再预留 **8–14 人周**的专门 QA／native／台架、设计、领域及所有者 review，基础工作量为 **25–48 人周**；加入 20–30% 集成／返工余量，预算约 **30–63 人周**。

按三名研发加兼职支持，架构验收里程碑参考 **3–5 个月**。菜单和按需加载按小片段提前交付，不必等到最后的 Flight Plan／Drape。

### 11.2 全部计划与可选范围

- 完整工程加专门支持 **17–31 人周**，共 **63–119 人周**，尚未加余量。
- 若完整 QC 也纳入推迟中的 M7／M8，再加 **4–8 工程人周**；包含这些依赖及 20–30% 余量时，完整预算约 **81–166 人周**。
- 推荐团队的整体日历工期参考 **8–12 个月**，前提是决策及时、目标设备可用、后期测量范围最终获准；这不是固定日期承诺。
- 可选 AF-14C／14K 真实 Configuration／Calibration 再加约 **6–12 工程人周**及专门台架／QA，不包含在架构门槛或上表总量中。

这是把原 SW／TM 范围与明确架构／发布工作重新归属后的估算，**不能再把旧 Survey／工具估算直接加到这些数字上**。AF-00 实测及首轮 facade／工具迁移后重新校准。

### 11.3 设备与数据何时需要

- **架构阶段：** 开发机、CI／SITL、主桌面平台、代表性 PX4 台架目标、USB／遥测通道，以及协议／所有权／窗口／进程故障夹具；复用现有 Motor／RTK 资产。
- **Monitor／Execution：** 已验证简单任务、读回快照、带时间戳遥测／回放、已知坐标／高度参考，不需要新 Drape 生成器。
- **最后 Flight Plan／Drape：** 代表性 DTM／DSM／控制点、水准面网格、相机／LiDAR 契约和数据，以及相应地形外场验证。
- **可选 Configuration／Calibration：** 明确机型／过程清单及对应硬件／台架观察；硬件相关结论在软件夹具之后验证。

金额预算使用实际综合人员成本和缺失资产清单。主要技术不确定性是 native 进程打包、目标重启／身份、共享协议所有权，以及后期地形／实飞一致性。

## 12. 与旧工作包的对应

| 新归属 | 吸收的旧范围 |
| --- | --- |
| AF-00–04 | TM-01–04 基础，SW-04／05 中通用版本／所有权部分 |
| AF-05–07 | TM-02–04 的现有工具迁移 |
| AF-08 | SW-14A，模块驱动菜单 |
| AF-09–10 | SW-04／05／12／14B 中执行快照／监控生命周期部分 |
| AF-11–14 | TM-04／05 和扩展性证明；TM-06／07 只通过可选 AF-14C／K 实施 |
| AF-15 | SW-06／08 和来源 ID 的语义应用 |
| AF-16 | SW-09–11 |
| AF-17 | SW-01–03、SW-04／07 的 Drape 部分、SW-12／13 剩余范围及获准 M 依赖 |
| AF-18 | 跨模块发布验收，复用累积夹具 |

旧工作包的拆分是有意的：例如上传身份提前，Drape 计算缓存延后。每个实际实施 ticket 必须有一个 AF 所有者，避免重复排期、重复计费。

## 13. 所有者 review 与首个授权片段

请重点 review：

1. 架构优先顺序，以及 G-ARCH 通过后再做 Flight Plan／Drape。
2. 首要桌面／飞行器目标，以及独立 Runtime 进程里程碑。
3. 必做扩展性证明与可选真实 Configuration／Calibration 的边界。
4. Runtime 操作／权限／按需加载契约及与现有 ADR 的兼容。
5. 当前单作业飞行器交付范围，以及仅在接口层预留未来多机兼容。
6. 架构阶段人员／预算和后期功能预算。

获准后首先只做 **AF-00**：清单、基线实测、最小契约／错误签名和具体 AF-01 验收夹具。遵循仓库一次一个小任务、接口先 review 再实现的规则。

本文是规划资料。创建本文没有执行任何 AF 实现、应用迁移、飞行任务或硬件测试。

## 14. 未来多无人机扩展

### 14.1 可以扩展，但本轮要预留哪些边界

**可以，通过保留明确 Vehicle／Target 上下文，避免不带目标的全局状态，为以后扩展创造条件。** 所有者当前决定是先准备架构，短期不开发多无人机支持。`ActiveLink` 集合是起点，但它是连接集合，不是完整机队会话模型。

明确区分：

| 身份 | 含义 |
| --- | --- |
| `VehicleId` | 应用登记的逻辑飞行器身份 |
| `VehicleSessionId` | 当前 Runtime／飞行上下文；它本身不证明物理飞控启动身份 |
| `LinkId` + transport generation | 具体 I/O 连接实例及替换边界 |
| MAVLink system／component ID | 已验证链路／网络中的路由地址，不是全局唯一机体身份 |
| `FlightSessionId`／`ExecutionId` | 某架飞机的一次实际执行及记录 |
| `SelectedVehicleId` | UI 焦点／默认选择，不授权重定向已有 Operation |

使用可验证身份和明确绑定。缺失／重复 UID 或相同 system ID 不能静默合并两架飞机；重启／身份仍遵循 ADR-028 的核查要求。

### 14.2 Runtime 模型

下图是**未来拓扑草图**，不是本轮必须实例化多架作业飞行器的要求。近期可由同样的受限 API 承接一个飞行器上下文。

```text
Runtime
  TransportRegistry：LinkId → I/O 所有者／generation
  VehicleRegistry：VehicleId → VehicleContext
    身份与已验证路由
    遥测和参数值
    Command／Mission Operation 与资源声明
    执行基线和 Monitor
    Recorder／Session 引用
    高度 anchor 和传感器校准绑定
  RouteTable：VehicleId ↔ 已验证 LinkId + MAVLink target + generation
  共用不可变资源：DEM 瓦片／水准面网格／profile 定义

Shell：SelectedVehicleId
Tools：显式固定的 VehicleContext
```

公共类型避免永久绑定“一架飞机等于一个端口”的假设；真正的一机多链路、共享链路多机路由属于未来独立实现／验收。短期仍在明确支持的单作业目标拓扑内运行。

可变参数值、上传 hash、命令进度、Recorder、drape monitor、估计器高度 anchor、活动校准绑定，都不能是一个不带目标的全局值。不可变 DEM／水准面／profile 可以共享，同时保留逐执行 pin 和预算。

RTK 基站等独立共享设备保留自己的 Source session 所有权；多机可消费同一数据源，但各机注入状态、目标路由、新鲜度和带宽核算分别维护。

### 14.3 命令、事件和工具行为

- 每个 Operation 明确 `VehicleId`、已验证目标／路由、session／generation、operation／request ID；ACK 和 Mission current 关联包含飞行器上下文。
- 每机具有独立协议／操作状态；短注册表锁不能变成长时间包住某一架飞机配置／清理的全局大锁。
- 为 A 打开的工具，在主地图选中 B 时仍固定 A。任何“跟随选中目标”行为都应显式，且符合工具权限性质。
- 遥测／事件携带 VehicleId、来源链路、时间及 revision；boot time／UTC 映射逐机维护，日志不能仅按裸 sequence 混合。
- 只读链路选择与控制链路替换采用不同策略，新的连接不自动继承 Motor／write 权限。
- 未来批量操作包含明确子目标和逐机结果；部分失败可见，不伪装成所有飞机原子成功。

### 14.4 后续能力分级

| 阶段 | 内容 | 与本计划关系 |
| --- | --- | --- |
| MV0：设计准备 | 明确目标／飞行器上下文、会话所有权、可扩展接口 | 纳入 AF-00–02 普通设计，不设独立多机开发包 |
| MV1：多机监视 | 多连接、飞机列表、独立健康／轨迹／日志 | 架构验收后的产品扩展 |
| MV2：多机独立任务 | 每机独立计划／上传／执行／监控，明确目标控制 | 复用后期 Flight Plan／Drape 和逐机 Execution |
| MV3：协同航测 | 任务／测线分配、航次调度、空域冲突处理、协同中断策略 | 独立 Fleet Coordinator 及专门验收范围 |

多机监视、独立任务、协同飞行是不同验收层级。MV1／MV2 不代表已具备集群控制或自动避碰。当前“Flight Plan／Drape 最后”的顺序保持不变；MV1–MV3 是未来产品阶段，不暗中插入本轮前期。

### 14.5 延后的多机验收与容量

当前门槛验证已支持的单作业目标、目标替换、旧请求、已有独立设备角色的所有权；双机端到端及同时任务验证不属于近期门槛。

未来多机获准后，再增加双机不同电量／模式／参数、相同命令／任务序号交错、操作中切选择、独立取消／断开的夹具；共享网络身份歧义和路由拓扑届时专门验证。

后续容量指标应注明飞机数量、遥测速率、链路拓扑、活动 Monitor 和记录负载。采用逐机配额加全局预算；后台显示可以降采样，但不能抑制必需监控／记录和关键健康事件。集合类型本身不证明支持无限数量飞机。

最小接口预留纳入 AF-00–02。多机实现、同时外场验证、协同调度，在所有者另行授权并确定数量／拓扑后再制定计划。
