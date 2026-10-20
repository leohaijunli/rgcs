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

## Addendum (2026-10-17): the CGG2013 stage is real (plan H7)

The conversion chain this ADR defined is now backed by the **real NRCan
CGG2013 grid** (via the PROJ-data CDN mirror `ca_nrc_CGG2013n83.tif`,
EPSG:9246, nominal 0.03 m; OGL — Canada). `dem-prep geoid` converts it to
the committed `cgg2013_bc_sw.mggeoid` extraction and `core::height`
embeds it (`cgg2013_model()`), so `H + N(CGG2013) = h` runs offline and
fails closed outside the extraction. Sign convention verified against
PROJ `cct` (EPSG:9246): the grid stores `N = h − H` — the standard
undulation `core::height` defines, no negation. Two-stage control-point
checks against PROJ: CGG2013 stage within ±0.02 m, EGM96 stage (15″
reference vs the 5′ extraction) within ±0.2 m. The pre-H7 "orthometric H
≈ AMSL" shortcut was measured at 0.52–1.27 m across BC-SW — beyond the
0.5 m budget — and is replaced by the real chain plus a fail-closed AMSL
export gate; details in the lifecycle plan's H7 execution record.
