# Q-Mag — Feature Guide

2026-10-10 · What each view/window does and how to operate it. Architecture
in [ARCHITECTURE.md](ARCHITECTURE.md); open work in [issues.md](../issues.md).

## Application shell and tool headers

- The application is **Q-Mag**. The title/project entry, tools, Settings,
  workflow selector, and vehicle status share one compact top bar.
- **Workflow** shows the current stage as `n/5`. Open it to choose Prepare,
  Plan, Preflight, Fly, or Review, or use Previous/Next. Arrow keys and
  Home/End move focus; Enter selects and Escape closes the menu.
- At widths of at least 1600 px, Inspector and Motor test have inline
  shortcuts. At smaller widths they are available in **Tools**, alongside
  Parameters, Logs, and Export diagnostics. The map pop-out entry is removed.
- The top bar uses the Q-Mag wordmark above 1366 px and its **Q** mark at
  narrower widths. Inspector, Motor test, and Parameters use the same pattern
  before their titles; Logs and Settings use a compact Q in the dialog header.
- StartPage and **Settings → About** show the full application logo and
  Quaternion Aerospace company logo. About contains the runtime version,
  license, and elevation-data attribution. Company artwork follows the
  resolved theme.
- The start page scrolls when recent projects and the create form exceed the
  available height, including in Field mode.

Screenshots and manual review steps are in the
[frontend README](../frontend/README.md).

## Connection

- Settings → Connection: UDP/TCP/serial endpoints, presets (PX4 SITL,
  QGC forwarding, serial telemetry), auto-connect on launch. The LINK pill in
  the top bar opens a popover with vehicle identity, endpoint and
  connect/disconnect.
- Serial ports: only operator-pluggable ports are listed (`/dev/ttyACM*`,
  `/dev/ttyUSB*`, Windows `COM*`); the list refreshes live on hotplug, and
  "Fix USB permissions" installs a udev rule through the system password
  prompt (ADR-009; undo by deleting `/etc/udev/rules.d/99-maggcs.rules`).
