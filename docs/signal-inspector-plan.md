# MagGCS 实时信号监视器(Signal Inspector)实施方案

> 状态:草案,待确认 P0 签名后进入实现。
> 基于对 `rgcs-main` 的静态阅读(`crates/core`、`crates/app-tauri`、`frontend/src`)。**未编译验证**,未打开确认的地方已标注。

## 1. 需求

- 在线查看 PX4 经 MAVLink 发来的数据,实时绘制原始曲线。
- 可对实时数据应用多个算法,滤波结果与原始数据**同时绘制**。
- 可新增绘图窗口(Plot);每个 Plot 可定义同框显示的多个 MAVLink 字段;每个字段可选不同滤波算法。
- 整个监控页面是**独立弹出窗口**,从主界面触发。
- 可设置滤波参数,可在线 FFT 分析。
- 初始算法:二阶低通、高通、滑动平均、detrend、实时 FFT。
- 架构良好,易于扩展新算法。
- 绘图交互参考 Simulink Simulation Data Inspector(SDI)。

## 2. 现有代码对设计的约束

1. **现有遥测通路不能复用。** `TelemetryAggregator` 只处理 6 种消息(Heartbeat、GlobalPositionInt、Attitude、SysStatus、BatteryStatus、GpsRawInt),hub 以 20 Hz 节流推快照。用它做 FFT,奈奎斯特频率最高 10 Hz,且无法取任意字段。需要**旁路**:直接订阅 `ConnectionHandle::subscribe_route(MessageRoute::all())`(该 API 已存在)。
2. **任意字段可通用提取。** workspace 的 `mavlink` 已启用 `serde` feature,消息可序列化。用自定义 `Serializer` 收集数值字段为 `(字段名, f64)`,按 message id 缓存字段表,无需逐消息手写映射。
3. **窗口必须是真 OS 窗口。** 现有 `TelemetryDialog`/`QcDialog` 是主窗口内的 Radix Dialog。独立窗口需新建 Tauri webview;`capabilities/default.json` 目前只授权 `"windows": ["main"]`,需加新 label。
4. **每个 webview 是独立 JS 上下文**,Zustand store 不共享,监视器窗口需自行订阅数据。
5. **uPlot 已在依赖中**,CSP 已允许 `worker-src blob:`,无需引入新绘图库。

## 3. 总体架构

```
PX4 ──MAVLink──▶ ConnectionHandle (broadcast<ConnectionEvent>)
                    ├─▶ TelemetryHub → 20Hz 快照 → 主窗口(现有,不动)
                    │
                    └─▶ core::signals::SignalTap   (仅在监视器打开时订阅)
                          · 通用字段提取 → 实时 Catalog(消息/字段/Hz)
                          · 按订阅集合只产出被选中的信号样本
                                 ▼
                       core::dsp  Pipeline / Analyzer(每条 Trace 一份)
                          · Processor 链:LPF2 / HPF2 / MovAvg / Detrend …
                          · Analyzer:FFT → 频谱帧
                                 ▼
                       app-tauri::inspector_service
                          · tauri Channel(~30 Hz 批量帧)
                                 ▼
                       Inspector 窗口(独立 webview)
                          · 环形缓冲(非 React state)→ uPlot
```

### 关键决策

**决策 A:DSP 放在 Rust `core`,前端只负责渲染(拟写入 ADR-009)。**

- 符合 ADR-001(领域逻辑在 core,headless server 与 Phase 4 QC 可复用)。
- 可用 `cargo test` 对 scipy 黄金向量做数值验收(AGENTS 规则 3、4)。
- 原始与滤波曲线由同一批样本产生,时间轴天然对齐。
- 代价:改参数需一次 IPC(毫秒级,可接受)。
- 备选:前端 Web Worker 做 DSP,延迟更低,但会出现 TS/Rust 两套实现。

**决策 B:信号源抽象为 `SampleSource` trait。** 目前只有 live MAVLink,将来可挂 ULog 回放(`testdata/` 已有 ULog)。

## 4. 数据采集层 `core::signals`

