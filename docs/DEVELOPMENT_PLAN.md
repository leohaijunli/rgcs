# MagGCS Development Plan v2.2

2026-10-05 · Adds to v2.1: connection-layer hardening and diagnostics, settings
page rework, WSL2/Docker SITL development agreement, and a revised QGC
coexistence approach (see Section 2).

2026-10-03 · v2.1 added to v2.0: RTK base-station connection and RTCM
forwarding, automatic USB udev rule configuration, tech-style UI and UgCS-like
layout.

## 1. Positioning and Scope

**Positioning**: an open-source ground control station running side-by-side
with QGroundControl, for survey/geophysics drone operations. Flight-controller
setup, sensor calibration, Airframe and Radio configuration stay in QGC.

**Three differentiating pillars**

1. Survey-grade planning: 3D survey lines, tie lines, terrain draping,
   climb-rate constraints; mission parameters and DEM version archived
   together.
2. Realtime data & QC: magnetic-field curves, track deviation, AGL deviation,
   RTK status at a glance.
3. Open integration: QGC `.plan` compatibility, headless mode, REST/WebSocket
   API.

**Target airframe**: multirotor first. **Out of scope**: sensor calibration,
Airframe, Radio, firmware flashing.

## 2. Change Summary

### v2.2 changes (2026-10-05, from a code review of rgcs-main and SITL bring-up)

- **Connection-layer hardening (Phase 0 wrap-up; prerequisite for Phase 1)**:
  fix the UI subscription race, heartbeat-loss detection being reset by other
  traffic, the GCS not sending HEARTBEAT, and swallowed connection errors; add
  link diagnostics and a four-level link state (see the Phase 0 addendum).
- **Settings page rework**: close the missing i18n keys and add a CI check;
  fixed dialog size; remove the redundant standalone connect dialog so the
  connection entry point lives in Settings; structured endpoint form.
- **ADR-003 revision, new ADR-011**: a unicast UDP port can have only one
  listener, so running alongside QGC needs a splitter; new decision on the link
  state model and the GCS heartbeat.
- **Development agreement**: MagGCS on Windows + PX4 SITL in Docker inside
  WSL2; network prerequisites in the Phase 0 "development agreement".
- **Acceptance addendum**: see the Phase 0 addendum acceptance.

### Current progress (from code review; not item-by-item against numeric acceptance)

- `crates/core` now has `mavlink`, `telemetry`, `mission`, `commands`,
  `devices`, `height`; `rtk`, `terrain`, `survey`, `mag`, `qc` are not started.
- Phase 0 skeleton exists: connection management and reconnect, telemetry
  aggregation, device enumeration, UI shell (three views, themes, settings
  dialog, Playwright screenshot baseline), CI workflow (not item-checked).
- Phase 0 numeric acceptance (0 heartbeat losses over 30 minutes, position
  latency < 200 ms, coexisting with QGC) is not yet proven in code; re-run per
  the addendum acceptance below.
- The SITL ↔ MagGCS UDP link is connected (Windows 11 26200, WSL 2.7.3, PX4
  v1.17.0 SITL in Docker under WSL).

### v2.1 changes

- New `core::rtk`: RTK base-station connection, configuration, status display,
  and forwarding of base-station RTCM to the flight controller over MAVLink
  (Section 8).
- New `core::devices`: USB device identification and udev rule
  generation/installation (Section 9).
- New UI design spec: dark tech style, UgCS-like layout (Section 6); design
  tokens and the UI shell moved to Phase 0 to avoid later rework.
- Phase reorder: Phase 2 is now RTK; terrain and Survey move to Phase 3. RTK
  and terrain are independent and can be developed in parallel.

## 3. Technical Decisions (each recorded as an ADR in `docs/adr/`)

