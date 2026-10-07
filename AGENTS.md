# MagGCS — Agent Instructions

Open-source ground control station for survey/geophysics drone operations,
used side-by-side with QGroundControl. All user-facing code and docs are in **English**.

## Positioning

- MagGCS differentiates on: survey-grade mission planning, real-time data/QC,
  and open integration (QGC `.plan` compatibility, headless mode, REST/WebSocket API).
- First target airframe: multirotor.
- **Out of scope (handled by QGC):** sensor calibration, airframe setup, radio
  setup, firmware flashing. Do not build these.

## Repository layout

- `crates/core` — library crate: scaffolded `mavlink`, `telemetry`, `mission`,
  `commands`, `devices`, `height`; planned `terrain`, `survey`, `mag`, `qc`,
  `rtk`.
- `crates/app-tauri` — desktop entry (scaffolded).
- `crates/server` — headless: REST + WebSocket (placeholder only; not scaffolded).
- `helpers/udev-installer` — privilege-raising helper (placeholder only; udev rules only).
- `tools/dem-prep` — LAS → DTM/DSM → COG + metadata (placeholder only).
- `frontend` — React UI (scaffolded).
- `scripts/sitl` — PX4 SITL integration test scripts.
- `testdata` — small DEM, ULog, `.plan`, RTCM recordings.
- `docs/adr` — architecture decision records.

## Height and coordinate conventions

- Three height datums in use: CGVD2013 orthometric (`H`, BC LiDAR),
  WGS84 ellipsoid (`h`, Cesium), AMSL (`PX4 MAV_FRAME_GLOBAL`).
  Conversion chain: `H + N(CGG2013) = h`, `h − N(EGM96) = AMSL`.
- **Never** represent an absolute height as a bare `f64` in `core`. Use the
  datum-tagged `height::Height` type. Conversions live in `core::height` only.
- AGL is relative to DTM by default; a separate minimum-clearance check
  relative to DSM may be added.
- Acceptance: known control points converted through the chain must be within
  `< 0.5 m` of PX4 AMSL (threshold pending field validation).

## Communication with QGC

- MAVLink 2, independent system/component IDs, UDP or mavlink-router for
  traffic splitting. IDs are validated against PX4 docs (ADR-003).

## Engineering rules (AI collaboration)

1. Do one small task at a time; never generate a whole phase in one shot.
2. First output struct/enum/trait/function signatures and error types;
   get confirmation before implementing.
3. Every task carries an automated or numerically measurable acceptance
   criterion; after finishing, self-check and state how it was verified.
4. Backend modules must ship with unit tests or SITL test scripts; frontend
   tasks ship with screenshots and manual verification steps.
5. Privilege escalation, serial writes, and base-station config writes require
   a failure/risk analysis up front (wrong config, wrong device, etc.).
6. Rust production code uses `Result`, never `unwrap`; TypeScript strict mode;
   magic numbers extracted to constants or config.
7. Update README/docs per module; record important decisions as ADRs; when
   unsure, ask or present option comparisons.
8. Execution order: MAVLink & telemetry → UI shell → Mission protocol →
   waypoint editing → RTK → DEM & height conversion → Survey planning →
   magnetic data & QC.