- `SignalId { sys, comp, msg, field, index? }`:带 sys/comp 为将来多机预留(`AppState` 已按 `links` map 设计);数组字段展开为 `field[i]`。
- `NAMED_VALUE_FLOAT` / `DEBUG_*` 等按名字区分的消息,单独处理为 `NAMED_VALUE_FLOAT/<name>`,自定义传感器通常走这条路。
- **Catalog**:每个 `(msg, field)` 记录最近值、速率估计(EMA)、最后到达时间;变化时约 2 Hz 推送,填充前端信号树。
- **订阅引用计数**:所有 Plot 用到的信号取并集;其余消息只做 id 比较即丢弃。窗口关闭则 tap 任务退出,开销为零。
- **时间戳**:PX4 将多条消息打包进同一 UDP 包,接收时间成团、抖动大,会抹开 FFT 峰。有 `time_boot_ms` / `time_usec` 的消息优先用 FC 时间戳,按「最小 `rx−fc` 偏移」映射到主机时间轴;没有时间戳的退回接收时间,并在 UI 标出该 trace 的计时精度。
- **生命周期**:`InspectorService` 放 `AppState`(不放 `ActiveLink`)。`connect` 时 `attach(handle)`,`disconnect` 时 `detach`。重连后 Plot 配置保留,滤波状态重置并在曲线留断点。
- **事件总线**:`EVENT_CAPACITY = 4096`,tap 须在独立任务中尽快消费;`Lagged` 时像 hub 一样计数并暴露给 UI。

## 5. DSP 层 `core::dsp`

### 扩展点

```rust
pub trait Processor: Send {
    fn configure(&mut self, p: &ParamValues, fs: f64) -> Result<(), DspError>; // 热更新,尽量不丢状态
    fn reset(&mut self);
    fn process(&mut self, x: f64) -> f64;
}

pub trait Analyzer: Send {            // 时域→频域,输出类型不同,单独 trait
    fn push(&mut self, t: f64, x: f64);
    fn poll(&mut self) -> Option<SpectrumFrame>;
}

pub struct AlgorithmDescriptor {      // 自描述:前端据此自动生成参数表单
    id: &'static str,
    name: &'static str,
    kind: Kind,                       // Processor | Analyzer
    params: &'static [ParamSpec],     // key / label / unit / Float{min,max,step,log} | Int | Enum | Bool / default
    factory: fn(&ParamValues, fs: f64) -> Result<Box<dyn Any>, DspError>,
}
```

**新增算法 = 一个 Rust 文件 + registry 一行,前端零改动。** 前端启动时调用 `inspector_list_algorithms` 取得描述表,参数表单由 `ParamSpec` 渲染。类型经 ts-rs 导出(沿用 ADR-002 流程)。

`Pipeline` 为 `Vec<Box<dyn Processor>>`,一条 Trace 可串多级(如 detrend → LPF)。UI 第一版允许一级,数据结构一开始按链设计。

### 算法一览

| 算法 | 参数 | 在线实现要点 |
|---|---|---|
| 二阶低通 | fc、Q(默认 1/√2) | 双线性变换 + 预畸变,Direct Form II Transposed。K = tan(πfc/fs),norm = 1/(1 + K/Q + K²);b0 = K²·norm,b1 = 2·b0,b2 = b0,a1 = 2(K²−1)·norm,a2 = (1 − K/Q + K²)·norm |
| 高通 | fc、Q | 同一 biquad 模块,b0 = norm,b1 = −2·norm,b2 = norm,a1/a2 同上 |
| 滑动平均 | 窗口(样本数或毫秒) | 环形缓冲 + 运行和,O(1);定期重算和以避免浮点漂移 |
| Detrend | 窗口、模式(常数/线性) | detrend 本质是批处理;在线版用滑窗最小二乘(运行和 Σx、Σtx),取拟合线在当前点的值后相减,O(1)/样本 |
| 实时 FFT(Analyzer) | N(256–8192)、窗函数(Hann/Hamming/Blackman/Flat-top/矩形)、重叠、预处理 detrend、平均(无/指数/峰值保持)、刻度(幅值/PSD/dB) | `realfft`;按 FC 时间戳线性插值重采样到均匀网格;输出附 fs、Δf = fs/N、Nyquist |

### 共性处理

- 采样率 `fs` 按每个信号的时间戳用 EMA 估计,变化超过阈值才重算系数。
- `dt > GAP_FACTOR × 标称 dt` 视为丢包:`reset()`,并向曲线写入 `NaN` 以断线(SDI 同样显示缺口)。
- `fc ≥ fs/2` 等非法参数返回 `DspError::InvalidParam`,不 panic(AGENTS 规则 6)。
- 因果滑动平均有 (N−1)/2 样本群延迟:Trace 提供「显示时补偿延迟」开关,仅影响显示,不改数据。
- 魔数(GAP_FACTOR、EMA 系数、窗口上下限等)统一提取为常量。

## 6. 窗口与前端

### 独立窗口

