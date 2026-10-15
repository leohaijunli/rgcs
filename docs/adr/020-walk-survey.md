# ADR-020: Walk magnetometer survey — DroneCAN instruments over MAVLink

- Status: Accepted (2026-10-15; operator-confirmed instrument/GNSS data path)
- Date: 2026-10-15
- Relates to: plan `docs/plans/lifecycle-survey-hrdem-plan.md` §5B WP **W0**;
  ADR-003 (QGC coexistence), ADR-011 (link state), ADR-015 (DSP in core)
- Supersedes: the plan's original §5B assumption that the walk survey has **no
  flight controller / no MAVLink** and that the GCS parses instrument frames
  itself (plan line 9, §5B table).

## Decision (operator-provided, 2026-10-15)

1. **All sensor data is on MAVLink.** The walking rig carries a **PX4 that is
   used as an instrument hub only — it is not flying.** PX4 reads the DroneCAN
   magnetometer and the RTK GNSS and republishes them on MAVLink; the GCS
   consumes magnetometer and GNSS data from that existing stream. The GCS does
   **not** open serial/TCP/Bluetooth links to the instruments and does **not**
   parse vendor frames. In a walk session the GCS **never arms, takes off, or
   sends flight commands**; the flight-safety WPs (L2 RTL/Hold, L5 arm, U4
   command grading, battery/RTL checks) do **not** apply to the walk profile.
2. **Magnetometers are DroneCAN nodes** that output a magnetometer sensor
   measurement, surfaced by PX4 on MAVLink. Per project there is a **single**
   sensor (RM3100 **or** Bartington UAS-Mag — project alternatives, not a
   gradient pair). No gradient (dual-sensor) survey.
3. **GNSS is a Holybro H-RTK ZED-F9P on PX4, with RTK.** Position, fix type
   and time come from MAVLink. The pilot survey uses RTK.
4. **Collection mode**: continuous walking (station mode remains a W4 feature
   but is not the primary mode).
5. **Data path**: **PX4 SD/ULog is the primary full-rate record**; the GCS also
   consumes live magnetometer + position over MAVLink for guidance and live QC.
6. **Delivery format**: **CSV** as the initial interchange format (fields
   below; finalised in W2). Other formats (XYZ etc.) not required yet.
7. GCS responsibilities stay: **navigation guidance, live recording/QC,
   diurnal correction (M7/M8), archive**. DroneCAN node configuration, node
   IDs, parameters and mag calibration remain with PX4/QGC (AGENTS.md out of
   scope) — the GCS never writes DroneCAN/parameter config.

## Context

The plan's §5B was written assuming a controller-less walk survey: an external
GNSS speaking NMEA and an external magnetometer on serial/TCP/BT, with the GCS
parsing frames. The operator's answer reverses that: the instruments are
integrated on a PX4 (carried on the walking rig, **idle — not flying**) via
**DroneCAN** and published over **MAVLink**, and the positioning is RTK. This
makes the walk-survey *data path* identical to the UAV path while keeping the
*session* a non-flight activity (no arm/RTL), and lets W1 reuse the existing
decoders instead of building a new serial/NMEA stack.

Reusable code already in `core`:

- `core::telemetry` normalizes `GPS_RAW_INT` (incl. `fix_type`, `lat/lon/alt`,
  `eph/epv`, `satellites_visible`) and `GLOBAL_POSITION_INT`
  (`crates/core/src/telemetry/mod.rs`).
- `core::signals` extracts magnetic messages (`HIGHRES_IMU`, `RAW_IMU`, …) and
  converts the wire unit to nanotesla (`crates/core/src/signals/extract.rs`,
  `signals/mod.rs`).

## Instruments

| Role | Device | Link | Data on MAVLink | Notes |
|---|---|---|---|---|
| Magnetometer (per project) | RM3100 **or** Bartington UAS-Mag, 3-axis, DroneCAN node | DroneCAN → PX4 → MAVLink | **`HIGHRES_IMU`** mag fields (X/Y/Z, gauss → nT) as the primary/only source; sensor 1 | single sensor, **PX4-calibrated**, ≈**100 Hz**; client supplies the model per project; no vendor protocol in repo |
| GNSS / RTK | Holybro H-RTK ZED-F9P (on PX4) | PX4 → MAVLink | `GPS_RAW_INT` (`fix_type` RTK float=5/fixed=6), `GLOBAL_POSITION_INT`, `GPS_STATUS`/`GPS2_RAW` | RTK in use; RTCM injection already handled by `core::rtk` (ADR-008) |
| Time | PX4 | MAVLink | `SYSTEM_TIME`, `GPS_RAW_INT.time_usec` | host↔GPS↔FC clock alignment, reuse M8 |

No vendor private protocol is assumed or documented here; the transport is
DroneCAN/MAVLink (open). Per plan §0 rule 9 and rule G (M7), vendor documents
and firmware are never committed.

## Responsibility split