| ADR | Decision |
| --- | --- |
| 001 | Architecture: `core` library crate + `app-tauri` (desktop) + `server` (headless); Tauri is itself Rust, no separate backend. **Accepted.** |
| 002 | Frontend types generated from Rust via `ts-rs` (or `specta`); never hand-write two copies. **Accepted.** |
| 003 | Coexistence with QGC: MAVLink 2, independent system/component IDs, UDP or mavlink-router splitting (IDs validated against PX4 docs). **v2.2 revision**: a unicast UDP port has only one listener, so two GCSs cannot both `udpin` 14550; run alongside via mavlink-router, or let QGC's MAVLink forwarding target another port that MagGCS listens on (whether to set `SO_REUSEADDR` is TBD). |
| 004 | Terrain source: local GeoTIFF by default; Cesium ion optional only. |
| 005 | Terrain following: path A (GCS pre-computation) first; path B (onboard) later for comparison. |
| 006 | Height datum: the working datum is **AMSL** (`h − N(EGM96)`), matching PX4 `MAV_FRAME_GLOBAL`; every height is a datum-tagged `Height`, never a bare `f64`. **Accepted.** |
| 007 | License: **Apache-2.0**. **Accepted.** |
| 008 | RTCM source abstraction and forwarding strategy (single-source injection, fragmentation, bandwidth accounting). |
| 009 | udev privilege model: polkit + a separate helper; the main program never runs as root. |
| 010 | Frontend framework and design system: React + Zustand + Tailwind + Radix. **Accepted.** |
| 011 | Link state model and GCS heartbeat: four levels (disconnected / listening / data without FC heartbeat / FC online); the GCS sends HEARTBEAT at 1 Hz; heartbeat timeout is detected by an independent timer that other traffic cannot reset. **Accepted.** |

## 4. Repository Structure

```
maggcs/
├── AGENTS.md                 # positioning, out-of-scope list, height rules, AI rules
├── crates/
│   ├── core/                 # mavlink/ telemetry/ mission/ commands/ devices/ height/ (+ terrain/ survey/ mag/ qc/ rtk planned)
│   ├── app-tauri/            # desktop entry
│   └── server/               # headless: REST + WebSocket (placeholder)
├── helpers/udev-installer/   # privilege-raising helper (writes only this app's rule files)
├── tools/dem-prep/           # LAS → DTM/DSM → COG + metadata
├── frontend/                 # cesium/ views/ components/ design-system/ stores/ i18n/ generated-types/
├── testdata/                 # small DEM, ULog, .plan, RTCM recordings
├── scripts/sitl/
└── docs/adr/
```

## 5. Height and Coordinate Conventions

- Three datums: CGVD2013 orthometric (`H`, BC LiDAR), WGS84 ellipsoid (`h`,
  Cesium), AMSL (PX4 `MAV_FRAME_GLOBAL`). Conversion chain:
  `H + N(CGG2013) = h`, `h − N(EGM96) = AMSL`.
- `core` forbids bare `f64` for heights: use the datum-tagged type, with
  conversions centralized in one module.
- AGL defaults to DTM-relative; a separate minimum-clearance check relative to
  DSM is provided.
- **Acceptance**: known control points converted through the chain are within
  `< 0.5 m` of PX4 AMSL (threshold pending field validation).

## 6. UI Design Spec

**Visual style (tech)**

- Dark theme by default: deep blue-gray background with a cyan accent, thin
  borders, translucent frosted-glass panels, restrained glow.
- Status colors (green/yellow/red) mean state only, never decoration; the
  magnetic color scale is kept separate from status colors.
- Telemetry digits use a tabular font, bundled locally — no online fonts in
  the field.
- Motion is used only for state changes (panel expand, alarms). The number and
  area of frosted panels are capped to protect Cesium frame rate.
- A high-contrast light theme is provided (outdoor sunlight); touch targets are
  ≥ 44 px; en + zh i18n from day one.

**Layout (inspired by UgCS; its icons, colors and assets are not copied)**

- Center: full-screen 3D/2D map.
- Top: mode switch (Plan / Flight / Data) + link status bar (FC, RTK, devices).
- Left: mission/survey-line list, vehicle list, layer manager.
- Right: properties panel for the selected object (waypoint, survey line,
  region parameters).
- Bottom (collapsible): terrain/AGL profile along the survey line; switches to
  a realtime QC curve in flight.

**Implementation note (ADR-010 confirmed)**: React + TypeScript strict mode,
Zustand, Tailwind + Radix (unstyled primitives), uPlot (high-frequency realtime
curves). Colors/spacing/typography are all design tokens (CSS variables); theme
switching changes tokens only.

