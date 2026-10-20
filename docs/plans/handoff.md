# Handoff — MagGCS:执行 `docs/plans/lifecycle-survey-hrdem-plan.md`

> 本文件是**工作面文档**,给下一个接手的 AI/人用,不与用户可见文档同寿:
> 计划 §0 的收尾(并入 `FEATURES.md`/`ARCHITECTURE.md`)时,连同
> `lifecycle-survey-hrdem-plan.md` 一起删除。

## 0. 环境与作业方式(先读这一节,环境已和上一轮不同)

- 仓库 `/home/dev/rgcs`(MagGCS:Rust core + Tauri 2 + React/TS)。
  `export PATH="$HOME/.cargo/bin:$PATH"` 后 cargo 才可用。
- **权限已收紧(本轮中途变的)**:`workspace-write` + 网络受限。
  - `.git` 只读 → `git add/commit/push` 必须 `require_escalated`。
  - 网络受限 → **Playwright/Chromium 必须提权**,`curl localhost` 也可能被拦。
  - `cargo test` 里 `crates/app-tauri/src/terrain_server.rs` 的 6 个用例要绑定
    回环端口 → 沙箱内必失败(`cannot bind 127.0.0.1:0: Operation not permitted`),
    **这是环境问题不是代码问题**,提权重跑即全绿。
  - `cargo deny check` 要写 `~/.cargo` 的 advisory 锁 → 需提权。
  - 已批准的 prefix:`node frontend/scripts/`、`cargo test`、`cargo deny`、
    `git add`、`git commit`、`git push`。
- 操作员用**中文**回复;代码/文档/i18n 一律英文(`docs/plans/*.md` 除外,
  本目录历史文件都是中文)。
- **不要用 `rm`/`rm -f`** → 用 python `os.remove`。
- **不要提交**这 4 个"已删除未提交"项(不是我们删的):
  `comprehensive-implementation-plan.md`、`parameters-window-plan.md`、
  `fix-context-menu-click.patch`、`fix-set-home-marker-and-goto-target.patch`。
  → `git add` 必须列文件名,**不要 `git add -A`/`git add .`**。
- vite dev server 常驻 `http://localhost:5173`(dev 模式,带 Fast Refresh)。
  E2E 都直接打它;改了代码不用重启(整页加载会取最新模块)。

## 1. 已完成并推送

`git log --oneline -9`:

| commit | WP | 内容 |
|---|---|---|
| `a094ae1` | **U4** | 命令交互分级(HoldButton / ConfirmBar / 命令状态条),最新 |
| `bb0cf06` | A7 | 命令优先通道(Emergency 抢占在途 Normal) |
| `b657350` | L4 | 断开/退出统一护栏 + 关键字确认 |
| `4ace528` | L3 | 心跳丢失处理(横幅、置灰、STALE) |
| `09e6385` | L1 | STATUSTEXT → 告警中心 + 事件日志 |
| `c5c2c0b` | A1 | 生命周期监督器(disconnect/shutdown/关窗) |
| `5e4c9a2` | A3 | AlertCenter(角标/横幅/抽屉/store) |

`issues.md` N5 记录了 A3/A1/L1/L3/L4/A7/U4 的详细完成情况与验收方式,
N8 是 U4 期间发现的通用陷阱(见下文 §5)。

### U4(本轮)改了什么

- `frontend/src/design-system/components.tsx`:`HOLD_MS`、`HoldButton`、
  `ConfirmBar`、`CommandStatusStrip`、`formatElapsedMs`。
- `frontend/src/util/commandRisk.ts`(新):风险表 —— `takeoff/land/rtl/
  arm_disarm` = medium,`set_mode` = low,`modeRisk(option.confirm)` 把 core
  的 `confirm` 标记映射成 medium;high(强制解锁 L5、退出/断开 L4)仍是关键字层。
