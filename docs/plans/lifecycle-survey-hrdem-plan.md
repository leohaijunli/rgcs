# MagGCS 实施方案:全生命周期安全 · 架构 · 磁测(含 VIC 日变改正)· HRDEM · UI/UX · 项目管理 · drape 轨迹与监控

基线:`rgcs-main__1_.zip`(静态阅读,未编译、未运行)。放置位置:`docs/plans/lifecycle-survey-hrdem-plan.md`;落地后按仓库惯例并入 `FEATURES.md` / `ARCHITECTURE.md` / `issues.md` 并删除本文件。标 *verify* 的事实来自推断或记忆,执行前必须实测或查证,不得当作已知。标 **⏸** 的 WP 已推迟(见「范围与状态」):正文保留,**解冻前不得实施**。

## 范围与状态

**⏸ = 已推迟,不实施**(正文保留,供将来启用):

- P3:M1–M4、M7–M9、U6、U7、U10
- 连带阻塞:M6(依赖 M3)、PJ5(依赖 M6)

**不属于推迟**:drape 轨迹设计与监控(§5B,DR1–DR9),其中 DR5–DR7 不依赖 M1/M3/U6/M7;U7 中仅 HUD 的 AGL 项由 DR6 提供。M3 启用时复用 DR5 的偏差计算,U6 复用 DR6 的组件,不得重复实现。

**保留扩展点**(推迟期间不实现,但设计其他 WP 时不得堵死):AlertCenter 的来源枚举可扩展(A3);清单引擎的分组与模板可注册(A5/PJ2);事件日志为开放 JSONL(A4);`project.json`/`survey.json` 带 `schema_version` 与迁移(PJ1);`SurveyProfile` 注册表(PJ2)。

**HRDEM 取数方式**:STAC 查询 + COG 窗口读取,`dem-prep` 采用 GDAL/PROJ(`/vsicurl/` 窗口读取与 `gdalwarp` 重投影),不自行实现 Lambert 投影;GDAL/PROJ 仅限 `tools/dem-prep`,`core` 不依赖(见 §6、规则 13)。WCS 官方将淘汰,不作数据源;WMS 仅用于显示。

**ADR**:ADR-018(HRDEM)、ADR-019(项目模型)、ADR-021(记录器)、ADR-022(drape 算法选择、航点简化与监控匹配)。ADR-018 已接受的 §D 空变换决策优先于本文早期草案;若实现前需要改变该决策,先由操作员确认并修订 ADR,不得让 H2 默认为正规七参数变换。

## 2026-10-15 项目审查基线与执行顺序

本节记录按当前代码与自动化验收检查得到的实施状态,用于安排后续工作;不是新的实现承诺。每完成一个 WP,更新对应状态与验证证据。审查时 Rust 工具链曾不在 PATH;本次 L4 执行需安装/配置后运行对应测试。GDAL/PROJ 工具仍不属于 L4 验收范围。

> 状态表已于 2026-10-18 按 DR1–DR7/DR9、U17、L9/L10、A8 等执行记录同步更新;历史明细保留在下方各 WP 执行记录。

| 状态 | 工作包 | 依据/缺口 |
|---|---|---|
| 已实现 | A1–A4、A6–A10、L1–L10、U1、U2–U5、U8、U9、U17、PJ1、PJ2、PJ3、H0–H4、H7、H8、DR1–DR7、DR9(事后评估版) | 逐项证据见下方执行记录(2026-10-15 → 2026-10-18)。要点:H1 live 验收与 H0 夹具 ±2 px 逐像元一致;H2 强制 §D 空变换;H3 DSM-max 保峰实测锁定;H7 真实 CGG2013 格网 + 两段式控制点互校(**PX4 实机 <0.5 m 现场验收仍待**);A2 主链断开联合回归全绿;L5 强制解锁三处记录时间戳一致;L9 goto 距离/地形守卫;L10 场景测试集(core 757);A8 WebGL context lost 隔离与 Restart;U17a–d Plan 步骤就绪/阻塞/批量编辑/同步相位;PJ1–PJ3 项目模型、profile 注册表与强制启动页;DR1–DR7 全链(spec/solver/compile/preview/监控/记录导出)+ DR9 雷达偏置稳健估计。**DR8 已取消(操作员,不做方位角扫描)**;U7 中仅 HUD 的 AGL 读数由 DR6b 提供,其余 U7 内容仍 ⏸。 |
| 部分实现 | A5、PJ3(数据侧) | A5 清单引擎 + Drape 组 + PJ2 模板已就位。**A9 与 U5 收尾均已完成(2026-10-20,见执行记录)**:solve→compile 为唯一航点来源(`meshDrape.ts`/`terrain_drape` 已删);每个 Fail 项有修复入口,不可行区段逐项确认落地。PJ3 界面完成,但测量/会话**数据**要等 M4(⏸)才写入 project store。 |
| 未实现/未验收 | L8、A10、A11、U11–U16、PJ4、H5、H6、H10、H11 | L8 启动自检/连接确认;A10 诊断包;A11 跨窗口单一事实源(U15 前置,未完成前 U15 不得上线);U11 多窗口协调、U12 现场模式、U13 长任务、U14 设计系统整理、U15 地图分离、U16 图层管理;PJ4 项目资源与归档;H5 惰性分块、H6 来源自动解析、H10 对比验证、H11 点云路径。 |
| 明确推迟 | ⏸ 项 | 按「范围与状态」与规则 10 执行,不得因上述审查而擅自解冻。 |

**实机/现场待验收清单**(代码与自动化测试已完成,需真机):

- PX4 高度链 <0.5 m 控制点(H7/H10 现场验收)。
- AGL 落地自标定复验:开机 → mesh 开 → 起飞 AGL ≈ 0 → B 点降落 AGL ≈ 0。
- armed 状态下项目 open/create/close 拒绝与重启重开流程(PJ3)。
- DR6 手动走查(terrain 生成→上传回读→Fly 面板→越带/预瞻/净空告警→Acknowledge→断链 STALE);DrapePreview 截图(需真实 DEM)。
- DR5 检出率/误报率表随实机/SITL 数据出。
- issue 13 的 PX4 参数名核对(`DO_CHANGE_SPEED`、接受半径、爬升/下降限速、任务项上限)。

**后续执行批次**(一次一个 WP;1–5 批已完成,见各执行记录):

1. ~~L4 修正与验证~~(已实施,见 L4 执行记录)。
2. ~~L5 审计闭环~~(已实施,见 L5 执行记录)。
3. ~~A2 + A6/L7~~(已实施,见 A2/L7/A6 执行记录)。
4. ~~H1–H4/H7~~(已实施,见 H1–H4/H7 执行记录;实机控制点验收留清单)。
5. ~~PJ1→PJ2→PJ3 与 DR1→DR6(→DR7/DR9)~~(已实施,见各执行记录)。
6. ~~A9 收尾~~(已实施 2026-10-20,见 A9 执行记录)。
7. ~~U5 收尾~~(已实施 2026-10-20,见 U5 执行记录)。
8. ~~L8~~(已实施 2026-10-20,见 L8 执行记录)。
9. 再推进 P2/P4:~~A10~~(已实施 2026-10-20)、PJ4、H6、H10、U11–U14;**A11 完成后才可启动 U15/U16**;⏸ 项必须等待明确解冻。

### 验证基线 (2026-10-15)

#### A9 执行记录 (2026-10-20)

- **操作员决策(2026-10-20)**:① mesh 兜底路径直接删除(terrain 模式强制 DEM 覆盖);② 降落项重贴 DEM 表面(无 AGL、无 vertical offset);③ 旧 `core::terrain::drape`、`terrain_drape` 命令与 `meshDrape.ts` 一并删除。
- **发现接线缺口**:`drape_preview` 此前只调 `solve`、从不调 `compile`——`result.compiled` 恒空 → 监控门禁(`compiled.length===0` 即停)从未生效、preflight `waypoint_count` 恒 0、`MonitorBaseline` 静默回退设计剖面(违反 ADR-022 §8);旧 DEM 桥的 `terrain_drape` 命令签名还没有 `vertical_offset_m` 参数(offset 只在 mesh 路径生效)。
- **实现(后端 `terrain_service.rs`)**:`drape_preview` 在 `spawn_blocking` 内 `solve → compile`,新增 `waypoints`(compiled NAV 项经真实 H7 链转 `AMSL_EGM96`,共享 `vertical_chain_offset`,`VERTICAL_DATUM_CHAIN_ERROR` fail-closed)与 `landing_amsl_m`(降落点采样 DEM 表面、同链转换;`DrapeLineInput` 同 `[lon,lat]` 约定);速度项(178)与 datum 原样透传(z 是命令参数);`DrapeResult.compiled` 保持 DEM datum——监控基线与锚定 fix 同帧。
- **实现(前端)**:新纯函数 `mission/drapeApply.ts::applyCompiled`(编译产物按块替换 pattern block;块内 pinned 高度按位置容差恢复;`21/85` 降落项按位置键贴地;块外一律原样;返回重锚定的线表);`stores/mission.ts` 合并 `runDrapePreview`/`runTerrainDrape` 为单管线 `runDrapeOnce(apply)`(指纹、600 ms 防抖、A3 告警、unchanged 不置 dirty 语义保持);手动编辑(lastPattern 清空)→ fail-closed 报"regenerate the pattern",不再静默保留旧高度;DrapeBlock/SyncStep provenance(`dem:<folder>`、dem_ref.hash)改读新 `DrapeState`;删除 `meshDrape.ts`、`DrapeState.report`(DrapeReport)/`meshDrape` 字段与 `MESH_*` 文案。
- **验证**:`cargo test -p maggcs-core` 全绿(drape.rs 测试随模块删除,699 lib + 集成);`cargo test -p maggcs-app` 76 通过(新增 `drape_apply_tests` 3 条:H 链转换数值断言 ±0.02、链外 fail-closed、命令项透传);`drape_pipeline.rs`(solve→compile 真实文件 I/O)3 条通过;clippy `-D warnings`(core+app)、fmt、`git diff --check`;前端 `tsc`、production build、14 个 check 脚本 + `check:agl`(27 条,重定向到 `applyCompiled`)全绿;ts-rs 绑定重生成,删除 `DrapeReport/DrapeOptions.ts`。**行为变更**:terrain 模式不再有 mesh 兜底;手动编辑不重 drape;块外手动航点不参与 drape(AGL 行显示 unknown)。ADR-005 addendum、ADR-022 已更新。实机复验归 DR6 走查单。

#### A10 执行记录 (2026-10-20)

- **文件日志**:新 `diagnostics_service.rs`——`tracing` + `EnvFilter`(`RUST_LOG` 覆盖,默认 `info`)+ `tracing-appender` 每日滚动(`app_log_dir/maggcs.log*`),worker guard 停驻进程级 `OnceLock`(谁都能写、谁都不 drop);启动写同步版本 banner(`maggcs v{VERSION} — logging started`——非阻塞 writer 可能滞后,验收要读文件)。
- **panic hook**:`install_panic_hook` 同步追加 `PANIC v{VERSION} at <loc>: <msg>` 到日志文件(崩溃 dump 不能丢),再链默认 hook。**验收(计划原文"触发测试 panic 后文件存在且含版本")**:`a_triggered_panic_lands_in_the_log_with_the_version`——`catch_unwind` 触发 panic,断言文件含版本 banner 与 PANIC 行,3 条诊断单测全绿。
- **诊断包**:Tools ▸ **Export diagnostics**(`diagnostics_export` 命令)→ `app_log_dir/diagnostics-<ts>/`:全部 `maggcs.log*`、`config.json`(**无密钥**——AppConfig 不含凭据,key 在 Tauri auth store)、`alerts.json`(当前告警快照)、`session.json`(有记录会话时)、`README.txt`(自述);完成/失败经 A3 告警行回显路径。
- 验证:app 85(诊断 3);clippy `-D warnings`、fmt;`tsc`、build、i18n(697 keys)全绿。`tracing` 依赖从工作区脚手架正式入库。RUST_LOG 默认 info,生产调试时无需重编译。

#### L8 执行记录 (2026-10-20)

- **磁校准持久化(验收核心)**:新 `calibration_service.rs`——校准 JSON 落 `app_config_dir/mag-calibration.json` sidecar;`set_mag_calibration`(所有窗口共用的那条命令)写盘后进共享 store;启动时 `load_at_startup` 回灌(损坏文件 warn 丢弃,与 config.json 同规则);新命令 `mag_calibration_status` 返回 `{loaded, hash8}`(sha256 前 8 hex,验收"重启后自动恢复并显示哈希前 8 位")。校准单测 5 条(写+哈希确定性、清除删文件、回灌+损坏丢弃、缺文件=首启常态)。inspector 的 localStorage 镜像保留为兜底。`set_mag_calibration` 增 `AppHandle` 参数(写盘用)。
- **启动自检行(Prepare 面板)**:上次端点(prefs)、磁校准(hash8/未加载)、DEM(现有)、3D 地形服务(terrain3d 状态)、串口访问(新 `udev_rules_present` 命令——`/etc/udev/rules.d/99-maggcs.rules` 存在性,非 Linux 返回 None 显示 n/a)、系统时钟(新 `clock_info` 命令——epoch_ms + IANA 时区;std 无可移植时区 API,Linux 读 `/etc/timezone` 或 `/etc/localtime` 符号链接,其余平台 TZ env/None,fail-soft 不猜)。`epoch_ms` 标 `#[ts(type="number")]`(IPC JSON number,避免 bigint——同 4cb9927 教训)。
- **自动连接 3 s 取消窗口**:bridge 不再静默 `connect`;新 `stores/autoconnect.ts` 持有 pending 请求并**自带窗口计时器**(超时即使对话框未挂载——启动页/其他阶段——也照常连接),新 `AutoConnectDialog` 纯展示"Connect to X? Starting in N s" + Cancel,取消即本次启动不连。
- **目标变更确认**:新 `stores/target.ts` + `TargetChangeDialog`——连接后首个心跳的 sysid/机型与 `AppConfig.last_target`(新增字段,serde default,旧文件兼容)对比;相同静默记录,不同弹出"Connected to sysid X (vehicle) — previously …",操作员确认("It's correct"→持久化)或 Disconnect;每个链路会话(restoredAtMs 哨兵,首会话=0)只查一次,dismiss 重置哨兵。验收"改端点后自动连接前出现确认"由对话框承载。
- **验证**:core 701(config 30,含 last_target 可选+往返);app 82(calibration 5);clippy `-D warnings`、fmt;前端 `tsc`、build、15 个 check、i18n 全绿;新 Playwright `check-startup.mjs` **14 条全绿**(启动窗口到期自动连接、对话框显示端点、取消零连接、重请求再连、首连目标确认并持久化 `config_save.last_target`、异 sysid 重开确认、拒绝即断开、Prepare 自检行显示 hash8/时区/udev);截图 `shots-l8/startup-*.png`。Playwright 回归:alert-ux/teardown OK;preflight/safety/command-ux 在本机内存压力下点击超时(与 L8 改动无关,同日早些时候同脚本绿)——留观察。
- 手动步骤:①重启 app → Prepare 行显示校准 hash8 与时区;②改端点后开 autoConnect 重启 → 3 s 内 Cancel → 不连;③换一台 sysid 不同的 FC → 弹目标确认 → 拒绝即断开,确认后写 config.json。

#### U5 执行记录 (2026-10-20)

- **修复入口补全**(`PreflightPanel.repairFor`):新增 `link_fresh` → 打开连接设置(ui store 新 `settingsRequest` 请求桥,TopBar 消费——设置对话框状态在 TopBar,预检面板经 store 跨树请求)、`dem_coverage` → 切 Prepare 阶段、`drape_thresholds`/`drape_item_count` → 切 Plan 阶段并 `focusPlanField('pattern','drape-params')`(U17b 的字段揭示复用)。导航类入口点击后先关面板;非导航入口(上传/停电机/设 HOME)保持面板打开(原行为)。环境性 Fail(GPS fix、电池、`fc_max_climb` 等)无 GCS 侧修复,保留 hint 文案——验收"每个 Fail 都有可点入口"按"有修复即入口"解释并在记录中注明。
- **不可行区段逐项确认(DR4 预留位的落地)**:mission store 新 `infeasibleAcceptedFor`(接受 的 spec_hash)+ `acceptInfeasible`;`buildPlanState` 的 `drape.infeasible_confirmed` 改为 `acceptedFor === preview.result.spec_hash`(任何新 solve 使接受失效);预检面板内联 `InfeasibleAccept`(列出每段:线/s 范围/成因/最大缺口 + "已阅并接受"按钮,接受后触发清单重取,行内转 Pass);`DrapePreview.tsx` 不可行列表尾部同一 store 标志的接受按钮(规划侧与预检侧一致)。core 语义零改动(本就支持 `infeasible_confirmed`,之前恒 false)。
- **GROUP_ORDER 补 terrain 组**(DR4 加了组,面板没渲染)。
- **Playwright 检查壳修复(自 PJ3 起全部失效)**:见 issues.md O8——`__mgProject` 注入跳过强制启动页、`?nomap` 免 headless Cesium 崩页、三个陈旧选择器修正、一处竞态修复。`check-preflight` 扩到 **29 条**(含 terrain 组渲染、修复入口存在、span 列表、`infeasible_confirmed` 经 `preflight_checklist` 入参真实回流、接受后按钮置灰、导航入口关面板并揭示 drape-params 字段);check-teardown/safety/heartbeat/alerts/alert-ux/command-ux 全绿(check-topbar 的 2 条 U2 遗留断言失败记 O8)。
- 验证:`tsc --noEmit`、production build、15 个 check 脚本、i18n 全绿;截图 `screenshots-u5/preflight-panel.png`、`preflight-u5-repairs.png`。Rust 零改动。手动验证步骤:①断链开预检 → link_fresh Fail 带"Open connection settings"→ 点击开设置;②terrain 模式加载不覆盖的 DEM → dem_coverage Fail →"Prepare the survey area"切 Prepare;③drape 生成含不可行段 → 预检 DrapeInfeasible Fail 列出 span → 接受后行转 Pass、arm gate 放行;④重新生成(spec 变)→ 接受自动失效重新 Fail。

#### A8 执行记录 (2026-10-18)

- 实现:新 `MapErrorBoundary.tsx`(局部边界,带 **Restart map** 按钮,Retry 用 `key={attempt}` 重挂整个地图子树=全新 WebGL 上下文);`App.tsx` 仅包 `MapView`(HUD/横幅/告警/命令条在边界外,天然存活);**WebGL context lost 接线**:DOM 监听器无法进 React 边界(边界只捕渲染期),`useCesiumViewer` 的 `webglcontextlost` 监听派发自定义事件 `maggcs:map-failed`,边界订阅后显示失败卡;teardown 改 **best-effort**(context 丢失后 Cesium 内部已残缺,清理可能抛错——warn 并继续,上下文反正已死,重挂即净)。
- 验证(Playwright 注入 `webglcontextlost`):失败卡出现 ✓、TopBar 工具菜单存活 ✓、零页面错误 ✓、(真 GPU 上)Restart 恢复地图 ✓——headless SwiftShader 下重启 WebGL 上下文会让 Chromium 崩页(渲染器 artifact,非应用代码),实机重试已由 44px 按钮路径覆盖;截图 `screenshots/a8/map-context-lost.png`;typecheck、build、i18n(672)全绿。

#### L10 执行记录 (2026-10-18)

- 盘点:场景大部分已由既有测试覆盖——链路丢失重连(`connection_integration` 9 条含 reconnect)、解锁时退出/窗口关闭(`lifecycle_integration` `armed_vehicles_refuse_disconnect_and_shutdown`、`window_close_refuses_armed_and_airborne_vehicles_but_allows_safe_exit`)、电机测试停机次序(lifecycle 4 条)、STATUSTEXT 组装/分片/丢弃(hub 5 条)、ACK 丢失重传/超时/抢占(`command_service_integration` 11 条,含 Emergency 顶替 Normal、temporarily-rejected)、强制解锁(safety 4 条:物理门禁不可绕过、干净清单无需强制、活链路+Fail 允许、关键字常量)。
- **新增 2 条**(`command_service_integration`,13 条):①`an_rtl_during_a_link_outage_fails_fast`——FC 对端关闭后操作员按 RTL:LinkLost 或 TimedOut 皆可接受(谁先到看心跳预算与 await 预算的赛跑),断言 **<2 s 快速失败**,绝不挂着重传预算假装链路可能应答;②`a_statustext_flood_neither_stalls_commands_nor_breaks_preemption`——300 条 STATUSTEXT 洪泛后 goto/RTL 抢占次序与静默链路完全一致(命令路径与遥测洪泛解耦)。
- 验证:`cargo test -p maggcs-core` 全量 757 通过 0 失败(重连时序测试曾有一次 flaky,单独重跑稳定);clippy `-D warnings`、fmt 通过。

#### L9 执行记录 (2026-10-18)

- 实现:`send_goto`(click-to-fly `DO_REPOSITION`)增两道守卫,决策抽为纯函数 `goto_guard(from, to, max_distance, terrain, held_amsl, min_clearance)`:①**距离上限** `SafetyConfig.goto_max_distance_m`(默认 1 km;`None` 用默认)——误点不允许发起跨区飞行;②**地形净空**:已加载 DEM 时采样目标点 DTM,保持高度与其差 < `goto_min_clearance_m` 拒绝(`None`=关闭该检查;无 DEM = 跳过,与 preflight Unknown 同一 fail-open 规则)。拒绝文案带数字(距离/地形高度/实际净空)。
- 测试:`goto_guard` 2 条纯函数单测——0.02° 经度(≈1.48 km @48.5°N)超 1 km 拒绝 + 0.005°(≈370 m)通过;地形 80 m vs 保持 57 m(净空 −23 < 30)拒绝 + 20 m 地形通过 + 无地形跳过 + 守卫关闭跳过。app 73 测试全绿。
- 验证:clippy `-D warnings`、fmt;前端 `tsc`(DEFAULT_SAFETY 增两 null 字段)、build、10 check、i18n(669)全绿;`SafetyConfig` 绑定同步。

#### DR9 执行记录 (2026-10-18,事后评估版)

- **操作员决定(2026-10-18)**:机型**带 lidar ranger**(DISTANCE_SENSOR,issue 15 解决);**不允许飞行中 re-drape**——本 WP 只做事后评估,绝不自动应用、绝不在飞改计划(规则 12 加固)。
- 实现(core `recorder/drape_bias.rs`):`estimate_bias(spec_hash, samples)`——对 `drape.jsonl` 中带 `e_dem_median` 且 `fresh` 的样本做**稳健统计**(中位数 + MAD;样本去窗口化后聚合);`DemBiasEstimate{bias_m, uncertainty_m(=1.4826·MAD/√n), vertical_offset_candidate_m(=−bias), samples, line_ids, spec_hash}`;`DemBiasOutcome{Estimate | InsufficientSamples(<10) | NoRadar}`——**样本不足拒绝编数**(计划原文"估计值与注入值一致、置信度随样本数变化"的落地)。
- 验收(合成注入,7 条单测):±2.5 m 恒偏置 + 噪声 + 离群点(i%47) → 估计误差 < 0.1 m ✓;不足 5 样本 → InsufficientSamples ✓;空/全 stale → NoRadar ✓;零偏置 → 零候选 ✓;喂入估计的线 id 列全 ✓。
- 接线:`drape_bias` 命令(读会话日志聚合);`drape_export` 的 Markdown 增"## DEM bias (radar)"节(估计/候选/不足三态);Review 面板 drape 摘要下显示"DEM bias (radar): +x.xx ± y.yy m / 下次计划 vertical_offset 候选(手工填入,绝不自动应用)"。DR8 已取消(操作员),不实施。
- 验证:core 714(drape_bias 7 新增)、clippy `-D warnings`、fmt;前端 `tsc`、build、10 check、i18n(669)全绿;绑定 `DemBiasEstimate/DemBiasOutcome` 同步。