- PX4 SITL: `scripts/sitl/setup_px4_sitl.sh sim`; the initial vehicle position
  (Settings → Vehicle) is mirrored to the SITL helper so PX4 spawns at the
  same spot. SITL exposes only the MAIN output bank; AUX is real-hardware only.

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
  the local event ring (command acks, mode switches, arm/disarm, RTK, and
  the FC's own `STATUSTEXT` lines, tagged `fc`).

Behaviour (plan §3 A3, `core::alerts`): the same key raised again inside the
30 s dedup window only increments the count — the operator is not re-alerted;
a repeat after the window, or a severity escalation, alerts again; level
alerts (threshold conditions, L7) stay until the condition has been clear for
3 s. Raising the same key inside the window never adds a second row. Alert
sources today: MAVLink link failures, terrain-drape failures, refused
disconnects/quits (`system`) and the flight controller's own status texts
(`FC`, L1), plus battery, GNSS and telemetry-link health (L7). Magnetometer
and VIC follow in M2/M7. Rust owns the model, so every window sees the same
count and the same rows.

### Health thresholds (L7)

- Battery percentage and voltage, GNSS fix/satellite count, telemetry RSSI,
  free TX buffer and packet loss are evaluated on every telemetry snapshot.
  A condition alerts once when it appears or changes severity; clearing waits
  for the same 3 s hysteresis, and Critical stays latched until acknowledged.
- Thresholds come from `safety` in the Rust-owned `config.json`
  (`SafetyConfig`). Battery percentage defaults to warn below 30% and
  critical below 15%; the TopBar battery colour uses those same values.
- Voltage and RSSI thresholds default to unset because packs and radios
  differ. Set `warn_battery_voltage_mv` / `critical_battery_voltage_mv` and
  `warn_rssi` / `critical_rssi` to enable those checks. Missing or stale
  telemetry never raises a threshold alert.

### Flight controller messages (STATUSTEXT)

- Every `STATUSTEXT` the FC sends appears in the `Events` tab with the FC's
  own severity. Long messages are split into several frames by MAVLink; they
  are reassembled in Rust and shown once, whole.
- Anything the FC calls a problem (Emergency…Warning) also becomes an alert,
  so it colours the badge and the banner: `Critical`/`Error` are red,
  `Warning` amber. `Notice`/`Info`/`Debug` are log-only — PX4 emits them
  continuously and a badge that is never zero tells the operator nothing.
- When the FC rejects a command with `temporarily_rejected`, the reason it
  sends next is attached to the command: the alert and the log line read
  `arm_disarm rejected: Preflight Fail: …` instead of an anonymous sentence.
  That correlation happens in the telemetry hub, where both frames arrive in
  order, so it cannot race the ack.
- The recorder (A4) will write the same stream to the flight's event log.

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
- The motor test is stopped *while its link is still up*. Stop and Emergency
  Stop await the transport write for every motor's release command before
  reporting success. Failed/incomplete releases retain the session and link
  for retry; Emergency remains latched. A write receipt is not a PX4 ACK or
  electrical-output measurement. The configured PX4 test timeout is 0.3 s.
- Each stage has its own 2 s budget and the whole best-effort sequence a 10 s
  one; a hung service is cut off and reported, never blocks the exit. Stages
  that time out or fail raise a `warn` alert naming the stage.
- **Refusal**: Disconnect, Shut down, and closing the main window are refused
  while the vehicle is armed or airborne. The in-app error or native-close
  banner names the reason and the next step; in-app refusal also raises a
  `warn` alert. There is no override path and no automatic RTL/Land/Disarm.
  With no link, or with unknown telemetry, nothing is refused: a ground
  station that will not quit on a dead link is worse than the risk it guards
  against. OS-level process termination cannot be prevented by the GCS.
  Allowed-but-lossy in-app exits require the typed `DISCONNECT`/`QUIT` keyword.
- Once the refusal policy allows exit, closing the main window while
  sub-windows (inspector / motor test / parameters) are still open exits the
  whole process, by design. If armed/airborne, close is prevented and the main
  window remains available for the refusal explanation.

## RTK (Tools-less: TopBar GPS/RTK pill ▸ drawer)

- **Base station**: the drawer classifies operator-pluggable serial ports as
  FC, RTK candidate or unknown; known/in-use FC ports are excluded. It rechecks
  USB identity before opening. Choose preconfigured RTCM reading or **F9P
  Survey-In**, default **2.00 m / 180 s**. Configuration is identified,
  RAM-only, acknowledged and read back. Both survey targets and the receiver's
  completion flags are required (ADR-027).
- **Independent lifetime**: base observation works without an FC. Disconnecting
  the vehicle preserves the base; exiting the app stops it. Start returns a
  session ID so configuration can be cancelled and old Stop requests cannot
  affect a replacement. Native automatic connection defaults on for eligible,
  unambiguous devices; saved identity wins, and Disconnect persists a pause.
  Manual port/baud/mode controls remain available and take priority.
- **RTK/GPS instruments**: the drawer includes a HUD-style solution readout,
  satellites/HDOP/VDOP, RTCM age ring, local-TX history and separate Survey-In
  time/precision bars. Missing and stale data are explicit; bars alone do not
  establish receiver validity and local TX does not establish rover reception.
- **Injection**: RTCM3 frames from the base are reassembled, fragmented
  into `GPS_RTCM_DATA` and sent to the FC (`core::rtk`); PX4 forwards the
  fragments to the GNSS in arrival order (ADR-008 addendum). Single-source
  mutual exclusion; a bounded queue drops oldest on overflow. Complete RTCM3
  frames up to 1029 bytes are supported on the PX4 byte-stream path by using
  multiple sequenced groups of at most four 180-byte packets. Statistics count
  RTCM frames rather than transport groups; large frames no longer terminate
  the service, and Stop/Start remains available after them. Forwarding is
  immediate, with whole-frame deadlines, cancellation and transport-generation
  guards. Reconnection uses fresh corrections after the new transport opened.
- **Health**: 1 Hz stats — source, valid RTCM bytes per actual elapsed second,
  RTCM message-type counts (1074/1084/…), dropped chunks and source-data age.
  The 5 s stall alarm uses the last nonempty read, with a startup grace period;
  no data yet retains a null age. Valid RTCM freshness is displayed separately
  from raw source traffic. Throughput reflects successful local transport
  writes, not rover receipt. A slow stats consumer cannot block Stop. Vehicle
  disconnect clears forwarding state; session/revision guards reject old events.
  Stop stays available after FC heartbeat loss and surfaces failures for retry.
- **Rover**: fix type (RTK FIXED/FLOAT/…) and satellite count from
  telemetry, with a stale-data annotation. Automated coverage and outstanding
  real-F9P/serial acceptance are recorded in the
  [detailed RTK / air–ground TODOs](plans/rtk-base-and-air-ground-todos.md).

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
- **Altitude Profile window (Tools ▸, operator request 2026-10-17)**: an
  independent window that follows the live flight — actual AMSL from
  telemetry, DTM ground and DSM surface sampled under each recorded fix
  from the loaded dem-prep tile (`terrain_sample`, gaps never
  extrapolated), and the rangefinder/lidar range from `DISTANCE_SENSOR`
  (decoded in core, R1; requested at 5 Hz at connect). The x axis is
  along-track distance in sync with the map's trail (both derive from the
  same telemetry events) and auto-follows the head; the DEM folder is
  picked per window and remembered. DR6's profile page will reuse this
  window instead of re-implementing it.