**UI acceptance**

- At 1920×1080 and 1366×768, no critical information is occluded; each view has
  a screenshot baseline (Playwright visual regression).
- No hardcoded colors in code (lint check); both themes pass the contrast check.
- The map stays ≥ 30 fps with side panels open (reference hardware).

## 7. Phased Plan

Principle: differentiating features first, generic controls later. Every
acceptance criterion must be automatable or carry an explicit number (the
numbers below are initial values, to be calibrated by measurement).

### Phase 0: skeleton, device enumeration, and UI shell

**Tasks**

1. Workspace, `AGENTS.md`, ADR drafts, license.
2. `core::mavlink`: UDP/serial/TCP connection, heartbeat monitoring, reconnect;
   parse HEARTBEAT, GLOBAL_POSITION_INT, ATTITUDE, SYS_STATUS, BATTERY_STATUS,
   GPS_RAW_INT.
3. `core::devices` (basic): enumerate serial ports and VID/PID/serial, hotplug
   events.
4. Frontend UI shell: design tokens, two themes, layout frame
   (top bar / side panels / bottom bar), i18n skeleton; basic Cesium scene +
   realtime position.
5. GitHub Actions running the PX4 SITL integration test.

**Addendum (Phase 0 wrap-up)**: merge the settings menu into a single tabbed
dialog (Connection / Theme·Language / Logs / Devices·udev / About), replacing
the old theme popover and reserving tabs for later phases (RTK, QC).

**Addendum (v2.2, connection-layer hardening and settings rework; 0.1–0.4 must
land before Phase 1, because the mission protocol depends on a reliable link
state and GCS heartbeat)**

| # | Task | Acceptance |
| --- | --- | --- |
| 0.1 | Fix the hub subscription race: pass the receiver returned by `spawn_connection` into `TelemetryHub` instead of dropping it | Unit test: the hub sees `Connected` within 100 ms of binding; with no packets the UI shows "listening", not "disconnected" |
| 0.2 | Detect heartbeat timeout with a timer created outside the loop (checked ≤ 500 ms), no longer reset by other frames | Unit test: keep injecting non-target frames; after the FC heartbeat stops, `HeartbeatLost` fires within timeout + 500 ms |
| 0.3 | Send a GCS HEARTBEAT at 1 Hz (`MAV_TYPE_GCS`, sysid 250; ID validated against PX4 docs per ADR-003) | SITL capture shows a 1 Hz heartbeat; a `udpout:` endpoint receives FC data |
| 0.4 | `connect` waits for bind success/failure before returning and passes errors through; frontend auto-connect no longer swallows errors | With 14550 occupied, clicking connect shows "Address in use" within ≤ 1 s; auto-connect failure is visible in the status bar |
| 0.5 | Link diagnostics surfaced in the Connection page; four-level link state per ADR-011 | One screenshot per state; "are packets arriving?" is visible at a glance |
| 0.6 | i18n key check: every key referenced by `t('...')` exists in `en.ts`; fill in the missing `settings.*` keys | CI reports 0 missing keys; the settings page shows no raw key names |
| 0.7 | Fixed-height settings dialog; tab switches do not resize it; remove the redundant `<ConnectDialog />` from `App.tsx` and unify the connection entry point under Settings > Connection; clicking the top-bar FC status jumps to that tab | Playwright per-page screenshots with a consistent dialog bounding box; no stray placeholder at 1366×768 |
| 0.8 | Structured endpoint form: type (UDP listen / UDP client / TCP / serial) + address and port; presets (PX4 SITL 14550, QGC forwarding port); serial dropdown from `enumerate_devices`; remember the last endpoint and the auto-connect toggle | Invalid input is flagged under the field; the endpoint survives a restart |
| 0.9 | Touch targets ≥ 44 px; connection errors are shown inline in the settings dialog, not covered by the overlay | Screenshot check; the error is visible with Settings open |

**Development agreement (MagGCS on Windows + PX4 SITL in Docker inside WSL2)**

- PX4 SITL sends GCS data to `127.0.0.1:14550` by default. Under WSL2's default
  NAT mode, the WSL and Windows loopbacks are separate, so MagGCS on Windows
  receives nothing.
