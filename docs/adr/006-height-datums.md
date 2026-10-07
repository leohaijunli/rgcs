# ADR-006: Height datum tagging in core

- Status: Accepted (implemented)
- Date: 2026-10-03
- Accepted: 2026-10-07 — Datum-tagged `height::Height` type; working datum AMSL.

## Decision

- `core` never represents an absolute height as a bare `f64`.
- All heights carry a datum tag via `core::height::Height { datum, meters }`.
- **Working datum: AMSL** (`h − N_EGM96`), matching PX4 (`MAV_FRAME_GLOBAL`)
  and QGroundControl. Telemetry altitudes are tagged AMSL at ingest; the
  UI displays AMSL. Ellipsoid height (`h`) is used only inside the Cesium
  scene and in conversions; CGVD2013 orthometric (`H`) is used for BC LiDAR
  survey data.
- Datums in use: WGS84 ellipsoid (`h`), AMSL EGM96 (`h − N_EGM96`),
  CGVD2013 orthometric (`H`), with `H + N(CGG2013) = h`.
- All conversions live in `core::height` (geoid models CGG2013, EGM96).
- Relative altitudes (deltas) are `f64` meters and clearly documented as deltas.

## Rationale

- Mixing datums silently is the #1 source of survey data corruption.
- Conversion-chain acceptance: control points within `< 0.5 m` of PX4 AMSL.

## Consequences

- Telemetry fields that are absolute heights use `Height` with the appropriate
  datum (e.g. GLOBAL_POSITION_INT.alt → `Height { datum: AmslEgm96, .. }`).
- Geoid grid interpolation is deferred to Phase 3; the type surface is fixed now.