# MagGCS Development Plan v2.1

2026-10-03 · Adds to v2.0: RTK base-station connection and RTCM forwarding,
USB device udev auto-configuration, tech-style UI and UgCS-like layout

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

## 2. v2.1 Change Summary

- New `core::rtk`: RTK base-station connection, configuration, status display,
  and forwarding of base-station RTCM over MAVLink to the flight controller
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
| 001 | Architecture: `core` library crate + `app-tauri` (desktop) + `server` (headless). Tauri is Rust itself; no separate backend process. |
| 002 | Frontend types generated from Rust via `ts-rs` or `specta`; no hand-written duplicates. |
| 003 | Coexistence with QGC: MAVLink 2, independent system/component IDs, UDP or mavlink-router splitting (ID values validated against PX4 docs). |
| 004 | Terrain sources: local GeoTIFF is the default; Cesium ion is optional. |
| 005 | Terrain following: Path A (ground-station precomputation) first; Path B (onboard) cross-checked later. |
| 006 | Height datum: all heights carry a datum tag; working datum is AMSL (PX4/QGC-consistent); conversions centralized in `core::height`. |
| 007 | License: Apache-2.0 or GPLv3; must be decided within Phase 0. |
| 008 | RTCM source abstraction and forwarding policy (single-source injection, fragmentation, bandwidth stats). |
| 009 | udev privilege strategy: polkit + dedicated helper; the main process never runs as root. |
| 010 | Frontend stack: React + Zustand + Tailwind + Radix (confirmed); English-only i18n from day one. |

## 4. Repository Layout

```
maggcs/
├── AGENTS.md                 # positioning, out-of-scope list, height conventions, AI rules
├── crates/
│   ├── core/                 # mavlink/ mission/ telemetry/ terrain/ survey/ mag/ qc/ rtk/ devices/
│   ├── app-tauri/            # desktop entry
│   └── server/               # headless: REST + WebSocket
├── helpers/udev-installer/   # privilege-raising helper (app-owned rule files only)
├── tools/dem-prep/           # LAS → DTM/DSM → COG + metadata
├── frontend/                 # cesium/ views/ components/ design-system/ stores/ i18n/ generated-types/
├── testdata/                 # small DEM, ULog, .plan, RTCM recordings
├── scripts/sitl/
└── docs/adr/
```

## 5. Height and Coordinate Conventions

- Three heights: CGVD2013 orthometric (`H`, BC LiDAR), WGS84 ellipsoid
  (`h`, Cesium), AMSL (PX4 `MAV_FRAME_GLOBAL`). Conversion chain:
  `H + N(CGG2013) = h`, `h − N(EGM96) = AMSL`.
- `core` never represents an absolute height as a bare `f64`; heights must use
  the datum-tagged type; conversions live in a single module.
- AGL is relative to DTM by default; a separate minimum-clearance check
  relative to DSM may be added.
- **Acceptance**: known control points converted through the chain must be
  within `< 0.5 m` of PX4 AMSL (threshold pending field validation).

## 6. UI Design Spec

**Visual style (tech)**

- Default dark theme: deep blue-gray background + cyan accent, hairline
  borders, translucent glass panels, restrained glow.
- Status colors (green/yellow/red) indicate status only, never decoration;
  the magnetic color scale is kept separate from status colors.
- Telemetry numbers use a tabular (monospace) font, packaged locally — no
  online fonts (field sites may be offline).
- Animation is reserved for state changes (panel expand, alerts). The number
  and area of glass panels are capped to protect the Cesium frame rate.
- A high-contrast light theme for outdoor sunlight; touch targets ≥ 44 px;
  English-only i18n from day one.

**Layout (inspired by UgCS; no copying of its icons, colors or assets)**