- `frontend/src/components/ConfirmContextBar.tsx`(新):确认条的数据源 ——
  模式 / 相对高度 / 电量;高度走 `cesium/heights.ts` 的同一套地面参考
  (**DEM/3D mesh 优先**,其次 HOME 平面),采样节流 2 s / 25 m。
- `frontend/src/stores/command.ts`:`sentAtMs`、`lastElapsedMs`、
  `useCommandProgress()`(在途 200 ms 刷新耗时,落地冻结在 ACK 时刻)。
- `frontend/src/components/FlightMenu.tsx`:四个命令全部改成 `HoldButton`
  (弹出层顶部加 `ConfirmContextBar`,导航条常驻 `CommandStatusStrip`);
  L3 的置灰分支保留。
- `frontend/src/components/ModePopover.tsx`:core 标 `confirm` 的 Return/Land
  改成按住发送,其余模式仍单击;顶部加 `ConfirmContextBar`。
- i18n `plan.cmd.*` / `modePopover.*`(旧的 `confirmTakeoff/...` 已删)。
- 验收:`frontend/scripts/check-command-ux.mjs`(30 条断言,全过);
  截图 `frontend/screenshots/u4/{command-menu,command-result}.png`
  (`check-command-ux.mjs <url> <shotsDir>` 会另外生成 `command-holding.png`)。

**验收口径(已实测)**:四个飞行命令逐个单击都不发送;未满窗口的按住不发送;
满窗口按住才发送且只发一条;状态条三个阶段 + 耗时正确;**RTL 从飞行视图 2 次
交互发出**;键盘按住 Enter 可发且自动重复不发第二条;模式弹层 Hold 单击即切、
Return 单击无效而按住才切。

## 2. 下一步:P0 剩余顺序

1. **L2 常驻 RTL/Hold**(依赖 A7 + U4,均已就绪):导航条上一键可达的
   RTL 与 Hold 常驻按钮。直接复用 U4 的 `HoldButton`(RTL 用 medium)+
   `ConfirmContextBar` + `CommandStatusStrip`;`send('rtl')` / `sendSetMode(AUTO,
   LOITER)` 都已存在。验收口径:任一阶段发 RTL ≤ 2 次交互(导航条常驻按钮
   应为 1 次),误触单击不发。
2. **U2 TopBar 信息层级**:三组(状态/健康/工具)、统一控件高度、单一告警角标、
   ≤1366 折叠。注意 `--mg-panel-w` = 288 px 与 `RightInspector` 写死 264 px 的
   不一致(计划 §7.1 已记)。
3. **U3 告警中心界面**:Critical 顶部横幅(全窗口)、Warn toast、抽屉过滤 +
   STATUSTEXT 控制台、分级声音 + 限时静音、`aria-live`。A3 的 store/badge/
   横幅/抽屉骨架已在,这一 WP 是把交互与分级补齐。

更远(P1/P2)见计划 §2 与 §8;`issues.md` N5 末尾记着 P0 顺序。

## 3. CI 等价命令(改完必跑)

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo test --all-targets            # 需提权(terrain_server 绑回环端口)
cargo deny check                    # 需提权(写 ~/.cargo 锁)
git diff --exit-code -- crates/core/bindings   # 有 #[ts(export)] 新类型才变
npm --prefix frontend run typecheck && npm --prefix frontend run build
cd frontend && bash scripts/check-colors.sh && npm run check:contrast \
  && node scripts/check-i18n.mjs && node scripts/check-planfile.mjs \
  && node scripts/check-mission-sync.mjs && node scripts/check-coords.mjs \
  && node scripts/check-agl.mjs && node scripts/check-progress.mjs \
  && node scripts/check-polygon.mjs