- Prerequisite: set `networkingMode=mirrored` in `%UserProfile%\.wslconfig`
  (Windows 11 22H2+, WSL 2.0+), then `wsl --shutdown`. **(This is the fix used
  in the bring-up; to be confirmed.)**
- Docker must be the Docker Engine inside the WSL distro with `--network host`;
  Docker Desktop's host network points at its own VM, not WSL, so it would need
  port mapping instead (unverified).
- Debug order: first confirm PX4 has a heartbeat from WSL with `sitl_monitor`;
  then check `Get-NetUDPEndpoint -LocalPort 14550` on Windows to confirm only
  MagGCS is listening; finally check that the Windows firewall allows inbound
  UDP to MagGCS.
- Doc fix: `docs/sitl-wsl-windows.md`'s "the GCS sends commands back to 14540"
  is inaccurate. 14540 is PX4's remote port for offboard/onboard; in `udpin`
  mode, command replies go to the source address of the first packet received,
  so a packet must arrive before commands can be sent.
- `scripts/sitl/run_sitl_docker.sh` targets `jmavsim` by default; whether that
  works on PX4 v1.17.0 is TBD; a simulator-free target (e.g. `none_iris`, TBD)
  can be used when only the link is being verified.

**Acceptance**: 0 heartbeat losses over 30 continuous SITL minutes; position
latency < 200 ms (local); coexisting with QGC connected to the same SITL; UI
acceptance per Section 6.

**Addendum acceptance (v2.2)**

- 0 heartbeat losses over 30 continuous minutes with MagGCS on Windows + SITL
  in Docker under WSL2.
- With sustained traffic of non-target frames mixed in, an FC heartbeat
  interruption is still detected within timeout + 500 ms.
- The three error classes — port in use, invalid address, silence from the peer
  — each have a clear UI message that does not rely on backend logs.
- Coexistence with QGC is verified per the ADR-003 revision (mavlink-router or
  QGC forwarding), recording the actual configuration.

### Phase 1: Mission protocol + planning view

**Tasks**: Upload/Download/Clear/Set Current state machine (timeouts,
retransmission, reconnect recovery, `MISSION_ITEM_INT`); `MISSION_CURRENT`
extension support; planning view (left list, right properties, waypoint click
and drag, three altitude modes); QGC `.plan` import/export; Pause/Continue, RTL.

**Prerequisite**: Phase 0 addendum tasks 0.1–0.4 complete (reliable link state,
GCS heartbeat, command reply address).

**Acceptance**: 100 waypoints uploaded and downloaded item-for-item identical;
upload still completes with 10% packet loss; `.plan` round-trips without
information loss.

### Phase 2: RTK base station and RTCM forwarding

Tasks and acceptance in Section 8. Independent of terrain; can be developed in
parallel.

### Phase 3: terrain + Survey planner (MVP, alpha-ready)

**Tasks**: `ElevationSource` trait (GeoTIFF default implementation), height
conversion chain, polygon → parallel survey lines + tie lines (azimuth,
spacing, extension, turn radius explicit), densify and smooth along the DEM
(climb-rate/slope constraints), AGL profile, DEM metadata display, mission
archive (parameters + DEM version + waypoints).

**Acceptance**: regenerating waypoints from the same archive is point-for-point
identical; on rugged terrain the SITL flight (requires Gazebo) deviates from
the target AGL by < 2 m; over-limit climb rates are detected and warned.

### Phase 4: magnetic data layer and realtime QC (complete the flight view)

**Tasks**: realtime magnetic-field curves, cross-track error, AGL deviation,
noise metrics and alarms; load GeoJSON/GeoTIFF/CSV/tiles with color scale,
opacity, legend; import ULog post-flight and overlay track and crossover;
flight-view HUD and bottom QC curve.

**Acceptance**: > 100k points at 30 fps; injecting yaw/altitude deviation via
SITL triggers a QC alarm within 2 s; ULog crossover results match the offline
script.

### Phase 5: fill in generic controls on demand

Read-only parameters, necessary parameter writes, Guided Goto, Set Mode, basic
multi-vehicle, log replay; evaluate fixed-wing.

