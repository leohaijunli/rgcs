# MagGCS — Session Handoff

Snapshot of the current session (issues.md backlog clearance). Issue statuses
live in `issues.md`; keep both in sync when a task lands.

## Latest landing — Settings -> Vehicle: initial vehicle position

`Settings -> Vehicle` edits the initial vehicle position (lat/lon), persisted in
`localStorage` (`maggcs.initialPosition`, default 48.6493/-123.3982 from
`cesium/constants.ts`). It drives the mock feed origin, the HOME marker and the
initial camera, and `goHome()` flies back to it. Out-of-range input is refused
with an inline error, and `Use map centre` copies the current map centre.

- Code: `desktop/prefs.ts` (state + validation), `cesium/scene.ts`,
  `cesium/entities.ts`, `hooks/useCesiumViewer.ts` (camera, HOME, go-home),
  `telemetry/mock.ts` (mock origin), `components/dialogs/SettingsDialog.tsx`.
- Verified with Playwright against `frontend/dist`: invalid latitude is
  rejected without persisting, a valid position persists and survives a
  reload, no console errors. Screenshots:
  `frontend/screenshots/dark-settings-vehicle-{1920x1080,1366x768}.png` and
  `...-invalid-1920x1080.png`.
- Rebuild `frontend/dist` *and* the app before relaunching: the Tauri binary
  embeds the frontend at compile time.

## Latest landing — issues.md backlog cleared

The whole `issues.md` backlog (P0 #1–#9, P1 #10–#17, P2 #18–#26, P3 #27–#32)
is implemented in the working tree; the only deliberately skipped items are the
UAV-model refinements the user asked to defer. Highlights:

- **#17** lossless link-error channel: `TelemetryError { kind, message, at_ms }`
  on a `broadcast` (cap 64) plus a dropped-frame `watch`; surfaced in the
  frontend as an error banner with history.
- **#18** `MissionService` moved into `core::mission::service`; app-tauri is now
  a thin `app.emit` adapter (headless-reusable).
- **#19/#20** `TelemetryHub` pump is event-driven `select!`; new
  `core::mavlink::router` (`MessageRoute` / `RoutedEvents`,
  `ConnectionHandle::subscribe_route`) so hub/mission/command filter by
  sysid/compid + message id in the connection layer.
- **#21** app state is multi-vehicle ready (`LinkId` / `ActiveLink` /
  `BTreeMap` + `primary`); `connect`/`disconnect`/`shutdown_app` run under
  `AppState::ops()` and connect swaps links atomically.
- **#22–#24** Tauri CSP + least-privilege capabilities, CI `deny` job
  (`cargo deny check`, new `deny.toml`), ts-rs sync check, frontend
  `npm audit --omit=dev`.
- **#26** docs/ADRs reconciled: `docs/DEVELOPMENT_PLAN.md` is English v2.2
  (the `_v2.1` copy is deleted), new `docs/adr/011-link-state-model.md`,
  ADR-001/002/006/010 → Accepted, ADR-007 → Apache-2.0 with an Apache-2.0
  `LICENSE`, `AGENTS.md` layout refreshed.
- **#27/#28** prominent MOCK DATA banner; `docs/design/telemetry-channels.md`
  freezes the Phase 4 high-rate channel design (keep 20 Hz JSON state, add a
  Tauri `Channel` binary batch + frontend ring buffer).
- **#29** `MapView.tsx` split from 538 → 46 lines into
  `frontend/src/cesium/{constants,uav,scene,entities,waypoints,follow}.ts` +
  `hooks/useCesiumViewer.ts` (see `docs/design/map-view-modules.md`).
- **#30** `vite.config.ts` no longer copies all of Cesium `Assets`; the default
  imagery (`NaturalEarthII`), maki pins and water-normal maps are dropped.
  `dist/cesium` 7.7 MB → 6.2 MB, `dist` 23 MB → 19 MB.
- **#31** device hotplug watcher no longer aborts on an enumeration failure
  (`spawn_watcher` returns `DeviceManagerHandle`, `hotplug_events()` extracted
  and unit-tested).

