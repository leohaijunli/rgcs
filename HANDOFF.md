# MagGCS — Session Handoff

Snapshot of the current session (issues.md backlog clearance). Issue statuses
live in `issues.md`; keep both in sync when a task lands.

## Latest landing — map height reference fixed, drop line grounded

The operator reported the vehicle marker flew **below the drawn flight-plan
trajectory** and the AGL "drop line" never appeared (issues.md #40):

- Root cause: the plan, its labels and its height sticks are drawn in **AMSL**
  (`MapView.toDisplayItems` adds HOME's AMSL), but the vehicle marker was drawn
  at PX4's `relative_alt` (0 = HOME) — so on a site at ~115 m MSL a 30 m AGL
  flight hung the marker ~115 m below the path. A leftover `+5 m` lift
  on the drawn path/points/sticks pushed the trajectory another 5 m above the
  true altitude.
- `useCesiumViewer.setSnapshot` now places the marker at `HOME AMSL +
  relative_alt_m`, and `entities.ts::updateDropLine` takes explicit top/bottom
  metres so the drop line runs **from the vehicle down to the plan's ground
  plane** (HOME AMSL), labelled with the AGL. The `+5 m` lift was removed from
  `renderWaypoints`/`previewWaypoint` so the path sits exactly at the planned
  altitude. HOME's marker and the add-tool ghost stand on the same ground plane
  and follow the store's `home`.
- Companion fix (9a3e355): AGL now compiles to `MAV_FRAME_GLOBAL_RELATIVE_ALT_INT`
  instead of the terrain frame PX4 v1.17 rejects on upload; a foreign
  terrain-frame `.plan` still reads back as AGL. This is exactly the relative
  frame, so the AGL mode becomes a true AGL profile once WS-D drapes the path.

Verified: frontend typecheck/build, `check:planfile/i18n/progress/
mission-sync/coords`.

## Next — irregular-boundary sweep + adjustable pattern centre (agreed 2026-10-08)

From the operator review: "how do I adjust the cloverleaf/sweep centre, and how
do I sweep an irregular (concave) field?" The agreed decomposition (also in
`docs/DEVELOPMENT_PLAN.md` Phase 3) — land one step at a time:

- **B1** `core::survey`: replace `clip_to_convex` with a simple-polygon (concave
  OK) line clipper that returns several segments per crossing line;
  `build_parallel_lines` emits one `PatternLine` per segment; drop the
  convexity check and add `SurveyError::SelfIntersectingPolygon`.
- **B2** golden-geometry tests (L-shape / U-shape: segments, lengths, line
  table, fly order).
- **B3** polygon map tool (`stores/ui.ts` `mapTool: 'polygon'`,
  `cesium/tools/polygon.ts`): click vertices, double-click / first-vertex
  closes, drag vertices, self-intersection flagged, area/perimeter readout;
  ADR-014 to be recorded.
- **B4** `PatternPanel` generates the sweep from the drawn polygon.
- **A1** `InsertedPattern` remembers centre + params; `PatternPanel` gains lat/lon
  centre inputs + "use map centre".
- **A2** draggable centre handle on the map (cloverleaf `center`, sweep rectangle
  centre), regenerate preview; one drag = one commit.

## Latest landing — flight progress, line colours, heights, wire-precision sync

Five operator reports, all in the map/plan/mission readout (issues.md #35-#39):

- **Tie vs spacing lines are now visibly different** and the height of every
  waypoint can be read off the map. `mission/lineKinds.ts` (`kindsBySeq`,
  `splitRuns`, `heightRuns`) turns `PatternLine[]` into per-seq kinds; the map
  draws one polyline per same-kind run (survey = accent, tie = warn width 3,
  calibration = mag) and, with the heights toggle on, a muted stick **from the
  ground (HOME AMSL) up to the waypoint** plus a `seq · N m` label.
- **The Layers panel does something**: the two layers the map really has (OSM
  imagery, offline grid) toggle through `stores/ui.ts` →
  `useCesiumViewer::setLayers` → `scene.ts::applyLayerVisibility`; DTM/DSM are
  listed as "arrives with the DEM (Phase 3)" instead of pretending to be
  switches. The Missions drawer shows the real plan (count, sync status,
  current WP, pattern lines) instead of three hardcoded rows.
- **The waypoint list means something while flying** (#37): `mission/geo.ts`
  (haversine + bearing) and `mission/progress.ts` (`missionProgress`, with
  `formatDistance`/`formatDuration`) turn `MISSION_CURRENT` + the latest fix
  into active-WP index, distance/bearing to it, target altitude, remaining path
  and ETA. `panels/MissionProgressCard.tsx` renders it in the flight inspector
  and in the drawer's flight list; the planning list marks the FC's actual
  current waypoint (it used to badge seq 0) and shows each waypoint's distance
  from the vehicle.
- **Selecting a waypoint shows it on the map** (#39): a white halo rings the
  selected point, and list clicks call `focus(seq)` — a one-shot store request
  the Cesium layer consumes by flying to the waypoint, so a map pick never
  moves the camera. Fixed alongside it: the drag preview still updated a
  `layer.line` that no longer exists after the per-run polylines landed;
  `WaypointLayer.runs` now records each run's seqs and is redrawn on preview.
- **"FC mission differs from the uploaded plan" was a false alarm** (#38): `z`
  and `params` are **f32** on the wire (`core::MissionItem` is `Vec<f32>`/`f32`)
  but the frontend compiled f64, and `itemsHash` compared exactly — so any
  altitude that is not f32-exact (an imported HOME + 50 m, say) mismatched on
  every read-back. `compile.ts::wire()` and `hash.ts` now narrow with
  `Math.fround`.

New guard: `npm run check:progress` (16 scenarios: sphere geometry, cardinal
bearings, no-MISSION_CURRENT / no-fix / unknown-seq / empty plan, ETA and
formatting). `check:planfile`, `check:mission-sync` (16), `check:coords` and
`check:progress` are now wired into CI, where before only typecheck/i18n ran.

Verified: frontend typecheck/colors/contrast/i18n/planfile/mission-sync/coords/
progress/build; `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p maggcs-core --lib` (167), `cargo test -p maggcs-app`,
`cargo deny check`, `scripts/desktop-smoke.sh` (PASS, window paints).

## Latest landing — command items are sent as MAV_FRAME_MISSION

The operator's upload of a generated sweep failed with
`mission ack denied (type 3)` — PX4's `MAV_MISSION_UNSUPPORTED` (issues.md #34).

- Root cause: `compile` gave **every** item the mode's global frame, but PX4
  (`mavlink_mission.cpp::parse_mavlink_mission_item`) only accepts a global
  frame for a whitelist of commands; `DO_CHANGE_SPEED` — which the sweep preset
  inserts at seq 0 whenever `speed_mps` is set — must be `MAV_FRAME_MISSION`.
- `core::mission::command_uses_coordinate` + the TS mirror
  `compile.ts::commandUsesCoordinate` hold the list; `MissionFrame` gained
  `Mission` (`MAV_FRAME_MISSION`), which also makes a *download* of a plan with
  DO items work (frame 2 used to be `UnsupportedFrame`).
- Non-coordinate commands compile to the mission frame and keep their stored
  `z` (a command argument, not an altitude). `.plan` interop maps frame 2,
  imports coordinate-less command items instead of dropping them, and no longer
  derives HOME from a command item.
- Still open, same root cause: AGL mode compiles to `GLOBAL_TERRAIN_ALT_INT`,
  which PX4 also rejects — terrain following belongs to the ground station
  (ADR-005/WS-D).

Verified: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p maggcs-core --lib` (167), frontend
typecheck/colors/contrast/i18n/planfile/mission-sync/coords.

## Latest landing — mission sync state, local Clear plan, pattern replace

The operator reported that clicking Upload kept showing "Unsaved changes — not
the FC plan" with no way to drop the plan (issues.md #33):

- `mission/sync.ts::planSyncStatus` replaces the bare `dirty` flag with one
  status the panel renders: `empty` / `unsynced` ("Local plan — not on the FC
  yet") / `dirty` ("Unsaved changes") / `mismatch` (red) / `synced` ("In sync
  with the FC"). A plan that was simply never uploaded no longer claims to
  differ from the FC, and a completed upload now says so.
- `stores/mission.ts::clearPlan` discards the **local** plan (waypoints, complex
  blocks, home, baseline) without a link; the panel's `Clear plan` button is
  two-click confirmed and sits next to Import/Export. The FC's button is renamed
  `Clear FC`, and Upload/Clear FC explain themselves when disabled.
- Two state-machine holes: a `failed` event now resets `verifying` (a lost
  read-back used to swallow the next download), and `linkLost` (called from
  `bridge.ts` when `fc_alive` goes false) clears `busy`/`verifying` — the
  mission service task exits with its connection, so no terminal event would
  ever arrive and Upload stayed disabled forever.
- Re-clicking Generate replaces the previously generated preset trajectory
  instead of stacking a duplicate (`InsertedPattern.count`; any manual edit or
  an import releases the block, so those paths still append).

`npm run check:mission-sync` (new) bundles the real store with esbuild and
drives it through the upload/download event sequences — 15 scenarios covering
upload success, upload failure, mismatch, link loss, clear, and pattern
replace.

Verified: frontend typecheck/colors/contrast/i18n/planfile/mission-sync/build,
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p maggcs-core --lib` (163), `cargo test -p maggcs-app`,
`cargo deny check`, `scripts/desktop-smoke.sh` (56.5% bright, PASS).

## Latest landing — map click-to-add waypoints (WS-G part 1)

The planning map can now create waypoints directly (finding 13):

- `stores/ui.ts`: `mapTool: 'select' | 'add'`; `setMapTool`; `Esc` and leaving
  the planning view reset it to `select`.
- `cesium/waypoints.ts::installMapTools` (renamed from `installWaypointDrag`)
  handles both tools in one `ScreenSpaceEventHandler`: select/drag as before,
  plus add — a press/release <= 3 px picks the ground and appends a waypoint
  while a drag pans the camera (finding 17), and a ghost point follows the
  cursor (`entities.ts::createGhostPoint`). The canvas shows a crosshair while
  the tool is active.
- `stores/mission.ts::addWaypointAt` inherits the previous waypoint's AMSL
  altitude (falls back to home + 50 m).
- `MapToolbar` gains an Add-waypoint button (planning view only) and a hint bar
  shows "Click the map to add a waypoint · Esc to finish".

Still open in WS-G: polygon tool, midpoint insert handles, context menu,
coordinate entry, measure, snapping, profile linkage, touch targets.

Verified: frontend typecheck/colors/contrast/i18n/planfile/build,
`cargo test -p maggcs-app`, `scripts/desktop-smoke.sh` (53% bright, PASS).

## Latest landing — preset pattern UI (planning view)

`PlanningPanel` now embeds a `PatternPanel` (Presets): pick Survey sweep or
Cloverleaf, edit the parameters, and Generate appends the geometry to the plan.

- Tauri commands `survey_generate_sweep` / `survey_generate_cloverleaf`
  (`crates/app-tauri/src/commands.rs`) call `core::survey`; pure geometry, no
  link required.
- `frontend/src/mission/patterns.ts`: typed `invoke` wrappers, a rectangle
  builder (mirrors `core::survey::LocalProjection`) and default parameters.
- The sweep's polygon is a rectangle around the current map centre (the polygon
  tool is WS-G); move the map before generating.
- `stores/mission.ts`: `insertPattern(plan, label)` appends the generated
  waypoints and keeps the `seq -> line` table for the readout (cleared by any
  edit).
- `check:planfile` now covers the TS rectangle/defaults as well.

Verified: frontend typecheck/colors/contrast/i18n/planfile/build, `cargo fmt`,
`cargo clippy --all-targets -- -D warnings`, `cargo test -p maggcs-app`,
`scripts/desktop-smoke.sh` (54% bright, PASS).

## Latest landing — preset parameterised patterns (`core::survey`)

New `crates/core/src/survey` generates complete trajectories from parameters,
so the planner can offer presets rather than only hand-placed waypoints:

- `SurveyPattern` — parallel-line sweep clipped to a convex polygon, with
  optional perpendicular **tie lines**, lead-in/lead-out and an alternating
  (serpentine) option; `speed_mps` prepends a `DO_CHANGE_SPEED`.
- `CloverleafPattern` — N-petal rose (`petals` even >= 4) for calibration
  flights; generates a closed, smooth path for magnetometer calibration.
- `PatternPlan` carries AMSL `PlannedWaypoint`s plus a `PatternLine`
  (`seq` range + kind + length) table for post-flight line segmentation.
- `survey::LocalProjection` is a dependency-free local tangent-plane projection
  (sub-metre over a survey block); UTM 10 lands with the `geo`-crate clipper in
  WS-C. Non-convex polygons fail closed (`SurveyError::NonConvexPolygon`).

Tests (10): clipped lengths, tie-line counts, serpentine reversal, speed
prepend + line-table shift, non-convex/parameter rejection, cloverleaf closure
and radius bound, projection round trip. ts-rs bindings exported and copied to
`frontend/src/generated-types/`.

Verified: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p maggcs-core --lib` (163 passed), `cargo deny check`.

Next: the frontend preset UI (parameter form → generate → insert as a
read-only block), then DEM draping.

## Latest landing — frontend migrated onto the planned (AMSL) model

The mission store now holds planned waypoints in absolute AMSL
(`PlannedWaypoint`/`Height`, ADR-013) instead of `MissionItem`s whose `z`
depended on the selected mode:

- `stores/mission.ts`: `waypoints: PlannedWaypoint[]`; new
  `updatePosition`/`updateAltitude`/`setWaypoints`; `compiled()`/`flyable()`
  compile to the current frame; `upload` and the sync hash use `flyable()`.
  Downloads/imports convert back to AMSL with `waypointFromItem`.
- `mission/compile.ts`: TS mirror of `core::plan` (`compileWaypoints`,
  `frameToAmsl`, `waypointFromItem`, `FRAME_BY_MODE`). AGL is frontend-only
  (terrain frame, flat-ground assumption) until a DEM lands.
- Switching the altitude mode is now lossless and instant (it only changes the
  compile target), so the finding-1 confirmation bar and
  `mission/altitude.ts` were removed.
- `PlanningPanel` edits lat/lon/AMSL and shows the compiled/flyable count and
  read-only complex-item blocks; `MapView` and the drag handler use the
  compiled list (`updatePosition`).

Verified: `npm run typecheck`, `check:colors`, `check:contrast`, `check:i18n`,
`check:planfile` (Survey round trip + compile/datum cases), `npm run build`,
`cargo test -p maggcs-app`, `scripts/desktop-smoke.sh` (53% bright, PASS).

Known transitional gap: complex-item children keep the frames stored in the
QGC file and are not re-compiled on a mode switch (documented in ADR-013).

## Latest landing — geoid grid interpolation (finding 12 groundwork)

`crates/core/src/height/grid.rs` gives `core::height` a working `GeoidModel`:

- `GeoidGrid { spec, values }` + `GeoidGridSpec` describe a regular
  latitude/longitude grid (row-major, lat-first) of undulation values.
- `GeoidGrid::new`/`constant` validate shape, positive steps, `nlat*nlon`
  length and finiteness; malformed input returns `HeightError::InvalidGrid`.
- `GeoidGridModel` holds one grid per `HeightDatum` and interpolates
  bilinearly; a query outside coverage returns
  `HeightError::UndulationUnavailable` (never extrapolated, matching ADR-004),
  and a datum without a grid returns `HeightError::ModelNotLoaded`.
- `HeightError` gained the `InvalidGrid` variant.

Tests cover constant/linear fields, corner/edge nodes, out-of-coverage and
missing-datum failures, invalid grids, and the AMSL→ellipsoid→AMSL chain
through a grid. The real EGM96/CGG2013 grids (and the COG reader) land in
Phase 3, which is the only remaining part of finding 12.

Verified: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p maggcs-core --lib` (149 passed), `cargo deny check`.

## Latest landing — `core::plan` (WS-B, ADR-013)

New planning model, separated from the wire model:

- `crates/core/src/plan/{mod,types,error}.rs`: `PlannedMission { home, waypoints,
  blocks, meta }` with datum-tagged AMSL `Height` altitudes, and
  `PlannedMission::compile(FramePolicy)` producing `MissionItem`s.
  `FramePolicy::{GlobalInt, GlobalRelativeAltInt}` differ only in `z`/`frame`
  (`GlobalInt` = AMSL `z`, relative = AMSL − HOME AMSL); terrain-alt frames are
  never emitted (ADR-005). `PlanError` rejects non-AMSL inputs, out-of-range
  coordinates, and >65535 items.
- `docs/adr/013-plan-height-policy.md` records the decision (ADR-012 is the
  desktop CSP, so the map-tools ADR becomes ADR-014).
- ts-rs bindings exported and copied to `frontend/src/generated-types/`.
- Acceptance test: one plan compiles to both frames with identical `x`/`y`/`seq`
  and `z` differing by exactly the home altitude.

Not yet wired: the frontend store still holds `MissionItem`s in the selected
mode and converts via `frontend/src/mission/altitude.ts`; migrating it onto
`PlannedMission` is the follow-up (tracked in the ADR).

Verified: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p maggcs-core --lib` (141 passed), `cargo deny check`.

## Latest landing — improve_plan findings 1/3/4/5/14–19

Hardening of the Phase 1 planning path from `improve_plan.md` (verified
findings). Finding 12 (`core::height` has no `GeoidModel` implementation) is a
WS-D project needing EGM96/CGG2013 grid data + ADR-013 and is left for Phase 3.

- **finding 3/4/5, lossless `.plan` (WS-A5)**: `mission/planfile.ts` keeps QGC
  complex items (Survey, …) as opaque `PlanBlock`s (`raw` written back
  verbatim) instead of flattening them; `orderedMissionItems()` derives the
  flyable order from `base.mission.items`; child waypoints get a synthetic
  `seq` base (`BLOCK_SEQ_BASE`) so map/selection ids never collide; unset
  (`null`) params are coerced to 0; `mission/hash.ts::itemsHash` drops `seq` and
  `current`. `stores/mission.ts` carries `blocks` and uploads the flyable
  order; `PlanningPanel` shows blocks as read-only rows and exports them.
- **finding 1, datum-aware mode switch**: new `mission/altitude.ts` converts
  `z` between relative/AMSL/AGL (anchor = `plannedHomePosition` AMSL; ground
  assumed flat at the home altitude until a DEM lands, WS-D). Switching the
  mode with items present now shows a confirm bar (Convert / Keep numbers /
  Cancel) instead of silently reinterpreting `z`.
- **finding 14/15/17/18, drag + safety**: `cesium/waypoints.ts` previews on
  `MOUSE_MOVE` and commits once on release (one undo entry, one `dirty`), a
  press/release ≤ 3 px (`CLICK_DRAG_THRESHOLD_PX`) selects instead of moving,
  and camera rotate/translate are restored from a shared `finish()` also wired
  to `window` `pointerup`/`blur` and `Escape`. Editing is gated to the planning
  view (`enabled: () => ui.view === 'planning'`).
- **finding 16, terrain-aware pick**: new `cesium/pick.ts::pickLatLon` uses
  `globe.pick` when a real terrain provider is loaded, else `pickEllipsoid`;
  returns `null` for the sky.
- **finding 19, North**: `MapToolbar`'s compass now calls
  `useCesiumViewer.lookNorth()` (3D only, stops follow first).

Verified: `frontend` typecheck/colors/contrast/i18n + new
`npm run check:planfile` (esbuild+Node: Survey round-trips byte-identical,
null params, hash identity, datum round-trips) + `npm run build`;
`cargo test -p maggcs-app`; `scripts/desktop-smoke.sh` (53% bright, PASS).

## Latest landing — SITL spawns at the Settings -> Vehicle position

The SITL world origin now follows the operator's Settings -> Vehicle initial
position: the desktop shell mirrors `maggcs.initialPosition` into
`sitl-home.json` in the app config dir (`~/.config/io.maggcs.desktop/`, or the
Windows AppData dir under WSL2), and `scripts/sitl/sitl-home.sh` resolves it
for PX4 (env overrides win, then the file, then the built-in Sidney BC default).
The format is covered by a unit test (`commands::tests::sitl_home_file_round_trips`).

- Code: `desktop/prefs.ts` (`syncSitlHome`), `desktop/bridge.ts` (sync on
  startup), `crates/app-tauri/src/commands.rs` (`set_sitl_home`),
  `scripts/sitl/sitl-home.sh`, `scripts/sitl/run_sitl_docker.sh`.
- Verified: `cargo test -p maggcs-app` (10 passed), `npm run typecheck`,
  and `bash scripts/sitl/sitl-home.sh` (default / env / file / corrupt-file
  cases).

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
