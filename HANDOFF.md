# MagGCS — Session Handoff

Snapshot of the current session (map/UAV work). Issue statuses live in
`issues.md`; keep both in sync when a task lands.

## Repo state

- Branch `main`; `origin/main == b7a77c3`.
- Pushed: **#5** (flight commands / RTL through the command session) and
  **#32** (real glTF UAV model, MAVLink-driven attitude, forward-only
  prediction).
- Uncommitted working tree (this session, not yet pushed):
  - `crates/core/src/telemetry/mod.rs` — `parse_attitude` test now asserts
    roll/pitch/yaw **and** the rate signs.
  - `frontend/src/telemetry/mock.ts` — the mock path is now self-consistent
    (derives NED velocity + heading from its own sine path); was 44 m/s actual
    vs 4 m/s reported before.
  - `frontend/src/stores/ui.ts` — added `setFollow(follow)`.
  - `frontend/src/components/MapView.tsx` — dead-reckoned marker, custom
    camera follow (Cesium `trackedEntity` removed), one-shot chase framing,
    `goHome` clears follow.

## Open work (everything raised after #5)

Ordered; nothing here is finished unless marked.

1. **UAV nose direction** — investigated this session; the dev build is
   geometrically correct (see next section). Verify against the *app build*
   before changing code.
2. **Map jitter** — "地图整体会间歇性的抖动". Dead reckoning already removes the
   4 Hz position stepping; still to confirm the residual is not a rendering
   artifact. See below.
3. **Attitude must match the incoming MAVLink `ATTITUDE`** — roll/pitch/yaw
   signs need a clean isolated re-verification (an earlier test was invalid
   because `follow` overrode the injected camera).
4. **UAV not on the ground at init / after landing** — position uses
   `relative_alt_m` and the home marker is on the ellipsoid; a landed vehicle
   should sit at 0. Re-check in SITL.
5. **Link status wrong** — FC light is red yet the UI still shows *connected*.
6. **Flight-mode display wrong** — mode pill does not show the real PX4 mode;
   during RTL it read *Acro*.
7. **Link status light is grey** in some states.
8. **Plan page mission items** look like survey/stand data, unrelated to the
   actual flight plan.
9. **Missing shutdown button** in the app.
10. **SITL preflight always fails**, "found 0 compass".
11. **UAV initial position not on the ground** on the map.

## Issue #32 follow-up — is the nose correct?

**Short answer: yes, the current code is geometrically correct in the dev
build.** `UAV_MODEL_NOSE_YAW_OFFSET_DEG = -90` aligns the asset's front with
the reported heading. Two independent checks:

- **Cesium axis correction** (verified in `Build/CesiumUnminified/index.js`,
  `ModelUtility.getAxisCorrectionMatrix`, default `upAxis=Y, forwardAxis=Z`):
  glTF `(x,y,z)` maps to body `(z,x,y)`, i.e. glTF `+Z → body +X`,
  glTF `+X → body +Y`, glTF `+Y → body +Z`. `uavQuaternion` calls body `+Y`
  the nose, so the asset's `+Z` needs a −90° yaw correction.
- **Live in-app measurement**: with the offset applied, the world bearing of
  the model's glTF `+Z` equals the injected yaw exactly (yaw 0 → 0.0°;
  yaw 90 → 90.00001°). The dashed prediction line and the nose agree.

**Asset geometry (corrected).** The glTF root node `Sketchfab_model` carries a
`matrix` (rotation −90° about X + uniform scale 4.0684); an earlier analysis
that ignored `node.matrix` reported wrong coordinates. Corrected world bounds:
`x ∈ [-0.52, 0.52]`, `y ∈ [0, 0.27]`, `z ∈ [-0.39, 0.41]` (~1.03 m × 0.27 m ×
0.79 m). The four arm/motor pods are at `(±0.25, 0.09, ±0.25/+0.23)`, props at
`(±0.34, 0.23, ±0.30/-0.28)`, and the **only** centerline mesh below the body
is the camera gimbal (`Object_183`) at `(0, 0.05, +0.17)`. So the camera sits
under the front and the X-frame's two front arms bisect `+Z`: gimbal and
X-frame forward are the same axis.

**Rendered confirmation.** In an isolated Cesium overlay (north = red arrow,
east = green arrow) the model reads as an X-config quad with the camera lens
facing `+Z`; in the app at yaw 90 with eastward velocity the gimbal tab points
east, matching the east-pointing prediction line.

**So the user-visible "机头方向不对" is most likely one of:**
- running a **stale app build** (`frontend/dist` is current — rebuilt 01:33,
  newer than the sources — but the Tauri binary may predate it); or
- a different intended "front" than the asset's camera end (the user said not
  to use the gimbal; note that for this asset it gives the same axis).
Do **not** flip the offset until the app build is rebuilt and re-checked.

## Map jitter — current approach

`MapView.tsx` dead-reckons the marker between fixes (cap `DEAD_RECKON_MAX_S =
0.5 s`) and re-centres with a hand-rolled chase instead of `viewer.trackedEntity`
(whose offset comes from the bounding sphere, which `minimumPixelSize` makes
view-dependent → oscillation). Measured in dev: camera step p95 0.62 m vs
9.5 m before. Residual jitter still needs to be reproduced on the **Tauri app**
(at the real feed rate), not only in the dev browser.

## Verification tooling (sandbox notes)

- Port binding and any localhost network call need escalation.
- `frontend/dist` is what the Tauri app loads (`frontendDist:
  ../../frontend/dist`) — after frontend edits run `cd frontend && npm run build`
  before rebuilding/relaunching the app.
- Dev tooling used for measurement (both still running this session):
  - Vite dev server: `cd frontend && npm run dev -- --port 8731 --strictPort
    --host 127.0.0.1`.
  - Headless Chromium over CDP on port 9226, plus the `/tmp/cdp9.mjs` helper
    (`eval` / `shot` / `metric` modes); `/tmp/expr-setup.js` injects known
    snapshots and camera modes.
- Isolated model inspection: `/tmp/analyze2.mjs` (per-mesh bounds honouring
  `node.matrix`), and an overlay Cesium viewer with north/east arrows.

## Verification commands

- `cargo clippy --all-targets -- -D warnings`, `cargo test -p maggcs-core`,
  `cargo test -p maggcs-app`
- `cd frontend && npm run typecheck && npm run check:colors &&
  npm run check:contrast && npm run build`
- SITL: `scripts/sitl/run_sitl_docker.sh` (PX4 v1.17, container `px4-sitl`).
