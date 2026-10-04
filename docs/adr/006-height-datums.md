# ADR-006: Height datum tagging in core

- Status: Draft
- Date: 2026-10-03

## Decision

- `core` never represents an absolute height as a bare `f64`.
- All heights carry a datum tag via `core::height::Height { datum, meters }`.
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