- **Height datum chain (plan H7/ADR-006)**: the CGVD2013 → WGS84 → AMSL
  conversion runs on the **real NRCan CGG2013 grid** (committed BC-SW
  extraction, converted by `dem-prep geoid` from the PROJ CDN mirror,
  verified against PROJ control points), no approximation: draped
  orthometric (CGVD2013) heights become AMSL through
  `H + N(CGG2013) = h`, `h − N(EGM96) = AMSL`. Where the chain is
  unavailable under a draped waypoint the drape fails closed and the
  **AMSL plan export refuses** with "vertical datum unverified" — never
  silently approximate (the old shortcut was off by up to 1.3 m).
- **HRDEM fetch (CanElevation, plan H1–H3/ADR-018)**:
  `dem-prep hrdem <lat> <lon> [--width 100] [--height 100] [--buffer 20]
  [--resolution 1.0] [--out <dir>]` queries the NRCan STAC API
  (`hrdem-lidar`), refuses AOIs over the 100 m × 100 m survey-area cap and
  AOIs crossing project boundaries (no stitching), and window-crops the
  native EPSG:3979 DTM/DSM COG pair over `/vsicurl/` (GDAL invoked as
  subprocesses — see the ADR-018 addendum). Output: `native/*-3979.tif`
  (the source window) plus the reprojected WGS84 pair `dtm.tif`/`dsm.tif`
  the ground station loads — strictly one grid (asserted), 1 m default
  resolution, DTM warped bilinear and DSM with footprint-max so surface
  peaks are never shaved (H3, measured), nodata −32767 preserved and never
  filled. `provenance.json` records item id, acquisition date, asset
  hrefs, fetch time, licence, GDAL/PROJ versions, output SHA-256s, the
  ADR-018 §D null-datum pipeline actually used and what PROJ's default
  (rejected) would have been. The CLI prints a pre-warp size estimate;
  same request → cache hit, no network or GDAL needed. OGL – Canada 2.0;
  attribution in the provenance.
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
  is sampled under itself at most every 1 s / 25 m. Since A9 the terrain-mode
  drape runs the solver pipeline against the DEM tile only (the mesh is
  display-only), so the heights it plans are measured against the DEM the
  readouts sample (guard: `scripts/check-agl.mjs`, `cesium/heights.ts`,
  `mission/drapeApply.ts`).

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
- **Preflight and forced arm (L5)**: Arm runs the core checklist first
  (link freshness, GPS/fix, RTK when required, HOME, battery, sensor health,
  on-ground, mission upload/read-back when a planned mission exists, mode and
  motor-test idle). `Fail`/`Unknown` items block; `Warn` items need per-item
  confirmation. A blocked arm can be forced only with the typed keyword
  `FORCE ARM`, a fixed reason and an optional note. The override bypasses the
  **GCS** checklist only — PX4's own prearm checks still apply and the 21196
  kill code is never sent. A forced arm is refused when the recorder is not
  running; on success the bypassed items, reason, note and UTC time go to
  `events.jsonl`, `session.json` and a same-timestamp GCS `STATUSTEXT` marker
  in the tlog.
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

- **Metadata-first**: the official PX4 v1.17.0 `px4_fmu-v6x_default` catalog
  (1912 parameters, including `USB_MAV_MODE`; units, enum values,
  descriptions, reboot flags) renders the full list without a link; the FC
  value of the selected row is fetched lazily. "Pull all" stays explicit for
  snapshot/diff workflows, and a lost list request self-heals (re-issued
  within the retry budget).