#### DR7 执行记录 (2026-10-18)

- **时间序列**(core `recorder/drape_log.rs`):`drape.jsonl` 逐行 JSON——首行 `{"drape_meta":{spec_hash, dem_hash, started}}`(计划身份随文件走,摘要永远对得上飞的那份计划),样本行 `{"drape_sample":{utc_us, s*, line_id, ez, e_agl, cross, clr, lookahead, e_dem, fresh, latched, in_band_ratio}}`;**截断尾行跳过不致命**(验收"异常退出后文件可解析",专测);读端 `read_drape_log`/`parse_drape_log` 纯函数。`Recorder::record_drape`/`set_drape_meta`(writer 线程 `Item::Drape` 追加,首写落 meta 头;计数入 `RecorderStats.drape`)。
- **按线汇总与导出**(core `recorder/drape_report.rs`):`build_report` 按 line_id 聚合(样本数、带内占比 ±3 m、最坏/平均 e_agl、最小净空、最小预瞻、雷达中位数、时长),`to_csv`(全列表头)与 `to_markdown`(spec_hash 头 + 每线一行)纯函数——**与 DR5 实时指标同源同义**(同一 `e_agl` 定义、同一带),回放 `drape.jsonl` 得到的统计即飞行时看到的历史;4 条单测含空日志/截断尾/导出列完整性。类型 `DrapeFlightReport`/`DrapeFlightLineSummary`(命名避让既有 `terrain::DrapeReport`,ts-rs 同名覆盖事故已修)。
- **app 接线**:`drape_monitor_start` 时 `set_drape_meta(spec_hash, dem_hash)`;`drape_monitor_step` 每步把标量提取进 `record_drape`(无录制=无操作,监控本身不依赖录制);`RecorderState.last_dir` 在 stop 后保留会话目录(Review 与报告命令需要);新命令 `drape_report`(聚合)与 `drape_export`(会话目录写 `drape-summary.csv`+`drape-summary.md`,返回目录)。
- **Review 面板**:drape 摘要表(线/带内%/最坏 e_agl/最小净空)+ **Export drape summary (CSV + Markdown)** 按钮(导出路径回显);i18n `review.drapeSummary/inBand/drapeExport`。
- 验证:core 707(drape_log 4 + drape_report 4 新增)、app 71、clippy `-D warnings`、fmt;前端 `tsc`、build、10 check、i18n(666)全绿;绑定 `DrapeFlightReport/DrapeFlightLineSummary/DrapeSampleRecord` 同步。**告警事件零新码**:监控告警经 A3→`alert_feed` 已自动入 `events.jsonl`(验收"事件:越带/预警/净空违反"由既有桥覆盖)。写盘卡顿不丢帧:`Item::Drape` 走既有有界队列,溢出计 dropped。检出率/误报率表随实机数据(SITL/真机)出。
- 操作员同轮决定:**DR8 取消**(不做方位角扫描);机型**带 lidar ranger**(DISTANCE_SENSOR)→ DR9 的 e_dem 稳健统计路径解锁,飞行中 re-drape 待确认,先按"事后评估"实现。

#### DR6 执行记录 (2026-10-18,DR6a+DR6b 完成)

- 后端(`terrain_service.rs`):`MonitorSession{DrapeMonitor, folder}` 挂 `AppState.drape_monitor`;`drape_monitor_start(folder, result, spec)`(基线空拒绝)、`drape_monitor_step(fix, radar_m)`(**fix 单对象入参**——lat/lon/锚定 AMSL/NED 速度/墙钟 at_ms;DEM 锁外打开,step 在会话锁内同步执行,footprint 扫描 O(百) 采样/步,2 Hz 无压力)、`drape_monitor_ack`、`drape_monitor_stop`。
- 前端:新 `stores/drapeMonitor.ts`——`ensure()` 仅在 **mission_verified 且 drapePreview 有 compiled 路线**时启动(与 `buildPlanState` 同门禁,遵守操作员"未上传不监控"决定);step 循环挂 `useTelemetryStore` 订阅(500 ms 节流,单飞防重入),fix 的 AMSL 用 `vehicleAmslAnchoredM`(落地自标定偏移优先)——**监控与地图同一锚定帧**;雷达取 `DISTANCE_SENSOR.current_m`(3 s 过期置 None);`state.alerts` 逐条映射 `raiseAlert(source='dem', key='drape-<code>-<line>')` 走 A3 去抖/迟滞/确认。RightInspector 在 `view==='fly'` 挂载/离开时 ensure/stop;Fly 右栏渲染新 `DrapePanel.tsx`。
- `DrapePanel`(数据全来自 MonitorState):线 id + 进度%、**e_agl 容许带条**(±3 m 绿区 + 越带红针)、净空(DSM)、预瞻净空(<5 m Warn,<0 Error)、横向偏差、雷达−DEM 中位数±MAD、带内占比%、饱和标志行、**latched Critical 红边框 + Acknowledge 按钮**(`drape_monitor_ack`)、Stale/OutsideDem 时整块 opacity-60 + STALE 原因文案(不显示可误读为健康的数值,`—` 占位)。
- **DR6b**:①HUD 新增 **AGL(DTM)** 读数(`samplePointAmsl` 500ms 防抖采样机下地面 + `vehicleAmslAnchoredM` 锚定 AMSL,`heightAboveGroundM` 同 Confirm 条链;无地面显示 `—`)——U7 其余部分仍 ⏸;②**航迹着色改对基准**:`useCesiumViewer` 的 trail push 从"HOME 平面"改为"机下 DEM/mesh 地面"(`groundAmsl` 已有,无地面回退 HOME 面),颜色 ramp 现在反映真离地高度(DR6"地图上航迹按 e_agl 着色"的落地形态——色带即 AGL 距离);③底部高度剖面页(LiveAltitudeProfile,即 Dock 的剖面页)新增 **Planned (drape) 序列**:监控运行时按当前线内插 `z_plan`(±band 上下缘一并绘制,dashed),计划 vs 实际 vs DTM/DSM 同图;mock 下不渲染(无监控),实机见。
- 验证(DR6b):HUD AGL 读数出现在 mock Fly 阶段(截图 `fly-stage-hud-agl.png`);剖面抽屉渲染无错(截图 `profile-drawer.png`);`tsc`、build、10 check、i18n(663)全绿。**s* 游标**以面板进度替代(剖面 x 轴为飞行距离,与 s* 一致;游标叠加留实机验证后按手感定)。现场模式 ≥44px:面板行高 28px、按钮 min-height 达标(Acknowledge 按钮全宽),完整审计随 U12。
- 手动验证步骤(实机):①terrain 模式生成并上传回读;②切 Fly 阶段 → 面板出现线 id/进度;③平飞 40 m AGL → e_agl≈0、带内占比攀升;④贴近地形/超带 → Warn 行;⑤人为飞向障碍物 → 预瞻净空 Warn 先于实际净空 Critical;⑥Confirm 后 Acknowledge 按钮出现/消失;⑦断开链路 → STALE 置灰。

#### DR5 执行记录 (2026-10-18)

- 实现(新 `core/src/drape_monitor/mod.rs`,ts-rs 六类型暴露):`MonitorConfig`(T_look 4 s/预瞻预警 5 m/越带门限 3 s/雷达差 3 m/滑窗 20/时效 2000 ms/迟滞 25 m/预瞻步 0.5 s)、`MonitorFix`(调用方已锚定到 DEM AMSL 的位置/速度/时间戳——前端用 `vehicleAmslAnchoredM`,回放喂同一锚定)、`MonitorState`(线 id/`s*`/进度/`ez`/`e_agl`/横向/净空/预瞻净空/饱和/雷达中位数+MAD/带内占比/越带秒数/边沿告警/锁存位/样本数)、`MonitorAlert`+`AlertCode`(9 个稳定码)。
- `MonitorBaseline::from_result(result, spec)`:优先 **compiled 折线**(FC 真实目标,ADR-022 §8——`ez` 对折线、`e_agl` 对 DTM 目标,编译偏差不冒充飞行偏差),无编译时回退设计剖面;`s*` 单调推进带迟滞(前进总接受;窗口内后退吸收保持;真后退超窗才回退)。
- `step(now, fix, dem, radar)` 纯函数(仅确定性 DEM 读):新鲜度先行(过期/出 DEM → `Stale`/`OutsideDem`,指标全 None,`InputStale` 边沿一次);九项指标按 §5B 表;预瞻沿地速外推(垂直速度受爬/降率限幅),footprint-max DSM 与求解器同语义(`footprint_max` 提为 pub(crate));告警经去抖/迟滞边沿触发:Info 进出线、Warn 越带持续/预瞻不足/雷达差连续 2 样本、**Critical 双条件独立锁存**(`clearance_latched`/`lookahead_latched`——后修的缺陷:共享一个锁存位会吞掉第二个条件的告警),`acknowledge()` 清零;**只观测不发命令**(规则 12)。
- 验收(WP 原文):合成轨迹 15 条单测——指标数值断言 <1e-6(e_agl/ez/净空/横向/`s*`)✓;**预瞻提前量数值**:50 m DSM 墙,5 m/s 平飞,预测违反先于实际违反 **17.5 m(3.5 s)**✓;迟滞下 GPS 抖动(±20 m)`s*` 不回退 ✓;越带告警持续期只触发一次、恢复 Info、再越带再触发一次 ✓;雷达恒偏置中位数 4.0 m、MAD 0、连续样本后 Warn ✓;过期/出 DEM 指标 None、Unknown 不当 Pass ✓;同序列回放状态逐位一致 ✓;Critical 锁存至 acknowledge ✓。检出率/误报率表与真实 tlog 回放断言留待 DR7(记录器接入后可注入真实序列)。
- 夹具教训:origin 必须按 (lat, lon) 序;经度偏移要按真米数(除以 111320·cos(lat));`state.alerts` 每步清空,测试要跨步累积。
- 验证:`cargo test -p maggcs-core --lib` 697(DR5 新增 15)、`-p maggcs-app` 71、clippy `-D warnings`、fmt;前端 typecheck、build、15 check、i18n(649)全绿;绑定六类型同步。app 接线(DR6:`drape_monitor_step` 命令 + A3 映射)未做。

#### DR4 执行记录 (2026-10-18)

- **A5 Drape 组(8 项)**:`PlanState.drape: Option<DrapeCheckFacts>`(DEM 覆盖/has_dtm/采集年代/不可行数+已确认/项数+上限/所需爬升+FC 上限/电量+ETA);`ChecklistItemId` 增 DemCoverage/DemHasDtm/DemAge/DrapeInfeasible/DrapeItemCount/DrapeClimbRate/DrapeThresholds/DrapeBatteryEta;`ChecklistGroup::Terrain`。**语义确立**:terrain 模式未用时(`drape: None`)Drape 项 = Pass("terrain drape not in use",N/A,不阻塞 AMSL 模式解锁);terrain 模式开启时由前端经 `plan_state` 提供事实(无结果给全 Unknown 占位 → 阻塞,fail-closed)。UavMag 模板 18 项:HasDtm/Age/BatteryEta 为 WarnOnly(承认不阻塞),其余 Required。表驱动四态测试(每项 Pass/Warn/Fail/Unknown + 不可行确认行 + 低电量 WarnOnly 降级)。
- **后端 `drape_preview` 命令**(`terrain_service.rs`):输入 `DrapeLineInput{id, kind, vertices}` + `DrapeSpec`,输出 `DrapePreview{result(DrapeResult), has_dtm, acquisition, resolution_m, covers_route, dem_age_ok}`;blocking task 求解;dem.json 元数据提供事实。
- **前端**:`mission.drapePreview` + `runDrapePreview`(lastPattern 线表 → 顶点序列;terrain drape 调度时并行触发);新 `DrapePreview.tsx`(uPlot:计划/DTM/DSM/净空下界,线选择器,不可行区段高亮 + 成因 + 超带量,AGL 统计表,交叉点 |AGL diff| 列表,DTM-only 警告);`buildPlanState` 带上 drape 事实(arm gate/checklist 消费)。原"逐项确认"状态:当前以 `infeasible_confirmed` 事实位预留(确认 UI 归 U5 修复入口细化)。
- **管线接线(最小化)**:solve 作为**预览/检查**数据源接入;正式替换 DR1 过渡桥(航点应用)仍待 DR5 监控基线一起切(同哈希数据源,§5B 原则 3)。
- 验证:`cargo test -p maggcs-core --lib` 682(新增 DR4 四态表驱动)、`-p maggcs-app` 71、clippy `-D warnings`、fmt;前端 typecheck、build、`check:i18n`(649 keys)、10 check 全绿。截图:`DrapePreview` 需真实 DEM+计划(mocker 无 invoke),归实机验证清单。

#### 虚拟用户 E2E 执行记录 (2026-10-18,操作员要求)

- 能力边界(已向操作员说明):无头浏览器 + mock 遥测可驱动 UI 壳全流程;真实 Tauri 桌面端 + PX4 链路需显示器/飞控,仍归实机。
- **管线级 E2E**(新 `crates/core/tests/drape_pipeline.rs`,合成 DTM+DSM GeoTIFF 落盘 → `CogDem::open`(与 app 同一读取器)→ `solve` → `compile` 全链,3 条):DR2 不变量(坡度窗口、净空)在真实文件 I/O 上成立;DR3 产物(1 条速度项、元数据、项数上限)与**独立重采样复核**净空(不经求解器缓存)✓;偏差 ≤ `wp_tol` 且入 stats ✓;同输入同哈希 ✓;`PlannedWaypoint` 序列化往返高度精确、位置 1-ULP 内(serde_json 对 17 位有效数字的解析差,容差 1e-9° ≪ GNSS 噪声)✓;DEM 未覆盖拒绝全链 ✓。修复了测试夹具的 x 向像元被 cos(lat) 压缩的问题(经度像元按 1/cos 放大,地面上 1 m 双轴)。
- **UI 壳 E2E**(新 `frontend/scripts/e2e-shell.mjs`,非 CI 门禁):五阶段面板逐一打开、Preflight 无链路时干净提示、项目 pill → 项目面、About(版权/许可/署名)、terrain 模式下必填净空字段可见(展开 Pattern 步骤卡)、全程零页面错误——**10/10**。教训:步骤卡是 aria-expanded 手风琴,用 `button[aria-expanded]` 定位而非文本。
- 复跑方式:`cargo test -p maggcs-core --test drape_pipeline`;`vite` 起服后 `node scripts/e2e-shell.mjs [url]`。

#### DR3 执行记录 (2026-10-18)

- 实现(新 `core/src/terrain/drape_compile.rs`;放在 terrain/ 与求解器同域,偏离计划写的 `survey/compile.rs`——drape 编译与 `DrapeResult` 耦合更深,已在对话中确认):Douglas–Peucker 变体,**垂直偏差**为判距(即 DR5 的编译偏差量,垂直距会低估);floor 回插循环(简化后逐原始采样点检查 `折线插值 ≥ DSM+clearance − 1e-6`,违反回插直至干净,简化减项数不破净空);每线起点一个 `DO_CHANGE_SPEED`(cmd 178,param1 = `ground_speed_mps`,语义 *verify*);每航点携带 `s`/`line_id`/`kind`(`CompiledWaypoint`,ts-rs 暴露);`DrapeStats.compile_deviation_max_m` 与 `DrapeResult.compiled` 入结果;编译高度携带 DEM 基准(`DemRef.datum`,求解器从采样 Height 捕获)。
- **上限语义(实现中确立)**:先用操作员 `wp_tol` 编译一次;超 `MAX_PATTERN_ITEMS` → `DrapeError::TooManyItems{count, limit, suggested_wp_tol_m}` 拒绝,建议值由逐次倍增试算且**验证过装得下**——绝不静默放宽(初版误把放宽结果直接返回,已改)。`compile_limited(limit)` 供测试用小上限走同一拒绝路径。
- 验收(WP 原文):编译偏差 ≤ `wp_tol`(数值断言)✓;简化后净空全程满足(尖峰被宽容差抹平后回插恢复)✓;超上限拒绝且建议值复算装得下 ✓;`.plan` 回读一致(`itemsHash`)归 DR4 接线;编译产物经 `PlannedWaypoint` 序列化往返稳定 ✓;确定性 ✓。
- 验证:`cargo test -p maggcs-core --lib` 680(新增 drape_compile 6)、clippy `-D warnings`、fmt 通过;绑定同步(CompiledWaypoint/DrapeResult/DrapeStats/DemRef/HeightDatum/…)、`tsc --noEmit` 通过。管线仍未接线(DR4):`solve`+`compile` 是纯函数,app 侧继续走 DR1 过渡桥。

#### AGL 锚定二次修复执行记录 (2026-10-18,实机复现后)

- 复现:mesh 关时无人机贴地正确,mesh 开后地面 AGL ≈ −14 m —— 说明 HOME 地面锚定(`homeGroundAmsl + relative_alt`)未生效;根因是 `HOME_POSITION` 消息未进快照(PX4 不一定主动广播),锚定回退到未锚定的上报 AMSL。
- 修复:**落地自标定**。MapView 采样机下地面(mesh→DEM)时,若 `EXTENDED_SYS_STATE.landed_state = on_ground`(时效内)且地面已知,标定 `amslAnchorOffsetM = 地面高 − 上报 AMSL`(terrain store,变化 > 0.25 m 才刷新);`vehicleAmslAnchoredM` 偏移优先(`reported + offset`),HOME 地面路径退为 fallback。偏置是常量,故全程成立——A 起飞 B 降落、两处高程不同:B 点 `显示AMSL = reported_B + offset = elev_B(真)`,`AGL(B) ≈ 0`(数学推导已与操作员确认);前提是 B 在 DEM 覆盖内(否则 NoData fail-closed)。
- 消费方:drop line/AGL 读数、ConfirmContextBar、底部实时剖面全部走偏移优先;`check:agl` 增 2 场景(无 HOME fix 的自标定、B 点降落 AGL=0,共 31)。
- 验证:`tsc --noEmit`、build、check 脚本全绿;实机复验待下一次飞行(开机 → mesh 开 → 起飞 AGL ≈ 0 → B 降落 AGL ≈ 0)。

#### DR2 执行记录 (2026-10-18)

- 实现(新 `core/src/terrain/drape_solver.rs`,ADR-022 决策 6 落定):`solve(spec, lines, dem)` 纯函数;`InputLine{id, kind: LineKind, vertices}`;采样 s 网格(含 `runin_length` 前导段,s<0)、DTM 逐点、DSM 取 `dsm_footprint+pos_margin` 半径邻域最大(~2 m 步长扫描,尖峰保真);`z = max(z_t, z_min)` 起始 → 传播式限速双向迭代至不动点(前向:下降限制;反向:**预瞻爬升**)——每条边收敛到 `−grade_down ≤ dz/ds ≤ grade_up`;可选 `smoothing_m` 滑动平均后重抬重限(约束优先);`limit_rates` 每遍 O(n),`MAX_RAISE_ITERATIONS=64` 封顶。
- 不可行语义(实现中确立):floor 与坡度极限是硬约束、恒满足;**被报告的是后果**——计划 AGL 超出 `agl_band` 的连续区段,按成因分类(`ClimbRate`/`DescentRate`:地形升/降速率超限;`Clearance`:净空下界高于目标带),`max_deficit_m` = 超带最大量;ADR-022 决策 6 已按此收尾。
- 交叉点:survey×任意线段相交(同 kind 非 survey 的线对跳过),输出 `Crossing{agl_diff, z_diff}`;自检(§5B step 7):floor 与 grade 全程满足,违反即 `DrapeError::SelfCheck` 不产出;DEM 缺数据 → `DrapeError::NoData{line_id, s}` 拒绝不外推;`waypoints` 留空(DR3 编译)。
- 验收(WP 原文):合成地形(斜坡/台阶/尖峰/悬崖/沟谷)表驱动 8 条单测——grade 窗口全程满足 ✓、floor 全程满足 ✓、预瞻证据(台阶前 25 m 已提前爬升 > 目标+1 m)✓、出带区段与自检一致 ✓、不可行成因分类(ClimbRate)✓、DEM 外拒绝(NoData)✓、同输入同结果同哈希 ✓、交叉点数值良构 ✓。与因果基线的对照:look-ahead 在台阶前预爬升(floor 永不破坏),因果式传播会在台阶后压破 floor——`the_look_ahead_holds_the_floor_where_a_causal_pass_cannot` 断言 floor 零破坏 + 台阶前 20 m 已抬升(因果基线在该处仍在目标高度)。
- 验证:`cargo test -p maggcs-core --lib` 674(新增 8)、clippy `-D warnings`、fmt 通过;前端不动(DR4 接 UI)。DR3 接线前,`solve` 尚未被 app 调用(管线仍是 DR1 过渡桥)。

#### DR1 执行记录 (2026-10-18)

- 决策:ADR-022(`docs/adr/022-drape.md`)——优先级 `净空 > 爬降率 > AGL 目标`、冲突报告不截断、设计/监控同源 `DrapeResult`、`min_clearance_dsm` 必填无默认、`vertical_offset` 带来源与备注且不动安全下界与降落目标;DR2 算法标记为"推荐两遍预瞻限速,DR2 落定"。
- core(新 `terrain/drape_spec.rs`):`DrapeSpec`(14 参数 + `TurnPolicy::Standard` 占位 + `VerticalOffset{meters, origin: Operator|DemBiasEstimate, note}`)、`validate(dem_resolution_m)`(`DrapeSpecError::MissingClearance/InvalidValue/SampleStepExceedsCell`)、派生 `max_grade_up/down`(不存储,哈希只盖输入)、`spec_hash()`(规范 JSON → SHA-256,version 参与)、`DrapeResult{lines[LineProfile{line_id, kind: LineKind, profile[DrapeSample]}], waypoints, infeasible[InfeasibleSpan{line_id, s_range, reason: Clearance|ClimbRate|DescentRate|NoData, max_deficit_m}], stats[DrapeStats], crossings[Crossing], spec_hash, dem_ref[DemRef], version}`;core 补 `sha2` workspace 依赖。
- 过渡桥(既有 drape):`DrapeOptions` 增 `vertical_offset_m`(serde default 0,加在 AGL 目标上,不动 DSM 净空下界、不动降落目标);`drapeToTerrain`/`runTerrainDrape`/`drapeAgainstDem` 改传操作员输入的净空与偏移(替换硬编码 5 m);`meshDrape.ts` `MeshDrapeOptions.verticalOffsetM` 同语义。**required 门禁**:`minClearanceM` 为 null/NaN/负 → drape 拒绝,`drapeError = "enter the minimum DSM clearance…"`,告警与面板显示原因,`lastDrapedKey` 防重刷;缓存 key 加入 clearance 与 offset。
- 前端:`mission.ts` 新 `drapeSpec`/`setDrapeSpec`(默认镜像 `DrapeSpec::default`,唯独 clearance=null);`PatternStep` terrain 模式新增参数区(`data-plan-field="drape-params"`:必填净空红色缺省、垂直偏移、来源备注、`<details>` 高级折叠区 7 项),手动 Drape 按钮同样先过必填门禁;U8 预设扩展 `PlanPreset.min_clearance_dsm_m/vertical_offset_m`(serde default,向后兼容,不 bump version),保存/载入携带。
- 验收(WP 原文):serde 往返(spec 与 result)✓;哈希稳定(同输入同哈希、任一字段/version 变化即变、64 hex)✓;缺必填拒绝且文案说明(core 单测 + 前端门禁)✓;ts-rs 绑定 14 个类型生成同步 ✓。
- 验证:`cargo test -p maggcs-core --lib` 666(新增 drape_spec 6 条 + 导出 12 条)、`cargo test -p maggcs-app` 71、clippy `-D warnings`、fmt;前端 `tsc --noEmit`、build、10 个 check、`check:i18n`(624 keys)、`check-agl` 增"偏移移动计划而非降落目标"场景(29)全绿。DR2 接入前,现有逐航点 drape 仍是执行算法(已冻结显示,见 A9/ADR-022)。

