# ADR-005: Terrain following — ground-station precomputation first

- Status: Draft
- Date: 2026-10-03

## Decision

- Path A (ground station precomputes draped waypoints from the DEM) is the
  primary terrain-following strategy.
- Path B (onboard terrain following) is only cross-checked later; not
  implemented first.

## Rationale

- Precomputation keeps the flight deterministic, auditable stub
  (mission archive = parameters + DEM version + waypoints), and avoids
  dependency on the FC's terrain database.

## Consequences

- Survey planner must densify and smooth lines against the DEM with climb-rate
  and slope constraints (Phase 3).
- Onboard terrain data is not required for takeoff.