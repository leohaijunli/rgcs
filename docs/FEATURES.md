# MagGCS — Feature Guide

2026-10-10 · What each view/window does and how to operate it. Architecture
in [ARCHITECTURE.md](ARCHITECTURE.md); open work in [issues.md](../issues.md).

## Connection

- Settings → Connection: UDP/TCP/serial endpoints, presets (PX4 SITL,
  QGC forwarding, serial telemetry), auto-connect on launch. The LINK pill in
  the top bar opens a popover with vehicle identity, endpoint and
  connect/disconnect.
- Serial ports: only operator-pluggable ports are listed (`/dev/ttyACM*`,
  `/dev/ttyUSB*`, Windows `COM*`); the list refreshes live on hotplug, and
  "Fix USB permissions" installs a udev rule through the system password
  prompt (ADR-009; undo by deleting `/etc/udev/rules.d/99-maggcs.rules`).
- PX4 SITL: `scripts/sitl/run_sitl_docker.sh`; the initial vehicle position
  (Settings → Vehicle) is mirrored to the SITL helper so PX4 spawns at the
  same spot.

## Alerts (TopBar badge ▸ drawer)

One alert center backs three surfaces, so they can never disagree:

- **TopBar badge** — how many alerts still need attention, coloured by the
  worst severity; opens the drawer (with or without alerts).
- **Banner stack** (bottom right) — the newest unacknowledged *Critical*
  alert, plus the existing link-error banner with history. A Critical alert
  is latched in the backend, so it disappears only when acknowledged, never
  because the next message pushed it aside.
- **Drawer** — `Alerts` tab: every alert (severity, source, text, count,
  time, ack button) with `Ack all` and a history-clear action; `Events` tab:
  the local event ring (command acks, mode switches, arm/disarm, RTK).

Behaviour (plan §3 A3, `core::alerts`): the same key raised again inside the
30 s dedup window only increments the count — the operator is not re-alerted;
a repeat after the window, or a severity escalation, alerts again; level
alerts (threshold conditions, L7) stay until the condition has been clear for
3 s. Raising the same key inside the window never adds a second row. Alert
sources today: MAVLink link failures, terrain-drape failures and refused
disconnects/quits (`system`); STATUSTEXT, battery, GPS/RTK, magnetometer and
VIC follow in L1/L3/L7/M2/M7. Rust owns the model, so every window sees the
same count and the same rows.

## Exiting and disconnecting (guards)

Three paths stop the link or the app, and all three run the same ordered
teardown (`core::lifecycle`, plan §3 A1):

```
motor test → RTK → FC logs → parameters → mission/command → (recorder) → link → terrain
```

- Settings ▸ Disconnect stops the **link-scoped** services only: the local 3D
  terrain server keeps running, so a reconnect keeps the 3D view. A running
  motor test is stopped first, whatever link it drives (its own serial target
  or the primary link), so no props keep spinning after the GCS lets go.
- Settings ▸ Shut down and closing the main window stop everything and exit.
- The motor test is stopped *while its link is still up*, and the `Stop` is
  awaited (not fire-and-forget), so the release frames reach the flight
  controller before the socket closes instead of waiting for PX4's own 3 s
  timeout.
- Each stage has its own 2 s budget and the whole best-effort sequence a 10 s
  one; a hung service is cut off and reported, never blocks the exit. Stages
  that time out or fail raise a `warn` alert naming the stage.
- **Refusal**: Disconnect and Shut down are refused while the vehicle is armed
  or airborne — the error names the reason and the way out, and a `warn` alert
  is raised so it is visible in the alert drawer. Closing the main window is
  the forced exit and is never refused (L4 adds the typed `DISCONNECT`/`QUIT`
  confirmation on top). With no link, or with stale telemetry, nothing is
  refused: a ground station that will not quit on a dead link is worse than
  the risk it guards against.
- Closing the main window while sub-windows (inspector / motor test /
  parameters) are still open exits the whole process, by design.

## RTK (Tools-less: TopBar GPS/RTK pill ▸ drawer)