- **Version guard**: the FC's `AUTOPILOT_VERSION` is checked against the
  bundled metadata manifest (version, target, source and hashes). A live FC
  waiting for its version is distinguished from a disconnected FC; version
  mismatches show a notice. FC-reported parameters without metadata remain
  available, including an exact-name read from an empty search result.
- **Recovery**: a window opened before the FC connects and a window retained
  through disconnect/reconnect both refresh automatically. Old values and
  queued reads are invalidated; the selected row is re-read, and delayed
  replies from the old connection cannot overwrite the new state. Search
  changes reset the virtualized scroll position.
- **Values**: the table's **Current value** and **Default** columns are
  right-aligned under their matching headers, so the live value never reads as
  the default. The detail pane lists both values first, above the firmware
  description, and wraps long enum meanings in full instead of truncating them.
- **Editing**: inline type-aware editor (enum dropdowns show
  `value · description`; numbers carry units), Stage → staging tray →
  Apply with echo confirmation.
- **Reboot**: parameters flagged `rebootRequired` show a note; after Apply
  the banner lists them with a Reboot FC button
  (`MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN`) that reports the FC's real verdict
  — SITL denies it (a NuttX-only code path), the UI says so.

See [Parameters verification and screenshots](../frontend/src/params/README.md).

## Motor / mag interference test (Tools ▸)

Separate window, always-on-top while sending, Esc = emergency stop
(latched until Reset). The serial target card accepts a MAVLink system ID
(1–255, default 1) so this test can route to a specific vehicle when the
serial bus carries multiple systems. This is an expected/routing ID only: it
does not write or change the vehicle's own system-ID parameter; component ID
remains the PX4 autopilot default (1).

The window is responsive (Task 9): at 1366×768 and wider it keeps the
three-region layout (input + start/alarm controls / setup + plots /
target summary + RPM). Manual motors use a compact 2×2 grid. Capture/logging is
a bottom utility drawer spanning the center/right workspace: capture state and
Start/Stop remain visible while detailed configuration/retrieval actions default
closed. Each column has a bounded height and scrolls independently.
At 1024×640 the regions stack into one primary scroll region with the header
pinned (Stop and Emergency Stop stay visible). The left-column workflow comes
first: SITL details, the default-OFF Motor control switch, unlock/START, ESC
tones, then Mag alarm. Motor control must be enabled explicitly for each
target/connection; enabling is passive, and OFF keeps the window monitor-only
while telemetry and capture remain available. Inactive SITL details collapse
by default and open when Forward is selected. Logger configuration and log
retrieval live in the drawer behind the always-available capture controls.
The scope uses a Signal Inspector-style searchable/collapsible list, toolbar
and two independent stacked plots. A Top/Bottom switch in the list chooses which
plot a checkbox edits, so each plot has its own selection and the same signal may
appear in both (defaults: Output on top, Magnetic field below). Mixed units keep
separate axes. The list separator is draggable and keyboard-operable (Arrow
keys, Home/End, double-click reset); width is clamped to 220–560 px, defaults to
300 px and persists. Each signal has a persistent color override. X-axes are
cumulative seconds from the first plotted value; Clear/disconnect/reopen resets
0 s, while protocol, selection and color changes preserve it. Motor Test applies
saved theme and Field mode at startup and follows changes from the main window. See the
[workspace verification and screenshots](../frontend/src/actuator/README.md).

- **Output protocols and ESC RPM**: Motor Test renders and sends only the
  configured M1..MN prefix. Its three output families are PWM, OneShot and DShot;
  bitrate and bidirectional operation are DShot attributes. Detection uses each
  PX4 bank's `PWM_MAIN_TIMn` / `PWM_AUX_TIMn`, showing pulse output in µs and
  DShot in mixer counts. Different known families on MAIN/AUX are supported;
  unknown or conflicting protocols within a bank, `DSHOT_3D_ENABLE != 0`
  and unread ranges block START instead of guessing. ESC RPM from `ESC_STATUS` is shown
  live, stale (500 ms) or missing. These are display-only ESC indices, not motor
  assignments. Wire index 0 maps to ESC1–4 and index 4 to ESC5–8; aggregation,
  scope samples and per-ESC receive timestamps agree. Unseen ESCs remain absent,
  a reported zero is valid data, and each bank's freshness is independent.
  RPM hold, mapping-verification pulses, learned/manual RPM limits,
  feedforward and speed-performance verdicts have been removed from the UI,
  IPC commands and core modules (MR-06). The scope keeps command, magnetic,
  ESC and battery-current traces. RPM availability does not gate motor START.
  See [ADR-023](adr/023-motor-test-protocols-rpm-hold.md).