# E2E(需提权 + vite 在 5173)
node frontend/scripts/check-command-ux.mjs http://localhost:5173/ /tmp/cmdux-shots
node frontend/scripts/check-alerts.mjs     http://localhost:5173/ /tmp/alert-shots
node frontend/scripts/check-heartbeat.mjs  http://localhost:5173/ /tmp/heartbeat-shots
node frontend/scripts/check-teardown.mjs   http://localhost:5173/ /tmp/teardown-shots
node frontend/scripts/verify-terrain3d.mjs http://localhost:5173/ dem/BC-3d/tileset /tmp/terrain-shots
```

提交前 `git add <明确文件名>`,commit message 英文,进展写进 `issues.md`,
然后 `git push origin main`(需提权)。

## 4. E2E 写法(照抄 `frontend/scripts/check-command-ux.mjs` 的骨架)

- 假飞控放在 `page.addInitScript` 里:伪造 `__TAURI_INTERNALS__`(event
  listen/invoke),`window.__mock*` 钩子由 Node 驱动;`enumerate_devices` 必须
  返回 `[]`(否则 devices store 为 null 会拆掉整个外壳,见 N6)。
- **headless 下 Cesium 软件 GL 严重限流页面定时器**:所有等待都用 Node 侧
  `waitUntil(fn, timeout, step)` 轮询 DOM,不要信页面 `setTimeout` 精度。
- 命令类 E2E:`invoke('send_command'|'send_set_mode'|'send_arm_disarm')` 只记录
  到 `window.__mockCommands`,ACK 由 Node 用
  `window.__mockCommandEvent({command, kind, result})` 驱动 —— 这就是
  app-tauri `command_service` 发出的 `command` 事件载荷。
- 断言用 `data-testid`(`flight-menu`、`flight-cmd-<key>`、`mode-menu`、
  `mode-<id>`、`confirm-bar`、`command-status`、`heartbeat-banner`、
  `teardown-dialog`…),不要用文本正则找按钮。
- 截图:`/tmp/*-shots`;要进仓库的放 `frontend/screenshots/<topic>/`。
- Radix modal(如 Settings)打开后会拦 TopBar 点击 → 先 `Escape`。

## 5. 踩过的坑(别再踩)

1. **N8:弹层的"点击外部关闭"会被按下瞬间改动的 DOM 骗到。**
   `ModePopover`/`FlightMenu`/`ToolsPopover` 都在 `window` 上听 `pointerdown`
   再判 `rootRef.contains(e.target)`;而 React 的监听挂在 `#root`(冒泡更早),
   所以目标的 `onPointerDown` 先改 DOM。U4 的 `HoldButton` 起初把子节点从
   Fragment 换成字符串,React 摘掉了被按的那个 `<span>`,`e.target` 变成
   已脱离文档的节点,`contains()` 为 false → 弹层自己关了。
   现在 label 与 hint **常驻、只用 `hidden` 切换**。建议后续把这几个判断改成
   捕获阶段监听,或对 `!e.target.isConnected` 直接忽略。
2. **按住类控件的提交要用计时器,不要只靠 rAF**:被地图挤住的渲染循环会让
   rAF 迟滞;而且"按下超过窗口才松手"也应算完成(计时器可能迟到数秒),
   否则操作员真正按住 1 s 却被吞掉。
3. React 严格模式下组件会 mount→unmount→mount,dev 里看到两次 mount 属正常。
4. Vite Fast Refresh 会保留/重置组件状态,写 E2E 时不要让调试用的
   `console.log` 留在源码里(本轮已清)。
5. `data-testid` 透传到自定义组件是合法的(TS 对含连字符的属性不做检查)。

## 6. 未做/待确认(来自计划,别当成已完成)

- **H0 HRDEM 数据入口**、U1 五阶段导航、U5 飞行前清单、U6 测量运行面板、
  W0 步行仪器型号等仍待办;P1/P2 见计划 §2/§8。
- 计划 §9 的"待核实项"(MAG 仪器协议等)未动;非商业/离线优先的约束不变。
- `issues.md` 里 N1–N8 是既有问题记录,N6/N8 都值得顺手修。