- **Base station**: the drawer lists operator-pluggable serial ports
  (`COM*` / `ttyUSB*` / `ttyACM*` — the same enumeration as the devices
  page); pick one + baud (F9P), Start/Stop injection.
- **Injection**: RTCM3 frames from the base are reassembled, fragmented
  into `GPS_RTCM_DATA` and sent to the FC (`core::rtk`); PX4 forwards the
  fragments to the GNSS in arrival order (ADR-008 addendum). Single-source
  mutual exclusion; a bounded queue drops oldest on overflow.
- **Health**: 1 Hz stats — source, B/s, RTCM type counts, dropped chunks
  and a 5 s stall alarm (matching the PX4 GPS driver's link-switch
  timeout); all transitions land in the event log.
- **Rover**: fix type (RTK FIXED/FLOAT/…) and satellite count from
  telemetry.

## Map

- Imagery style selectable in Layers ▸: OSM streets, Esri satellite,
  Google satellite (WGS84-aligned), OpenTopoMap — always over the offline
  graticule (no grid-only mode).
- **Terrain (hillshade)**: Layers ▸ loads a dem-prep tile folder
  (hillshade.png + dem.json) and drapes the grayscale hillshade over the
  imagery by its geographic bounds, with the DEM version shown. Offline
  (ADR-004); the same folder feeds the Pattern-step drape.
- **Terrain (3D mesh)**: Layers ▸ serves a `dem-prep terrain` tileset over
  the local loopback (127.0.0.1:18823, ADR-004 addendum) and swaps the
  globe's `CesiumTerrainProvider` to it — real relief offline, imagery
  draped, areas outside the DEM stay ellipsoid. The toggle shows the
  served folder while on, surfaces the start error, and restores the
  previous state across reloads (ADR-004). No DEM on hand?
  `scripts/fetch-global-dem.py <lat> <lon>` downloads a Copernicus GLO-30
  patch and builds the tileset in one step (works offline afterwards;
  `--geoid` converts heights to ellipsoid with a matching grid).
  Toggling on reads the local DEM GeoTIFF folder directly (no dialog):
  `dem/BC` beside the app (or `$MAGCS_DEM_DIR`) is processed into a cached
  tileset on first use, then reused. DTM-only DEMs are fine — no DSM
  required. The popover's two buttons remain for a one-off download or a
  different source folder.
- Tool strip (top-left): Select (Esc), Add waypoint (W), Draw polygon (P) in
  Plan; Follow, 2D/3D in Fly. Layers ▸ (imagery style, grid, waypoint
  heights) and View ▸ (north up, go home) popovers.
- Waypoints: numbered markers colored by flight state (flown dimmed, target
  green halo, selected white halo), height sticks + AGL chips, drag to move,
  right-click a polygon vertex to remove.
