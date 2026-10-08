# ADR-013: Planning model, absolute heights, and compile-time frames

- Status: Accepted (implemented)
- Date: 2026-10-07
- Supersedes: none. Related: ADR-003 (QGC coexistence), ADR-005 (GCS-side
  terrain following), ADR-006 (height datums). Note: ADR-012 is taken by the
  desktop webview CSP, so the height policy is ADR-013 (the map-tool ADR
  proposed as "012/013" elsewhere becomes ADR-014).

## Decision

- A planned mission is modelled separately from the wire mission, in
  `core::plan::PlannedMission`: HOME plus waypoints whose altitude is an
  absolute, datum-tagged `height::Height` (AMSL/EGM96).
- The wire frames are chosen at the boundary, in
  `PlannedMission::compile(FramePolicy)`. `FramePolicy::GlobalInt` writes AMSL
  `z`; `FramePolicy::GlobalRelativeAltInt` writes `z = AMSL − HOME AMSL`.
- `MAV_FRAME_GLOBAL_TERRAIN_ALT` is never emitted. Terrain following is
  precomputed by the GCS (ADR-005).
- QGC complex items are carried as opaque `PlanBlock` provenance and are not
  compiled here; their geometry already lives in `waypoints`.

## Rationale

- A relative frame's `z` silently changes meaning when HOME moves; keeping the
  plan in AMSL makes it unambiguous and lets one plan compile to either frame
  with identical geometry (the acceptance in `core::plan` tests).
- Storing absolute heights matches the datum discipline (ADR-006) and makes a
  future mission archive reproducible (parameters + DEM version + plan).
- Keeping the compile step explicit means the GCS, not the flight controller,
  owns the altimetry, so the flight is deterministic and auditable.

## Consequences

- Planning code (frontend store, survey generator) should hold AMSL heights;
  the frontend `MissionItem`-based editor is migrated to `PlannedMission` as a
  follow-up. Until then the frontend still converts altitudes itself
  (`frontend/src/mission/altitude.ts`).
- Ellipsoid or CGVD2013 planning would need a `GeoidModel` to reach AMSL;
  `compile` therefore rejects non-AMSL inputs with `PlanError` (Phase 3 adds
  the geoid grids, ADR-006 height chain).
- The wire model (`MissionFrame`, `MissionItem`) is unchanged; this ADR adds a
  layer above it.
