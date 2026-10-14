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

## Addendum — drape reference is the live 3D mesh first (2026-10-14)

Operator bug: in `terrain` altitude mode the Pattern input (a survey AGL) was
labelled in the waypoint list as absolute AMSL, and the drape only ever ran
against a 2D dem-prep tile — with just the 3D mesh switched on, the requested
`40 m AGL` stayed a bare `40.0 m AMSL`.

- The reference is now picked **mesh → 2D DEM tile → nothing**. The mesh is
  what the operator sees, so the survey AGL is measured against it. It is a
  Cesium tileset served to the webview, which `core::terrain::drape` cannot
  read, so the mesh path is a frontend twin (`mission/meshDrape.ts`) with the
  same contract: command items and locked waypoints pass through, an
  unsampled waypoint is left alone and counted (never guessed), optional
  return-climb floor.
- Datum: an `ellipsoidWgs84` tileset (built with `--geoid`) serves `h`, so
  `AMSL = h − N(EGM96)` per waypoint (one `geoid_sample` call); an
  orthometric tileset already serves CGVD2013 `H` (≈ AMSL, ≤ ~20 cm). Stored
  waypoints stay **absolute AMSL** (ADR-013) — only `altitude.meters` moves,
  and the render offset (`meshOffset`) is unchanged.
- A tileset carries no DTM/DSM pair, so the mesh path has **no clearance
  check**: the report says "no DSM" instead of showing a margin that was not
  measured. The operator must keep a DSM-capable DEM tile when clearance
  matters.
- Waypoint rows now show both numbers, AGL leading in terrain mode; when no
  surface is loaded the row says "AGL unknown" rather than a bare AMSL that
  looks terrain-following. This surfaced a second defect: both map ground
  effects built the sample pair as `[y/1e7, x/1e7]` — `x` is latitude and `y`
  is longitude — so every point missed the DEM and the rows said "unknown"
  with terrain loaded. Fixed with one pure helper, `mission/compile.ts`
  `itemLatLon`, guarded by `scripts/check-coords.mjs`. A failed drape records its input fingerprint so
  the error line cannot retry the same failing input forever (the previous
  behaviour hammered the backend every 600 ms).
- Provenance (ADR-004) in exported plans: `maggcsDemVersion` as before for
  the tile path, plus `maggcsTerrainRef: mesh:<folder>` for the mesh path —
  a tileset has no content hash to record.