**Addendum: PX4 log download/save**. `core` implements the MAVLink LOG protocol
(`LOG_REQUEST_LIST`, `LOG_REQUEST_DATA`, `LOG_ERASE`) to pull ULog from the FC;
written to the local log directory; the settings page's Logs tab offers
"refresh list / download / save as / erase". Shares the MAVLink link and storage
layer with log replay. (The Logs tab is currently a disabled placeholder; until
then it can be used as a MAVLink link log — rx/tx counts and error history —
replacing the flash-in-the-pan error banner.)

### Phase 6: release

Tauri packaging (Linux/Windows), udev helper packaging and polkit policy,
offline bundle (sample DEM + geoid grid), documentation, plugin-interface
stabilization.

## 8. RTK and RTCM Forwarding Design

**Data flow**: RTK base receiver (USB serial) → `rtk::source` reads the RTCM3
byte stream → `rtk::rtcm` parses frames and validates CRC → `rtk::forward`
wraps them as MAVLink `GPS_RTCM_DATA` → sent over the FC link → the FC GPS
driver injects them into the onboard RTK receiver.

**Source abstraction**: the `RtcmSource` trait. Phase 2 implements a serial base
station; NTRIP client and TCP/UDP forwarding are later optional
implementations. Only one source may inject at a time.

**Forwarding notes**

- A `GPS_RTCM_DATA` packet holds at most 180 bytes; longer RTCM frames must be
  fragmented per the protocol (up to 4 fragments, numbered). Boundary cases
  such as an exact multiple of 180 follow the MAVLink docs, with unit tests.
- When coexisting with QGC, QGC may also inject RTCM: provide an "RTCM
  injection" toggle, warn on multiple sources, and avoid dual injection.
- Bandwidth: RTCM volume depends on message types and constellations and can
  saturate a low-rate telemetry radio. Measure actual B/s, show a usage hint in
  the UI, and allow selecting a message set (e.g. prefer MSM4 over MSM7).
- Alarm when base-station data is interrupted beyond a threshold (default 5 s,
  configurable).

**Base-station configuration** (first supported device: u-blox ZED-F9P; others
via the trait)

- Survey-in (minimum duration, target accuracy) or fixed-coordinate mode; output
  RTCM3 message set; port output configuration; save to RAM/Flash.
- Preview before applying; read back to verify after applying.

**Status display**

- Base: mode, survey-in progress and current accuracy, satellite count, per-type
  RTCM message rate, data age, cumulative bytes.
- Onboard: `fix_type` (RTK Float = 5, RTK Fixed = 6), satellite count, HDOP,
  time to reach RTK Fixed.
- Top status bar and alarms: green = Fixed, yellow = Float, red = no RTK or
  interrupted.

**Testing and acceptance**

- RTCM frame-parsing unit tests (frame sync, coalesced packets, partial packets,
  bad CRC) using recorded samples; reassembled bytes match after fragmentation;
  the forwarding path verified with a mock onboard receiver; record the
  time-to-RTK-Fixed in an open environment (threshold set after measurement).
- Base-station link interruption triggers an alarm within 5 s.

## 9. USB Devices and udev Auto-configuration

**Goal**: plug in a known USB device and get a stable device name and
permission rule generated automatically, with no hand-written rules.

**Scope**: native Linux. Windows/macOS have no udev, so only device
identification and port-name mapping. Under WSL2 USB must first be attached with
usbipd-win, and whether udev takes effect depends on the WSL systemd setting, to
be verified separately.

**Flow**

1. Watch USB serial hotplug; read VID/PID/serial/vendor/product.
2. Match against the device database (common: PX4/Pixhawk, u-blox, FTDI, CP210x,
   CH340) and suggest a role (FC / RTK base / other); the user confirms or picks.
3. Preview the generated rule: permissions (`MODE`/`GROUP` or `TAG+="uaccess"`),
   a stable symlink `/dev/maggcs/<role>`, and a ModemManager ignore tag
   (`ID_MM_DEVICE_IGNORE`) so it does not claim the port.
4. Multiple devices with the same VID/PID: distinguish by serial first; with no
   serial, by physical port path, flagging that the rule binds to the interface.