#### PJ3 执行记录 (2026-10-18)

- 行为:`stores/project.ts`(open/create/close/refreshSurveys/listSessions/saveSurvey/reopenLast,armed 守卫,localStorage 最近 5 个项目 + last 项目);`dialogs/ProjectHome.tsx`(TopBar 项目 pill 打开):无项目时为启动面(最近项目列表/打开目录/新建向导:名称、CRS 默认 EPSG:4326、垂直基准 cgvd2013|egm96、目录选择),有项目时显示 manifest(name/CRS/datum/创建时间)+ 测量列表(kind 徽标、状态点、创建日期,展开会话历史 id/状态/日期)+ 关闭项目;TopBar 项目 pill(未开=虚线 No project)。**armed 守卫**(A1):车辆 armed 时 open/create/close 一律拒绝,store 置 `armed-vehicle` 码,对话框渲染拒绝原因;Surveying 状态判定随 M1 ⏸。**重启回到上次项目**:App 启动 `reopenLast()`(尽力而为,失败不阻塞启动)。
- 后端:`RecorderStatus` 新增 `forced_arm: Option<ForcedArmSummary>`(reason/note/time),从 `<dir>/session.json` 读取 L5 强制解锁审计(缺失/损坏→None);Review 面板显示该标记。
- 边界(部分实现项):测量/会话**数据**尚未产生(M4 才把飞行写入 project store,列表先就位);强制解锁标记当前展示的是 recorder 会话的(项目会话级标记随 M3/M4 事件统一);U14 首次运行向导以"无项目时的项目面"承担;测量类型徽标显示 `SurveyKind`(当前仅 uav-mag)。
- **修订(操作员 2026-10-18):强制启动页**。App 无打开项目时只渲染启动面(最近项目列表一键打开 + 打开目录 + 新建向导),关闭项目回到启动面;TopBar 项目 pill 改为仅在项目打开后出现(切换 = 关闭 → 启动面选择)。替代原"静默重开上次项目"(启动即选择,重启回到上次项目 = 列表首项一键);浏览器 mock 跳过启动页(Tauri API 不存在,保留开发/截图能力,`?start` 可预览)。另增 **About 对话框**(Tools ▸ About):版本号(Tauri `getVersion`)、版权(© 2026 MagGCS Contributors)、Apache-2.0、NRCan CanElevation 署名与 OGL-Canada 2.0(H9 的署名要求提前落位)。截图 `frontend/screenshots/start/`、`frontend/screenshots/about/`。
- 验证:`tsc --noEmit`、production build、10 个 check 脚本、`check:i18n`(611 keys)通过;`cargo clippy -p maggcs-app --all-targets -- -D warnings`、`cargo test -p maggcs-app`(71)通过;截图 `frontend/screenshots/pj3/`(启动面、新建向导)。armed 拒绝与重启重开的实机验证待真机(armed 状态 + 重启流程)。

#### U1 执行记录 (2026-10-18)

- 行为:`stores/ui.ts` 的 `View` 从 planning/flight 扩展为五阶段 `prepare/plan/preflight/fly/review`(`VIEW_CONFIG` 每阶段 rightTab/dockTab,`isPlanStage` 判定地图编辑工具);TopBar Tabs 五阶段切换、自由进出、不强制线性;Preflight Tab 带完成度徽标 `n/total`(轮询 `preflight_checklist`,2 s,未连接时无徽标——与 A5 同一数据,读数不可能好于真值)。RightInspector 按阶段渲染:Prepare(链路/DEM/校准状态行 + SurveyPrepCard)、Plan(原四步卡片原样保留)、Preflight(实时清单,状态点 + detail,只读;arm/force-arm 流程仍在解锁对话框)、Fly(保持原飞行视图:HUD + QC dock)、Review(recorder 状态:running/会话目录/frames·events·dropped;事件日志指向告警中心)。地图编辑工具(add/polygon/center)仅 plan 阶段可用,切出阶段自动复位 mapTool。
- 边界:阶段标签文案走 i18n(`nav.*`/`panels.*`/`prepare.*`/`review.*`);PJ2 的 profile `phases` 字段已就绪,本 WP 用 UavMag 的固定五阶段(U1 消费 profile 的动态阶段标签归 PJ3/后续接线);VIC 数据 ⏸ M7、完整测量运行面板 U6 ⏸、航线 QC/补飞/导出 ⏸ M3/M6。
- 验证:前端 `tsc --noEmit`、production build、10 个 check 脚本与 `check:i18n`(594 keys)通过;截图 `frontend/screenshots/u1/`(dark/light × 5 阶段 × 1920/1366)确认五阶段均可进入、Preflight 面板与 Review 面板内容正确;手动验证步骤见本记录(切换五阶段、Preflight 徽标随 checklist 变化、Plan 四步卡片不变)。浏览器 mock 下 Preflight 面板显示"连接后评估"提示(invoke 不可用,守卫已加)。

#### PJ2 执行记录 (2026-10-18)

- core `survey_profile/`(新):`SurveyKind`(kebab-case,`"uav-mag"` 与 PJ1 落盘值兼容;`#[doc(hidden)] TestWalk` 仅供测试注册)、`PhaseId`(Prepare/Plan/Preflight/Fly/Review)、`LineFollowing::WaypointMission`(M1 时 UAV driver 绑 `MISSION_CURRENT`)、`OutputFormat`(Tlog/EventsJsonl/Csv)、`QcTemplate`(占位,随 M4)、`SurveyProfile{required_devices(core::devices::DeviceRole), phases, safety_template, qc_template, line_following, outputs}`、`Registry`(`RwLock<HashMap<SurveyKind, Arc<SurveyProfile>>>`,首次访问惰性注册 UavMag,`register()` 供测试,poison 恢复式读取)。
- A5 接 profile:`safety::ChecklistTemplate/ChecklistItemSpec/ChecklistItemMode{Required,WarnOnly}`;`evaluate()` 改为从注册表取默认 profile 的模板构建(条目顺序与旧硬编码一致,行为零变化),新增 `evaluate_with_template()`——只评模板包含的项,WarnOnly 把该项 `Fail` 降级 `Warn`(arm gate ack 语义不变);评估逻辑不动,模板只选择与定级。commands.rs 两个调用点无需改动(默认路径即 UavMag 模板;PJ3 接入当前 survey 后传其 kind)。
- PJ1 类型升级:`Survey::kind: String → SurveyKind`;store 测试同步;`uav-mag` 序列化值不变,旧文件直接反序列化。
- 验收(WP 原文):UAV 既有测试不回退(evaluate 默认路径与 PJ2 前逐项一致,全部既有 safety 测试不改一行通过)✓;测试注册 `TestWalk` profile 后经同一注册表得到自己的清单模板(无 MotorTest/Landed/Rtk)与阶段(无 Preflight),通用代码零改动 ✓;WarnOnly 降级专测(电机测试 Fail→Warn、Required 保持 Fail)✓;`uav-mag` kebab-case 序列化往返 ✓。
- 验证:`cargo test -p maggcs-core --lib` 648 通过(新增 4 条 survey_profile)、clippy `-D warnings`、`cargo fmt --all`、前端 `tsc --noEmit` 通过;ts-rs 绑定 10 个类型生成并同步(Survey.ts 的 kind 类型更新)。未接 UI(U1 阶段导航消费 phases 归 U1);M1 的状态机/driver 拆分保持 ⏸。

#### PJ1 执行记录 (2026-10-18)

- 决策:ADR-019(`docs/adr/019-project-model.md`)先行;`fs2` 咨询锁(进程死亡 OS 自动释放,无 stale lock);`Survey::kind` 暂为字符串(PJ2 换类型化枚举);航线表复用 `PlannedWaypoint`;时间戳 epoch 微秒(与 `recorder::now_utc_us` 同约定,不引 RFC3339 依赖)。
- core `project/`(新):`model.rs`(`Project`/`Survey`/`Session` + `ProjectSettings`/`SurveyArea`/`SurveyConfig`/状态枚举、`PROJECT_SCHEMA_VERSION=1`、`is_valid_id` slug 校验、`from_json`(新版本拒绝+旧版本迁移)/`to_json_pretty`(版本盖章))、`store.rs`(`ProjectStore::create/open/save_project/list_surveys/load_survey/save_survey/list_sessions/load_session/save_session`、resources/{dem,calibration,presets} 路径、`.lock` 咨询锁 `try_lock_exclusive`、`atomic_write`(tmp+flush+rename))、`error.rs`(`ProjectError`: Io/Corrupt/Missing/UnsupportedVersion/Locked/Exists/InvalidId)。文件类型经私有 `ProjectFile` trait 统一 parse/serialize,保证落盘版本始终最新。
- app-tauri `project_service.rs`(新):`project_create/open/close/status/save(_survey/session)/list_surveys/load_survey/list_sessions` 十条命令;`AppState.project: Mutex<Option<ProjectStore>>` 持有唯一打开的 store;`project_create` 用当前 A6 配置的 `plan.presets` 播种 `ProjectSettings`(ADR-019 §6;app 配置副本保留到前端切换,见 ADR 附录);命令注册进 `lib.rs`。
- 前端绑定:`Project/ProjectSettings/Survey/SurveyArea/SurveyConfig/SurveyStatus/Session/SessionStatus` 八个 ts-rs 类型生成并同步 `frontend/src/generated-types/`(未被 UI 消费,供 PJ2/PJ3/UI WP)。
- 验收(PJ1 四条):往返(create→save→reopen 三层逐字段相等)✓;写入中途被杀(残留 `.tmp` 忽略、目标保持上一份好文件)✓;旧版本 v0 迁移 + 新版本 v99 拒绝 ✓;同一项目双开 `Err(Locked)`、drop 后可重开 ✓。另有损坏→`Corrupt`、缺失→`Missing`、非法 id(`.`/`..`/路径穿越)→`InvalidId`、二次 create→`Exists` 共 11 条 core 测试 + 2 条 service 测试。
- 全量验证:`cargo test -p maggcs-core --lib` 635 通过(含 15 条 project)、`cargo test -p maggcs-app` 71 通过、clippy `-D warnings`(core+app)、`cargo fmt --all`、前端 `tsc --noEmit` 通过。未做 Tauri 原生窗口双开实测(锁由 OS 级咨询锁保证,单测覆盖);项目 UI 与 A6 迁移收尾归后续 WP。

#### 3D mesh 水平/垂直基准修正 + 测区 AOI 放大执行记录 (2026-10-18)

- 水平偏移修复:`dem-prep process`(`tools/dem-prep/src/main.rs::process_raster_tile`)原按 `dlon = dlat/111132.954*111319.49*cos(lat)` 计算经度跨度,把 `cos` 用乘而非除,导致投影瓦片 `dem.json` 的 `bounds.east` 只有真值 ~44%(BC 例 0.0730° vs 0.1661°)。改为共享 `degrees_per_cell(res_m, lat)`(`dlon = res_m/(M_PER_DEG_LON_EQUATOR·cos(lat))`),UTM 与 passthrough 分支均据此派生 `south/east`、`resolution_m` 与 hillshade;`dem-prep fetch` 的经度常量同步更正。数值验收:同一原始瓦片 reprocess 后 `east=-123.398517`(真值 -123.39854),`dem-prep terrain` 生成的 `layer.json.bounds` 与 `dem.json.bounds` 逐位一致。仅影响 `process` 产物(HRDEM 会话的 `dlat_deg/dlon_deg` 路径本无此错),且 `reuse` 会复用旧 processed → 需删旧缓存重建。
- HRDEM dem.json 容忍:`cog::DemMetadata` 的 `source_file`/`source_points`/`has_dsm`/`source` 加 `#[serde(default)]`,使无这些字段的 HRDEM 会话 `dem.json` 可被 `dem-prep terrain` 读取(写入不变)。修复前 `dem-prep terrain <hrdem 会话>` 报 `missing field 'source_file'`。
- 垂直基准(ellipsoid mesh):`dem-prep terrain` 新增 `--geoid-embedded`,用嵌入的 CGG2013 BC-SW 提取(`core::height::cgg2013_model()`)把 DTM 正高 `H` 转 WGS84 椭球高 `h=H+N(CGG2013)`,`layer.json.verticalDatum=ellipsoidWgs84`;区域外(`geoid_best_effort`)回退正高网格,绝不外推。app 三条 mesh 构建路径(`survey_prepare`/`mesh_from_tile_dir`/`build_tileset`)均传 `--geoid-embedded`。数值验收:140 m 测区 mesh 中点瓦片 header 高度由 17.73→-1.11 m(z19),`verticalDatum=ellipsoidWgs84`。前端以 `layer.json.verticalDatum` 决定是否给渲染层加 `N(EGM96)`(`MapView` `meshOn=meshEllipsoid`),与 mesh 的 `N(CGG2013)` 组合即 `H→h→AMSL` 全链,航线贴地、AGL 基准一致。
- 大 AOI mesh 尺寸上限:`terrain::cap_maxzoom_for_bounds`(上限 `MAX_TILES_AT_DEEPEST=4096`)逐级降 z,避免 12 km 级 processed 瓦片生成数万 tile(BC 12×11 km → z16/3229 tiles)。
- 运行时大瓦片读取修复(`GeoTIFF error: decoder limits exceeded`):`core::terrain::cog::read_raster` 原用 `tiff` crate 默认 limits(decoding 256 MiB),拒绝 `dem-prep process` 写出的全尺寸 BC LiDAR 瓦片(可达 ~15 000 × 11 000 f32 ≈ 660 MB);改为 `decoder_limits()`(decoding 2 GiB、intermediate 256 MiB、ifd 16 MiB),镜像 `dem-prep` 的 `geotiff.rs`。回归用例 `reads_a_raster_larger_than_the_default_decoding_limit`(`#[ignore]`,写 ~268 MB 夹具,断言默认 limits 拒绝、`read_raster` 成功),显式运行:`cargo test -p maggcs-core --lib terrain::cog::tests::reads_a_raster_larger_than_the_default_decoding_limit -- --ignored`;另有常驻单测 `decoder_limits_exceed_the_crate_defaults`。
- 测区 AOI 放大(决策 3 修订):HRDEM 取窗改为**测区矩形 + 每侧 250 m margin**,总窗口硬上限 **2 km**(`hrdem::AOI_LIMIT_M=2000`、`DEFAULT_BUFFER_M=250`、`DEFAULT_SURVEY_M=100`;app `SURVEY_MARGIN_M=250`、`SURVEY_AOI_LIMIT_M=1500`),替换原 100 m 核心 + 20 m 缓冲。测区 3.15 ha 的 drape "point outside the DEM coverage"(`TerrainError::OutsideCoverage`,`core::terrain::mod`)即因旧上限覆盖不到航线。
- drape 失败文案:`util/drapeError.ts::drapeErrorReason` 把后端 `point outside the DEM coverage`/`DEM void` 折叠为可操作文案(`DEM_NOT_COVERED_ERROR`:"load a DEM that covers the whole survey area, or shrink the plan"),其余原样透传;`mission.ts` 的 `drapeError`/告警、`PatternStep` 的 `plan.pattern.drape.failed` 均用它,不再回显后端原文。
- 验证:`cargo test -p dem-prep` 64(62 通过/2 ignored,新增 `projected_tile_east_bound_matches_the_true_span`、`longitude_degrees_per_cell_is_divided_by_cos_latitude`、`embedded_geoid_falls_back_outside_coverage`、`large_aoi_gets_a_coarser_deepest_zoom`、`hrdem_dem_json_without_source_fields_parses`)、`cargo test -p maggcs-app` 通过、`cargo clippy -p dem-prep -p maggcs-app --all-targets -- -D warnings`、`cargo fmt --all` 通过;前端 `check:drape-error`(新增 5 场景进 CI)、`check:i18n`(571 keys)、`tsc --noEmit`、`check:plan-steps/surface/agl/coords` 通过。`connection_integration::gcs_heartbeat_is_announced_automatically` 在全量并行下有偶发超时(单独运行稳定通过),与本改动无关。

#### A2 执行记录 (2026-10-16)

- 归属：`ActiveLink` 持有 RTK `RtkRuntime`(core service task、stats forwarder、foreign `GPS_RTCM_DATA` observer)；A1 的 RTK 阶段停止/等待并在超时 abort task。Core RTK service shutdown signal、reader join 均处于 `RtkRuntime::stop(deadline)` budget 内；`SourceSession::Drop` 在 abort 情况也会置 reader stop flag。新增 reader resource test 验证 stop 后 source Drop 已发生；fake-link integration 明确 stop 两个 source、shutdown service 并 join task。Motor-test target 和 SITL source 使用 `AuxiliaryLinkRuntime` 拥有其连接及 telemetry/SERVO observer，显式停止时先 abort/join observer，再 shutdown link；应用内关窗先执行 `StopAndWait`，再 stop 两条 auxiliary runtime。
- 电机测试风险与边界：电机命令路由到独立 target PX4，实时输入来自独立 SITL；两条物理 MAVLink 连接不是 primary `ActiveLink` 的子任务。armed/freshness 的实时门禁与 PX4 ACK 拒绝仍在 core motor-test loop；target/SITL disconnect/reconnect 和应用内关闭现使用 `StopAndWait`，只有 release frames 经 connection worker 确认底层 transport 写成功后才关闭/替换连接，超时返回错误并保留连接。核心 stop 确认由 worker 自身返回是否正在发送，不依赖延迟的 Tauri session-state 转发；发送失败会结束 service worker、丢弃确认 sender，使调用方收到错误而非成功。原生 `Destroyed` 是不可拦截通知，因此只能异步 best-effort 发 Stop；如未成功，会发 warning，PX4 0.3 s command timeout 是最后防线。辅助连接 owner 上提到通用 lifecycle runtime 与完整 resource regression 尚未完成。
- 失败/风险分析：错误关闭 target 会中断 `ACTUATOR_TEST` release；错误关闭 SITL 会继续用陈旧 slot 到 forward stale watchdog 生效。本次在主动断开/替换路径先等待 release，再关闭相应 link；停止超时保持 link 存活。操作系统强杀仍无法保证退出前释放电机测试。
- 验证：`cargo fmt --all -- --check`、`git diff --check`；`cargo check -p maggcs-app --tests` 通过且无 warnings；`cargo test -p maggcs-app auxiliary_runtime_tests -- --nocapture` 通过(2 项，确认 task 被析构及 stop 幂等；连接 transport 先关闭后 stop 仍在预算内释放 observer)；新增 `auxiliary_link_lifecycle_tests` 通过(2 项，stop 失败保留旧链接且 stop 成功后才执行 close)；`cargo test -p maggcs-core --test motor_test_service_integration --test rtk_service_integration` 通过(RTK 1 项、motor-test 3 项)。motor-test loopback 验收确认 `StopAndWait` 返回前 fake FC 已收到全部 4 台电机的 release frame；transport shutdown 测试确认写失败不返回成功。无实机/原生 Tauri 窗口验证；真实主链断开后的三 runtime 联合资源回收仍待补。

#### A2 主链断开联合回收回归 (2026-10-17)

- 场景：在 Tauri `MockRuntime` 上启动 `AppState`，用真实 loopback UDP `ConnectionHandle` 建立 primary link，同时挂载 `LogsRuntime`、`ParamsRuntime` 与 `RtkRuntime`，然后调用生产路径 `lifecycle_service::run(StopReason::Disconnect)`。
- 断言：`Rtk`、`Logs`、`Params`、`Link` 四个 stage 均为 `Stopped`；shutdown report 无 incomplete；`AppState::link_count()` 归零；三个 runtime 取回句柄均为 `None`；原 UDP 端口在超时内可重新绑定。期间不使用 mock runtime 替身替代被验证的 runtime。
- 发现并修复：参数 protocol task 原先只收到 `ParamService::cancel()`，命令通道仍由 runtime 持有一份 sender，任务继续阻塞在 `recv()`，导致 disconnect 的 Params stage 2 s 超时。core 新增 `Command::Shutdown` 与 `ParamService::shutdown()`：取消会话、将 pending read/set 解决为 timeout、直接退出任务；`ParamsRuntime::stop` 改为发送 shutdown 后再 join。即使调用方仍持有 `ParamService` clone 也能退出，新增 core loopback 集成测试覆盖。
- 验证：新增 app 联合回归 1 条与 core params shutdown 集成测试 1 条；`cargo test -p maggcs-app --lib -- --test-threads=1` 65 通过 + 1 ignored；`cargo test -p maggcs-core --test params_service_integration -- --test-threads=1` 2 通过；`cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings` 与 `cargo fmt --all -- --check` 通过。

#### 电机测试 Start 回落修复 (2026-10-17)

- 现象(操作员报告):电机测试窗口点击 Start 后瞬间 Stop、无法工作;场景为主链路连接飞行中的 PX4 SITL,且未连接串口电机测试目标机。
- 根因:`actuator_service::fc_link` 在没有专用串口目标时回落到**主链路**(`state.connection()`),`motor_test_target_system_id` 取默认 1(PX4 SITL 默认 sysid)→ 电机测试会话被静默挂到飞行中的 SITL 上,core 路由过滤器(`from(1,1)`)恰好匹配其心跳 → armed 门禁正确地触发 `Stopped(Armed)`。A2 已确立"电机测试不并入 primary link",但该回落路径漏删;此前地面(未解锁)测试恰好掩盖了它。这不是 09b8fe6 首心跳宽限修复的复发。
- 修复:`fc_link` 只认 `actuator_connect_target` 建立的专用目标链接,无目标时三个 start(`actuator_start_manual`/`actuator_start_preset`/`actuator_start_forward`)全部以 "no link — connect the serial PX4 first" 拒绝;前端 ActuatorApp 不再吞掉 start 错误,在头部横幅显示该错误(连接目标后自动清除)。要对 SITL 做电机测试,把该 SITL 的端点经 `actuator_connect_target` 作为目标连接即可(受支持的路径)。
- 风险/边界:方向严格更安全——无目标链接时不再对主链路上的任何机体尝试 `ACTUATOR_TEST`;失去的仅是"借用主链路"的开发便利,替代路径明确。停止/关窗/teardown 路径不变。
- 验证:新增回归用例 `motor_test_start_refuses_without_a_serial_target_even_with_a_primary_link`(MockRuntime + 真实回环 UDP 主链路在挂载 `ActiveLink` 后 `fc_link` 必须为 `None` 且无会话生成);`cargo test -p maggcs-app` 66 通过 + 1 ignored;core `motor_test_service_integration` 4 通过;clippy `-D warnings`、fmt 通过;前端 `typecheck` 与 production build 通过。未做实机串口验证。