| Aspect | PX4 / QGC | MagGCS (this project) |
|---|---|---|
| DroneCAN node config, node IDs, params | yes | **no** |
| Mag calibration (`CAL_MAG*`) | yes | no (loads calibration JSON for tests/records only) |
| Full-rate log | **yes** (SD/ULog) | no |
| Live mag + RTK position consumption | produces | **yes** |
| Navigation guidance (cross-track, speed, progress) | no | **yes** (W3) |
| Diurnal correction + QC | no | **yes** (M7/M8, W5) |
| Archive/manifest | no | **yes** (M4) |

## Delivery format (CSV, initial — finalised in W2)

Delimited text, one row per sample, UTC timestamps:

| Column | Unit | Source |
|---|---|---|
| `utc` | ISO-8601 (`YYYY-MM-DDThh:mm:ss.sssZ`) | GNSS/PX4 time (M8 mapping) |
| `latitude` / `longitude` | deg (WGS 84) | `GLOBAL_POSITION_INT`/`GPS_RAW_INT` |
| `alt_ellipsoid` | m | GNSS ellipsoid height |
| `alt_amsl` | m | `GLOBAL_POSITION_INT.alt` (AMSL) |
| `field_total` | nT | mag vector magnitude |
| `mag_x`, `mag_y`, `mag_z` | nT | `HIGHRES_IMU` mag (converted) |
| `fix_type` | enum | `GPS_RAW_INT` |
| `hdop` | – | `GPS_RAW_INT` |
| `satellites` | count | `GPS_RAW_INT` |
| `line_id` | string | survey session (W4) |
| `quality` | flag | W5 QC |

A raw-record sidecar (the recorder's lossless stream) is kept alongside the
CSV so the CSV is a view, not the source of truth (W2).

## Consequences

- **W1 is rescoped** from "NMEA/vendor-frame parser + serial/TCP/BT transport"
  to a **MAVLink instrument source**: extract typed mag + RTK-GNSS samples from
  the existing telemetry/signals decoders, add timestamping and rate/staleness
  monitoring (reuse A3/L3). No NMEA parsing, no serial transport. The synthetic
  generator emits **MAVLink mag + GPS** messages instead of NMEA frames.
- **W2** records mag/GNSS samples (plus raw MAVLink frames already in the tlog)
  and emits the CSV above; the "raw NMEA + instrument frames" wording is gone.
- **W3** takes its GNSS input from MAVLink (RTK); the walk-navigation math is
  unchanged.
- **W6** preflight "instrument online & rate" checks become MAVLink stream checks
  (mag rate, GPS fix/RTK, time alignment), not serial link checks.
- **§5B line 9 / the §5B comparison table are corrected**: the walk survey
  **does** use PX4 + MAVLink; "no controller" is removed.
- No `core::instruments` serial layer is needed; the work lands as a thin
  adapter over `core::telemetry` + `core::signals`.

## Open items

- **Resolved (operator, 2026-10-15)**: the magnetometer is read from
  **`HIGHRES_IMU`** (already **PX4-calibrated**), sensor 1, at ≈**100 Hz**.
  Delivered `field_total`, `mag_x/y/z` are therefore the calibrated values.
  `RAW_IMU` remains a non-primary supported fallback in the decoder.
- Final CSV column set / ordering and delimiter (W2). Whether an XYZ/other
  export is later required.
- Confirm the ZED-F9P is the only GNSS source (no separate external logger).
- Pole height (the walk profile uses a fixed rod height) — a project/operator
  input, recorded in the manifest (W4/W6).

## Addendum (2026-10-15): same sensors and data flow as the UAV

Operator clarification: the walk survey uses the **same sensors and the same
data flow as the drone — the only difference is that the rig is held in hand**
instead of flying.

- It is the **same airframe/payload**: the same PX4, the same DroneCAN
  magnetometer, the same ZED-F9P RTK GNSS, the **same MAVLink message set and
  rates**, and the same recording stack. There is **no walk-specific sensor
  protocol, sensor configuration, or data pipeline**.
- "PX4 as an instrument hub only" in the Decision above means *this very PX4*
  is carried but **idle (not flying)**; it is not a different hub or a
  parallel instrument bus.
- What differs is purely **operational/session** behaviour, not the data path:
  no arm/takeoff/flight commands, low-speed heading suppression, hand-carried
  magnetic-cleanliness checks, and the walk navigation/QC/profiles.

Consequences for the WPs:

- **W1** is the *same* MAVLink instrument access the UAV path uses — a thin
  adapter over `core::telemetry` + `core::signals`; no walk-specific decoding.
- **W2** **extends the existing A4 recorder** (tlog + extracted mag/GNSS
  samples + events + CSV) rather than introducing a new pipeline.
- **W3** consumes the same MAVLink RTK position; the navigation math is
  unchanged.

## Verification

- W0 is a decision WP: the acceptance is the operator answering the open
  questions and the ADR/instrument tables above (done 2026-10-15). W1/W2 carry
  the automated tests (synthetic MAVLink streams) once implemented.