- **Header and motor tracking**: the fixed header shows the target identity,
  armed/heartbeat state, active output protocol, START/STOP state and owned failures.
  One test-state label distinguishes Locked, Unlocked, Starting, Running,
  Releasing and Emergency; the raw backend session is labelled Command transport.
  Unavailable firmware/rate placeholders and capture state do not crowd the
  safety header. Output blockers remain visible with their reason. The
  Motor tracking table keeps Motor, mapped output, source value, expected output,
  target-reported output, Δ and state together. Expected is inverted from the
  `ACTUATOR_TEST` value actually sent (after safety limits), and `tracking`
  means feedback is within ±5 µs for pulse outputs or ±2 mixer counts for DShot.
  Rows expand to show function→pin
  mapping, range/reversal/THR_MDL_FAC and the encoded command. Missing models
  or feedback stay `—`, never inferred. Tracking cannot fold while running,
  stale or blocked; user-expanded motor details remain pinned when a temporary
  warning clears. Connection, mapping, capture and log errors persist until dismissed.
- **Safety gate**: unlock → hold Start 3 s for every command source; a short
  click never starts. All tests use a fixed 0.3 maximum normalized demand and
  2.0/s slew. Any source/preset/target/mapping change or manual motor edit while
  idle re-locks. Ordinary STOP retains the unlock; emergency reset re-locks it.
  Start also requires a fresh
  heartbeat from the exact connected target showing PX4 and disarmed; no motor
  command is sent before that heartbeat arrives.
- **Command sources**: Manual sliders (sync-all, ~30 Hz merge, latest value
  wins); Preset waveforms (step/ramp/square/sine — bounded duration,
  self-stopping); Realtime forward — the FC's motors follow a PX4 SITL's
  live actuator outputs (latest-value slot, stale feed auto-stops). The
  source connection also selects the MAVLink system/component ID and MAIN or
  AUX bank; channels 1–4 from any other node or bank are ignored. Forwarding
  uses the same fixed output cap and slew limit.
- **Output mapping and feedback**: the serial target's `PWM_MAIN_FUNC1..8`
  and `PWM_AUX_FUNC1..8` are read on connect and by Refresh mapping. PX4
  functions 101–104 correspond to Motor1–4, so feedback for each motor is
  read from its actual physical MAIN/AUX channel (for example M2 MAIN5),
  never assumed to be servo2. The right panel shows the mapping, reports
  unreadable or duplicate assignments, and leaves unmapped feedback blank.
  Source and mapping controls are frozen while sending.
- **PWM matching**: each mapped channel's `PWM_*_MIN/MAX/REV/DISn` and the
  target's `THR_MDL_FAC` become one `PwmModel` per motor. Models are sent
  before every start. Manual and preset values are fractions of the target
  range and are compensated before encoding; forwarded SITL pulses are
  converted through the same target model, so a reversed or narrow-range
  output still reproduces the requested pulse. The right panel shows the
  endpoints, reversal and thrust factor. The tracking table always shows
  each motor's FC-read range. Missing/invalid ranges are explicitly marked
  fallback and block starting until refreshed.
- **Protocol-aware output curves**: the sender converts queued, compensated
  ACTUATOR_TEST values through the same target models used for sending.
  PWM/OneShot are µs; DShot/BDShot are mixer counts. Every command and mapped
  FC-feedback sample carries the target connection, configuration revision,
  motor, physical MAIN/AUX channel and unit. FC curves use `fc.motorN`, selected
  from the actual output mapping rather than an assumed servo number. A context
  change clears only output history and rejects late/untagged samples; mixed
  units use independent axes. Release is a typed gap, not a synthetic zero or
  minimum. Stale telemetry inserts one deadline boundary, and run completion
  breaks existing command, mapped-feedback and RPM traces; every affected series
  resumes without spanning the invalid interval. The MAVLink wire value remains
  normalized as required.
