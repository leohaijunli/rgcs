# Signal Inspector 代码评审与优化计划

> 范围:`core::signals` → `app-tauri/inspector_service.rs` → `frontend/src/inspector/*`
> 日期:2026-10-09
> 说明:静态评审。此后已逐步落实:前端补丁跑过 `tsc`;Rust 已 `cargo test -p maggcs-core signals`(19 通过)与 `cargo build` 通过;PX4 6X 1.17 实机验证(见下"实机发现")。标注"待核实"的条目需先复现再修。

## 进度(2026-10-09 更新)

- **S0 完成**:补丁验证(`cargo test -p maggcs-core signals` 通过、`cargo build` 通过)、P0-2 修(FFT 持久 uPlot + setData + destroy)、P0-3 修(`trimBuffer` 保底 1024 点)。未做:devtools 人工核对、SITL 实跑(有硬件可做)。
- **实机发现(已修复)**:PX4 6X + 固件 1.17 连接时报 `InvalidEnum { MavEventCurrentSequenceFlags, value: 0 }`。根因:`CURRENT_EVENT_SEQUENCE.flags=0` 是正常值,但 mavlink 0.17 生成的 `MavEventCurrentSequenceFlags` 只有 `RESET=1`,无 0 变体;UDP 传输会静默丢弃坏帧,TCP/serial 会把错误浮上来。修复:`run_worker` 将单帧解析失败(InvalidEnum/InvalidFlag/UnknownMessage)视为可恢复,跳过该帧并计数(`is_recoverable_read_error`),不再断开重连。**已在实机验证**。附测试 `decode_failures_are_recoverable_but_io_is_not`。**已随 `1929c79` 提交并推送**。
- **信号浏览器分组(用户要求,原 S3 首项提前)**:左栏按消息分组、可折叠、带搜索,`msgName()` 提供 PX4 消息名映射(30/105/147/22/33/105 等,未知回退 `msg <id>`)。
- **ADR-016 已对齐实现** (`5ccec12`):写入实际帧类型 `SampleFrame`/`TraceSample`、tap 生命周期细节、时间戳决策;新增 "Implementation notes" 记录待办缺口(P0-4/P0-5/P0-6)。
- **S1 完成** (`3341748`):窗口 `Destroyed` 清理、可取消 tap(`oneshot` stop)、幂等 `inspector_connect` + 链路出现时自动 attach + `commands::connect` 重连后重新 attach、`inspector_status` 暴露 `TapStats`/链路状态、前端状态栏(msg/s、dropped、link)与错误提示、每来源时间映射(`time_boot_ms`/`time_usec` 各自 baseline)与 FC 重启回退检测(回退 >1 s 时重新取 baseline)。`cargo test -p maggcs-core signals` 由 13 增至 16 通过。
- **S2 完成**:数据模型重构(见 §3 S2)。`InspectorApp.tsx` 改为 plot/trace 模型,按 `trace_id` 路由,uPlot 与缓冲均以 `trace_id` 为 key;帧改为列式;`CatalogEntry.message_name` 由 mavlink 提供。前端 `tsc` 通过、mock 模式 Playwright 冒烟(`frontend/inspector-check.mjs`)0 错误、`core` lib 234 测试通过、`maggcs-app` 编译通过。
- **S3 完成**:暂停改为冻结视图 + 后台继续缓冲;每个字段可配多级滤波链(`+ Filter` 增删级、每级参数,编辑器移到属性面板);FFT 改走 Rust analyzer(`SampleFrame.spectra`),可选原始/滤波后(`Trace.analyzer_source`),显示 fs/Nyquist/Δf,分析器在 Rust 侧独立缓冲、与视图窗口解耦(视图缓冲保底仅 mock 保留);plot 窗口可新增/删除,多个 trace 叠加到同一个 plot(每个 plot 一个 uPlot、共用合并时间轴),每 trace 可选颜色并随工作区保存;mock 模式补了算法表,滤波/FFT UI 在浏览器可交互。
- **S4/S5 部分**:S4 tap 订阅集合改快照(`contains_message`,不再每条消息分配 `HashSet`,完成);S5 新增 Rust 测试:NaN 序列化、无时间字段消息时间戳单调、analyzer_source=Filtered(完成)。其余(前端环形缓冲/rAF、catalog 节流、Lagged 测试、前端 mock NaN 注入)未做。

---

## 1. 总评

分层是对的:tap、catalog、session、DSP 都在 core,Tauri 层很薄,新增算法也容易。问题集中在三处:

1. **落地和计划不一致。** 文档写"P0–P7 已完成",实际有几块是空的(见 P0-4)。
2. **前端数据通路还是原型写法。**
3. **边界情况(NaN、lag、重连、窗口关闭)基本没处理。**

---

## 2. 发现

### P0:正确性或功能不可用