- Center: fullscreen 3D/2D map.
- Top: mode switch (Plan / Flight / Data) + link status bar (FC, RTK, devices).
- Left: mission/survey-line list, vehicle list, layer management.
- Right: property panel for the selected object (waypoint, survey line, area).
- Bottom (collapsible): terrain/AGL profile along the survey line; switches to
  realtime QC curves during flight.
- Floating HUD: attitude, speed, altitude, battery, GPS/RTK status.
- The UgCS layout details have not been pixel-checked; confirm against
  current UgCS screenshots.

**Implementation (ADR-010, confirmed)**: React + TypeScript strict mode,
Zustand, Tailwind + Radix (unstyled), uPlot (high-frequency realtime curves).
Colors/spacing/typography are all design tokens (CSS variables); theme
switching only changes tokens.

**UI acceptance**

- At 1920×1080 and 1366×768 key information is never occluded; every view has
  a screenshot baseline (Playwright visual regression).
- No hardcoded colors in code (lint check); both themes pass the contrast
  check.
- Map keeps ≥ 30 fps with side panels open (reference hardware specified).

## 7. Phased Plan

Principle: differentiate features first, generic controls later. Every
acceptance criterion must be automatable or carry explicit numbers (all
values below are initial and need field calibration).

### Phase 0: Skeleton, Device Enumeration, UI Shell

**Tasks**

1. Workspace, `AGENTS.md`, ADR drafts, license.
2. `core::mavlink`: UDP/serial/TCP connection, heartbeat monitoring,
   reconnection; parse HEARTBEAT, GLOBAL_POSITION_INT, ATTITUDE, SYS_STATUS,
   BATTERY_STATUS, GPS_RAW_INT.
3. `core::devices` basic: enumerate serial ports with VID/PID/serial number;
   hotplug events.
4. Frontend UI shell: design tokens, both themes, layout framework
   (top bar / left-right panels / bottom bar), i18n skeleton; Cesium basic
   scene + realtime position.
5. GitHub Actions runs PX4 SITL integration tests.

**Phase 0 wrap-up**: settings consolidated into a single tabbed dialog
(Connection / Appearance / Logs / Devices·udev / About) replacing the original
theme Popover, leaving tab slots for later phases (RTK, QC).

**Acceptance**: SITL: 0 heartbeat losses over 30 minutes; position latency
< 200 ms (local); MagGCS and QGC connect to the same SITL simultaneously;
UI acceptance per Section 6.

### Phase 1: Mission Protocol + Planning View

**Tasks**: Upload/Download/Clear/Set Current state machine (timeouts,
retransmission, link-loss recovery, MISSION_ITEM_INT); MISSION_CURRENT
extension compatibility; planning view (left list, right properties, waypoint
click-and-drag, three altitude modes); QGC `.plan` import/export;
Pause/Continue, RTL.

**Acceptance**: 100-waypoint upload then download matches item-for-item;
upload completes at 10% packet loss; `.plan` round-trips without data loss.

### Phase 2: RTK Base Station + RTCM Forwarding

Tasks and acceptance in Section 8. Independent of terrain; parallelizable.

### Phase 3: Terrain + Survey Planner (MVP, alpha-ready)

**Tasks**: `ElevationSource` trait (GeoTIFF default implementation), height
conversion chain, polygon → parallel survey lines + tie lines (explicit
azimuth, spacing, extension, turn radius), DEM densification and smoothing
(climb-rate/slope constraints), AGL profile view, DEM metadata display,
mission archive (parameters + DEM version + waypoints).

**Acceptance**: regenerating waypoints from the same archive matches point for
point; on rolling terrain SITL (Gazebo required) real flight deviates
< 2 m from the set AGL; climb-rate-exceeding survey lines are detected and
alerted.

### Phase 4: Magnetic Data Layer + Realtime QC (flight view completion)

**Tasks**: realtime magnetic-field curves, cross-track error, AGL deviation,
noise metrics and alerts; load GeoJSON/GeoTIFF/CSV/tiles with color scale,
opacity, legend; post-flight ULog import with track overlay and crossover;
flight-view HUD and bottom QC curves.