#### 高度剖面独立窗口 (操作员需求 2026-10-17)

- 行为:新增独立窗口(Tools ▸ Altitude profile,`profile_open`),**与飞行轨迹同步**——x 轴为沿航迹距离,与地图轨迹同源自 `telemetry` 事件(每 fix 按 ≥2 m 距离抽稀,与 trail 一致),窗口自动跟随航迹头部(>2 km 后滑动);序列:实际 AMSL(遥测)、**DTM 地面**与 **DSM 表面**(经 `terrain_sample` 按已记录 fix 批量采样,1.5 s 节流,空洞不外推)、**激光雷达距离**(`DISTANCE_SENSOR`,core 新增解码,cm→m,连接时按 5 Hz 请求,3 s 过期不显示)。
- 定位:这是 DR6 剖面页的**先行独立切片**(操作员要求"生成独立的 altitude profile"),R1 的 DISTANCE_SENSOR 输入随之落地(*verify* 15 的机型可用性仍待实机);**DR6 实施时必须复用本窗口,不得重复实现**。
- 验证:core 新增 `DistanceSensor` 解码/快照/field-age/速率表与专测(cm→m、id、age),610 通过;前端 typecheck、production build(profile.html 已入多页构建)、i18n 检查通过;clippy `-D warnings`、fmt 通过。未做实机/SITL 验证(需带 DISTANCE_SENSOR 的机型)。

#### drape/剖面 DEM 自动解析 (操作员需求 2026-10-18)

- 行为:Drape to terrain 按钮与高度剖面共用同一入口 `ensureTile()`(`stores/terrain.ts`),现按优先级**自动解析**所需 DEM:① 已加载 tile → ② 记住的 dem-prep 目录(`sessionStorage`)→ ③ 测区 HRDEM 会话(`survey_prepare_status` 为 `ready` 且有 `folder`,即"准备测区"产物;其会话目录本就是合法单 tile 目录 `dem.json`+`dtm.tif`,无需再选文件夹)→ ④ `terrain3d_dem_source` 自动探测(应用旁 `dem/BC` 或 `MAGCS_DEM_DIR`)→ ⑤ 仅当自动来源全部落空才弹目录选择器。进入 **terrain 高度模式**时经 `setAltitudeMode` 以 `allowDialog=false` 自动确保 DEM 并重新 drape,后台路径绝不弹窗。
- 实现:抽出纯函数 `util/surface.ts::firstSurface(...sources)`(`ensureTile` 构造来源列表,按序取首个产出路径者,`null`/抛错均穿透,永不抛出);`check-surface.mjs`(6 场景:首个命中、全空、穿透、全抛、命中后不再调用后续来源)进 `package.json` 与 CI;`ensureTile(allowDialog?)` 签名与 JSDoc 同步。
- 验证:typecheck + production build 通过;全套 `check:*`(13 个,含新 `check:surface`)通过。截图从略——UI 无视觉变化,改动为 Tauri 调用路径下的解析行为(浏览器 harness 无 Tauri 命令)。

#### H7 执行记录 (2026-10-17)

- 数据源(N2 核实):真实 CGG2013 格网公开可得——NRCan 官方格网经 PROJ-data CDN 镜像(`ca_nrc_CGG2013n83.tif`,NAD83(CSRS)v6→CGVD2013/EPSG:6647,EPSG:9246 标称 **0.03 m**,10.5 MB,OGL-Canada;NRCan 官网 BYN 产品页现需登录)。选 `CGG2013n83`(非 `a` 变体):与 HRDEM 的静态 CGVD2013 配对,且与 ADR-018 §D 的 CSRS≡WGS84 空变换一致(格网插值 CRS 即 NAD83(CSRS) 地理坐标)。
- 工具链:新增 `dem-prep geoid <grid.tif> --datum CGVD2013 --bbox <w s e n> [--out] [--note]`——`gdal_translate -unscale -projwin` 裁剪去缩放(Int32×0.001,像元对齐、零重采样、逐值保真),写 `.mggeoid`(出处/许可/抓取日期入文件头);生产夹具 `crates/core/testdata/geoid/cgg2013_bc_sw.mggeoid`(90×90 @2′,与 EGM96 提取区同域)。core 新增 `cgg2013_model()` 嵌入镜像 `egm96_model()` 模式。
- 符号约定(实证,PROJ `cct` EPSG:9246):格网存 **N = h − H**(正向 `z_out = z_in + N`,维多利亚 N=−18.8346;`+inv` 得 H = h − N)——与 core `GeoidModel` 语义一致,转换器**不取负**(修正了设计阶段"需取负"的假设,硬断言防护:BC-SW 的 N 必为负)。
- 链路与门禁:`Height::convert` 全链贯通(H+N(CGG2013)=h,h−N(EGM96)=AMSL);app 新增 `vertical_datum_offset` 命令(offset = N(EGM96)−N(CGG2013),AMSL = H − offset),替换前端 mesh drape 的 "H≈AMSL ≤20cm" 错误近似(实测该近似偏差 **0.52–1.27 m**,超 0.5 m 预算,有专测锁定);被 mesh 覆盖的航点缺链条 → drape fail-closed 报 `VERTICAL_DATUM_UNVERIFIED_ERROR`(Warn 告警),**SyncStep AMSL 导出在该错误未清除时拒绝**。
- 验收(两段式控制点,PROJ `cct` 独立互校,4 点夹具域内):CGG2013 段 ±0.02 m、EGM96 段(15″ 参考对 5′ 提取)±0.2 m;全链 `Height::convert` H→AMSL 与两段参考一致(±0.25 m,插值差吸收)、往返精确;越界 fail-closed(双格网);符号翻转守护断言。**<0.5 m PX4 实机验收按计划保留为现场待测项**。
- 风险/边界:格网 2′ 分辨率 + 双线性插值与 PROJ 二次插值差 cm 级,远小于 0.5 m 预算;`a` 变体(历元归算)与静态 CGG2013 差 cm–dm,选静态并记录,若 M9/H7 实机控制点显示系统性偏差再评估;提取域仅 BC-SW(47.5–50.5N,125.5–122.5W),域外 fail-closed 而非外推;2D DEM 后端 drape 的"同基准内工作"妥协(ADR drape 模块注)未在本 WP 重构——属 DR2/A9 的基准语义统一,此处不越界。

#### H4 执行记录 (2026-10-17)

- 行为:core `DemMetadata` 扩展(H4 字段全部 `serde(default)`,旧 dem.json 兼容):`has_dtm`(旧文件缺省 true;北部仅 DSM 覆盖未来写 false,§6.5 的 AGL=离表面警告依据)、`dlat_deg/dlon_deg`(写入者精确像元角)、`stac_item_id`/`project_name`/`acquisition`/`original_crs`/`provenance_sha256`/`gdal_version`/`proj_version`;`DemSource` 新增 `hrdem`。`CogDem::open` **优先使用精确像元角**,缺省时回落到与旧行为逐字节一致的中纬度常数路径。dem-prep 侧:hrdem 会话在发布前写入 `dem.json`(core 类型经 serde 序列化,单一事实来源)——placement 取自 warp 产物自身的 GeoTIFF 标签(非请求参数),`version` = DTM+DSM 内容哈希前 16 位,`provenance_sha256` = provenance.json 全文 sha256;缓存判定要求 dem.json 齐备。
- 验收对照:旧瓦片仍可打开(旧格式解析测试断言 has_dtm 缺省 true、H4 字段 None;legacy 常数路径测试不变);新瓦片 1 m 行列漂移 < 0.1 像元——精确像元角使漂移构造性为零,精确优先有专测(同一点精确角解析到列 2.99,常数路径解析到 ~2.01,可区分);live 用例新增:CogDem 经 dem.json 打开会话目录,网格结点采样与直接读栅格逐值相等(≥3 个有效格,倾斜角格按 nodata 语义校验)。
- 验证:core 603 通过(新增:旧格式兼容断言、hrdem dem.json 往返、`source:"hrdem"`+`has_dtm:false` 解析、精确像元角优先);dem-prep 48 通过 + 2 ignored,两 ignored 用例(全链路 live、warp 策略)均通过;缓存测试覆盖 dem.json 缺失即 miss;ts-rs 绑定再生成并同步(`DemSource` 增 `"hrdem"`),前端 typecheck + production build 通过;clippy `-D warnings`、fmt 通过。
- 风险/边界:常熟度常数回退路径与 H4 之前逐字节一致(grid.rs 旧管线产物不受影响);hrdem 会话目录自此可被 GCS 直接加载(`CogDem::open(dtm.tif, dsm.tif, dem.json)`),为 H8/H9/U9 铺路;北部 DSM-only 区域的 `has_dtm=false` 语义仅在 DR4 规划面板消费,本 WP 只落字段。

#### H3 执行记录 (2026-10-17)

- 行为:重采样策略落地——DTM `-r bilinear`(平滑地面),**DSM `-r max`(保守,尖峰只可高估一个足迹、绝不削平)**;`-srcnodata/-dstnodata -32767` 两侧显式(nodata 不参与极值、不填补倾斜窗口外角);`-te/-tr/-tap` 参数不变,DTM/DSM 落盘后仍断言严格同网格;CLI 在取数前打印**预估输出尺寸**(45° 旋转上界,补偿 Lambert 网格倾斜;实测估算 198 vs 实际 187),取数后打印实际尺寸;出处 JSON 记录 `dtm=bilinear, dsm=max`。
- *verify* 17 实测(合成 5×5 尖峰 DSM,GDAL 3.10.3):`-r max` 输出值集恰为 {−32767, 18, 25}——单格 25.0 尖峰**逐值保留**、零发明值、nodata 原样保留;`-r bilinear` 把尖峰平均到 22.74(其自身格内)——证实 DSM 禁用 bilinear。结论:`gdalwarp -r max` 满足要求,**无需自写后处理**;测量行为已由 ignored 用例 `hrdem_warp_policy_preserves_dsm_peaks_and_nodata` 锁定(合成数据经生产参数路径)。
- 风险/边界:max 可能把仅触及足迹边缘的源像元计入(高估障碍 ≤1 个足迹,安全方向);bilinear 在 nodata 边缘由 GDAL 掩膜,不向有效地形扩散(用例断言输出 ∈ {−32767} ∪ [18, 25])。
- 验证:dem-prep 单测 48 通过(新增预估尺寸数学 1 条)+ 2 ignored;`hrdem_warp_policy_preserves_dsm_peaks_and_nodata` 实跑通过(无网络,仅本地 GDAL);live 取数用例新增"实际尺寸 ≤ 预估+1"断言后通过;CLI 实跑显示真实数据下 DSM-max(20.82)≥ DTM-bilinear(20.72);clippy `-D warnings`、fmt 通过。

#### H2 执行记录 (2026-10-17)

- 行为:`dem-prep hrdem` 现在产出两级:会话目录 `native/{dtm,dsm}-3979.tif`(H1 原生窗口,见上)+ 顶层 `dtm.tif`/`dsm.tif`(**gdalwarp 到 WGS84 规则栅格**,GCS 可直接加载)。`--resolution` 默认 1 m(决策 3),像元角 `dlon = res/(m/deg·cos(lat))`、`dlat = res/m`;`-te` 取原生窗口四角的经纬度 bbox(同一 null 变换)、`-tap` 对齐,DTM/DSM 用同一组参数并在落盘后断言**严格同网格**;重采样暂用 `-r near`(不发明值、不削峰,H3 定策略);nodata −32767 由 gdalwarp 传播,倾斜窗口外的角格不外推。
- **空基准强制(§D)**:实测 PROJ 9.6.2 对 EPSG:3979→EPSG:4326 的默认选择是 **EPSG:1946 七参数**(≈1.29 m 水平位移)——正是 §D 拒绝的管线,放任默认即违反计划。所有变换改在两条共享 GRS80 椭球、无基准标识的 proj 字符串之间执行(`gdaltransform -s_srs/-t_srs`、`gdalwarp -s_srs`),基准步在结构上不可能发生;出处 JSON 记录实际使用的 `datum_pipeline` 与 PROJ 默认会选的 `proj_default_operation`(仅审计对比,不使用)。
- **H0 夹具关系修正**:已提交夹具的窗口(rasterio,PROJ 默认)与 null 窗口恰好相差 1 像素(原点 −2029879/411180 vs −2029880/411181),对齐后 19,321 格逐像元零差异——§D 残差首次"可见"。live 测试断言 ±2 px 对齐而非固定偏移(偏移随 PROJ 版本合法移动)。
- 风险/边界:重采样/网格策略属 H3;版本矩阵与 CI 固定待 CI 引入 GDAL 步骤时做(单测不需要 GDAL,live 用例保持 ignored);三平台安装说明见 ADR-018 addendum。
- 验证:新增单测 3 条(像元角数值、projinfo 默认管线解析、resolution 参与哈希)并更新 provenance/缓存布局用例,`cargo test -p dem-prep` 47 通过 + 1 ignored;live 用例扩展四组断言——原生窗口 140±1、WGS84 对 DTM/DSM 同网格(187×187,与 bbox 跨度一致)、**放置=bbox 级控制点**(出处 te 与文件 tag 偏差 ≤1.5 px、像元角 ≤1e-9)、**§D 判别式**(PROJ 默认与 null 对 AOI 中心之差必须落在 §D 记录的 ~1 m 量级)、中心值一致(≤0.5 m)、夹具 ±2 px 对齐;clippy `-D warnings`、fmt 通过。GDAL 3.10.3/PROJ 9.6.2 实测。

#### H1 执行记录 (2026-10-17)

- 行为:新增 `dem-prep hrdem <lat> <lon> [--width/--height/--buffer/--out]`(默认 100 m × 100 m + 20 m 缓冲,核心区硬上限 `AOI_LIMIT_M=100`)。POST STAC `/search`(`hrdem-lidar`)→ 必须恰好一个项目,空覆盖报 `NoCoverage`、跨项目报 `MultipleProjects`(列 ID,不拼接);资产按 `dtm`/`dsm` 键解析(带唯一 href 后缀回退,歧义即错)。**窗口语义**:对 AOI 中心做 4326→3979 变换后,取轴对齐矩形(core/2+buffer 每侧)——不用四个地理角点的投影包络:离中央经线约 28° 处 Lambert 网格收敛约 25°,角点包络会得到 ~66×187 m 的错误窗口(实测发现并修正,与 H0 夹具的 140×140 语义一致)。`gdal_translate -of COG -projwin` 经 `/vsicurl/` 窗口裁剪原生 DTM/DSM(不重投影、不重采样,H2/H3 另行处理);staging 目录原子发布,失败不落半成品;`provenance.json` 记录 item id、采集时间、href、抓取时间、OGL 许可、GDAL/PROJ 版本(PROJ 9 的版本行在 stderr,已兼容)、输出 sha256、spec_hash 与 §D 空基准说明;同请求经 `cache_lookup` 命中缓存,无需网络与 GDAL,文件缺失/JSON 损坏/哈希不匹配一律视为 miss。GDAL/PROJ 经子进程调用而非链接(决策与理由见 ADR-018 addendum)。
- 风险/边界:AOI 经纬度包络按中纬度换算引入 <1 cm 误差,由缓冲吸收;projwin 落在项目栅格边缘时由 GDAL 报错显式暴露;STAC `limit=10` 在单项目场景足够,超出时报 MultipleProjects 而非静默截断。
- 验证:dem-prep 单测 12 条新增(AOI 上限/非法值、缓冲包络、spec_hash 稳定与敏感性、STAC 解析/去重/跨项目/资产缺失/歧义/空覆盖/后缀回退、provenance 往返、缓存命中/脏缓存/损坏 JSON),`cargo test -p dem-prep` 45 通过 + 1 ignored;clippy `-D warnings`、fmt 通过。**live 验收已完成**(GDAL 3.10.3/PROJ 9.6.2 经 micromamba 装入 `~/.local/gdal-env`,需 `PROJ_DATA` 指向 conda 前缀):ignored 用例 `hrdem_live_fetch_from_stac` 通过(断言 140×140 防回归角点包络);CLI 实跑取回 H0 夹具 AOI,网格与夹具完全一致(origin -2029879/411180,140×140),**DTM 与 DSM 各 19,600 像元逐格对比 max |diff| = 0.0**;二次运行命中缓存。*verify* 16(/vsicurl/ 窗口读取)实测可用,未加额外重试配置。**H2 修正**:H1 最初实现用 `gdaltransform` 的 EPSG 默认(七参数),H2 落实 §D 空变换后窗口较夹具偏 1 像素(见 H2 记录),夹具对比相应改为 ±2 px 对齐断言;初版记录的"与夹具逐像元零差异"指当时同管线下的对比。

#### L5 执行记录 (2026-10-17)

- **审计事件排序修复 (2026-10-17)**:发现 `writes_a_replayable_tlog_and_manifest` 间歇性失败(约 1/5)——`record_event` 经有界队列由写线程落盘,而 `record_force_arm` 持锁直写 `events.jsonl`,两条写路径导致先入队的普通事件可能晚于审计落盘(文件不再按入队顺序)。修复:审计事件改为经同一队列由写线程串行落盘,`record_force_arm` 经 ack 通道等待落盘确认(≤5 s,常微秒级),manifest 先写、事件 fsync 成功后才 ack,失败仍回滚 manifest;新增 `RecorderError::AuditUnconfirmed`(超时=拒绝强制解锁,不会出现"已授权未记录")。风险/边界:仅 `record_force_arm` 有界阻塞(app 侧已在 `spawn_blocking`),hub 生产者仍永不阻塞;写线程死亡时 ack 通道 drop 立即返回错误;慢落盘的迟到确认最多多一条审计记录,方向安全。验证:原失败用例连续 20 次通过并扩展断言(审计前/后各一条事件,文件严格按入队顺序);`cargo test -p maggcs-core --lib` 600 通过;app 65 + 1 ignored;clippy `-D warnings` 与 fmt 通过。另注:期间 app 测试出现过一批 EDQUOT 瞬时失败,系 /tmp tmpfs 被并行会话占满,与代码无关,空间恢复后全绿。
- 行为：`send_arm_disarm(force=Some(...))` 仍先经过 `evaluate_force_arm`（无链路/已解锁/清单已通过时拒绝强制），随后把未通过项、固定理由、可选备注和 UTC 微秒写入 `force_arm_audit` 事件与 `session.json` 的 `forced_arm`；两条记录使用同一 `time_utc_us`。只有审计持久化成功后才发送 Arm 命令，recorder 未运行或写盘失败时直接拒绝。
- tlog 标记：同一时间戳追加一条 GCS 发起的 MAVLink 2 `STATUSTEXT`（`NOTICE`，文本 `MAGGCS FORCE ARM: <reason> (<n> items)`，系统/组件 ID 取 GCS 的 250/250 或连接配置值），回放时可见且不会被误认为飞控消息。该标记在审计事件与 manifest 落盘后才入队；队列满载/崩溃可丢失标记，但不会丢失授权记录。ADR-021 已记录这是 tlog 原始帧规则的唯一合成例外。
- 风险/边界：标记是审计的可见性补充，不是命令发送的前置条件；真正的强制依据仍是 `events.jsonl` + `session.json`。PX4 自身的 prearm 检查照常生效，GCS 不发送 21196。旧 session 无 `forced_arm` 字段时仍可读取（`serde(default)`）。
- 验证：`cargo test -p maggcs-core --lib recorder::`（10 通过，含 tlog 标记可解码、时间戳一致、manifest/JSONL 同步写入、审计失败不进 manifest、关闭后拒绝）；`cargo test -p maggcs-app`（61 通过，含“无 recorder 时强制解锁拒绝”）；`cargo test --all-targets -- --test-threads=1` 全绿（并行跑时 `gcs_heartbeat_is_announced_automatically` 曾因回环端口竞争超时，单独重跑通过）；`cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check` 干净；前端 `typecheck`、colors/contrast/i18n、planfile/mission-sync/coords/AGL/progress/polygon 与 production build 通过。前端既有 `check-preflight.mjs` 覆盖关键字、理由和单次命令发送；未做真实 Tauri 原生窗口/飞控实机验证。

#### A6 执行记录 (2026-10-17)

- 行为：`AppConfig` 新增 `safety: SafetyConfig`，与端点、自动连接、初始位置、模型姿态和音量同存于版本化 `config.json`。`SafetyConfig::normalized` 修复不可用值（stale 500–60000 ms、卫星数 ≤32、电量 0–100 且 critical 不高于 warn），单个坏字段不会丢弃其余配置。启动时 `load_at_startup` 读取并规范化后缓存到 `AppState`；`config_save` 原子写成功后才更新缓存；`commands::safety_config` 直接返回缓存中的 `SafetyConfig`，不再返回硬编码默认值。
- 前端：`AppConfig` ts-rs 绑定加入 `safety`；`appConfig.ts` 在加载时应用、保存时原样带回安全阈值，避免尚无编辑器时（L7/U5 前）被无关设置重置。
- 风险/边界：缺字段/旧 v1 文件通过 `serde(default)` 得到安全默认值；损坏文件仍在 `load_from_path` 警告并回退默认值，不阻止启动。配置缓存只在启动和 `config_save` 更新；绕过该命令直接改文件需重启生效。
- 验证：core config 单测覆盖缺 `safety` 段回退与坏值修复；`cargo test -p maggcs-core --lib` 591 通过；app 63 通过（含 `preflight_thresholds_come_from_the_persisted_config`）；`cargo check -p maggcs-app --tests`、`npm run typecheck` 通过；ts-rs 绑定已重新生成并同步到 `frontend/src/generated-types/`。

#### L7 执行记录 (2026-10-17)