| # | 问题 | 位置 |
|---|---|---|
| 1 | 已在补丁 `inspector-blackscreen-fix.patch` 中修复:NaN→`null` 导致 `toFixed` 抛错而黑屏;`received_at.elapsed()` 使时间戳恒约 0;`Lagged` 让 tap 永久退出;暂停/恢复后 Rust 侧 trace 被清空;缓冲区从不裁剪;tap 结束后 `batch_loop` 空转。 | tap / service / 前端 |
| 2 | **FFT 视图每帧泄漏。** `SpectrumView` 中 `ref={(el) => el && renderSpectrum(...)}` 是内联函数,每次渲染都会被调用,每次 `new uPlot(...)` 且从不 destroy。开着 FFT 时 30 Hz 创建图表,内存和 DOM 持续增长。 | `InspectorApp.tsx` |
| 3 | **补丁中的 `trimBuffer` 会让 FFT 拿不到数据。** FFT 需要最近 1024 点,而 10 s 窗口在 30–50 Hz 流速下只有 300–500 点,永远显示 "need 1024 samples"。原代码保底 `max(1024, …)`,补丁去掉了,需改回。 | `trimBuffer` |
| 4 | **Rust 的 FFT 是死代码。** `TraceConfig.analyzer` 前端永远传 `null`,`poll_spectra()` 在 app-tauri 中没有调用。界面上的 FFT 是 `dsp.ts` 的 TS 镜像,只算原始数据,不能算滤波后的数据。 | `session.rs` / `inspector_service.rs` |
| 5 | **窗口用 X 关闭时没有清理。** 清理只靠 React effect cleanup,窗口销毁时不一定执行。tap 和 session 继续运行,向已销毁的 channel 发送(错误被 `let _` 吞掉),`subs` 引用计数也可能泄漏。 | `inspector_service.rs` |
| 6 | **窗口先于链路打开或链路重连后没有数据。** `inspector_connect` 失败后被 `catch` 吞掉,界面只显示 "Waiting for data…",没有重试和状态提示。重连后是否重新 attach 待核实(未追 `commands.rs::connect`)。 | 前端 / service |

### P1:与需求不符或数据可信度

- **数据模型与需求不匹配。** 需求是"多个 plot 窗口,每个窗口选若干字段,每个字段各自选滤波器";现在是"每勾一个信号一个图,一图一个滤波器"。`Session.traces` 以 `SignalId` 为 key,同一信号放进两个图配不同滤波器会触发 `DuplicateTrace`。需要引入 `trace_id`,把 Plot → Trace[] → Pipeline[] 分开。
- **滤波链只用了一级。** Rust 支持 `pipeline: Vec`,UI 只能选一个算法,做不了 "detrend → LPF"。
- **信号 key 会冲突、会写错。** `signalKey` 不含 system/component,多机会撞;`MESSAGE_NAMES` 只有 30 和 105,其余显示成数字。`signalFor` 在 catalog 里找不到时硬编码 `message_id: 30`,恢复工作区时可能静默订阅错误信号。
- **时间轴可能错乱(待核实)。** 所有消息共用一个 `TimestampMapper`,而 `time_boot_ms` 与 `time_usec` 可能不在同一时基。FC 重启(SITL 重启但不断链)后最小偏移是旧的,新数据会落在过去的时间点。
- **暂停语义。** 现在暂停是丢弃新数据,恢复后曲线有断档。SDI 的做法是冻结视图、后台继续缓冲。
- **错误被吞。** `inspector_set_traces(...).catch(() => undefined)` 会让非法参数、`DuplicateTrace` 完全无提示。`TapStats`(dropped 等)也没有暴露给 UI。

### P2:性能

- **tap 每条消息都做全量提取。** 每条消息 serde 展开、锁 catalog、锁 stats;`subs.message_ids()` 每次分配一个 HashSet 再比较。PX4 SITL 的高频消息下开销明显。
- **帧格式冗余。** 每个 `TraceSample` 带完整 `SignalId`(含 `String`),JSON 序列化。应改成按 trace 分组的列式数组,用 `trace_id: u32`。
- **前端每帧整体重渲染。** `bump()` 以 30 Hz 触发整个 `InspectorApp` 重渲染,包括几百行信号列表。数据应放 ref,由 `requestAnimationFrame` 驱动 uPlot 重绘,React 只管结构。
- **缓冲区用 `number[]` + `splice`,每帧 `Float64Array.from` 拷贝。** 应改为预分配 Float64Array 环形缓冲;点数超过像素宽度时做 min/max 抽稀。
- **信号列表平铺。** PX4 有几百个字段,需要按消息分组的树和搜索。

### P3:工程质量