- **Sending**: `MAV_CMD_ACTUATOR_TEST` at 10 Hz addressed to the configured
  target system ID; only that system's heartbeat/ACK and target telemetry are
  used for the safety gate and plots. A session is bound to its connection
  and output-configuration revision: reconnecting the target or refreshing its
  configuration while idle replaces the session, while a sending session must
  be stopped first. FC-side timeout is 0.3 s per command (PX4 caps at 3 s),
  with the fixed output cap and slew limit, a disarmed-only interlock
  (armed or stale heartbeat → immediate
  release), ACK denial stops the test. PX4 needs `COM_MOT_TEST_EN=1`.
- **Main serial reuse**: explicitly borrow the main window's compatible PX4
  serial link and parameter cache. Attach starts monitor-only; detach/tool
  close preserves main I/O. Main-link failure/reconnect revokes permission and
  expires the attachment. Exact target/generation checks reject stale offers
  and writes. Use Motor Test's Capture/Logs while attached; main telemetry and
  Parameters remain available. See [ADR-026](adr/026-main-serial-target-reuse.md).
- **Live curves**: one selected Output, Magnetic field, ESC telemetry or Battery
  view, with a 10/30/60/120 s rolling window (30 s default). Raw/calibrated total
  field, per-ESC current/RPM and total battery current remain selectable signals.
- **Mag alarm**: threshold watchdog over the EMA-smoothed total field —
  debounce (1 s), hysteresis (50 nT), cooldown (30 s) prevent false and
  repeated triggers; alarm = window flash + beeps + Acknowledge. Enable and
  threshold are in the left panel.
- **FC prerequisites**: `COM_MOT_TEST_EN=1`; test runs only while disarmed.
- **Stopping**: normal and emergency stops release all configured functions
  with `ACTUATOR_TEST.param2=0`, within a 2 s overall command/write budget.
  Active commands remain stopped after a failed pass, and the full release
  batch is retried. Reset cannot clear an incomplete emergency release.
- **AM32 ESC tones**: independent saved Start tone and Stop tone selections,
  with Off, Rising/1, Falling/2, Sweep/3 and Triple/4. Defaults are Rising for
  start and Falling for stop. Playback uses the target's MAVLink shell and
  checks live `dshot status` bank/channel/function/mask data before addressing
  physical outputs. Only configured DShot motors receive beacon commands.
  The start cue precedes output; the stop cue follows confirmed releases with
  a 1 s coast allowance, including preset completion. A pending start can be
  cancelled through STOP, CANCEL START or Escape; an already dispatched sound
  retains its playback fence before the next start. Stop-tone failures do not
  delay or invalidate motor release. MR-21 software/IPC acceptance passes;
  actual sound and timing on the installed ESC firmware remain pending.
- **Target parameters**: reads and writes go to the explicitly selected PX4,
  using its dedicated link or the borrowed main serial connection. Integer parameters are decoded from
  PX4's byte-concat encoding, so `COM_MOT_TEST_EN=1` shows "enabled" — not a
  denormal float. A write is reported only after the FC's own echo **and** a
  read-back match; a clamped or mismatched value is an error that shows the
  actual value. Connecting reads (never writes) `COM_MOT_TEST_EN`,
  `SDLOG_MODE`, `SDLOG_BACKEND` and `THR_MDL_FAC`. The motor-test window shows
  `SDLOG_MODE` and `SDLOG_BACKEND` read-only; both are restart-required, so a
  running logger may not match the stored value. Change them in the Parameters
  window, not per test.
- **SD-file capture**: the Logging card separates **PX4 SD capture** (read-only
  logger parameters, capture status, Check/Start/Stop) from **Log retrieval**
  (list, download, delete). The motor-test window owns one target-shell
  controller per serial PX4. "Check logger" reads `logger status` and reports either Ready,
  Recording (this test's own file) or Existing recording (a file already
  running, which is a valid observation rather than a failed check). Start
  sends `logger on` and reports Recording only after PX4 names a new `Log file:`
  path. Manual capture start/stop is independent of the motor-test session and
  remains available while motors are running; motor START/STOP never starts or
  stops capture. The manual start still requires a disarmed target and does not
  authorise motor commands. Stop waits for stable "Not logging" evidence inside
  a 10 s finalization window. Motor STOP/E-STOP bypass capture's operation lock:
  MR-16 loopback and UI tests hold Check/Start/Stop replies until the motor
  releases finish, then verify that late capture replies do not restart motors
  or clear Emergency. Stop capture is available after a failed check, an
  existing recording or a timed-out exchange, so the operator can always
  recover. Missing storage, a non-file logger backend, rejected commands,
  timeouts and path reuse are explicit errors. `SDLOG_MODE` labels follow v1.17
  (0 armed→disarm, 1 boot→disarm, 2 boot→shutdown, 3 AUX1, 4 first arm→shutdown).
