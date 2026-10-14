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

## Addendum — landing and HOME target the terrain, not the HOME plane (2026-10-14)

Operator report (SITL): after `Land` completed, the aircraft held altitude over
lower ground instead of touching down — the landing target was the HOME plane.

- `MAV_CMD_NAV_LAND` no longer goes out as `[0.0; 7]`. PX4 reads `param7` as an
  **AMSL** altitude (same as takeoff), so the GCS now sends the ground under
  the vehicle: `param5/6` = current position, `param7` = terrain AMSL sampled
  by the frontend (mesh → DEM tile → the current fix as the last resort, which
  is right for a vehicle already on the ground).
- The drape places a **landing waypoint** (`NAV_LAND` / `NAV_VTOL_LAND`, i.e.
  21/85) on the ground (`DrapeOptions.land_clearance_m`, default 0) rather
  than `agl_m` above it, and skips the DSM clearance for it — a clearance
  raise would strand the landing above the surface. Locked items still keep
  their pin, and voids/coverage behave like any other waypoint. Both paths
  report a `landed` count (Pattern step).
- `DO_SET_HOME` no longer takes the vehicle's own altitude. The frontend
  samples the terrain under the **clicked point** (`terrain.samplePointAmsl`,
  mesh → DEM) and sends it as `param7`; the command falls back to the current
  fix only when no terrain is loaded. Setting home while airborne over the
  next site used to bake a home plane far above the ground, and every
  HOME-referenced height (relative legs, the landing target) then sat too
  high.
- Still FC-side: PX4's own land detector decides touchdown; nothing here
  changes `MPC_LAND_*` or the EKF. If a SITL landing still holds altitude,
  the remaining suspect is the FC's landing parameters, not the GCS payload.