- **测试盲区。** `inspector-check.mjs` 只跑 mock 模式,mock 数据没有 NaN/null,抓不到这次的崩溃。tap 现有测试用 `time_boot_ms`,掩盖了 `elapsed()` 的问题。
- **文档与实现不符。** `signal-inspector-plan.md` 状态需按上面的发现更新。
- **TS 镜像与 Rust DSP 重复。** 长期只保留 Rust,TS 镜像仅用于 mock。

---

## 3. 分阶段计划

### S0:验证补丁(约 0.5 天)

- [x] `cargo test -p maggcs-core signals`、`cargo build`(通过:signals 13 passed、build 编译干净)
- [ ] 打开 inspector 的 devtools,确认不再报错(需人工;mock 模式 Playwright 检查已 0 错)
- [x] 修 P0-3:FFT 缓冲保底 1024 点(`trimBuffer` 保底 `max(1024, 窗口样本数)`)
- [x] 修 P0-2:FFT 图改为持久的 uPlot 实例 + `setData`,卸载时 destroy(`SpectrumView`)

### S1:稳定性(1–2 天)

- [ ] 在 Rust 侧监听窗口 `CloseRequested` / `Destroyed`,执行 disconnect
- [ ] `inspector_connect` 幂等;失败时界面显示状态并自动重试;订阅链路状态事件
- [ ] 暴露 `TapStats` 与链路状态,界面显示 dropped、msg/s
- [ ] 不再吞错误,在界面提示(`set_traces` 等)
- [ ] 每个来源独立的时间映射,并检测时间回退

### S2:数据模型重构(2–3 天,破坏性改动,需确认)

- [x] 新增 `Plot { id, traces: Trace[] }` 与 `Trace { id, signal, pipeline: Vec<AlgoConfig>, analyzer }`;`Session` 以 `trace_id` 为 key(`by_signal` 做 signal→trace 路由),允许同一信号多个 trace;`SessionError::DuplicateTrace` 改为按 trace id
- [x] `CatalogEntry` 增加 `message_name`(`tap` 从 `mavlink::Message::message_name()` 取);前端 key 改为 `sys:comp:msg.field`;工作区升级 v2 保存完整 `SignalId` 与 `pipeline`
- [x] 帧改为列式:`TraceFrame { trace_id, t[], raw[], filtered[] }`,一条 `SampleFrame` 携带多条 trace;缓冲、uPlot、CSV 均按 `trace_id`

### S3:UI 对齐需求与 SDI(2–3 天)

- [x] 信号树按消息分组,带搜索(2026-10-09 提前完成,含可折叠分组;拖到 plot 未做)
- [x] 每个字段单独选滤波链(可多级)、参数(2026-10-09:`+ Filter` 增删级,每级独立算法/参数;mock 与 Rust 均跑整条链)
- [x] 可新增、删除 plot 窗口(把多个 trace 合并到一个 plot);每个 trace 可选颜色(2026-10-09:每个 plot 一个 uPlot,多 trace 的 raw/filtered 叠加在合并后的共用时间轴上;`New plot`/`+ Signal`/`×` 增删;颜色板可覆盖并随工作区保存)
- [x] FFT 改走 Rust analyzer,可选原始或滤波后;显示 fs 与 Nyquist;使用独立 FFT 环形缓冲,与视图窗口解耦(2026-10-09:`Session::poll_spectra` 经 `SampleFrame.spectra` 下发,`Trace.analyzer_source` 选原始/滤波后)
- [x] 暂停改为冻结视图、后台继续缓冲(2026-10-09:`onMessage` 不再丢数据,暂停只冻结 uPlot 视图)

### S4:性能(1–2 天)

- [x] tap 内订阅集合改为快照(`Subscriptions::contains_message`,不再每条消息分配 `HashSet`)
- [ ] catalog 更新节流到约 10 Hz(未做:与 `rate_hz` 的 EMA 语义冲突,需先设计按间隔计数)
- [ ] 前端环形缓冲、rAF 重绘、min/max 抽稀
- [ ] 目标:8 个 trace、各 200 Hz,连续 30 分钟,堆内存不持续增长,单核占用有上限(阈值在 S0 基线测量后确定)

### S5:测试与文档(1 天)

- [x] Rust:带 NaN 的 `CatalogEntry` 序列化测试(`catalog_entry_with_nan_serializes_as_null_not_a_panic`)
- [x] Rust:tap 时间戳单调且接近真实间隔的测试(含无时间字段的消息,`timestamps_without_a_time_field_are_monotonic`)
- [ ] Rust:Lagged 之后 tap 继续工作的测试
- [ ] 前端:mock 加 null/NaN 注入,Playwright 跑 5 分钟,要求无 `pageerror`、无内存增长
- [ ] 更新 `signal-inspector-plan.md` 的状态

---

## 4. 建议顺序

先做 **S0 + S1**,再决定 S2 是否全量重构。S2 会改动 IPC 类型和工作区格式,需要先确认。