- 行为：新增 `core::safety::health::HealthMonitor`，每个 telemetry snapshot 评估电池百分比、电池电压、GNSS fix、卫星数、RSSI、TX buffer 与丢包率。活跃条件只在该条件首次出现或 severity 变化时调用 `AlertCenter::level(active=true)`；恢复后持续送入 inactive 采样，由 AlertCenter 的迟滞窗口决定何时发出 `Cleared`。Unknown/过期字段不产生告警。
- 阈值来源：`SafetyConfig` 新增 `warn_battery_voltage_mv`/`critical_battery_voltage_mv`、`critical_satellites`、`warn_rssi`/`critical_rssi`、`warn_txbuf_percent`/`critical_txbuf_percent`、`warn_packet_loss_percent`/`critical_packet_loss_percent`。电压与 RSSI 默认 `None`（不同电池/电台不可安全泛化）；卫星、TX buffer、丢包使用保守默认值。`normalized()` 保证 critical 不会比 warn 更宽松。
- 接入：`AlertState::apply_health` 一次加锁 AlertCenter、批量发布所有 transition；`telemetry_pump` 在发送 `"telemetry"` 前调用同一个 monitor，因此 UI 快照与告警状态来自同一采样。`TopBar.tsx` 电量颜色改用持久化的 warn/critical 百分比。
- 锁存修正：`AlertCenter::raise` 的 `latched` 改为粘性 OR，Critical 之后即使收到较低 severity 的重复也不会绕过操作员确认；新增回归测试。
- 风险/边界：丢包率来自 `RADIO_STATUS.rxerrors`/`fixed` 的相邻采样差值，计数器回绕或电台重启时先丢弃一个样本；首个样本不产生告警。电压/RSSI 未配置时完全关闭对应检查。告警声音与横幅复用 A3/U3 的现有路径；本次未新增设置编辑器，阈值仍通过 `config.json` 配置。
- 验证：core `safety::health` 表驱动单测 8 条（Unknown 不触发、百分比/电压、fix/卫星、RSSI/TX buffer、丢包 delta、跨阈值只触发一次、恢复迟滞、Critical 锁存）；alerts 新增锁存回归 1 条；app `apply_health` 发布链路单测 1 条；`cargo test -p maggcs-core --lib` 599 通过（仅跳过需回环端口的 raw-tap 用例一次）、app 64 通过 + 1 ignored；`cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings`、`cargo fmt --all -- --check` 通过；前端 `typecheck`、production build、colors/contrast/i18n/planfile/mission-sync/coords/AGL/progress/polygon 通过；ts-rs 绑定已重新生成并同步。未做真实 Tauri 原生窗口/飞控实机验证。

#### L4 执行记录 (2026-10-15)

- 行为：`WindowClose` 与 Disconnect/ShutdownApp 共用 core refusal gate;检测 HEARTBEAT armed 与 `EXTENDED_SYS_STATE` 非地面状态。被拒时 native close 保持窗口开启、重置 close single-flight 标志，并以 `teardown_refused` 将原因送到主窗口 banner；重试在地面/解锁后可进行。无自动飞行命令。
- 风险/边界：native event 与退出命令都重新从 telemetry snapshot 判定，仍存在飞控状态在判定之后改变的短竞态；该功能只防止 GCS 自身退出，不能阻止 OS 强杀/断电。遥测缺失/未知状态不会阻止退出。
- 验证：`cargo fmt --all -- --check`; `cargo test -p maggcs-core lifecycle::tests` (18 passed); app `lifecycle_service::tests` (2 passed)、`state::tests` (6 passed)、拒绝文案测试 (1 passed); `cargo test -p maggcs-core --test lifecycle_integration` (2 passed); `cargo check -p maggcs-app`; clippy (`-p maggcs-core -p maggcs-app --all-targets -- -D warnings`);前端 typecheck/i18n/colors/contrast/build;Playwright teardown 22 assertions 全通过。没有真实 Tauri 原生窗口/飞控实机验证。

- 已通过:前端 `typecheck`、颜色/对比度/i18n、planfile/mission-sync/coords/AGL/progress/polygon、production build;Playwright mock 的 preflight、safety、command UX、teardown、alert UX、heartbeat、topbar 检查。以上是此前审查实跑结果,非本次文档编辑后的重新运行。
- L4 实跑:见下方本次命令结果；未涉及 GDAL/PROJ。其它 Rust 模块与 GDAL/PROJ 仍未因此获得验证;H1/H2 开始前按计划固定版本安装 GDAL/PROJ。
- Playwright 场景检查尚未纳入 `.github/workflows/ci.yml`;逐批把稳定的、非依赖本机 GUI 的脚本纳入 CI。

---

## 0. 给执行 AI 的规则

1. 先读 `AGENTS.md`、`docs/ARCHITECTURE.md`、`docs/FEATURES.md`、`issues.md`、`docs/adr/004,005,006,013,017`。本文不覆盖它们;冲突时以 ADR 为准,并新增 ADR 说明改动。
2. **一次只做一个 WP**。WP 标 ◆ 表示涉及飞行安全、串口/MAVLink 写入、外部数据源或数据格式变更:先输出类型/函数签名、错误类型和失败/风险分析,**停下等确认**再实现。其余可直接实现。
3. 每个 WP 自带可自动化验收(单元测试、`crates/core/tests` 假飞控集成测试或 SITL 脚本);前端 WP 附截图与手动验证步骤(AGENTS 规则 4),并满足 §6 的可度量 UI 验收。完成后写明"如何验证的"。
4. 沿用仓库约束:Rust 生产代码不用 `unwrap`;绝对高度一律 `height::Height`,转换只在 `core::height`;TypeScript strict;常量不写魔法数;用户可见文案与文档为英文(`i18n/en.ts`);跨边界类型走 ts-rs;颜色用设计令牌;提交前跑 `.github/workflows/ci.yml` 的全部命令。
5. 新逻辑放 `crates/core`,不得依赖 Tauri;`app-tauri` 只做适配(ADR-001)。
6. **范围之外**:传感器校准、机型/遥控设置、固件刷写(归 QGC);磁力仪与 GNSS 接收机本身的出厂校准、固件与参数设置(归厂商工具);机载地形跟随(ADR-005);闭环控制;从 UI 发送 21196 强制码;PX4 固件改动;步行/手持磁测。
7. 新 ADR 从 018 起编号。
8. **外部数据合规**:不得把 NRCan 地磁数据放进仓库、测试夹具或安装包;测试用合成数据(见 M7)。
9. **新增测量类型走 profile**(PJ2),不得在通用代码里按类型硬编码分支。
10. **推迟项(⏸)不实施**:P3(M1–M4、M7–M9、U6、U7、U10;M5 已并入 §5B)及连带阻塞的 M6、PJ5,在操作员明确解冻前不得开始实现,也不提前写 `survey_session`、`diurnal`、`qc` 模块或任何 NRCan/VIC 网络访问。推迟期间,其他 WP 只需遵守「范围与状态」中的"保留扩展点",不必实现。
11. **解冻流程**:按 §8 的依赖顺序,先复核该 WP 的 *verify* 项与依赖,再按规则 2 处理 ◆。
12. **drape 只规划与观测**:drape 产出的是上传前的航点与监控基线;不做机载地形跟随(ADR-005),监控不进控制回路、不自动发命令;飞行中任何改航点(DR9)都必须操作员确认并走任务上传与回读流程。
13. **GDAL/PROJ 仅限 `tools/dem-prep`**:不得进入 `crates/core` 与 `app-tauri` 的运行时依赖;`dem-prep` 的输出仍是 `core` 可直接读取的 WGS84 COG;出处 JSON 记录 GDAL 与 PROJ 版本;CI 使用固定版本。

---

## 1. 已确认的决策

| # | 决策 | 对计划的影响 |
|---|---|---|
| 1 | 接入 **VIC(Victoria)地磁观测台**数据作为日变基准 | 新增 M7–M9;飞行前检查与归档纳入日变状态。**⏸ 决策有效,实施随 P3 推迟** |
| 2 | HRDEM 取**按项目镶嵌**,先限定 **BC 温哥华岛维多利亚地区** | 数据入口用 `hrdem-lidar` 集合;H0 先确定覆盖该地区的项目;AOI 跨项目时报警而不是拼接 |
| 3 | 精度取**可提供的最高**;**单个测区默认 100 m × 100 m**(2026-10-18 修订:取窗=测区矩形 + 每侧 250 m,总窗口硬上限 2 km) | 默认 1 m,不做降采样;AOI 硬上限为测区矩形加缓冲后的总窗口 2 km(`hrdem::AOI_LIMIT_M`),超出拒绝并说明原因;大面积相关工作(H5)降为 P2;亚米级只能走点云路径(H11,可选) |
| 4 | **允许强制解锁** | L5 采用"带理由的强制解锁",仅绕过 GCS 侧检查清单,不绕过 PX4,记入日志与归档 |

---

## 2. 优先级总览

| 线 | 目标 | WP |
|---|---|---|
| **L** 生命周期安全 | 启动 → 连接 → 飞行前 → 飞行中 → 异常 → 断开/关闭 | L1–L10 |
| **A** 架构 | 统一清理、告警、配置、记录;去掉重复实现 | A1–A11 |
| **M** 磁测 | 航线级记录与 QC、VIC 日变改正、可复现归档、面向磁测的 drape | ⏸ M1–M4、M6–M9;M5 → DR |
| **H** HRDEM | 按项目镶嵌的 1 m DTM/DSM、基准链、离线流程 | H0–H11 |
| **PJ** 项目与类型 | Project → Survey → Session;测量类型 profile;项目界面与归档 | PJ1–PJ5 |
| **DR** drape 轨迹与监控 | 相对 DTM 恒定 AGL 的轨迹设计、DSM 净空约束、航点编译、飞前检查、实时监控、记录与飞后 | DR1–DR9 |
| **U** UI/UX | 工作流导航、告警、命令、飞行前与测量运行界面、现场可用性、地图图层管理、Flight Plan 规划体验、地图可分离 | U1–U17(⏸ U6、U7、U10) |

| 级别 | WP |
|---|---|
| P0 | L1 L2 L3 L4 A1 A3 H0 U2 U3 U4 |
| P1 | L5 L6 L7 L8 A2 A4 A5 A6 A7 H1–H4 H7 U1 U5 U8 U9 U17 PJ1 PJ2 PJ3 DR1 DR2 DR3 DR4 DR5 DR6 |
| P2 | L9 L10 A8 A9 A10 A11 H5 H6 H8 H9 H10 H11 U11 U12 U13 U14 U15 U16 PJ4 DR7 DR8 DR9 |
| **⏸ 推迟** | P3:M1–M4 M7–M9 U6 U7 U10(M5 已并入 DR);连带阻塞:M6 PJ5(原优先级保留在各 WP 行) |

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
| **A2** P1 (已完成 2026-10-17) | **服务归属链路**:日志、参数与 RTK runtime 由 `ActiveLink` 持有并在 A1 lifecycle 阶段有界停止；RTK source reader、service worker、stats forwarder 与 foreign observer 显式停止/join。Motor-test 的 PX4 target 与 SITL source 是独立辅助连接，不并入 primary link；主动断开/替换及应用内关闭须在核心 `StopAndWait` 确认 release frames 写出后才关闭连接，超时则保留连接。应用层 loopback 覆盖 stop 失败保留旧 link、stop 成功 release-before-close；core loopback 覆盖 release 写确认与 transport shutdown 失败。armed heartbeat 触发 `Stopped(Armed)` 并发送 release 是 A2 的安全门禁,不是允许飞行中 actuator-test 的路径。主链断开联合回归在 mock Tauri runtime + 真实 UDP link 上同时挂载三个 runtime，验证 lifecycle 停止顺序、句柄清空与 socket 释放。 | `state.rs`、各 `*_service.rs` | 日志/参数 worker 测试、RTK source-release 与 fake-link 测试、motor-test loopback release-before-ack、transport failure、主链断开三 runtime 联合回收全部通过 | M | A1 |
| **A3** P0 | **AlertCenter**:core 新增 `alerts`。`Alert{id, severity(Info/Warn/Critical), source, text, first_ts, last_ts, count, latched, acked}`;去重/节流/锁存/确认;来源:STATUSTEXT、链路、电池、GPS/RTK、mag 告警、命令失败、DEM(VIC 数据 ⏸ 待 M7,来源枚举预留)。Tauri 侧统一 `alert` 事件;前端一个 store;`ErrorBanner`、事件日志抽屉、TopBar 角标都是它的视图。 | 新 `core/src/alerts/`;`app-tauri`;`frontend/src/stores/` | 单测:去重计数、Critical 锁存到 ack、迟滞;前端三处视图一致 | M | — |
| **A4** ◆ P1 | **飞行记录器(已完成 2026-10-15,ADR-021)**:core 新增 `recorder`;并给 `mavlink::connection` 加**原始帧旁路(方案 B,引用计数门控,不录制时零开销)**。tlog(带时间戳原始 MAVLink 帧,格式对齐 QGC:每帧前置 8 字节**小端**微秒时间戳,见 ADR-021)+ 事件日志 JSONL(告警、命令、模式、航线开始/结束、任务同步、强制解锁)+ `session.json`。专用写线程 + 有界通道,**绝不阻塞 hub**;满则丢最旧并计数告警;停止时 flush+fsync。命令/模式/任务事件源随 L5 等 WP 接入;航线事件源随 M1(⏸)接入,接口预留。 | 新 `core/src/recorder/` | 回放 tlog 与源帧逐字节一致;写盘卡顿时 hub 延迟不增加;异常退出后文件可解析到最后一次 flush | L | A1 |
| **A5** ◆ P1 | **安全/飞行前引擎**:core 新增 `safety`。纯函数 `evaluate(&TelemetrySnapshot, &PlanState, &SafetyConfig) -> Checklist`,每项 `{id, group, status(Pass/Warn/Fail/Unknown), detail, fix_hint}`;阈值集中在 `SafetyConfig`;无 IO,可无头复用。 | 新 `core/src/safety/` | 表驱动单测覆盖每项四种状态;`Unknown`(缺数据/过期)不得等于 Pass | M | L6 |
| **A6** P1 | **Rust 侧配置存储**:`app_config_dir` 下版本化 JSON(端点、autoConnect、告警阈值、磁校准路径、DEM 目录、drape 参数、UI 现场模式;VIC 数据源设置待 M7 启用时追加)。前端经命令读写;首次启动迁移 localStorage;`SafetyConfig` 从此读取。 | `desktop/prefs.ts`、新 `app-tauri/config_service.rs`、core `config` | 迁移测试;损坏文件回退默认并发 Warn,不崩溃 | M | — |
| **A7** ◆ P1 | **命令优先通道**:`CommandService` 增加 `Priority::{Normal, Emergency}`。Emergency(RTL/Land/Disarm)可顶替在途 Normal 命令,被顶替者上报 `Superseded`;Emergency 不被顶替;仍保持同一时刻最多一条在途命令。 | `core/src/commands/{mod,service}.rs`、`command_service.rs` | 假飞控:在途 `goto` 时发 RTL → RTL 立即发出并得到 ACK,`goto` 收到 `Superseded`;丢 ACK 重传不乱序 | M | — |
| **A8** P2 | **前端隔离**:地图(Cesium)包 `ErrorBoundary` 并处理 WebGL context lost;地图崩溃/卡死时 TopBar、Flight 菜单、告警仍可用。 | `App.tsx`、`MapView.tsx`、`useCesiumViewer.ts` | 手动注入地图异常:控件仍可操作;截图 | S | — |
| **A9** ◆ P2 | **单一 drape 引擎**:Rust 为唯一实现;网格仅用于显示;AGL 参考统一来自 core `ElevationSource`(HRDEM 瓦片)。DR2 起 Rust 为唯一算法实现,`meshDrape.ts` 冻结(仅允许修 bug);Rust 与 TS 两版先做黄金测试,一致后删除 `meshDrape.ts`,更新 ADR-005 addendum。 | `core/src/terrain/drape.rs`、`mission/meshDrape.ts`、`stores/terrain.ts` | 逐点误差 < 1e-6 m;删除后 `scripts/check-agl.mjs`、`check-coords.mjs` 通过 | M | H4 |
| **A10** P2 | **可观测性**:`tracing` + 滚动文件日志 + panic hook;"导出诊断包"(日志、配置、清单、最近告警,不含密钥)。 | `app-tauri/lib.rs`、新 `diagnostics` | 触发测试 panic 后文件存在且含版本 | S | A6 |
| **A11** ◆ P2 | **跨窗口单一事实源**(U15 的前置):任务航点、选中项、图层开关、相机视角、drape 结果等"地图与规划共享状态"下沉到 core(或明确的 Rust 侧同步层),各 webview 只持有投影;所有写操作带版本号,冲突时以 core 为准并回告;地图交互(加点、拖动、框选、goto)经命令回传,不直接改前端 store。撤销/重做(U8)基于同一事实源。 | 新 `core/src/shared_state/`;`app-tauri` 事件桥;`stores/*` | 两个 webview 同时订阅:任一侧操作后另一侧状态哈希一致;乱序/重复事件不造成漂移;窗口重开后从 core 全量恢复 | L | A1, A3 |

---

## 4. 生命周期安全(L)

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **L8** P1 | **启动自检 + 连接确认**:启动状态卡(上次端点、磁校准是否加载、DEM 版本与就绪、地形服务、串口权限、系统时钟/时区)。自动连接前显示"将连接到 X"并留 3 s 取消窗口;连接后显示目标 sysid/机型,与上次不同则要求确认。磁校准路径持久化(现在只在内存里的 `CalibrationStore`,重启即丢)。 | `desktop/bridge.ts`、`SettingsDialog.tsx`、`state.rs`、A6 | 手动:改端点后自动连接前出现确认;重启后校准自动恢复并显示哈希前 8 位 | M | A6 |
| **L6** P1 | **补充遥测**:`HOME_POSITION`、`EXTENDED_SYS_STATE`(着陆状态)、`ESTIMATOR_STATUS`、`VIBRATION`、`RC_CHANNELS`/`RADIO_STATUS`;连接后用现有 `set_message_interval` 请求速率;`TelemetrySnapshot`/`TelemetryFieldAges` 同步扩展。 | `core/src/telemetry/{mod,hub}.rs`、`commands.rs` | 假飞控帧单测解码正确;缺失/过期字段为 `None`,不是 0 | M | — |
| **L5** ◆ P1 | **起飞前检查 + 强制解锁**(决策 4):基于 A5 的清单。全部 Pass 才一键 Arm/Takeoff;Warn 需逐项确认;Fail 阻止,但允许**强制解锁**:输入 `FORCE ARM`、从固定列表选理由(可附备注)、再点确认;强制记录写入事件日志、tlog 标记与 `session.json`(`forced_arm:{items, reason, time}`),并在会话历史(PJ3)中标记;M4 清单与飞后 QC 标记 ⏸,解冻后由其读取该字段。**强制只绕过 GCS 清单**:PX4 自身的 prearm 检查照常生效,不发送 21196;无 FC 心跳等物理上无法发命令的情形不可强制。基础项:链路新鲜;GPS fix 与卫星数;RTK(如启用);HOME;电池;传感器健康掩码;着陆状态 on-ground;任务已上传且回读一致(`itemsHash`);模式正确;电机测试未运行。 | `core/safety`、`FlightMenu.tsx`、新 `PreflightPanel.tsx`、`command.ts` | 假飞控场景表:每项 Fail 时 Arm 被阻止且给出原因;强制路径需要全部步骤才发命令,且产生日志条目与 `session.json` 字段;PX4 拒绝时 UI 仍显示 PX4 原因 | L | A5, L6, U5 |
| **L1** P0 | **STATUSTEXT**:core 解码(含分片 `chunk_seq`),进入 AlertCenter(MAV_SEVERITY 映射),落事件日志与记录器;命令被拒(`temporarily_rejected`)时把紧随其后的 STATUSTEXT 关联到该命令展示原因。 | `core/src/telemetry`、A3、`FlightMenu.tsx`、`EventLogDrawer.tsx` | 单测:分片拼接;SITL 脚本:Arm 被拒后 UI 显示 prearm 原因(*verify* 原因文本确实随 STATUSTEXT 到达) | M | A3 |
| **L2** ◆ P0 | **常驻 RTL / Hold**:地图或 TopBar 上始终可见 RTL 与 Hold 按钮(不进下拉菜单);确认条显示当前模式、相对高度、电量;走 A7 Emergency 通道。命令状态条持续显示"已发送/等待 ACK/结果/耗时",拒绝时保留到操作员确认。交互细节见 U4。 | `TopBar.tsx`、`FlightMenu.tsx`、`stores/command.ts` | 从任一视图、任一窗口状态,发出 RTL ≤ 2 次交互;在途 goto 时 RTL 仍立即发出 | M | A7, U4 |
| **L7** P1 | **分级告警**:电池(电压与百分比,双阈值)、GPS(卫星数/fix 降级)、链路质量(丢包/RSSI)的 Warn/Critical,带声音与持续提示,阈值来自 `SafetyConfig`(替换 `TopBar.tsx` 里硬编码的 15/30%);去抖+迟滞同 mag 告警模式。 | `TopBar.tsx`、`core/safety`、已有告警音封装 | 表驱动单测;跨阈值只触发一次,不重复刷屏 | M | A3, A6 |
| **L9** P2 | `DO_REPOSITION`(点击飞到)加最大距离与高度保护;确认条显示距离。 | `commands.rs::send_goto` | 超限被拒并提示 | S | A6 |
| **L3** ◆ P0 | **FC 心跳丢失处理**:持久横幅(丢失时长计时、最后位置/高度/电量、声音);Flight 菜单置灰并说明原因,不再整体消失;遥测数值置灰+时间戳,地图飞机标"旧数据"。重连后自动重读模式、解锁状态、任务同步状态并提示"已恢复"。 | `bridge.ts`、`FlightMenu.tsx`、`TopBar.tsx`、`useDataAge.ts`、`mission` store | 假飞控:断开心跳 3 s 后出现横幅,恢复后状态重读并清除;截图 | M | A3 |
| **L4** ◆ P0 | **断开/退出护栏**:`disconnect`、设置退出、主窗口 `CloseRequested`(X/Alt-F4)在已解锁或飞行中时都拒绝并说明;需要时输入 `DISCONNECT`/`QUIT` 确认。窗口关闭事件必须 prevent-close 并把拒绝原因送到仍可见的主窗口 UI,不能调用强制退出绕过策略;不得自动发 RTL/Land/Disarm。操作系统强制结束进程不在 GCS 可保证范围内,需记录限制。退出前电机测试 Stop 要**等待发送结果**,超时策略与剩余清理顺序须显式记录;提示未上传/未保存的任务。 | `commands.rs`、`lib.rs`、`actuator_service.rs`、`SettingsDialog.tsx`、A1 | 假飞控/桌面测试:armed 与 airborne 下三条入口均不退出,CloseRequested 被拦截且 UI 显示原因;落地且 disarm 后可确认退出;电机测试退出时 Stop 先于 link;关窗时子窗口行为明确。边界:OS/进程外部强杀无法拦截 | **M（实现完成，原生桌面测试待执行）** | A1 |
| **L10** P2 | **场景测试集**:链路丢失时按 RTL、解锁时退出、电机测试中断线、重连后状态恢复、STATUSTEXT 洪泛、ACK 丢失、强制解锁路径;全用现有假飞控框架。 | `crates/core/tests`、`crates/app-tauri/tests` | 全部进 CI | M | 以上 |

---

## 5. 磁测专项(M)⏸ 整节推迟(P3)

> **⏸ 推迟**:M1–M4、M6–M9 不实施(M5 已并入 §5B),正文保留供解冻使用。M6 另因依赖 M3 而连带阻塞。推迟期间的缺口:① ADR-005 承诺的归档清单(M4)暂未兑现,过渡方案为 H1 出处 JSON + A4 的 `session.json`;② 无日变改正与航线 QC,磁测数据质量只能事后离线评估;③ drape 轨迹设计与监控见 §5B(M5 已并入其中)。

