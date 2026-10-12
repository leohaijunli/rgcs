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

## Addendum (2026-10-12): Plan B — offline quantized-mesh 3D terrain

The 3D globe shows real relief without any network source: `dem-prep
terrain` turns a processed DEM tile folder into a local **quantized-mesh
tileset** (`layer.json` + `{z}/{x}/{y}.terrain`, gzip-compressed), a
loopback HTTP server in the desktop app serves it, and the map swaps its
`CesiumTerrainProvider` to it. Imagery drapes the mesh; areas outside the
DEM fall back to the smooth ellipsoid.

### Pipeline and tile counts

- `dem-prep process <las|tif folder> --resolution 4` → per-tile
  `{dtm.tif,dsm.tif,dem.json}` (ADR-004 original); then `dem-prep terrain
  <processed-dir> --out <dir> [--maxzoom N]` → tileset.
- The pyramid is complete from `z0` up: each level adds 4× tiles of the
  previous, and tile count grows with DEM extent and resolution. A
  4 × 2.8 km fixture at 4 m grid → z17, **1124 tiles / 2.7 MiB**; the
  committed BC sheet set (4 × ~2.6 km LiDAR tiles) → z19, 14,882 tiles.
  `--maxzoom` caps the deepest level for storage budgets; the default is
  derived from the DEM resolution (`log2(180·111320 / (32·res_m))`).

### Layer.json contract (verified against Cesium 1.120.1 source)

- **`available` entries are JSON objects** `{"startX","startY","endX",
  "endY"}` — Cesium reads them property-style (`range.startX`); plain
  4-number arrays decode to `undefined`/`NaN` availability and no tiles
  are ever requested. Enforced by the `AvailabilityRect` struct in
  `tools/dem-prep/src/terrain.rs`.
- **`CesiumTerrainProvider.fromUrl(url)` derives `layer.json` itself**
  (`resource.appendForwardSlash()` + a derived resource). The contract is
  therefore the tileset **base URL with a trailing slash**
  (`http://127.0.0.1:18823/`), and `terrain3d_start` returns exactly that —
  not the manifest file URL.
- Tiles use the TMS Y (0 = south); Cesium mirrors availability with
  `yStart = yTiles - range.endY - 1`, so no server-side flip is needed.

### Heights: orthometric CGVD2013 as-is

Tiles store the DTM **orthometric (CGVD2013) heights directly** — no
geoid adjustment in `dem-prep terrain`. Cesium then places them relative
to the WGS84 ellipsoid, i.e. offset by the (currently unapplied) geoid
undulation. This is a deliberate interim choice: the 3D globe is visual
context, not a survey-height source, and the full
`H + N(CGG2013) = h` conversion (AGENTS height chain) is gated on the
CGG2013 geoid grid (issue N2). The core/height conversion remains the
single source of survey-grade altitudes.

### Server and security (app-tauri, port 18823)

Hand-rolled loopback HTTP server — no new runtime dependencies. GET/HEAD/
OPTIONS only; path sanitization rejects `..`, `%`, `\` and NUL and
`canonicalize().starts_with(root)` gates every request; only `layer.json`
and `*.terrain` are served, `Connection: close`, a wake-up connect on
shutdown, `Drop` joins the accept thread. The frontend only reaches
`http://127.0.0.1:18823` (CSP `connect-src` pin, ADR-012). Commands:
`terrain3d_start(dir) → base URL`, `terrain3d_stop`, `terrain3d_status`,
`terrain_discover`.

### Coverage gaps

`layer.json` availability gates every Cesium request, so outside the DEM
extent the globe renders the **ellipsoid** (imagery drapes it, no holes,
no errors); the server 404s anything a client asks outside the declared
ranges Conversely, requesting terrain where none exists is safe by
construction.

### Verification

- Real-data smoke test (`TERRAIN_SMOKE_DIR=<tileset> cargo test -p
  maggcs-app --lib serves_a_real -- --ignored`) caught the availability
  shape bug before release.
- E2E `frontend/scripts/verify-terrain3d.mjs` (13 checks): toggle on/off,
  real `layer.json` + tile fetches, start-failure rollback, and a WebGL
  **pixel diff asserting the mesh visibly changes the render vs the
  ellipsoid** (24% on the synthetic fixture).
- Synthetic fixture `scripts/make_hill_las.py` (z 20..662 m) exists
  because the committed BC test DEM spans only -1.8..19 m — flat, so no
  screenshot could ever show relief; the fixture makes the "you can see
  terrain" claim reproducible.