- Rust 侧提供 `inspector_open` 命令:窗口已存在则聚焦,否则 `WebviewWindowBuilder` 创建(单例)。从 Rust 创建可避免给前端放开 `core:webview:allow-create-webview-window`。
- `capabilities` 的 windows 增加 `"inspector"`。
- Vite 改多页面:新增 `inspector.html`,窗口用 `WebviewUrl::App("inspector.html")`,监视器窗口不加载 Cesium。(`vite.config.ts` 未打开确认,细节实现时核对。)
- 主界面在 TopBar 或 IconRail 增加入口按钮(具体位置未细看,按现有图标风格放置)。
- 修改后需跑 `crates/app-tauri/tests/desktop_csp.rs`。

### 界面布局(对照 SDI Inspect 视图)

```
┌ 工具栏:▶/⏸ 运行 │ 窗口时长 10s/30s/60s │ +Plot  布局(1×1,2×1,2×2…) │ 光标 │ 轴联动 │ 清空 │ ● 录制 ┐
├ 左:信号浏览器 ───┬ 中:Plot 网格 ────────────────────────────┬ 右:属性面板 ───────────┤
│ 搜索框            │ ┌Plot 1 [时域|频谱|分屏]────────────┐  │ 选中 Trace:            │
│ ▾ ATTITUDE  50Hz  │ │ roll(raw)  ── roll(LPF2 fc=5Hz) ──  │  │  来源信号 / 颜色 / 线宽 │
│   ☑ roll          │ │                                     │  │  滤波链(+ 添加算法)    │
│   ☐ pitch         │ └─────────────────────────────────────┘  │   └ 自动生成的参数表单   │
│ ▾ HIGHRES_IMU     │ ┌Plot 2 …                             ┐  │ Plot:Y 轴自动/手动、    │
│   …  拖拽到 Plot  │                                          │  FFT 设置、峰值标注     │
└───────────────────┴──────────────────────────────────────────┴────────────────────────┘
```

### 与 SDI 对应的功能

- 信号树勾选或拖入 Plot。
- 同一信号可放多条 Trace(原始、LPF、MA 叠加)。
- 子图网格布局。
- 子图时间轴联动(uPlot `cursor.sync`)。
- 双光标测量 Δt、Δy。
- 暂停显示但继续缓冲,可缩放平移历史。
- 右侧属性面板。
- 第一版不做 SDI 的 Compare(多次运行对比)。

### FFT 视图

- 每个 Plot 可切换:时域 / 频谱 / 分屏。
- 可选择对原始或滤波后的 Trace 做 FFT,并叠加显示,用于验证滤波器截止效果。
- 峰值标注(Top-N 频率)、光标读数、fs / Δf / Nyquist 显示。

### 渲染性能

- 数据放 `Float64Array` 环形缓冲,置于 React state 之外,用版本号通知,避免每帧重渲染。
- `requestAnimationFrame` 节流到约 30 fps,仅在有新数据且未暂停时重绘。
- 同一 Plot 内采样时刻不同的信号(如 50 Hz 的 ATTITUDE 与 200 Hz 的 IMU)用 `uPlot.join` 合并 x 轴;同一信号的原始/滤波 Trace 时间相同,无需对齐。
- 颜色使用 `design-system` token(`QcDialog` 已用 `cssVar`),Trace 调色板定义在 `theme.ts`;仓库有 `check:colors`、`check:contrast` 脚本(未打开确认),按其约束实现。
- 界面文案全部走 i18n 的 en 资源(AGENTS 要求用户可见内容为英文),需通过 `check:i18n`。

## 7. IPC

- 使用 `tauri::ipc::Channel`,后端约 30 Hz 批量推送:

  ```
  Frame { seq, signals: [{ idx, t[], traces: [{ id, y[] }] }] }
  ```

  同一信号的各 Trace 共用一份 `t`。
- 频谱帧单独推送,不超过 10 Hz。
- 先用 JSON,压测不达标再换二进制。
- 新增类型用 `#[derive(TS)] #[ts(export)]` 导出,沿用 ADR-002。

## 8. 分阶段任务与验收

按 AGENTS.md:每个任务先交付签名与错误类型,确认后再实现。验收数字为提议值,待 SITL 实测后校准。

