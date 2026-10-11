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

## Map

- Imagery style selectable in Layers ▸: OSM streets, Esri satellite,
  Google satellite (WGS84-aligned), OpenTopoMap — always over the offline
  graticule (no grid-only mode).
- Tool strip (top-left): Select (Esc), Add waypoint (W), Draw polygon (P) in
  Plan; Follow, 2D/3D in Fly. Layers ▸ (imagery style, grid, waypoint
  heights) and View ▸ (north up, go home) popovers.
- Waypoints: numbered markers colored by flight state (flown dimmed, target
  green halo, selected white halo), height sticks + AGL chips, drag to move,
  right-click a polygon vertex to remove.

## Plan view (right panel, step-by-step)

1. **Area** — draw the survey polygon on the map (readout: vertices, area,
   perimeter, self-intersection check) or set a rectangle + centre.
2. **Pattern** — Survey sweep / Cloverleaf; common fields (spacing, line
   azimuth, altitude, speed) always shown, advanced folded; generation via
   `core::survey` (tie lines and the calibration cloverleaf included).
3. **Waypoints** — list with distance-from-vehicle, coordinate entry (DMS or
   decimal), add/remove, inline editor under the selected row; rows colored
   by flight state.
4. **Sync** — Upload / Download, ⋯ menu: Import `.plan` (QGC), Export, Clear
   local plan, Clear FC mission (two-click confirms). One sync badge in the
   header (empty/unsynced/dirty/mismatch/synced — post-upload read-back
   verification).

Altitude mode (relative / AMSL / AGL) is a mission-wide setting in the
always-visible panel header.

## Fly view (right panel)

- **Commands**: Takeoff / Land / RTL — each behind a two-click confirm; ack
  outcome shown inline. (Pause/Continue removed: PX4 v1.17 NACKs
  `MAV_CMD_DO_PAUSE_CONTINUE`.)
- **Arm / Disarm**: two-click confirm, same status line; PX4 runs its own
  preflight checks for GCS arming and denies with `temporarily_rejected`
  plus a reason (ADR-017). The 21196 kill code is never sent from the UI.
- **Mode switching**: the MODE pill is a popover (`MODE ▾`). Entries and
  their grey-out reasons (link, mission sync, armed, home, RC) resolve in
  core (ADR-017); Return/Land need the confirm click; the switch shows
  "Switching…" until the heartbeat readback matches — never an optimistic
  display. Manual/Acro/Offboard sit behind the "Advanced modes" checkbox.
- **Mission progress**: active waypoint, distance/ETA, target altitude — the
  FC's mission is auto-fetched when the local plan is empty (e.g. after a
  restart or a QGC upload).
- **Plan items**: flyable-order list colored by flight state.
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
shown. The loaded calibration persists across window reopens.

## Settings

- Connection (endpoint, presets, auto-connect), Vehicle (initial position,
  **UAV model orientation offsets** pitch/roll/yaw for the 3D model),
  Appearance (theme), Logs (placeholder), Devices (serial list + permission
  fixer), About.