目标:以**航线**为单位组织飞行、记录、QC 与归档;用 VIC 数据做日变改正;让高程数据服务于离地高度一致性。

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **M1** ⏸ ◆ P1 | **测量会话状态机**(core `survey_session`):`Planned → Uploaded → Verified → Ready → Surveying(line k) → Complete/Aborted`。绑定任务、S1 的 `PatternLine` 表(survey/tie/cal)与 `MISSION_CURRENT`,发出 `LineStarted/LineEnded{line_id, kind}` 事件并写入记录器(issues N3)。磁数据本体仍由 FC/ULog 记录。 | 新 `core/src/survey_session/`;`MagTrigger::GcsSide` | 给定航线表与 `MISSION_CURRENT` 序列,事件序列正确(含中途改点、重复上报、断线恢复);SITL 脚本整任务跑通 | L | A4 |
| **M2** ⏸ ◆ P1 | **磁测飞行前检查**(扩展 A5):FC 日志已启用且有存储(参数名按 v1.17 核对 *verify*);磁传感器消息持续到达且速率达标;静止 10 s 噪声指标在阈值内;磁校准 JSON 已加载且哈希已记录;DEM 覆盖 AOI+缓冲且来源/版本已显示;任务与 DEM 版本一致;电量覆盖计划 ETA 加储备(用 `PatternStats`);RTK 要求满足;**VIC 数据可用性与扰动状态**(见 M7/M8);站点高度核对(M9)。 | `core/safety`、`calib`、`terrain`、`PreflightPanel.tsx` | 表驱动单测;缺校准/缺 DEM/磁数据中断/VIC 数据过期时给出对应状态与原因 | M | A5, L5, H4 |
| **M3** ⏸ P2 | **实时航线 QC**(core,滑窗):横向偏差、AGL 偏差(计划 vs 实际;有雷达时用 `DISTANCE_SENSOR`,即 R1)、地速波动、航向偏差、磁噪声指标(如四阶差分极值,算法与阈值可配);**经 VIC 改正后的残差**(M8)。每条航线汇总成 `LineQc{pass, flags}`;不合格航线进"补飞清单"。**只观测与告警,不进控制回路**。 | 新 `core/src/qc/`、survey 运行面板(U6) | 合成数据单测(注入偏差/尖峰/日变,有检出率与误报率数值);回放 ULog 验证 | L | M1, M8 |
| **M4** ⏸ ◆ P1 | **任务归档清单**(兑现 ADR-005):`survey_manifest.json` 含计划与航线表、DEM 来源/项目名/STAC item id/采集日期/哈希、大地水准面网格版本、磁校准哈希、FC 参数快照、**VIC 数据源/通道码/时间范围/取回时间/数据哈希/署名**、强制解锁记录、软件版本、操作员备注。与 tlog/事件日志同目录,并写入导出 `.plan` 扩展字段。 | 新 `core/src/archive/`、`mission/planfile.ts` | 往返测试:从清单可重建同一任务;哈希不匹配时 UI 警告 | M | A4, H4, M7 |
| **M5** → DR1–DR4 ◆ P1 | **已拆分并加强为 §5B 的 DR1–DR4,不再单独实施。** 原要点全部保留于其中:默认相对 DTM 恒定 AGL、相对 DSM 净空约束、平滑并限制爬升/下降率(上限取自 FC 参数,*verify*)、不可行处报告而非静默截断、AGL 统计与所需爬升率、站点高度偏差输入(M9 ⏸ 期间由操作员手动填写)。 | 见 §5B | 见 DR1–DR4 | — | H4 |
| **M6** ⏸ P2 | **飞后衔接**:导出航线表、事件 JSONL、改正后总场序列(含 UTC 时间)的文档化格式,供外部 QC 工具按同一张表切分;R2(ULog 展开 `DISTANCE_SENSOR`,与 drape 计划 AGL 对比)沿用现有 ULog 读取器。 | `core/ulog`、`docs/FEATURES.md` | 样例 ULog 产出对比曲线与偏差带 | M | M3, A4 |
| **M7** ⏸ ◆ P1 | **VIC 日变数据源**(core 新增 `diurnal`):`trait DiurnalSource` + `DiurnalSeries{utc, f_nt, quality}`;实现 ① **NRCan FDSN 在线源**(通道 `LFF` 秒级总场、`LFX/LFY/LFZ` 秒级矢量;`UFF` 等分钟级;位置码 `R0` 经互联网 *verify* 实际用哪个)② **IAGA-2002 文件导入**(离线/事后)③ 可选 INTERMAGNET 回退(分钟级,是否提供 VIC 秒级 *verify*)。本地缓存(按时间窗口),飞前预取过去 24 h 以评估当日磁扰,飞行中按可配周期轮询,飞后取完整时间窗。**合规**:数据不入库、不随安装包分发;设置页显示 NRCan 使用条款与署名(条款要点:仅供申请者及合作者使用,未经书面许可不得出售/向第三方分发;请向 NRCan 寄送报告副本;商业用户需联系 NRCan);商业使用前由操作员确认(见 §9)。测试一律用**合成数据**生成器。 | 新 `core/src/diurnal/`;`app-tauri` 适配;设置页 | 解码器对 IAGA-2002 样本(合成)与 FDSN 响应样本(合成,按公开格式构造)正确;断网/超时/无数据返回明确错误并发告警;缓存命中不重复请求 | L | A3, A6 |
| **M8** ⏸ ◆ P1 | **时间对齐与改正管线**:(a) 机上磁数据与 VIC 数据统一到 UTC:用 GPS 时间/`SYSTEM_TIME` 建立 boot 时间到 UTC 的映射并写入记录器,漂移/跳变检测;(b) 日变 `ΔF(t) = F_VIC(t) − F_ref`,参考值可选"测量开始均值/当日中位数/操作员指定",并记录;(c) 改正后总场 = 校准总场 − ΔF(插值到机上时间戳);(d) **扰动门限**:基于 VIC 的弦长(chord)与变化率,阈值可配(默认值由操作员按合同/规范设定,不得内置未经确认的数值),超限给 Warn/Critical 并标记受影响航线;(e) 记录 VIC 与测区的距离,距离大时提示空间相关性风险。VIC 约在维多利亚以北 16 km 的小萨尼奇山,测区离其越远,日变空间差异越大。 | `core/src/diurnal/correct.rs`、`recorder`、`qc` | 合成日变(已知幅度/周期)+ 合成机上数据:改正残差 RMS 低于阈值(给出数值);时间偏移注入 ≥ 2 s 时被检出;插值边界(数据缺口)显式标记不外推 | L | M7, A4 |
| **M9** ⏸ P2 | **站点高度核对**:起飞前在同一地点比较"DEM 推出的地面 AMSL"与"飞控着陆状态下的 AMSL"(需 RTK 固定、`EXTENDED_SYS_STATE` on-ground),给出偏差与建议;操作员可采纳为本次任务的高度偏差并记入清单。在 1 m 精度下,基准链误差与机体离地高度会盖过 DEM 本身误差,此项让误差可见。偏差含机体支撑高度,仅作指示,阈值可配。 | `core/height`、`safety`、`PreflightPanel.tsx` | 合成场景:注入已知偏差,检出值与注入值一致;未满足 RTK/on-ground 条件时状态为 `Unknown` | M | L6, H7 |

---

## 5A. 项目与测量类型(PJ)

> **当前范围**:本期只实现项目模型与 profile 注册表,仅注册 `UavMag`。下文的 `manifest.json`、`qc.json` 是**模型示意**,随 M3 / M4 解冻时落地。

### 概念模型

| 层级 | 含义 | 例子 |
|---|---|---|
| **Project** | 一个现场/合同的容器:坐标系与基准、测区、共享资源、备注 | "Saanich 2026-10" |
| **Survey** | 项目内的一种测量:**类型** + 测区 + 航线表 + 配置 | `UavMag` 100×100 m @ 40 m AGL;同一测区的重复测量 |
| **Session** | 一次实际的飞行:原始记录、事件日志、清单、QC 结果 | `2026-10-15T1432Z` |

- **同一项目可含多个 Survey**(例如同一测区的重复测量),各自独立执行,共享项目级资源(DEM 缓存、磁校准、参数预设、VIC 设置)。
- **类型由 `SurveyProfile` 决定**,不是在各处按类型写条件分支:profile 声明所需设备、阶段导航、飞行前清单模板、QC 模板、航线跟随方式、输出格式。新增类型 = 新增 profile + 平台 driver。
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
      raw/                     # tlog(原始 MAVLink 帧)
      qc.json
```

### WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **PJ1** ◆ P1 | **项目/测量/会话模型与存储(ADR-019)**:core `project`:`Project`、`Survey`、`Session`;目录布局如上;`schema_version` + 迁移;原子写(临时文件 + rename);项目目录锁文件防双开;文件损坏回退并发 Warn。A6 的应用配置只保留应用级设置,**项目级设置**(DEM 目录、预设、校准路径)迁入项目;VIC 源设置待 M7 启用时追加。 | 新 `core/src/project/`;`app-tauri/config_service.rs` | 往返测试;写入中途被杀后可恢复;旧版本迁移;同一项目双开被拒 | M | A6 |
| **PJ2** ◆ P1 | **测量类型 profile 与注册表**:`SurveyKind` 与 `SurveyProfile{required_devices, phases, safety_template, qc_template, line_following, outputs}`;**本期仅注册 `UavMag`**。A5 的 `evaluate` 接收 profile 给出的清单模板;U1 阶段标签由 profile 决定(UAV:Prepare/Plan/Preflight/Fly/Review)。**"通用状态机 + 平台 driver"的拆分随 M1 ⏸ 推迟**:M1 启用时必须直接按本 profile 拆分(UAV driver 绑定 `MISSION_CURRENT`),不得先做 UAV 专用版再重构。 | 新 `core/src/survey_profile/`;A5 | UAV 既有测试不回退;注册一个**假的第二种类型**(仅测试用),不改通用代码即可得到其清单模板与阶段标签 | M | PJ1, A5 |
| **PJ3** P1 | **项目界面**:启动页(最近项目/新建/打开);新建项目向导(名称、坐标系、测区、备注);项目内测量列表(类型徽标、状态、最近会话);会话历史(日期、**强制解锁**标记;QC 结果列待 M3 解冻后加入);TopBar 显示当前项目与测量类型。**切换/关闭项目在已解锁时拒绝**(走 A1 policy;Surveying 状态判定随 M1 ⏸ 接入)。U14 首次向导并入"新建项目"。 | 新 `ProjectHome.tsx`、`TopBar.tsx`、`stores/project.ts` | 截图;已解锁时切项目被拒并说明原因;重启回到上次项目 | M | PJ1, PJ2, U1 |
| **PJ4** P2 | **项目级共享资源与归档**:DEM 缓存、磁校准、预设、VIC 设置挂在项目下,多个测量复用;项目导出/导入 zip,含项目文件、`survey.json`、A4 记录文件与事件日志;导入时校验哈希。**M4 清单与"排除 NRCan 日变原始数据"条款待 M4/M7 解冻后加入**。 | `core/project`、`core/archive` | 导入后可重建同一任务/航线表(来自 `survey.json`);哈希不匹配时拒绝 | M | PJ1, H9(M4 ⏸) |
| **PJ5** ⏸ P2 | **跨 Session 比较**:同一项目内多个 Session(重复测量)在重叠区比较改正后总场(差值、交叉点残差、统计),离线复盘用。 | 新 `core/src/compare/` | 合成数据:注入已知偏置/高度差,检出值与注入值一致 | M | PJ1, M6 |

---

## 5B. Drape 模式航测轨迹设计与监控(DR)

> M5 已并入 DR1–DR4。DR5–DR7 不依赖 M1/M3/U6/M7;M3、U6 解冻时复用其计算与组件。

### 目标与原则

1. **磁测要的是离地高度一致**:磁信号随到地面磁性源的距离衰减,AGL 起伏直接变成数据噪声。所以主质量量是"沿线 AGL 与目标的偏差",安全净空是硬约束。
2. **相对 DTM 恒定 AGL 为目标,相对 DSM 的净空为约束**。冲突时优先级 `净空 > 爬升/下降率 > AGL 目标`,**报告偏差与不可行区间,不静默截断**。
3. **设计与监控用同一份数据**:`DrapeResult` 的哈希同时进入上传回读(`itemsHash`)、飞前检查、监控基线与记录。
4. **只规划与观测**(规则 12):不进控制回路,飞行中不自动改航点。
5. **阈值可配,不内置未经确认的数值**:`min_clearance_dsm` 必填(未填不能生成);监控阈值未设置时仅显示数值,飞前清单给 Warn"阈值未设置"。
6. **高度基准链沿用 H7**:缺 CGG2013 时不得导出 AMSL 任务;drape 内部以 DEM 高程加 `vertical_offset` 表示,偏移的来源与备注记录在结果里。
7. **DEM 不外推**:航线超出 DEM 覆盖或落在 nodata 上,该段标 `NoData`,生成被阻止或要求操作员处理。

### 数据模型(DR1)

`DrapeSpec`:`target_agl`(m,相对 DTM)、`agl_band`(±m,监控与验收用)、`min_clearance_dsm`(m,**必填**)、`pos_margin`(m,水平位置余量,净空检查时对 DSM 取该半径内最大值)、`dsm_footprint`(m,机体/传感器足迹半径)、`max_climb_rate` / `max_descent_rate`(m/s,取 FC 参数与机型限制中较小者,*verify* 参数名)、`ground_speed`(m/s,据此得到 `max_grade = rate / ground_speed`,上下分别计算)、`sample_step`(m,默认不大于 DEM 像元)、`smoothing`、`runin_length`(m,测线前导段)、`turn_policy`、`wp_tol`(m,航点简化的最大偏离)、`vertical_offset`(m,操作员输入,含来源与备注;M9 解冻后可自动填入)。

`DrapeResult`:每条测线(`PatternLine` id、kind)的 `profile[(s, x, y, z_plan, dtm, dsm, agl_plan, clearance)]`;`waypoints`;`infeasible[{line, s_range, reason, max_deficit}]`;`stats{agl_mean, agl_std, agl_min, agl_max, min_clearance, max_grade, max_climb_needed}`;交叉点 `crossings[{line_a, line_b, agl_diff, z_diff}]`;`spec_hash`、`dem_ref{item id, 项目名, 采集日期, 哈希}`、`version`。

### 轨迹设计方法(DR2)

对每条测线按下列步骤,全部在 `core` 内用 Rust 实现:

1. 沿线按 `sample_step` 采样 DTM 与 DSM;DSM 取 `dsm_footprint + pos_margin` 半径内最大值(尖峰不被削平,见 H3)。
2. 目标剖面 `z_t(s) = DTM(s) + target_agl + vertical_offset`;下界 `z_min(s) = DSM_fp(s) + min_clearance_dsm`。
3. 求 `z(s)`:在 `z ≥ z_min`、上爬 `dz/ds ≤ max_grade_up`、下降 `dz/ds ≥ −max_grade_down` 的约束下,使 `∫(z − z_t)²ds` 加平滑项最小。**爬升与下降限制必须预瞻**:在陡坡前就开始爬、在悬崖后才开始降,而不是在坡脚才爬(否则坡顶 AGL 会掉下去)。实现择一并写入 ADR-022:① 反向+正向两遍速率限制加下界抬升迭代至收敛(推荐:可解释、O(n)、易测);② 带约束二次规划。
4. 约束互相冲突时输出 `agl_dev(s)` 与 `infeasible` 区间及最大缺口,不削平。
5. **线首/线尾**:每条测线前加 `runin_length` 的同向延长段,进入测线时已到稳态高度与速度;**测线段内的高度变化只来自地形**;线间连接(转弯段)承担线与线之间的高度过渡。
6. **交叉点一致性**:survey 线与 tie 线交点处计算计划 AGL 之差与 AMSL 之差并输出;超过可配阈值时提示(磁测交叉点校平依赖这一致性)。
7. **自检**:对最终 `z(s)` 回算 AGL、净空与坡度并与约束比较;任一违反都是内部错误,不得输出结果。

### 航点编译(DR3)

- 把 `z(s)` 简化为折线(Douglas–Peucker 变体),与设计曲线的最大偏离 ≤ `wp_tol`;**简化不得使折线低于 `z_min`**(违反处回插点)。
- 航点间 PX4 按直线段飞行(*verify*),所以编译后的折线才是飞控真正的目标;编译偏差在结果里单列。
- 检查航点总数与任务项上限(*verify*);超限时给出可行的 `wp_tol` 建议,不静默放宽。
- 每个航点带 `s`、线 id、kind,便于监控匹配与记录;速度项按线设置;接受半径(水平/垂直)按 *verify* 的参数写入检查项。
- 高度帧选择与换算沿用 `height::Height` 与现有高度模块,不在此处换算。

### 监控(DR5)

输入:遥测位置/速度/高度(带时效)、`DrapeResult`、DEM(`ElevationSource`)、可选 `DISTANCE_SENSOR`。位置到计划折线的投影 `s*` 单调推进并带迟滞;不依赖 M1,线 id 取自结果。

| 指标 | 定义 | 说明 |
|---|---|---|
| 垂直误差 `ez` | `z_actual − z_plan(s*)`(折线基线) | 同时报告折线与设计曲线之差(编译偏差) |
| AGL 偏差 `e_agl` | `(z_actual − DTM(pos)) − target_agl − vertical_offset` | 与 `agl_band` 比较;同时输出 DEM 局部坡度与不确定带 |
| 横向偏差 | 到计划折线的水平距离 | 与现有航线偏差复用 |
| DSM 净空 `clr` | `z_actual − DSM_fp(pos)` | 与 `min_clearance_dsm` 比较 |
| **预瞻净空** | 沿当前地速方向 `T_look` 秒内,按当前垂直速度外推(上限取爬升率限制)的最小 `z_pred − DSM_fp − min_clearance` | 在撞上之前预警"前方净空不足" |
| 剖面追踪饱和 | 实际 `abs(vz)` 达到限制且 `ez` 在扩大 | 提示"跟不上地形",区别于单纯偏差 |
| 雷达差 `e_dem` | `agl_radar − agl_dtm`,滑窗中位数与 MAD | 有 `DISTANCE_SENSOR` 时估计 DEM 偏差与局部不确定度(R1) |
| 新鲜度 | 位置、DEM 覆盖、雷达时效 | 过期或飞出 DEM 为 `Unknown`,不外推、不当作 Pass |
| 带内时间占比 | `abs(e_agl) ≤ agl_band` 的累计占比与越界时长 | 按线汇总 |

告警(经 A3,带去抖与迟滞,阈值来自配置):**Info**——进入/离开某条测线的监控;**Warn**——`e_agl` 越带持续超过门限、预瞻净空低于预警值、雷达差持续超限(提示 DEM 可能不可信)、监控输入 `Unknown`;**Critical**——`clr < min_clearance_dsm`,或预瞻预测违反并持续,锁存到确认。Critical 只提示并高亮 L2 的 RTL/Hold,**不自动执行命令**。

### WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **DR1** ◆ P1 | **`DrapeSpec` / `DrapeResult` 数据模型与参数输入**(ADR-022):类型经 ts-rs 暴露;`min_clearance_dsm` 必填校验;`spec_hash`;参数预设沿用 U8;`vertical_offset` 手动输入并记来源。 | `core/src/terrain/drape.rs`(类型)、`PatternStep.tsx` | 序列化往返与哈希稳定;缺必填项时生成被拒并说明原因 | S–M | H4 |
| **DR2** ◆ P1 | **轨迹生成算法**:上节的 7 步(预瞻爬升/下降限制、净空下界、冲突报告、run-in 与转弯过渡、交叉点一致性、自检)。Rust 为唯一实现;`meshDrape.ts` 冻结,仅用于显示。 | `core/src/terrain/drape.rs`、新 `core/src/terrain/drape_solver.rs` | 合成地形(斜坡/台阶/尖峰/悬崖/沟谷)单测:`z ≥ z_min − 1e-6`、`abs(grade) ≤ max_grade + 1e-6`;预瞻方案在陡坡处的最大 AGL 偏差**小于**因果限速基线(给数值);不可行用例报出区间与缺口;nodata 段不外推;同输入同哈希 | L | DR1 |
| **DR3** ◆ P1 | **航点编译**:折线简化(不低于 `z_min`)、`wp_tol`、任务项数量检查、接受半径与速度项、航点携带 `s`/线 id/kind;编译偏差入结果。 | `core/src/survey/compile.rs` | 简化后净空仍满足;编译偏差 ≤ `wp_tol`;超任务项上限时拒绝并给出建议;导出 `.plan` 回读一致(`itemsHash`) | M | DR2 |
| **DR4** ◆ P1 | **规划与飞前 drape 检查**:A5 新增 **Drape 组**——DEM 覆盖航线加缓冲、`has_dtm`(仅 DSM 时警告 AGL 实为离表面高度)、DEM 采集年代、`infeasible` 为空或已逐项确认、航点数与上限、所需爬升率与 FC 参数一致、阈值已设置、电量覆盖 ETA 加爬升余量。规划界面:沿线**剖面预览**(DTM/DSM/计划/下界/容许带)、不可行区高亮并可定位到航点、AGL 统计表、交叉点一致性、`Generate` 不可用时就地显示原因。 | `core/safety`、`PatternStep.tsx`、`AltitudeProfile` | 表驱动单测每项四种状态,`Unknown` 不等于 Pass;每个 Fail 有"原因 + 修复入口"(U5);截图 | M | DR3, A5, L5 |
| **DR5** ◆ P1 | **drape 监控引擎**(core `drape_monitor`):上节指标与告警;`s*` 单调投影带迟滞;纯函数式 `step(snapshot, plan, dem, cfg) -> MonitorState`,无 IO,可无头与回放复用;输入过期或飞出 DEM 为 `Unknown`。**只观测**。 | 新 `core/src/drape_monitor/` | 合成轨迹(注入 AGL 偏差/过高爬升/净空违反/雷达偏置/数据过期):指标与解析值差 < 1e-6 m;预瞻告警早于实际违反(给提前量数值);迟滞下跨阈值只触发一次;`Unknown` 不为 Pass;tlog 回放结果可复现 | L | DR1, A3, A4, H4 |
| **DR6** P1 | **监控界面**:Fly 阶段 **Drape 面板**(当前线与进度、`e_agl` 条与容许带、净空与预瞻净空、地速与垂直速度/饱和标志、雷达差、带内时间占比);`Dock` 启用**高度剖面页**(计划 vs 实际 vs DTM/DSM 与容许带,游标跟随 `s*`);HUD 增加 **AGL(相对 DTM)**(U7 其余部分仍 ⏸);地图上航迹按 `e_agl` 着色;陈旧时整块置灰并显示 STALE + 秒数;告警走 U3。 | `RightInspector.tsx`、新 `DrapePanel.tsx`、`Dock.tsx`、`Hud.tsx`、`entities.ts` | 回放/SITL:面板、剖面与地图着色同步;陈旧注入显示正确;现场模式下可交互元素 ≥ 44 px;截图 | M | DR5, U1, U3 |
| **DR7** P2 | **记录与飞后**:经 A4 记录器写入 drape 时间序列(UTC、`s*`、`z_plan`、`z_actual`、DTM、DSM、`e_agl`、`clr`、`e_dem`)与事件(越带、预警、净空违反);按 `PatternLine` 汇总每线统计;导出 CSV 与 Markdown 摘要(计划 vs 实际 AGL 对比、带内占比、最小净空),与 `DrapeResult` 哈希关联;不依赖 M1。 | `core/src/recorder/`、`core/src/drape_monitor/report.rs` | 回放 tlog 生成的统计与实时一致;写盘卡顿时监控不丢帧;异常退出后文件可解析 | M | DR5, A4 |
| **DR8** P2 | **测线方位角扫描与建议**:对候选方位角(步长可配)重算并给出不可行长度、最大/总爬升量、AGL 偏差、预计 ETA 与电量,表格与曲线展示;**只建议,不自动改方位**。 | `core/src/terrain/drape.rs`、`AreaStep.tsx`/`PatternStep.tsx` | 合成山脊地形:扫描结果与手算一致;最优方位随地形旋转正确变化 | M | DR2 |
| **DR9** ◆ P2 | **DEM 偏差估计与确认式 re-drape**:有 `DISTANCE_SENSOR` 时,用 `e_dem` 的稳健统计给出偏差估计与置信度;事后可作为下次 `vertical_offset` 候选;若 8 号待确认项允许飞行中 re-drape:仅限线间、对**剩余航线**用新偏移重新生成、操作员确认后走上传与回读流程,旧结果与新结果并存可回退;**绝不自动应用**。 | `core/src/drape_monitor/`、`mission` 上传流程 | 合成雷达偏置:估计值与注入值一致、置信度随样本数变化;未确认时不发生任何上传;重新上传后回读哈希一致 | M | DR5, L5 |

### 验收汇总

- DR2 约束满足与可复现(数值断言进 CI);Rust 与 TS 显示曲线一致性按 A9 的黄金测试。
- DR5 在合成数据上给出检出率与误报率数值;不得用放宽阈值来通过验收。
- 回放真实 tlog(A4 格式)得到与实时一致的监控结果。
- SITL 整任务:设计 → 上传回读 → 飞行 → 监控 → 记录 → 飞后摘要,全流程通过;SITL 地形与 DEM 的对应关系须在 *verify* 后写入脚本说明。

---

## 6. HRDEM 引入(H)

### 6.1 已核实的事实

- 来源:NRCan CanElevation。许可 **Open Government Licence – Canada**,引用与署名按其要求。
- 数据入口:AWS 开放数据桶 `s3://canelevation-dem/`(区域 `ca-central-1`,**匿名访问**)。按项目镶嵌在前缀 **`hrdem-lidar/`**;STAC API:`https://datacube.services.geo.ca/stac/api/search?collections=hrdem-lidar`。1 m/2 m 全国镶嵌是 `hrdem-mosaic-1m/`、`hrdem-mosaic-2m/`,本计划不用。
- 产品:DTM + DSM,基准 **CGVD2013**,重投影到 **EPSG:3979(Canada Atlas Lambert)**,以 **COG** 提供;项目之间**没有拼边处理**;南部由机载 LiDAR 生成 DTM 与 DSM,北部多为仅 DSM。
- **取数方式**:STAC 查询 + COG 窗口读取是官方推荐的直接访问方式;WCS 官方将淘汰,不作数据源(仅 H10 可选对照);WMS 仅用于显示,不进入计算(NRCan《HRDEM 镶嵌访问指南》v2.0,2025-02)。
- 项目 item 以"省/机构 + 项目名 + 分辨率"命名(例如 `MB The Pas 1m`),带采集时间范围。
- **已由 H0 核实(ADR-018)**:覆盖维多利亚地区的项目 = `BC-Vancouver_Island_Sunshine_Coast_2018-1m`(采集 2019-10-01);资产 `-dtm.tif`/`-dsm.tif`(S3 HTTPS 免认证);nodata **−32767**;COG 512×512 分块、Float32、LZW、8 级 overview;垂直精度按产品规格"一般优于 1 m"(项目级数值未随 STAC 提供)。