5. Privileged install: call a separate helper via polkit (pkexec) that may only
   write this app's named files under `/etc/udev/rules.d/` and reload rules; the
   main program never runs as root. Headless mode offers a CLI subcommand.
6. List/uninstall/rollback installed rules; on failure, print the command to run
   manually.

**Acceptance**

- Rule-generator golden-file test: given device input, the rule text is
  byte-identical.
- The helper refuses to write any other path or filename (security test).
- On real hardware, `/dev/maggcs/<role>` appears within 5 s and opens without
  sudo; uninstalling removes the rule and symlink.

## 10. Task Card: DEM Preprocessing (tools/dem-prep)

- Input: GeoBC LiDAR point cloud (LAS 1.4, NAD83(CSRS)/UTM10, CGVD2013,
  CGG2013); if a raster product already exists, skip rasterization and only
  check metadata and datums.
- Output: DTM (class-2 points, COG, 1–2 m), DSM (max of class-1 points, canopy
  safety layer), metadata JSON (project, date, density, nominal accuracy,
  datum, void mask).
- Requirement: voids and water are explicitly marked, never silently
  interpolated.
- Acceptance: checkpoint error within the metadata's nominal accuracy; unit
  tests cover voids and coordinate datums.

## 11. AI Collaboration Rules

1. Do one small task at a time; never generate a whole phase in one shot.
2. First output struct/enum/trait/function signatures and error types; get
   confirmation before implementing.
3. Every task carries an automated or numerically measurable acceptance
   criterion; after finishing, self-check and state how it was verified.
4. Backend modules must ship with unit tests or SITL test scripts; frontend
   tasks ship with screenshots and manual verification steps.
5. Privilege escalation, serial writes, and base-station config writes require a
   failure/risk analysis up front (wrong config, wrong device, etc.).
6. Rust production code uses `Result`, never `unwrap`; TypeScript strict mode;
   magic numbers extracted to constants or config.
7. Update README/docs per module; record important decisions as ADRs; when
   unsure, ask or present option comparisons.
8. Execution order: MAVLink & telemetry → UI shell → Mission protocol →
   waypoint editing → RTK → DEM & height conversion → Survey planning →
   magnetic data & QC.

## 12. Kickoff Instructions

> Start from Phase 0. First create the workspace skeleton and `AGENTS.md`,
> output the `core` crate's struct/enum/error types and function signatures
> (including `mavlink` and `devices`), and wait for confirmation before
> implementing UDP connection management and HEARTBEAT / GLOBAL_POSITION_INT
> parsing, with a SITL integration test script.
>
> When done, output: file tree, key code, how to run/verify, and a self-check
> against the Phase 0 acceptance criteria.

### v2.2 current task

> Start from Phase 0 addendum task 0.1. First output the new signature of
> `TelemetryHub::spawn` and the new `LinkStatus` fields (the 0.5 fields, types
> first), and wait for confirmation before implementing. Each task ships with a
> unit test or SITL script and a note on how to verify it; settings-page tasks
> ship with 1920×1080 and 1366×768 screenshots.

## 13. Open Questions

1. The specific RTK base-station model (whether a u-blox ZED-F9P class),
   which decides the first supported configuration protocol. **(Confirmed:
   u-blox ZED-F9P.)**
2. Whether NTRIP is needed as an RTCM source.
3. Frontend framework and component library (ADR-010). **(Implemented as
   recommended: React + Zustand + Tailwind + Radix; ADR-010 is now Accepted.)**
4. License: Apache-2.0 or GPLv3. **(Decided: Apache-2.0, ADR-007.)**
5. Whether GeoBC data is actually provided as a point cloud or a raster, and
   its coverage.
6. Each phase's acceptance numbers must be calibrated on SITL and real flight
   data.
7. The splitting method for running alongside QGC: mavlink-router or QGC
   forwarding to another port; whether rust-mavlink's UDP listener sets
   `SO_REUSEADDR` (decides the exact ADR-003 revision).
8. The SITL target under PX4 v1.17.0 (whether `jmavsim` works, `gz_x500` needs
   Gazebo, which target to use for link-only verification), synced with
   `scripts/sitl`.
9. Docker mode: Docker Engine inside WSL with `--network host` as the official
   development setup; whether Docker Desktop needs support.