**Acceptance**: 30 fps above 100k points; injected yaw/altitude deviation
triggers a QC alert within 2 s; ULog crossover matches the offline script.

### Phase 5: General Features on Demand

Read-only parameters, necessary parameter writes, Guided Goto, Set Mode,
basic multi-vehicle, log replay; evaluate fixed-wing.

**Addendum: PX4 log fetch/save**. `core` implements the MAVLink LOG protocol
(`LOG_REQUEST_LIST`, `LOG_REQUEST_DATA`, `LOG_ERASE`) to pull ULogs from the
flight controller; writes to a local log directory; the settings Log tab
provides Refresh / Fetch / Save As / Erase. Shares the MAVLink layer and
storage with log replay.

### Phase 6: Release

Tauri packaging (Linux/Windows), udev helper packaging and polkit policy,
offline bundle (sample DEM + geoid grids), documentation, plugin interface
stabilization.

## 8. RTK and RTCM Forwarding Design

**Data flow**: RTK base receiver (USB serial) → `rtk::source` reads the RTCM3
byte stream → `rtk::rtcm` parses frames and verifies CRC → `rtk::forward`
wraps them into MAVLink `GPS_RTCM_DATA` → sent over the flight-controller
link → the FC GPS driver injects into the onboard RTK receiver.

**Source abstraction**: `RtcmSource` trait. Phase 2 ships the serial
base-station source; NTRIP client, TCP/UDP forwarding are later optional
implementations. Only one source may inject at a time.

**Forwarding essentials**

- `GPS_RTCM_DATA` payload limit is 180 bytes per packet; longer RTCM frames
  are fragmented per protocol (max 4 fragments, sequenced). Boundary cases
  (exact multiples of 180) follow the MAVLink spec; unit tests required.
- When coexisting with QGC, QGC may also inject RTCM: provide an "RTCM
  injection" enable switch alerting on multi-source detection to avoid dual
  injection.
- Bandwidth: RTCM volume depends on message type and constellation count; a
  slow telemetry radio may saturate. Track actual B/s, show utilization in
  the UI, and support message-set selection (e.g. prefer MSM4 over MSM7).
- Base-station data stalls longer than a configurable threshold (default 5 s)
  raise an alert.

**Base-station configuration** (first target: u-blox ZED-F9P; others via the
trait)

- Survey-in (minimum duration, target accuracy) or fixed-coordinate mode;
  output RTCM3 message set; port output config; save to RAM/Flash.
- Preview before apply; read back to verify after apply.

**Status display**

- Base: mode, survey-in progress and current accuracy, satellite count,
  per-type RTCM message rate, data age, cumulative bytes.
- Airborne: `fix_type` (RTK Float = 5, RTK Fixed = 6), satellite count, HDOP,
  time to RTK Fixed.
- Top status bar and alerts: green = Fixed, yellow = Float, red = no RTK or
  interrupted.

**Testing and acceptance**

- RTCM frame parser unit tests (straddled/coalesced/half frames, bad CRC)
  using recordings; reassembled fragments byte-identical; forwarding path
  verified with a mock MAVLink receiver.
- SITL has no real GPS receiver: only correct transmission can be verified,
  not RTK Fixed. Real-vehicle validation uses base + onboard receivers;
  record time-to-RTK-Fixed in open terrain (threshold calibrated after
  field data).
- Base-link interruption triggers an alert within 5 s.

## 9. USB Devices and udev Auto-Configuration

**Goal**: once a known USB device is plugged in, auto-identify it and create
stable device names and permission rules without hand-editing.

**Scope**: Linux native. Windows/macOS have no udev — device identification
and port-name mapping only. WSL2 requires usbipd-win to attach USB and udev
effect depends on the WSL systemd setting (verify separately).

**Flow**