### 6.2 与现有代码的差距

- `core::terrain::cog` 只读地理坐标(WGS84 度)栅格;`dem-prep` 现有读取只认 UTM 与地理 GeoTIFF、没有联网获取。读取、重投影、联网窗口读取改由 GDAL/PROJ 承担(H1–H3),`core` 不变。
- 像元角度由中纬度常数换算:4 m 可接受;1 m 下漂移不可忽略(H4 修正)。
- DSM 降采样若用双线性会削平尖峰,净空检查偏乐观(本决策默认不降采样,但重投影仍需保守处理)。
- 默认处理分辨率为 4 m(`dem-prep process --resolution 4`)。

### 6.3 精度的真实瓶颈(100 m × 100 m,1 m 分辨率)

测区只有 1 万个栅格,内存与性能不是问题。限制精度的是:

1. **水平基准**:HRDEM 为 NAD83(CSRS),PX4 GNSS 为 WGS84;ADR-018 §D 已接受空变换,残留约 1 m 必须显式记录。H2 使用 GDAL/PROJ 执行 EPSG:3979 与 WGS84 网格间投影/重采样,但不得擅自附加 NAD83(CSRS)→WGS84 的非空基准变换;若 H7/H10 控制点显示系统偏差,先更新 ADR-018 并取得操作员确认,再评估 EPSG:1946。该决策是当前基线,不是待 H2 默认推翻的事项。
2. **垂直基准链**:`H(CGVD2013) + N(CGG2013) = h`,`h − N(EGM96) = AMSL`;缺真实 CGG2013 格网时 AMSL 精度无法保证(issues N2)。
3. **DEM 自身的垂直精度与采集年代**(树冠、地表变化)。
4. **机体离地高度与 GNSS 垂直精度**(M9 ⏸ 解冻后让其可见;此前靠 H7/H10 控制点)。

### 6.4 WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **H0** ◆ P0 | **调研与决策(ADR-018,已完成)**:(a) STAC 项目、资产、CRS、nodata、COG 结构与垂直精度已查证;(b) S3 HTTPS Range 访问及 fixture 已验证;(c) NAD83(CSRS)↔WGS84 当前按 ADR-018 §D 的空变换决定,不得由 H2 擅自改变;(d) 许可与署名已记录;(e) 140 m × 140 m DTM+DSM fixture 与 provenance 已提交。**仍待 H1/H2 实测**:`/vsicurl/`+GDAL 运行时读取、`gdalwarp -r max` DSM 尖峰行为、固定版本及 PROJ 数据包的 CGG2013 格网情况。 | `docs/adr/018-hrdem.md`、`testdata/hrdem/` | 已完成事实来源见 ADR-018;剩余实测作为 H1/H2 验收,不再重复创建 fixture | S–M | — |
| **H1** ◆ P1 | **`dem-prep hrdem` 子命令**(GDAL):输入测区中心+尺寸(或多边形/`.plan`)+ 缓冲;STAC 查询;AOI 跨项目时**报警并要求选择单一项目**,不拼接;用 GDAL `/vsicurl/` 窗口读取 DTM/DSM;记录出处(item id、项目名、采集日期、资产 href、抓取时间、sha256、许可文本、**GDAL 与 PROJ 版本**)。AOI 硬上限 100 m × 100 m 加缓冲(配置项,超限拒绝并说明原因)。离线明确报错;重复运行命中缓存。 | `tools/dem-prep/src/{main,hrdem.rs}`、`Cargo.toml` | 夹具 AOI 端到端产出原生栅格 + 出处 JSON;超限/跨项目/离线各有明确错误 | M | H0 |
| **H2** ◆ P1 | **GDAL/PROJ 集成与坐标变换**:`dem-prep` 引入 GDAL/PROJ(`gdal` crate 或调用 `gdalwarp` 命令行,择一写入 ADR-018 addendum),**不再自行实现 Lambert 投影**;用 GDAL/PROJ 将 EPSG:3979 重投影到 WGS84 规则栅格,遵守 ADR-018 §D 的**空基准变换**决定(不得隐式应用 NAD83(CSRS)→WGS84 七参数变换);若控制点要求改变该决定,先修订 ADR-018 并获操作员确认。GDAL/PROJ 仅限 `tools/dem-prep`(规则 13);CI 固定版本并在出处里记录;提供三平台安装/打包说明。 | `tools/dem-prep/{Cargo.toml,src/}`、`.github/workflows/ci.yml` | 已知控制点经投影/重采样后误差在 ADR-018 接受的精度内(阈值待实测,不得自行放宽);空基准变换可被测试/日志证实;三平台 CI 通过;缺 GDAL 时给出明确错误 | S | H0 |
| **H3** ◆ P1 | **重采样策略**(`gdalwarp`):默认**保持 1 m**(决策 3),只重投影到 WGS84 规则网格(`-tr`、`-te`、`-tap` 固定,DTM 与 DSM 用同一组参数,保证**严格同网格**);DTM 用 `-r bilinear`;**DSM 用足迹内最大值(`-r max`,保守;重投影到相近分辨率时的实际行为须实测,*verify*,不满足则改为自写后处理)**;nodata(−32767)显式设置并保留,不填补。预估输出尺寸并显示。 | `tools/dem-prep/src/` | 带尖峰的合成 DSM 经 `gdalwarp` 后峰值不被削低;DTM/DSM 网格一致检查;同输入同版本下输出可复现 | M | H1, H2 |
| **H4** ◆ P1 | **元数据扩展 `dem.json`**:`source:"hrdem-lidar"`、项目名、STAC item id、采集日期范围、精确像元角度(`dlat_deg`、`dlon_deg`,**消除运行时中纬度常数换算**)、原始 CRS、高度基准 `CGVD2013`、`has_dtm`/`has_dsm`、出处哈希、GDAL/PROJ 版本。对旧瓦片向后兼容。 | `core/src/terrain/{mod,cog}.rs`、`tools/dem-prep/src/cog.rs`、ts-rs 绑定 | 旧瓦片仍可打开;新瓦片在 1 m 下行列漂移 < 0.1 像元(与解析值对比) | M | H3 |
| **H7** ◆ P1 | **垂直基准链收尾**:issues **N2 升为阻塞项**。接入真实 CGG2013 格网(核对 NRCan 当前可用格网与格式 *verify*;PROJ 数据包若自带 CGG2013 可作独立对照,不替代 N2 的核实);`--geoid` 支持 HRDEM;**缺格网时 UI 明确标注"高度基准未验证",并阻止导出 AMSL 任务**;控制点验收按 AGENTS.md(偏差 < 0.5 m,阈值待实测,不得自行放宽)。 | `core/src/height/`、`dem-prep terrain --geoid` | 控制点两段式测试;缺格网时导出被阻止 | M | H4 |
| **H5** P2 | **惰性分块读取**(降级):测区上限为 100 m × 100 m 时不需要;保留为将来放宽面积上限的前置条件。`ElevationSource` 的 COG 实现改为按需读 tile + LRU + 内存上限;沿用"不外推"。 | `core/src/terrain/cog.rs` | 放宽上限后峰值内存低于配置;采样结果与整幅读取一致 | M | H4 |
| **H6** P2 | **来源选择与新鲜度**(2026-10-17 修订,操作员指示):**按 AOI 自动解析最高精度来源**——① `hrdem-lidar` 覆盖查询(1 m,首选)→ ② 本地已加工瓦片(BC LiDAR 1 m)→ ③ GeoTIFF 兜底(CDEM 等)→ ④ GLO-30 下载(30 m);解析器为 core 纯策略函数(候选源 + AOI → 选择),取数仍经 dem-prep(规则 13);UI chip 显示所选来源/项目/分辨率/采集年代、基准,允许手动覆盖(覆盖动作记入配置);采集年代过旧(阈值可配)提示植被/地貌变化风险。原"手动选择并记住"降级为手动覆盖路径。 | `terrain_service.rs`、`stores/terrain.ts`、`LayersPopover`、core 新增来源解析 | 解析器表驱动单测(覆盖优先级/精度排序/无覆盖回退);chip 截图 | S–M | H4, H9 |
| **H8** P2 | **3D 网格**:`dem-prep terrain` 以 HRDEM 瓦片生成;1 m、100 m × 100 m 下给出瓦片数/体积估算写入文档;网格仅用于显示(A9)。**已实现**:`--geoid-embedded` 把 DTM 正高转 WGS84 椭球高(`H+N(CGG2013)`),`layer.json.verticalDatum=ellipsoidWgs84`,前端据 manifest 加 `N(EGM96)`;大 AOI 经 `cap_maxzoom_for_bounds` 限 tile 数。 | `tools/dem-prep/src/mesh.rs`、ADR-004 addendum | 小 AOI 生成并显示;体积符合估算 | S | H4 |
| **H9** P2 | **"准备测区"一键流程**(Plan ▸ Area,UI 见 U9;`dem-prep`+GDAL 作为 sidecar 随应用分发,§9 第 9 条已决):画/输入 100 m × 100 m 后联网一键执行 H1→H3→H4→(H8),按 AOI 哈希缓存;完成后飞行前检查显示"DEM 就绪"。**默认自动**(操作员 2026-10-17),办公室预备目录保留为手动路径;**同区域重复请求复用已有文件**——取数层按 spec_hash 命中会话目录(H1 已实现),tileset/mesh 层按输入哈希复用,不重新下载或加工;外场离线只用缓存。About 与导出文件带 NRCan 署名与 OGL 文本。 | `AreaStep.tsx`、`terrain_service.rs`、`SettingsDialog.tsx` | 联网一次后断网重启,drape 与显示仍可用;**同一测区二次请求零下载、零 GDAL 调用、产物哈希一致**;署名出现在导出 `.plan` 与 About | M | H1–H4 |
| **H10** P2 | **验证**:(可选)与国家镶嵌的 WCS 输出做一次性交叉对照(WCS 官方将淘汰,只作验证,不作数据源);HRDEM 与 BC LiDAR 处理瓦片在重叠区对比(RMSE、偏差、最大差、空洞率);控制点验证基准链;同一航线 AGL 差异记录。 | `testdata/`、`docs` | 对比报告含数值;可复跑 | M | H7 |
| **H11** P2 可选 | **亚米级点云路径**:若需要优于 1 m,只能从 LiDAR 点云自行生成(现有 `dem-prep process` 的 LAS 路径,点云来源与许可另议)。默认不启用;启用前由操作员确认点云密度足以支撑所选分辨率。 | `tools/dem-prep` | 密度不足时拒绝生成并说明 | S | 操作员确认 |

### 6.5 磁测使用约定

> M5 已拆为 §5B 的 DR1–DR4;本节约定同时约束 DR 系列与 A9 统一后的实现。

- **AGL 默认相对 DTM**(AGENTS.md);净空检查相对 DSM。UI 与报告必须分别标注。
- **仅 DSM 区域**:`has_dtm=false` 时 AGL 实为离表面高度,规划面板必须显示警告,QC 报告标注该前提。
- 项目采集年代写入 M4 清单(⏸;期间保留在 H1 出处 JSON 中);采集年代与现状差异大时 H6 提示。

---

## 7. UI/UX 优化(U)

### 7.1 现状观察(来自 `frontend/src`)

- 只有两个视图(`planning` / `flight`);`RightInspector` 只在 `planning` 渲染内容(`view === 'planning' ? <PlanSteps/> : null`),**Flight 视图的右栏是空的**;`Dock` 的 `TABS` 为空、不渲染。
- TopBar 高 48 px,控件高度不统一:状态/菜单约 32 px,`Tools` 按钮 44 px,`IconButton` 28–32 px;文字令牌 `xs=11px`、`sm=12px`,HUD 还有硬编码 `text-[10px]`。对手套、日光下的平板或笔记本偏小。
- `RightInspector` 写死 `w-[264px]`,而设计令牌 `--mg-panel-w` 为 288 px。
- 规划默认值:矩形区域 500 × 500 m,默认 AGL 50 m(文档中的 drape 例子是 40 m);与 100 m × 100 m 的实际测区不符。
- 确认方式不统一:飞行命令"5 秒内点两次",电机测试"输入 `START PROPS` + 按住 3 s"。
- 告警分散(见 A3);子窗口(Inspector/Actuator/Params)是否显示链路丢失等告警 *verify*。
- 地图目前把 imagery、graticule、height sticks、AGL trail、hillshade、3D terrain 等开关收在 `MapToolbar` 的 Layers popover;缺少统一的图层分组/顺序/透明度管理,多个栅格/地形状态由 `terrain` store 与 Cesium scene 分散维护,视图中也缺少明确的可用性/加载状态汇总。
- Flight Plan 规划目前是右侧四个折叠步骤卡片(Area → Pattern → Waypoints → Sync),上方固定高度模式和同步状态;多项配置分散在卡片与地图工具间,步骤完成状态、关键任务摘要、危险/无效配置的定位与返回编辑路径仍可优化。QGC `.plan` 兼容和 mission-sync 语义必须保留。
- 已有可复用资产:设计令牌与主题(dark/light/system/night-red)、`Pill`/`Popover`/`Tabs`/`StatusDot` 等组件、gallery、基线截图、磁告警声音。

### 7.2 设计原则

1. **按作业阶段组织**,不是按功能堆叠:准备 → 规划 → 飞行前 → 飞行 → 复盘。
2. **关键状态一眼可读**:解锁、模式、链路、电量、GPS/RTK 永远可见(日变状态 ⏸ 待 M7 解冻后加入),颜色之外同时有图标/文字。
3. **危险操作的确认方式按风险分级**并全局一致。
4. **每个阻止项都给出原因和下一步**(带"去修复"入口),不让操作员猜。
5. **现场可用**:大目标、高对比、夜间红光、键盘可达、少依赖鼠标悬停。
6. 不做乐观显示:命令只显示"待确认/已确认"(沿用 ADR-017 原则)。

### 7.3 WP