- **Test run coordinator**: the coordinator is a separate orchestrated path;
  it requires an idle motor session and no active run before starting capture,
  then after a confirmed recording and 1 s pre-roll accepts START. Manual capture
  does not enter this path. A confirmed motor release starts a
  1 s post-roll, then `logger off`. Abort releases the motors first. Every
  run writes an atomically updated manifest at
  `<app_data>/motor-tests/<test_id>/manifest.json` (test id, log path, FC
  identity, mapping, read-only logger parameters, boundaries, stop reason);
  a reconnect reconciles pending runs and refuses a new run while one is
  active.
- **Log retrieval**: `LOG_REQUEST_LIST` / `LOG_REQUEST_DATA` (ended with
  `LOG_REQUEST_END`, which stops on-board logging) list and download SD logs;
  the serial target's logs use the target link. `LOG_DATA` replies are checked
  against the fixed 90-byte frame and the declared size before any buffer
  grows, so a malformed reply is an explicit error instead of a panic, and a
  chunk beyond the collected tail is reported as a gap. Progress re-anchors
  the request timeout, so long downloads no longer exhaust the retry budget
  after ~6 s; broadcast lag or an ordinary link lifecycle event no longer stops
  the download task, and replacing the link aborts the old runtime.

## Standard MAVLink stream catalog

The core library provides a searchable catalog of **229 standard messages and
1883 fields** from the locked, compiled `common` dialect (MR-18a). Descriptors
carry numeric ID, exact name, field wire types/array sizes, units and documented
PX4 v1.17 source support. Exact names and IDs rank first; partial/field searches
never silently select a message. Unknown and ambiguous selections return typed
errors. Source support does not imply a particular vehicle is streaming the
message or honoring a requested rate.

The **Streams…** analysis action adds the live manager for Motor Test's dedicated
PX4 link (MR-18b). It provides explicit selection/fields/units, immediate Hz
editing, disable and observed/waiting/stale states. Saved requests are scoped
to endpoint/system/component and restored once per new transport generation;
default scope requests cannot overwrite them. Wrong/expired targets and invalid
rates are rejected before guarded writes. Motor STOP/E-STOP remains independent
of stream operations. Six verified PX4 v1.17 uORB mappings expose source links
and interpretation notes. Exact source aliases resolve to their standard
message; shared `vehicle_imu` requires an explicit choice between 26/105, and
unmapped topics send no request. [MR-20 SITL verification](verification/mr20-standard-streams-20261025.md)
passed required stream rates, disable/stale and persisted reconnect behavior on
PX4 v1.17.0. ESC_STATUS had no source in that SIH fixture; physical hardware
acceptance remains separate. See the
[catalog API and regeneration checks](../crates/core/src/mavlink/README.md).

## Signal Inspector (Tools ▸)

Separate window; raw signal tap over MAVLink or a ULog replay
(open `.ulg`, seek/speed), per-trace filter chains (low/high-pass, moving
average, detrend) and FFT — all executed in Rust. Angles are displayed in
degrees; the trace palette is token-based and theme-aware. Workspaces
(plots/traces/colors) persist locally.

The header's **Live link** status requires a live primary FC heartbeat.
After FC loss it shows **FC disconnected** and zero live message rate even
while the transport handle is retained for reconnect. ULog replay is an
independent source. See [Inspector verification](../frontend/src/inspector/README.md).

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
  fixer), About. The versioned `config.json` also stores the **safety
  thresholds** (stale telemetry, minimum satellites, battery warn/critical
  percentages and preflight toggles); the Rust arm gate reads that one copy,
  and invalid values are repaired rather than discarding the rest of the file.
