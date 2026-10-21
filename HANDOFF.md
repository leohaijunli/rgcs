# MagGCS — Session Handoff (2026-10-21)

Snapshot of the 2026-10-20/21 session (plan `docs/plans/lifecycle-survey-hrdem-plan.md`
work packages). Issue statuses live in `issues.md`; the plan file holds the
per-WP execution records and the authoritative status table.

## Repo state

- Branch `main`; one commit per work package, pushed as it landed
  (`23b1196` … `e77de57`; the newest commit also fixes the detached map
  window's Cesium boot).
- Automated verification at handoff: **core 721 lib tests, app 86,
  clippy `-D warnings`, `cargo fmt --check`**; frontend **`tsc` +
  production build + i18n (728 keys) + 14 static check scripts**;
  **13 Playwright scripts green** (`check-startup` 14, `check-preflight`
  29, `check-safety` 22, `check-command-ux` 30, `check-topbar` 28,
  `check-heartbeat` 24, `check-alerts` 18, `check-alert-ux` 25,
  `check-teardown` 22, `check-field-mode` 6, `check-map-window` 10,
  `check-layers` 14, `check-surface` standalone).
- Known repo hygiene item: `survey/` (runtime HRDEM sessions) was
  accidentally committed once and removed again; the blobs remain in
  history. A history rewrite is required to purge them — operator call.

## Landed this session (each with an execution record in the plan file)

A9 (solver-only drape) · U5 (repair entries + infeasible acceptance) ·
L8 (startup self-check, auto-connect window, target confirmation,
calibration persistence) · A10 (file log, panic hook, diagnostics
bundle) · PJ4 (project export/import zips) · H6 (AOI source resolver +
override + chip) · U11 (tools status badges) · U12 (field mode + `?`
sheet) · U13 (TaskProgress, `terrain3d_cancel`, units) · U14 (gallery) ·
A11 phase 1 (versioned shared state; layer toggles) · U15 phase 1
(detachable map window) · U16 (grouped layers panel, opacity, reset,
persistence).

Plus field-driven fixes: EKF-origin altitude compensation at upload
(waypoints flew ~5 m low), altitude-profile curve (ragged uPlot series +
frozen follow window), Signal Inspector reconnect settling gate (O7),
header layout/RtkDrawer robustness (O8, resolved).

## Needs verification — field / real shell (the priority list)

Run the app with `npm run tauri dev` (the detached map and the L8 flows
need the Tauri shell; a stale binary lacks the new commands — rebuild
with `npm run tauri build` for a packaged test).

1. **EKF-origin altitude compensation** (highest value): land on site →
   confirm the Prepare row / Sync step shows the bias → upload the plan →
   fly a straight leg → the HUD AGL should read the planned AGL (before:
   ~5 m low). Also confirm a relative-frame plan behaves the same.
2. **HUD AGL(DTM)** with a 3D mesh on and no 2D tile: the readout must
   equal the planned AGL (it now mirrors the map's ground reference).
3. **Altitude profile**: curve scrolls, x-window follows the track head;
   wheel = y-zoom, drag = box zoom, double-click = reset; "Planned
   (drape)" appears when a drape preview exists; the `N fixes · last Xs
   ago` chip distinguishes a stalled telemetry stream from a draw
   problem. Compare against the DTM/DSM series.
4. **Signal Inspector reconnect (O7)**: with a filter enabled, rebuild
   the link — status bar shows "settling…" ~2 s, then the curve starts on
   stable data (no shaking segment). If a very low-cutoff filter still
   ramps briefly, raise `SETTLE_MS`.
5. **Detached map (U15 phase 1)**: Pop out map → the map window shows the
   globe, vehicle, mission and tracks (the telemetry bridge + Cesium base
   URL are wired now); the main window drops the map area entirely (slim
   strip + auto-opened altitude profile, HUD/commands/checklist/alerts
   intact); closing the map window re-embeds; closing the main window
   closes the map window too; RTL still ≤2 interactions from the main
   window. **Camera/selection cross-window sync is NOT implemented**
   (A11 follow-up slices parked — issues O12), so views move
   independently.
6. **U16 layers panel (real globe)**: every toggle matches the Cesium
   visibility (base map, grid, hillshade + opacity, 3D mesh, coverage,
   heights, AGL-coloured track); unloaded/failed layers never read "on";
   Reset restores defaults without touching the data source; preferences
   survive a restart.
7. **L8 self-check rows** on the real machine: udev rules present,
   timezone string, DEM/terrain service states; the auto-connect 3 s
   cancel window on a real startup; the target-change dialog on a
   different sysid.
8. **A10 diagnostics**: Tools ▸ Export diagnostics writes the bundle
   folder (log/config/alerts/session) and the alert line shows the path.
9. **PJ4 archive**: export a project to zip, import it elsewhere — the
   route table rebuilds and a tampered zip is refused.
10. **U13 long tasks**: 3D mesh build shows progress + elapsed and
    Cancel kills the `dem-prep` child; raw-LiDAR processing is visible.
11. **U11 tools badges**: motor test running shows "sending" in Tools ▾.
12. **Plan acceptance leftovers**: PX4 height-chain control points
    (<0.5 m, H7/H10), DR6 drape-panel walkthrough, DR5 detection/false
    -positive table from real data, issue 13 PX4 parameter names
    (`DO_CHANGE_SPEED`, acceptance radii, climb/descent limits, mission
    item cap).

## Open / parked

- **Cesium render error**: the operator saw "An error occurred while
  rendering. Rendering has stopped." — the detached window was missing
  `CESIUM_BASE_URL` + the locale shim (fixed in the newest commit). If it
  reappears, the next step is the A8 pattern: a `scene.renderError`
  listener dispatching `maggcs:map-failed` so the map shows the
  restartable failure card instead of a dead globe.
- **Parked by the operator** (issues): O9 H10 comparison, O10 H11
  sub-metre point cloud, O11 H5 lazy tiling, O12 A11 follow-up slices
  (selection/camera/drape cross-window sync).
- **Deferred plan items (⏸)**: M1–M4, M6–M9, U6, U7 remainder, U10, PJ5 —
  do not start without an explicit unfreeze.

## Dev commands

```bash
# Rust
cargo test -p maggcs-core --lib && cargo test -p maggcs-app --lib
cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings
cargo fmt --all -- --check

# Frontend (from frontend/)
npm run dev            # vite on :5199
npm run typecheck && npm run build && npm run check:i18n
npm run check:agl && npm run check:coords && npm run check:plan-steps

# Playwright checks need the dev server; each takes <url> <shotsDir>
npm run check:startup -- http://localhost:5199 /tmp/shots
npm run check:map-window -- http://localhost:5199 /tmp/shots
npm run check:layers -- http://localhost:5199 /tmp/shots
# plus check-{preflight,safety,command-ux,topbar,heartbeat,alerts,
#   alert-ux,teardown,field-mode}
```