1. Listen for USB serial hotplug; read VID/PID/serial/vendor/product.
2. Match against the device database (PX4/Pixhawk, u-blox, FTDI, CP210x,
   CH340), suggest a role (FC / RTK base / other); user confirms or overrides.
3. Generate a rule preview: permissions (`MODE`/`GROUP` or `TAG+="uaccess"`),
   stable symlink `/dev/maggcs/<role>`, ModemManager ignore marker
   (`ID_MM_DEVICE_IGNORE`) so the modem manager does not grab the port.
4. Multiple devices with the same VID/PID: prefer serial number; without a
   serial, distinguish by physical port path and note that the rule binds to
   the interface.
5. Privileged install: via polkit (`pkexec`) invoke a dedicated helper that
   only writes app-named files under `/etc/udev/rules.d/` and reloads rules;
   the main process never runs as root. Headless mode exposes a CLI subcommand.
6. Support listing/uninstalling/rolling back installed rules; on failure
   print the manually executable command.

**Acceptance**

- Rule generator golden-file tests: given a device, the rule text matches
  character for character.
- The helper rejects writing other paths and other file names (security
  tests).
- With a real device plugged in, `/dev/maggcs/<role>` appears within 5 s and
  is openable without sudo; after uninstall, rules and symlinks are removed.

## 10. Task Card: DEM Preprocessing (`tools/dem-prep`)

- Input: GeoBC LiDAR point clouds (LAS 1.4, NAD83(CSRS)/UTM10, CGVD2013,
  CGG2013); if a raster product is already available, skip rasterization and
  only verify metadata and datum.
- Output: DTM (class-2 points, COG, 1–2 m), DSM (class-1 point maxima, canopy
  safety layer), metadata JSON (project, date, density, nominal accuracy,
  datum, void mask).
- Requirement: voids and water bodies are explicitly marked, never silently
  interpolated.
- Acceptance: checkpoint error within the nominal accuracy in the metadata;
  unit tests cover voids and coordinate datum.

## 11. AI Collaboration Rules

1. One small task at a time; never generate a whole phase in one shot.
2. First output struct/enum/trait/function signatures and error types;
   get confirmation before implementing.
3. Every task carries an automatable or numerically measurable acceptance
   criterion; after finishing, self-check and state how it was verified.
4. Backend modules must ship with unit tests or SITL test scripts; frontend
   tasks ship with screenshots and manual verification steps.
5. Privilege escalation, serial writes, and base-station config writes require
   an upfront failure/risk analysis (wrong config, wrong device, etc.).
6. Rust production code uses `Result`, never `unwrap`; TypeScript strict mode;
   magic numbers extracted to constants or config.
7. Sync README/docs per module; record important decisions as ADRs; when in
   doubt, ask or present option comparisons.
8. Execution order: MAVLink & telemetry → UI shell → Mission protocol →
   waypoint editing → RTK → DEM & height conversion → Survey planning →
   magnetic data & QC.

## 12. Startup Instructions

> Start from Phase 0. First create the workspace skeleton and `AGENTS.md`,
> output the `core` crate's struct/enum/error types and function signatures
> (including the `mavlink` and `devices` modules), wait for confirmation, then
> implement UDP connection management and HEARTBEAT/GLOBAL_POSITION_INT
> parsing, and attach the SITL integration test script.
>
> On completion output: file tree, key code, run/verify method, and the
> self-check against Phase 0 acceptance criteria.

## 13. Open Items

1. RTK base-station model (u-blox ZED-F9P class?) — determines the first
   supported configuration protocol. **(Confirmed: u-blox ZED-F9P)**
2. Whether NTRIP is needed as an RTCM source.
3. Frontend framework and component library (ADR-010).
4. License: Apache-2.0 or GPLv3.
5. Whether GeoBC provides point clouds or rasters, and the coverage area.
6. Phase acceptance values must be calibrated on SITL and real flight data.