| 阶段 | 内容 | 验收(可测量) |
|---|---|---|
| P0 | ADR-009(DSP 放 core)、ADR-010(独立窗口 + Channel);交付 Rust trait 与 TS 类型签名 | 签名经确认 |
| P1 | `core::dsp`:biquad(LP/HP)、MovAvg、Detrend、窗函数、FFT、registry | LP/HP 在 fc 处增益 −3.01 dB ±0.1;LP 在 10·fc 处衰减约 −40 dB ±1;系数与 `scipy.signal.butter(2, …)` 黄金向量误差 < 1e-9;MovAvg 与 O(N) 朴素实现在 10⁵ 随机样本上差 < 1e-9;Detrend 与 scipy 对照;FFT 正弦峰位与幅值误差 < 1%;热更新参数后输出无 NaN |
| P2 | `core::signals`:提取器、Catalog、Tap、订阅引用计数 | ATTITUDE / HIGHRES_IMU 字段表与值正确;假 FC(参照 `connection_integration.rs` 的做法)以 100 Hz 发送,零丢样;无窗口时 tap 不订阅 |
| P3 | `app-tauri`:`inspector_service`、命令、Channel、第二窗口骨架(仅信号树) | SITL 下窗口可弹出并显示实时 Catalog;重复点击只聚焦不新建 |
| P4 | 前端:信号浏览器、Plot 网格、原始曲线、暂停、窗口时长、+Plot | 8 条 Trace × 200 Hz × 60 s 窗口 ≥ 30 fps;浏览器模式有 mock 数据源(参照 `telemetry/mock`),可截图 |
| P5 | 滤波链 UI(参数表单自动生成)、多 Trace 叠加、热更新 | 拖动 fc 滑块,曲线 < 200 ms 响应;原始与滤波曲线时间对齐 |
| P6 | FFT 分析器 + 频谱视图(峰值标注、原始/滤波频谱叠加) | 指标同 P1;UI 显示 fs、Δf、Nyquist |
| P7 | SDI 式增强:光标、轴联动、属性面板、布局预设、工作区保存/加载(`plugin-fs` 已授权读写文本)、录制导出 CSV、`SET_MESSAGE_INTERVAL` 调速 | 工作区往返保存后一致;导出 CSV 行数与缓冲一致 |

每个后端模块附单元测试或 SITL 脚本;前端任务附截图(`scripts/screenshot.mjs` 基线)与手动验证步骤。

## 9. 目录规划

```
crates/core/src/
  signals/    mod.rs  extract.rs  catalog.rs  tap.rs
  dsp/        mod.rs  registry.rs  biquad.rs  moving_average.rs  detrend.rs  fft.rs  pipeline.rs
  inspector/  session.rs                      # Trace = 信号 + Pipeline (+ Analyzer)
crates/app-tauri/src/
  inspector_service.rs                        # 薄适配层,同 mission_service / command_service
frontend/
  inspector.html
  src/inspector/
    InspectorApp.tsx
    store/        workspace.ts                # plots / traces / selection(Zustand)
    data/         ringBuffer.ts  channel.ts  mock.ts
    components/   SignalBrowser  PlotGrid  TimePlot  SpectrumPlot
                  PropertiesPanel  FilterChainEditor  ParamForm  Toolbar
docs/adr/         009-dsp-in-core.md  010-inspector-window-and-channel.md
testdata/dsp/     scipy 生成的黄金向量
```

## 10. 风险与假设

- **FFT 带宽受 MAVLink 流速率限制。** PX4 默认给 GCS 链路的流较慢(ATTITUDE 等约几十 Hz),奈奎斯特频率因而很低。做振动或高频分析,需先用 `MAV_CMD_SET_MESSAGE_INTERVAL` 提高目标消息速率(P7 做成 UI 控件),或在 PX4 侧用 `mavlink stream` 调整。`CommandService` 是否已支持该命令未核实,P7 时确认,不支持则补充。UI 明确显示当前 fs 与 Nyquist,避免误读频谱。
- **丢包与乱序**(UDP):由 gap 检测与断线处理覆盖。
- **未验证项**:`mavlink` 0.17 serde 序列化的字段形态(枚举、bitflags、数组)需在 P2 起步时用测试确认;`vite.config.ts` 多页面改法;TopBar / IconRail 入口位置;`check:*` 脚本的具体约束。
- **设计假设**:监视器窗口为单例,窗口内放多个 Plot;一条 Trace 可串多级滤波;`NAMED_VALUE_FLOAT` 等自定义消息在范围内;多机、ULog 回放不在第一期,但接口已预留(`SignalId` 含 sys/comp、`SampleSource` trait)。

## 11. 待确认

DSP 放在 Rust `core`(推荐)还是前端 Web Worker?确认后产出 P0 的 Rust trait 与 TS 类型签名(含 `ParamSpec`、`SignalId`、`Frame` 与错误类型)。
