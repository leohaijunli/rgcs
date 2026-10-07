# MagGCS — Session Handoff

Snapshot of the current session (issues.md backlog clearance). Issue statuses
live in `issues.md`; keep both in sync when a task lands.

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

## Repo state

- Branch `main`. This landing is the first commit since `abe03cc`; it is
  committed and pushed (see the commit message) — no uncommitted changes are
  expected after it.

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
5. **Continue `docs/DEVELOPMENT_PLAN.md`.** Issues are clear; next is the plan
   remainder — finish/verify the Phase 0 addendum 0.5–0.9 acceptance and the
   Phase 1 mission-protocol acceptance, then Phase 2 (RTK + RTCM forwarding).
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