| ID | 内容 | 触及文件 | 验收 | 规模 | 依赖 |
|---|---|---|---|---|---|
| **U1** P1 | **作业阶段导航**:把两个视图扩展为 `Prepare / Plan / Preflight / Fly / Review`。Prepare:连接、校准文件、DEM 准备(VIC 数据 ⏸ 待 M7);Preflight:检查清单(U5);Fly:HUD、命令状态条与 Drape 监控面板(DR6;完整测量运行面板 U6 ⏸);Review:本次飞行记录摘要(事件日志、强制解锁标记、记录文件位置;航线 QC、补飞清单与导出 ⏸ 待 M3/M6)。阶段标签上显示完成度徽标(如 `Preflight 9/11`);阶段间可自由切换,不强制线性。保留 `Plan` 现有四步卡片。 | `stores/ui.ts`(`View`、`VIEW_CONFIG`)、`TopBar.tsx`、`RightInspector.tsx` | 五个阶段均可进入;完成度徽标随检查结果变化;截图 | M | A5 |
| **U2** P0 | **TopBar 信息层级与一致性**:分三组——状态(Armed、Mode、Link)、健康(GPS/RTK、Battery、DEM 就绪;**VIC 日变** pill ⏸ 待 M7,预留槽位暂不显示)、工具(Tools、告警、设置);统一控件高度与最小点击目标;状态 pill 带图标;单一告警角标(数量+最高严重度)打开 AlertCenter;修复 `Tools` 44 px 与其他 32 px 不一致。窄屏(≤ 1366)时低优先级 pill 折叠进"更多",状态组永不折叠。 | `TopBar.tsx`、`design-system/components.tsx` | 1280 与 1920 宽度截图:状态组始终完整可见;点击目标尺寸脚本断言(见 §7.4) | M | A3 |
| **U3** P0 | **告警中心界面**:Critical 以顶部横幅显示并持续到确认(链路丢失横幅含计时与最后位置,L3);Warn 为不遮挡的 toast;角标计数;抽屉按严重度/来源过滤,含 STATUSTEXT 控制台视图;严重度各有**不同的声音模式**,可调音量,静音有时限并自动恢复;颜色之外有图标;`aria-live` 播报。所有窗口(含 Inspector/Actuator/Params)挂同一个告警横幅组件。 | `ErrorBanner.tsx`、`EventLogDrawer.tsx`、新 `AlertCenter.tsx`、各子窗口 `main.tsx` | 触发 Critical 后,在子窗口也出现横幅;确认后消失;重复告警不刷屏;截图 | M | A3 |
| **U4** ◆ P0 | **命令交互分级**(统一规则):**低风险**(切 Hold、改显示)一次点击;**中风险**(Takeoff、Land、Arm)按住 1 s 带进度环,期间显示将要执行的内容;**高风险/不可撤销**(强制解锁、退出、清飞控任务)输入关键字或再点确认。RTL/Hold 常驻按钮(L2),确认条显示模式、相对高度、电量。统一 `HoldButton`、`ConfirmBar` 组件替换现有"5 秒内点两次"。命令状态条持续显示"已发送 → 等待 ACK → 结果 + 耗时"。 | `FlightMenu.tsx`、`ModePopover.tsx`、`design-system/components.tsx`、`stores/command.ts` | 手动+截图;自动测试:误触单击不会发出中/高风险命令;从任一阶段发 RTL ≤ 2 次交互 | M | A7 |
| **U5** ◆ P1 | **飞行前清单界面**:按组显示(链路 / 导航 / 电源 / 任务 / 地形 / Drape(DR4);磁测、日变组 ⏸ 随 M2/M7 加入,分组可注册),每行"状态图标 + 一句话原因 + 修复入口"(如"加载磁校准…""准备测区");顶部显示 `9/11 就绪`;Warn 逐项确认;Fail 显示"强制解锁"入口,流程见 L5(关键字 + 理由 + 记录);进入飞行阶段前的总体"可飞/不可飞"徽标。 | 新 `PreflightPanel.tsx`、`FlightMenu.tsx` | 每个 Fail 项都有可点的修复入口;强制路径记入日志;截图 | M | A5, L5 |
| **U6** ⏸ P1 | **测量运行面板**(填充 Flight 视图的空右栏):当前航线 `Line 7/24 · survey · 63%`、横向偏差条(±m)、AGL 实际 vs 计划与允许带、地速、电量预测(完成时剩余)、下一航线预览、航线 QC 状态胶囊(通过/待定/标记)、日变迷你曲线(VIC ΔF 与当前扰动等级)。地图上航线按 QC 状态着色,当前航线高亮;`Dock` 启用"高度剖面"页:计划 vs 实际 vs DTM/DSM(复用 `AltitudeProfile`)。 | `RightInspector.tsx`、新 `SurveyRunPanel.tsx`、`Dock.tsx`、`entities.ts` | SITL 回放:航线切换时面板与地图同步;截图 | L | M1, M3, M7 |
| **U7** ⏸ P1 | **HUD 重做**:关键数值加大并用等宽数字字体(`--mg-font-mono`),加入 **AGL(相对 DTM)**,保留 REL/AMSL;陈旧数据时整块置灰并显示"STALE + 秒数",取代当前两个 10 px 小徽标;可拖到四角并记住位置;最小字号不低于 12 px(现场模式 14 px)。 | `Hud.tsx`、`instruments/` | 截图;陈旧注入测试显示正确 | M | L3 |
| **U8** P1 (已完成 2026-10-17) | **小测区规划体验**:新建测区默认 **100 m × 100 m**,默认中心取"飞机位置 → HOME → 上次测区"(按优先级);支持数值输入宽高与方位角并在地图上实时预览;生成前显示航线数、总长度、ETA、预估电量;**参数预设**(保存/载入如"40 m AGL · 5 m/s · 5 m 间距",经 `config.json` 的 `plan.presets` 持久化,重启仍在);Generate 按钮不可用时就地显示原因;规划编辑支持**撤销/重做**(`stores/mission.ts` 的 `undoStack`/`redoStack`,`Ctrl+Z`/`Ctrl+Shift+Z`,Plan 头部按钮;terrain 模式撤销后自动重 draped);默认 AGL 与文档一致。 | `AreaStep.tsx`、`PatternStep.tsx`、`PlanSteps.tsx`、`mission/patterns.ts`、A6 | 打开即为 100 × 100 m;预设保存后重启仍在;撤销/重做测试(`scripts/check-undo.mjs`,7 场景,进 CI) | M | A6 |
| **U9** P1 (已完成 2026-10-17) | **"准备测区"卡片**(H9):显示 HRDEM 项目名、采集年份、分辨率、DTM/DSM 是否齐全、基准状态、体积与下载进度(可取消);地图上预览覆盖范围;离线就绪徽标;失败原因(跨项目、无覆盖、离线、格网缺失)都有明确文案与下一步。 | `AreaStep.tsx`、`terrain_service.rs`、`LayersPopover` | 各失败场景截图;联网一次后离线可用 | M | H9 |
| **U10** ⏸ P2 | **VIC 日变面板**:状态胶囊(`VIC ● 1 s · 8 s 前`)、F(t) 曲线与 ΔF、距测区距离、扰动等级、数据源与署名;事后改正时对比"原始 / 已改正"。 | 新 `DiurnalPanel.tsx`、TopBar pill | 合成数据下曲线与数值正确;断网时状态变灰并给出原因 | M | M7, M8 |
| **U11** P2 | **多窗口协调**:`Tools` 菜单显示各工具窗口的运行状态(如电机测试"发送中"徽标)与"置前";记住窗口位置与大小 *verify 现状*;所有窗口共享告警横幅(U3)与链路状态指示。 | `TopBar.tsx`(Tools)、`actuator_service.rs` 等窗口创建处 | 手动:切换窗口不丢失告警;截图 | S–M | U3 |
| **U12** P2 | **现场模式与无障碍**:设置里的"Field mode":点击目标 ≥ 44 px、基础字号 14 px、高对比;与现有 `night-red` 主题兼容;完整键盘导航与焦点环;颜色之外的状态冗余(图标/形状);可减少动画;全局快捷键(如 `Esc` 关闭、`?` 显示快捷键表),**不为危险命令设单键快捷键**。 | `design-system/{theme,index.css}`、`SettingsDialog.tsx`、各组件 | 脚本断言:现场模式下所有可交互元素 ≥ 44 px;键盘可完成"连接 → 查看清单 → 发 RTL" | M | U2 |
| **U13** P2 | **反馈与长任务**:上传任务、DEM 下载/构建、日志下载(VIC 取数 ⏸ 待 M7)统一显示进度、已用时、可取消;空状态与错误状态有统一样式;单位集中设置(m、m/s、nT、坐标格式)。 | `components/`、各 store | 各长任务有进度与取消;截图 | M | — |
| **U14** P2 | **设计系统整理与首次使用**:修复 `RightInspector` 264 px vs 令牌 288 px、硬编码字号;补 `HoldButton`、`ConfirmBar`、`Toast`、`ChecklistRow`、`Gauge`、`Banner` 到组件库与 gallery;基线截图扩展到新阶段与现场模式(深/浅/夜间红光 × 1366/1920);首次启动向导(连接 → 校准文件 → HOME/车辆 → 准备测区;VIC 设置步骤 ⏸ 待 M7)与 `?` 帮助。 | `design-system/`、`gallery/`、`App.tsx` | 基线截图更新并通过;向导可跳过且可重新打开 | M | U1 |
| **U15** ◆ P2 | **地图可分离窗口(detachable)**:默认地图内嵌;"弹出地图"创建独立 webview(沿用 Inspector/Actuator/Params 的窗口模式),弹出后主窗口切换为**无地图布局**(放大 HUD、命令状态条、Drape 监控面板、清单;完整测量运行面板 U6 ⏸ 解冻后加入);关闭地图窗口自动回嵌且不丢状态。**主窗口必须自足**:TopBar、HUD、命令状态条、告警在无地图时完整可用,地图只是增强视图。地图窗口同样挂统一告警横幅与链路状态条(U3/U11),全屏地图时 Critical 仍可见。窗口语义:关闭地图窗口≠退出;主窗口关闭走 `Lifecycle::shutdown`,地图窗口随之关闭(L4)。地图渲染崩溃/WebGL context lost 时仅该窗口受影响(A8),主窗口显示"地图不可用"并可一键重开。窗口位置/大小/所在显示器记忆;目标显示器被拔掉时回落到主屏。 | `TopBar.tsx`、`App.tsx`、`MapView.tsx`、`useCesiumViewer.ts`、窗口创建处(`app-tauri`)、`stores/ui.ts` | 自动测试:注入地图窗口崩溃/强制关闭后,主窗口发 RTL ≤ 2 次交互且命令走 A7 Emergency;弹出/回嵌前后任务、选中、图层、相机状态哈希一致(A11);地图窗口内出现 Critical 横幅并可确认;拔掉第二显示器后窗口回落主屏(手动 + 截图) | L | A11, A1, A3, A8, U3, U11 |
| **U16** P2 | **地图图层管理优化**:将 Layers popover 梳理为清晰的分组/列表(底图、地形、任务/测量、飞行器/轨迹),显示每层名称、可见状态、数据来源/加载或错误状态;支持适用图层的透明度与顺序调整,提供"全部恢复默认"但不改变数据源/任务。现有底图、离线格网、地形 hillshade/3D mesh、航点/测区、航迹、AGL/高度标记的开关须语义一致;未加载/无覆盖/加载失败应明确说明,不显示误导性的已启用状态。图层偏好持久化并受 A11 共享状态约束;关键飞行安全层不得被静默隐藏。 | `MapToolbar.tsx`、`stores/ui.ts`、`stores/terrain.ts`、`cesium/scene.ts`、`useCesiumViewer.ts`、`A11` shared state | 自动/组件测试断言每个图层开关与 Cesium visibility 一致、terrain 加载/失败/无覆盖状态正确、透明度范围被限制、重启后用户偏好恢复;Plan/Fly 两视图及深/浅/夜间主题截图;弹出地图往返图层状态哈希不变 | M | A11, U15 |
| **U17** ◆ P1 (已完成 2026-10-18:U17a–U17d) | **Flight Plan 规划界面优化**:重整 Area → Pattern → Waypoints → Sync 的信息层级,提供明确步骤导航与完成/阻塞状态;常驻摘要显示测区尺寸/方位、pattern 类型与关键参数、航点数/航线数、垂直基准/高度模式及与飞控同步状态;阻塞项可定位到对应步骤/字段,修复后返回原流程。Waypoints 表与地图选择双向联动,批量编辑/删除及撤销边界明确;Sync 区分本地草稿、与 FC 不一致、上传中/等待回读/已一致,保留显式确认和 `itemsHash` 校验。响应式布局确保地图、步骤表单与摘要不互相遮挡;不改变 QGC `.plan` schema、编译高度语义或安全门禁。<br>**U17a(已完成)**:新增 `mission/readiness.ts`(`planSteps` 纯派生,`check-plan-steps.mjs` 10 场景进 CI);抽出 `PlanStepCard.tsx`(编号徽标 + 完成/阻塞状态点),新增常驻 `PlanSummary.tsx`(测区尺寸/方位 · pattern 类型+间距 · 航点数 · 高度模式 · 同步态);`PlanSteps.tsx` 由 readiness 驱动、高度模式改 2×2 网格、同步徽标移入摘要;`RightInspector` 264→288 px(对齐 `--mg-panel-w`)。<br>**U17b(已完成)**:`readiness.ts` 增 `planBlockers`(纯派生,`drape_failed`→Pattern/`drape` 字段、`fc_mismatch`→Sync/`sync` 字段),步骤 `blocked` 由 blocker 集合驱动(修复点所在步骤变红);摘要区渲染红色 blocker 条,点击经 `ui.focusPlanField` 打开该步并滚动/高亮/聚焦对应字段(`data-plan-field`,`.plan-field-flash`,尊重 reduced-motion);`check-plan-steps.mjs` 扩到 15 场景,新增 `screenshots/u17/plan-blockers.png`。<br>**U17c(已完成)**:`stores/mission.ts` 增 `removeWaypoints`/`setAltitudeMany`(各为单条 undoable 记录);`WaypointsStep.tsx` 行首复选框 + Shift 区间选择(锚点于 store 更新前捕获)+ Select all/Clear + 批量条(计数/设高度/红色删除);i18n `plan.batch.*`;`check-undo.mjs` 扩到 11 场景;新增 `screenshots/u17/{wp-list,wp-batch}.png`。<br>**U17d(已完成)**:`mission/sync.ts` 增 `SyncPhase`/`PlanSyncPhaseFacts`/`planSyncPhase`(在 `planSyncStatus` 之上追加进行中的 upload/download/clear 与 verifying 态:进行中优先于已存关系,`verifying` 优先于 `mismatch`);`mission/hash.ts` 增 `shortHash`(FNV-1a 32-bit → 8 hex,因 `itemsHash` 是用于相等的规范 JSON 而非摘要);新增共享 `SyncStatusLine.tsx`(态文本按 tone + `plan`/`FC` 短哈希,全串入 `title`),由 `PlanSummary` 与 `SyncStep` 复用;`PlanSteps` 经 `flyable()` 算 `planHash`、由 `lastSyncedHash` 得 `fcHash` 并派生 `phase`;`check-mission-sync.mjs` 增 phase 全序列 + `shortHash` 场景(21→25);截图 `screenshots/u17/u17d-sync-{unsynced,uploading,verifying,mismatch}.png`。 | `components/panels/plan/{PlanSteps,PlanSummary,SyncStep,SyncStatusLine}.tsx`、`mission/{sync,hash}.ts`、`i18n/en.ts` | Playwright 覆盖四步导航/阻塞定位/地图与航点选中同步/编辑撤销/上传回读状态;导入→编辑→导出 `.plan` 的 schema 与 `itemsHash` 往返测试通过;窄屏/桌面截图与键盘导航检查(U17a 以 `check-plan-steps.mjs` + `frontend/screenshots/u17/` 截图替代) | M | U1, U8, A7 |

### 7.4 可度量的 UI 验收(写成脚本,进 CI 或随 PR 提供)

- 任一阶段、任一视图下,**发出 RTL 不超过 2 次交互**。
- U16:图层面板中每一开关都与 Cesium 实际显示一致;加载中/失败/无覆盖状态有区别;重启和地图分离/回嵌不丢用户图层偏好。
- U17:Plan 四步可键盘访问;阻塞项可跳转到确切字段;Waypoints 选择与地图高亮一致;QGC `.plan` 导入/导出保持 schema 与 `itemsHash` 一致。
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
| **P0 安全底座** | H0(已完成)、A1、A3、L1、L2、L3、L4、U2、U3、U4 | 已解锁/飞行中时断开、应用退出和窗口 CloseRequested 均不能触发 GCS 自身退出;拒绝原因可见;STATUSTEXT 可见;RTL 常驻;心跳丢失横幅;所有窗口显示告警;命令确认分级统一;相关假飞控测试进 CI |
| **P1 记录与检查** | A7、A6、L6、A5、L5、U5、A4、L7、L8、A2、U1 | 飞行前检查能阻止解锁并可强制;每次飞行都有 tlog + 事件日志 + 清单骨架;五阶段导航可用 |
| **P2 HRDEM** | H1→H2→H3→H4→H7、U8、U9 | 夹具测区从联网获取到 drape 全流程通过;基准链控制点验收通过;规划默认 100 m × 100 m |
| **P2b 项目与测量类型** | PJ1、PJ2(仅 profile 模型与注册表)、PJ3 | 项目可新建/打开/重启恢复;注册假类型无需改通用代码即可得到清单模板与阶段标签;已解锁时切项目被拒 |
| **P2c Drape 轨迹与监控** | H4 → DR1→DR2→DR3→DR4;DR1+A3+A4 → DR5→DR6;随后 DR7、DR8、DR9 | 合成地形(斜坡/台阶/尖峰/悬崖/沟谷)下轨迹约束满足且结果可复现;规划界面可见剖面、不可行区与 AGL 统计;tlog 回放与 SITL 中监控指标与告警正确;Critical 经 AlertCenter 在所有窗口可见 |
| **P3 磁测与日变** ⏸ 暂缓,不排期 | M7→M8、M1、M2、M4、U6、U7,随后 M3、M9、U10(M5 已并入 P2c) | 一次 SITL 整任务:航线事件正确、日变改正残差达标(合成数据)、drape 报告可复现、归档清单可重建任务、测量运行面板与地图同步 |
| **P4 收尾** | A9、A8、A10、A11、H5、H6、H8、H9、H10、H11、L9、L10、U11–U16、PJ4(M6、PJ5 随 P3 ⏸) | 删除 TS drape;地图可分离且崩溃隔离通过;现场模式与基线截图完成;文档合并;`issues.md` 清理 |

依赖要点(当前有效):L4 window-close 强制退出偏差已修复;A3 → L1/L3/L7/U3;A1 → L4/A4/A2;A5+L6 → L5 → U5;H0 → H1/H2 → H3 → H4 → (H7, A9);U4 → L2;A11+A1+A3+A8+U3+U11 → U15;A6 → PJ1 → PJ2 → PJ3;H4 → DR1 → DR2 → DR3 → DR4(+A5、L5);DR1+A3+A4 → DR5 → DR6/DR7;DR2 → DR8;DR5 → DR9。

**⏸ 解冻后恢复的依赖**:L5 → U5 → M2;DR5 → M3(复用偏差计算);DR6 → U6(复用组件);M7 → M8 → M3/U10;A4 → M1/M4/M8;**PJ2 → M1**(通用状态机 + 平台 driver);M3 → M6;M6 → PJ5。

---

## 9. 待核实 / 风险 / 待操作员确认

**待核实**(标 ⏸ 的随 P3 推迟,暂不阻塞)

1. ~~覆盖维多利亚地区的 HRDEM 项目、采集年份、资产命名、nodata、垂直精度(H0)。~~ **已解决(H0/ADR-018)**:项目 `BC-Vancouver_Island_Sunshine_Coast_2018-1m`(2019-10-01),DTM+DSM,EPSG:3979/NAD83(CSRS),CGVD2013,nodata −32767;垂直精度按产品规格"一般优于 1 m"(项目级数值在项目 File Geodatabase,未取)。
2. ~~NAD83(CSRS)→WGS84 是否使用非空变换。~~ **当前决策已由 ADR-018 §D 接受**:按空变换处理,记录约 1 m 残差;只有 H7/H10 控制点发现系统偏差且操作员确认修改 ADR 后,才评估 EPSG:1946。H2 的 GDAL/PROJ 用于栅格重投影,不自动改变基准决策。
3. ~~CGG2013 格网的可获得性与格式(H7,issues N2);PROJ 数据包是否自带该格网。~~ **已解决(H7, 2026-10-17)**:真实格网经 PROJ CDN 公开镜像(NRCan 官方,Int32×0.001 GeoTIFF,2′);conda 最小 PROJ 包不含该格网,但 CDN 直接可得且由 `dem-prep geoid` 转换;两段式控制点互校通过(见 H7 执行记录)。
4. ⏸ VIC 经 NRCan FDSN 服务的实际可用通道(`LFF`/`LFX…` 与位置码)、秒级总场的真实采样率、近实时延迟、是否需要登记邮箱(M7)。
5. ⏸ VIC 是否在 INTERMAGNET 提供秒级数据(回退路径)。
6. ~~tlog 格式是否与 QGC 完全一致(A4)。~~ **已解决(A4/ADR-021)**:QGC 文档为"8 字节自 Unix 纪元起微秒的 `uint64_t`(`memcpy`,主机字节序)+ 原始 MAVLink 包(含起始符)";目标平台均小端,写端取小端 u64,读端按自定界重新分帧以兼容 QGC 文件。大端主机需另复核。
7. ⏸ PX4 v1.17 的日志参数名(M2);速度与爬升限制参数名见第 13 项。
8. 解锁被拒时 STATUSTEXT 原因是否稳定随后到达(L1)。
9. 主窗口先关而子窗口仍开时的进程行为(L4);子窗口是否显示链路/飞行告警(U3)。
10. 规划编辑是否已有撤销/重做、窗口位置是否已记忆(U8、U11)。
11. 地图与规划共享的前端 store 范围(任务、选中、图层、相机、drape 结果),决定 A11 的工作量(U15)。
12. Tauri 多 webview/窗口在目标平台(Windows/macOS/Linux)上的行为:Cesium 在独立窗口的 WebGL 初始化、窗口回落、context lost 恢复(U15、A8)。
13. PX4 v1.17 任务执行的垂直行为:航点间是否按直线 3D 段飞行、水平/垂直接受半径参数(如 `NAV_ACC_RAD`、`NAV_MC_ALT_RAD`)、最大爬升/下降速度参数(如 `MPC_Z_VEL_MAX_UP/DN`)、任务项数量上限、`DO_CHANGE_SPEED` 行为(DR2、DR3,参数名均须核对)。
14. `GLOBAL_POSITION_INT` 的 `alt`(AMSL)与 `relative_alt` 的速率与延迟;位置到 DEM 的水平基准差(NAD83(CSRS) 与 WGS84,ADR-018 空变换决策下约 1 m 残差)对 1 m 栅格上 AGL 的影响,尤其在陡坡处(DR5)。
15. 目标机型是否有 `DISTANCE_SENSOR`(量程、姿态补偿、更新率)(DR5、DR9)。
16. ~~GDAL `/vsicurl/` 对 NRCan S3 COG 的窗口读取是否稳定、是否需要额外配置(重试、缓存)(H1)。~~ **已解决(H1 live 验收 2026-10-17)**:GDAL 3.10.3 默认配置下窗口读取稳定可用(单次取回 ~10 s 含 STAC 查询),未加额外重试/缓存配置;取回结果与 H0 夹具逐像元一致。
17. ~~`gdalwarp -r max` 在 1 m 到 1 m 重投影时对 DSM 尖峰的实际行为,以及 DTM 与 DSM 能否保证严格同网格(H3)。~~ **已解决(H3 实测 2026-10-17)**:合成尖峰用例确认 `-r max` 逐值保留尖峰、零发明值、nodata 不参与不填补;同参数 warp 保证严格同网格(落盘后另有断言)。无需自写后处理。
18. 不同 GDAL/PROJ 版本对重采样结果的差异,以及应固定到哪个版本(H2)。

**风险**

- **NRCan 地磁数据条款**(M7 ⏸:推迟期间仓库不含任何 NRCan 数据或访问代码,风险休眠;**解冻 M7 前必须先处理**):仅供申请者及合作者使用,未经书面许可不得向第三方分发,并要求寄送报告副本,商业用户需联系 NRCan。若本项目的测量属商业性质,在上线 M7 前必须先取得许可或确认适用范围。代码层面已按"不分发、仅本地缓存"实现。
- ⏸ 日变空间相关性:VIC 在维多利亚以北约 16 km,测区距离越大改正越不可靠;M8 记录距离并告警。
- 1 m 数据下,基准链、水平基准和机体离地高度的误差可能超过 DEM 自身误差;H7/H10 的控制点验证用于让它们可见(M9 ⏸ 解冻后补充站点级核对),不得用放宽阈值来"通过"验收。
- HRDEM 项目采集年代可能较旧,与现场植被/地表不一致:靠 H1 出处 JSON(M4 ⏸)留痕与 H6 提示。
- **地图分离的状态漂移**:两个 webview 若各持一份状态会漂移;A11 未完成前不得上线 U15。
- **推迟带来的缺口**:见 §5 开头(归档清单、日变改正与航线 QC 未落地;drape 部分已由 §5B 承接)。
- **drape 的位置—DEM 匹配误差**:1 m 栅格加约 1 m 水平基准残差,在陡坡处会产生 AGL 伪偏差。监控必须同时显示 DEM 局部坡度与不确定带,不得仅凭 DTM 差值自动判定"偏离";有 `DISTANCE_SENSOR` 时用 DR9 区分"飞行偏差"与"地形模型误差"。
- **折线 vs 平滑曲线**:PX4 在航点间按直线段飞行(*verify*),实际垂直剖面是折线;DR3 用 `wp_tol` 约束编译偏差,DR5 把"编译偏差"与"飞行偏差"分开报告。
- **净空下限无默认值**:`min_clearance_dsm` 必填;DSM 为采集年代的地表,树木生长、新建障碍不在其中,净空只是数据层面的保证,不能替代操作员目视与现场勘查。
- **GDAL/PROJ 引入**:三平台安装与打包体积;版本差异导致重采样结果不可复现。缓解:仅限 `dem-prep`(规则 13)、出处 JSON 记录版本、CI 固定版本。
- **项目模型迁移成本**:PJ1 引入后,A6 与 M4 的存放位置会变;先定 ADR-019 再动 A6(M1 ⏸),避免二次迁移。

**需要操作员确认**(标 ⏸ 的在 P3 解冻前必须回答,推迟期间无需回答)

1. 测区中心坐标(H0 夹具需要;未提供则用占位 AOI)。
2. ⏸ 本项目使用 VIC 数据是否属商业性质(决定 M7 上线前是否必须取得 NRCan 许可)。
3. ⏸ 日变改正是否需要**飞行中实时**显示,还是事后改正即可(决定 M7 的轮询与延迟要求)。
4. ⏸ 日变扰动门限(弦长与变化率)的取值来源(合同/规范)。
5. 现场使用的设备类型(笔记本 / 平板 / 触屏)与是否戴手套(决定 U12 的默认现场模式与尺寸)。
6. 是否使用多显示器(决定 U15 的优先级;单屏/平板场景下建议保持内嵌默认)。
7. Drape 参数的取值来源(合同/规范/机型手册):目标 AGL、AGL 容许带、`min_clearance_dsm`、位置余量 `pos_margin`、预瞻时间、持续时间门限、tie 线交叉点 AGL 差阈值、机型最大爬升/下降率与巡航地速(DR1、DR5)。
8. 是否允许飞行中 re-drape(DR9):若允许,是否仅限线间、是否需要二人确认;不允许则 DR9 只做事后评估。
9. ~~准备测区(H9)在应用内运行,还是在办公室预先准备~~ **已决(操作员 2026-10-17):两种策略都要,自动处理为默认**。应用内自动流程需要把 `dem-prep` 连同 GDAL 运行库作为 sidecar 随应用分发(三平台打包方案随之确定);办公室预备的产物目录仍可手动指定使用。同区域重复请求必须复用已有文件(见 H9 的去重验收),不重新取数/加工。