Verified: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p maggcs-core` (lib 105 + integration all green), `cargo deny
check`, `cd frontend && npm run typecheck && npm run check:colors &&
npm run check:contrast && npm run build`. A Playwright headless-Chromium load of
the built app showed no console errors and no failing requests.

## Continuation — Phase 0 addendum 0.5–0.9 and Phase 1 wrap-up

After the backlog commit (`c01afcf`), the remaining `docs/DEVELOPMENT_PLAN.md`
Phase 0 wrap-up items and the Phase 1 gap were closed:

- **0.5** Connection page now shows the four-level link state
  (`settings.linkState.*`, derived by `util/linkLevel.ts` from the transport
  state + `fc_alive` + whether any telemetry field has arrived), the
  last-packet age, dropped-frame count, the endpoint and the recent link
  errors — so "are packets arriving?" is visible at a glance.
- **0.6** new `frontend/scripts/check-i18n.mjs` (`npm run check:i18n`, wired
  into CI) bundles `src/i18n/en.ts` with esbuild, flattens it and fails on any
  `t('...')` key that does not exist (plurals and `t(`prefix.${x}`)` templates
  handled). It immediately found 8 missing `settings.*` keys, now filled in:
  187 keys, 16 dynamic prefixes, 0 missing.
- **0.7** `ConnectDialog` deleted; the top-bar FC pill and the gear both open
  the single Settings dialog on the Connection tab; the dialog has a fixed
  height (`h-[min(85vh,560px)]`) so tab switches do not resize it.
- **0.8** structured endpoint form (`desktop/endpoint.ts`): type select
  (UDP listen/client, TCP server/client, serial), address + port or serial
  dropdown (from `enumerate_devices`) + baud, presets (PX4 SITL/QGC 14550,
  QGC forwarding 14551, serial telemetry), inline field validation. The last
  endpoint and the auto-connect toggle persist in `localStorage`
  (`desktop/prefs.ts`) and `desktop/bridge.ts` honours the toggle.
- **0.9** all Connection controls are `h-11` + `touch-target` (≥ 44 px); link
  errors are listed inline in the dialog instead of only behind the overlay.
- **Phase 1** Pause/Continue added: `send_command("pause"|"continue")` maps to
  `MAV_CMD_DO_PAUSE_CONTINUE` with param1 0/1 (unit-tested in
  `crates/app-tauri/src/commands.rs`); the flight-view buttons arm on the first
  click for the disruptive ones. PX4 v1.17 semantics still need a SITL check
  (a NACK shows as "Not supported by the FC").

**Phase 2 started**: `core::rtk::rtcm` (RTCM3 preamble/length/CRC-24Q parsing
plus a resynchronising streaming `RtcmFramer`) and `core::rtk::forward`
(`GPS_RTCM_DATA` fragmentation: ≤ 180 bytes, ≤ 4 fragments, sequence ids) are
implemented and unit-tested (24 tests; the CRC matches the standard
"123456789" = 0xCDE703 check value and a golden frame computed by an
independent implementation). Still to build: `RtcmSource` (serial base),
injection service + send path, u-blox base-station config, status alarms/UI.

Verified: `npm run typecheck`, `check:colors`, `check:contrast`, `check:i18n`,
`build`; `cargo fmt --check`, `clippy -D warnings`, `cargo test -p maggcs-core`
(105 lib) and `-p maggcs-app` (9, incl. the new command mapping), `cargo deny
check`. A headless-Chromium pass confirmed the form renders, presets apply, an
out-of-range port shows the inline error and disables Connect, the FC pill
opens the Connection tab, no raw i18n keys appear and the console is clean.

## Repo state

- Branch `main`, pushed to `origin/main`. Two landings since `abe03cc`: the
  backlog commit `c01afcf` and the Phase 0 addendum / Phase 1 wrap-up commit
  on top of it — no uncommitted changes are expected after them.

## Open work

1. **UAV model items — skipped by request.** HANDOFF items 1–4 ("UAV nose
   direction", "map jitter", "attitude must match `ATTITUDE`", "UAV on the
   ground") and issues.md #32 are considered done/deferred; do not reopen
   without the user asking.
2. **SITL re-verification of issues.md #6/#9 root-cause fixes.** Items 5–7
   (link status light) and 8 (plan page items) have a code fix but still need
   a SITL + real-QGC re-check on the app build.
3. **HANDOFF item 9 (shutdown).** The in-app Shut down control exists; confirm
   the intent (quit MagGCS vs. a vehicle/FC shutdown command).
4. **HANDOFF item 10 (SITL preflight "found 0 compass")** and **item 11 (UAV
   initial position on the ground)** — SITL-side, not yet reproduced here.
5. **Continue `docs/DEVELOPMENT_PLAN.md`.** Phases 0 and 1 are implemented and
   their acceptance is covered against the fake FC. **Phase 2 (RTK, §8)** is
   started: RTCM3 framing + `GPS_RTCM_DATA` fragmentation are done; next is the
   `RtcmSource` trait, a serial base-station source, the injection service over
   the FC link, and the base/onboard status UI. A live SITL pass for the Phase 0
   numeric acceptance and the Phase 1 upload/download/pause items is still
   outstanding.
   Execution order is §11.8: MAVLink & telemetry → UI shell → Mission protocol
   → waypoint editing → RTK → DEM & height → Survey → mag & QC.

## Reference — UAV nose analysis (issues.md #32)

`UAV_MODEL_NOSE_YAW_OFFSET_DEG = -90` is geometrically correct: Cesium maps the
asset's glTF `+Z` onto the body `+X` axis (`ModelUtility.getAxisCorrectionMatrix`,
default `upAxis=Y, forwardAxis=Z`), and `uavQuaternion` calls body `+Y` the
nose. Measured in-app: model glTF `+Z` world bearing equals the injected yaw
(yaw 0 → 0.0°, yaw 90 → 90.00001°). The asset's gimbal (`Object_183`) sits
under the front on the same centreline as the X-frame's forward bisector.
**Do not flip the offset** without a fresh measurement.

## Reference — desktop window rendered blank/black (2026-10-07)

Symptom: the Tauri window came up showing only the flat `--mg-bg` colour while
the process, WebKitGTK and the compositor were healthy.

Root cause: the CSP. `script-src 'self'` blocked CesiumJS during module
evaluation — WebAssembly instantiation (needs `'wasm-unsafe-eval'`) and
knockout's `new Function` binding parser used by `Cesium.Viewer`'s DOM (needs
`'unsafe-eval'`). The bundle threw, React never mounted, and only the CSS
background was painted. Fixed in `crates/app-tauri/tauri.conf.json`; decision
recorded in ADR-012.

Do not chase WebKitGTK DMA-BUF / NVIDIA compositing first: the same
`frontend/dist` renders in a plain WebKitGTK harness over HTTP (no CSP), and the
window does paint its CSS background, so the GPU path is fine. The
`WEBKIT_DISABLE_DMABUF_RENDERER=1` workaround that was added on that wrong
hypothesis is removed — it only disabled accelerated compositing.

Diagnosing a blank window: the page state can be read from the webview by
injecting a script that reports `#root` children / canvases over a Tauri
command, e.g. `[DIAG] rootKids=1 canvas=1 cesium=1` means the page booted. If
`rootKids=0`, look for CSP errors (`Refused to create a WebAssembly object`,
`Refused to evaluate a string as JavaScript`) before touching the GPU stack.