- AGL ground (all `m AGL` readouts — map chips, plan list, altitude profile,
  the vehicle's drop line and the track's height-above-ground colour) comes
  from one reference, by priority: the **3D mesh** under the point, then the
  loaded **2D DEM tile**, then the **HOME plane** (plan HOME elevation, else
  PX4's own HOME = `AMSL − relative_alt`, else 0). So the clearance tracks the
  terrain the vehicle is over instead of a flat home plane, and a landed
  vehicle reads 0 AGL rather than echoing its AMSL altitude. The live vehicle
  is sampled under itself at most every 1 s / 25 m. The terrain-mode drape
  uses the same reference (mesh first), so the heights it plans are measured
  against the surface the readouts are measured against
  (guard: `scripts/check-agl.mjs`, `cesium/heights.ts`,
  `mission/meshDrape.ts`).

## Plan view (right panel, step-by-step)

1. **Area** — draw the survey polygon on the map (readout: vertices, area,
   perimeter, self-intersection check) or set a rectangle + centre.
2. **Pattern** — Survey sweep / Cloverleaf; common fields (spacing, line
   azimuth, altitude, speed) always shown, advanced folded; generation via
   `core::survey` (tie lines and the calibration cloverleaf included).
   Generation routes through the Rust compiler (S1): line counts, total
   path, ETA and coverage are shown; magnetometer survey-line boundaries
   stay GCS-side by default (the line table + MISSION_CURRENT), with an
   optional camera-trigger gating mode.
   **Drape to terrain (40 m AGL)** rewrites the heights over the loaded
   surface, mesh first: with the 3D mesh on, the survey AGL is measured
   against the mesh the operator sees (no DTM/DSM pair exists for a
   tileset — the report says "no DSM" instead of showing an unmeasured
   margin, and exported plans record `maggcsTerrainRef`). Otherwise it runs
   over a dem-prep tile folder: constant AGL over the DTM, raised where the
   DSM (surface incl. trees) breaks the 5 m clearance; the report shows the
   tightest clearance, raised count and DEM version (recorded in exported
   `.plan`s as `maggcsDemVersion`). Fallback sources (CDEM / Copernicus
   GeoTIFF) are DTM-only — the clearance check is skipped with
   `has_dsm: false`. Either way the stored heights stay absolute AMSL
   (ADR-013) and the waypoint rows show the AGL next to the AMSL.
3. **Waypoints** — list with distance-from-vehicle, coordinate entry (DMS or
   decimal), add/remove, inline editor under the selected row; rows colored
   by flight state. A `NAV_LAND` item in an imported plan is draped **onto
   the ground** in terrain mode (the cruise `agl` and the DSM clearance do
   not apply to it), and the Pattern step reports how many landing targets
   were placed. Each row shows the height above ground and the absolute
   AMSL below it (terrain mode leads with the AGL — the value the Pattern
   input asked for).
4. **Sync** — Upload / Download, ⋯ menu: Import `.plan` (QGC), Export, Clear
   local plan, Clear FC mission (two-click confirms). One sync badge in the
   header (empty/unsynced/dirty/mismatch/synced — post-upload read-back
   verification).

Altitude mode (relative / AMSL / AGL / terrain) is a mission-wide setting in
the always-visible panel header. In terrain mode the Pattern altitude is a
**survey AGL** and every plan edit re-drapes. Waypoint rows show both the AGL
and the absolute AMSL; with no terrain loaded they say "AGL unknown" instead
of a bare AMSL that looks draped.

## Fly view (nav bar + map)

The Fly panel itself is gone (operator request): the nav bar and the map
carry everything.

- **Flight ▾** (nav bar dropdown): Takeoff / Land / RTL / Arm-Disarm,
  each behind a two-click confirm. Takeoff replicates PX4's `commander
  takeoff` flow — NAV_TAKEOFF (param7 = current AMSL + 10 m relative)
  → await ACK → arm (Commander.cpp:336-345; a bare mode intent never
  leaves the ground). **Land** carries the *ground under the vehicle*
  (param5/6 = current position, param7 = terrain AMSL, mesh first, else
  the current fix) so the descent targets the surface: with the old
  all-zero payload the vehicle held altitude over lower ground instead
  of touching down (SITL report). PX4 runs its own preflight checks for GCS arming
  and denies with `temporarily_rejected` plus a reason (ADR-017); the
  21196 kill code is never sent from the UI. (Pause/Continue removed:
  PX4 v1.17 NACKs `MAV_CMD_DO_PAUSE_CONTINUE`.)
- **Event log drawer**: every terminal command ack, mode switch,
  injection source change and RTK alarm lands in one timestamped side
  drawer (300-entry ring, clearable) — the nav bar stays quiet.
- **Mode switching**: the MODE pill is a popover (`MODE ▾`). Entries and
  their grey-out reasons (link, mission sync, armed, home, RC) resolve in
  core (ADR-017); Return/Land need the confirm click; the switch shows
  "Switching…" until the heartbeat readback matches — never an optimistic
  display. Manual/Acro/Offboard sit behind the "Advanced modes" checkbox.
- **Blinking target**: the waypoint the FC is flying toward pulses on the
  map (hard square-wave ring, 0.4 s on/off) and follows `MISSION_CURRENT`
  as the mission progresses; flown waypoints dim.
- **HUD** (bottom-left of the map): attitude indicator + numeric roll, pitch,
  heading, speed, altitudes — all angles in degrees. Collapsible.
- Top bar: armed / mode (decoded PX4 flight mode) / RTK / battery pills;
  alert banners for mock data and link errors.

## Parameters (Tools ▸)

Separate window; three panes: PX4 groups + operator **custom groups**
(create/rename/note/delete; right-click a parameter to add it) on the left,
a virtualized parameter table in the middle, and the editor/detail panel on
the right.

- **Metadata-first**: the bundled PX4 v1.17 metadata (units, enum values,
  descriptions, reboot flags) renders the full list without a link; the FC
  value of the selected row is fetched lazily. "Pull all" stays explicit for
  snapshot/diff workflows, and a lost list request self-heals (re-issued
  within the retry budget).
- **Version guard**: the FC's `AUTOPILOT_VERSION` is checked against the
  bundled 1.17.0 metadata — mismatches show a notice (descriptions may be
  outdated).
- **Editing**: inline type-aware editor (enum dropdowns show
  `value · description`; numbers carry units), Stage → staging tray →
  Apply with echo confirmation.
- **Reboot**: parameters flagged `rebootRequired` show a note; after Apply
  the banner lists them with a Reboot FC button
  (`MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN`) that reports the FC's real verdict
  — SITL denies it (a NuttX-only code path), the UI says so.

## Motor / mag interference test (Tools ▸)

Separate window, always-on-top while sending, Esc = emergency stop
(latched until Reset).

- **Safety gate** (risk-scaled, operator-declared prop load): no props /
  flat plate — unlock toggle → Start; full props — unlock → type
  `START PROPS` → hold Start 3 s. Any load/mode change re-locks.
- **Command sources**: Manual sliders (sync-all, ~30 Hz merge, latest value
  wins); Preset waveforms (step/ramp/square/sine — bounded duration,
  self-stopping); Realtime forward — the FC's motors follow a PX4 SITL's
  live actuator outputs (`SERVO_OUTPUT_RAW` 1–4 → motors 1–4, latest-value
  slot, stale feed auto-stops).
- **Sending**: `MAV_CMD_ACTUATOR_TEST` at 10 Hz, FC-side timeout 0.3 s per
  command (PX4 caps at 3 s), output cap + slew limit per declared load,
  disarmed-only interlock (armed or stale heartbeat → immediate release),
  ACK denial stops the test. PX4 needs `COM_MOT_TEST_EN=1`.
- **Live curves**: calibrated total field (baseline Δ), per-ESC current (A)
  and RPM (right axis) over a 60 s rolling window.
- **Mag alarm**: threshold watchdog over the EMA-smoothed total field —
  debounce (1 s), hysteresis (50 nT), cooldown (30 s) prevent false and
  repeated triggers; alarm = window flash + beeps + Acknowledge. Enable and
  threshold are in the right panel.
- **FC prerequisites**: `COM_MOT_TEST_EN=1`; test runs only while disarmed.

## Signal Inspector (Tools ▸)

Separate window; raw signal tap over MAVLink or a ULog replay
(open `.ulg`, seek/speed), per-trace filter chains (low/high-pass, moving
average, detrend) and FFT — all executed in Rust. Angles are displayed in
degrees; the trace palette is token-based and theme-aware. Workspaces
(plots/traces/colors) persist locally.

**Mag calibration:** load a calibration JSON (Overflow ▸ *Load mag
calibration…*); the Rust session then derives the calibrated total field
`A·(m−b)` as a `total cal (nT)` series on any plot carrying
xmag/ymag/zmag. Without a calibration the plain √(x²+y²+z²) total is
shown. Mag values are normalized to nanotesla at ingestion (MAVLink
`HIGHRES_IMU` mag is gauss on the wire). The loaded calibration persists
across window reopens.

## Settings

- Connection (endpoint, presets, auto-connect), Vehicle (initial position,
  **UAV model orientation offsets** pitch/roll/yaw for the 3D model),
  Appearance (theme), Logs (placeholder), Devices (serial list + permission
  fixer), About.
