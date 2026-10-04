# ADR-004: Terrain sources — local GeoTIFF default, Cesium ion optional

- Status: Draft
- Date: 2026-10-03

## Decision

- The default elevation source is local GeoTIFF (Cloud Optimized GeoTIFF
  preferred) loaded via an `ElevationSource` trait in `core::terrain`.
- Cesium ion terrain is an optional, network-dependent source only.
- Field sites often have no internet; the DEM lives on the operator's machine.

## Rationale

- Survey-grade planning needs deterministic, versioned terrain data.
- DEM version is stored with each mission archive (Phase 3).

## Consequences

- `core::terrain` must define the `ElevationSource` trait and a GeoTIFF
  implementation; network sources are behind a feature/plugin boundary.
- Terrain-aware functions only depend on the trait, never on the file format.