## Known flakes and tooling notes

- `crates/core/tests/command_service_integration.rs` occasionally fails when
  the whole suite runs concurrently (UDP port contention); it passes
  standalone. Pre-existing, not caused by recent changes.
- `frontend/scripts/screenshot.mjs` needs Playwright, which is not in
  `devDependencies`. Install transiently:
  `npm i --no-save --no-package-lock playwright && npx playwright install chromium`.
  The committed baselines are stale for this environment: the same build
  compared against them differs ~38–47%, and two consecutive runs of one build
  differ by up to 25% (`dark-planning-1920x1080`, OSM tiles + mock feed). Treat
  the pixel diff as noise and watch **console errors / failing requests**
  instead.
- `frontend/dist` is what the Tauri app loads (`frontendDist: ../../frontend/dist`)
  — rebuild the frontend before rebuilding/relaunching the app.

## Verification commands

- `export PATH="$HOME/.cargo/bin:$PATH"`
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`
- `cargo test -p maggcs-core` (and `-p maggcs-app`), `cargo deny check`
- `cd frontend && npm run typecheck && npm run check:colors &&
  npm run check:contrast && npm run build`
- SITL: `scripts/sitl/run_sitl_docker.sh` (PX4 v1.17, container `px4-sitl`).
- Desktop window (ADR-012): `cargo test -p maggcs-app` pins the CSP directives,
  and `scripts/desktop-smoke.sh` launches the real binary on a hidden Hyprland
  headless output and fails if the window does not paint.
