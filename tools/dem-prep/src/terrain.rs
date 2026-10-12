//! 3D terrain tileset: global-geodetic (EPSG:4326) tile math, the Cesium
//! `layer.json` manifest, and DTM sampling for quantized-mesh building
//! (plan B — local terrain draped on the 3D globe, ADR-004 addendum).
//!
//! Conventions, verified against Cesium's `CesiumTerrainProvider.js`:
//! - Level `z` holds `2^(z+1) × 2^z` tiles (level 0 = 2×1 world tiles).
//! - Files and `layer.json` `available` rects use **TMS y (0 = south)**;
//!   Cesium flips to its internal north-up y itself (`yTiles - y - 1`).
//! - Tiles are half-open `[west, east) × [south, north)`: a bbox edge that
//!   lands exactly on a tile boundary must not pull in the neighbour tile.
//! - `format` must start with `quantized-mesh-1.` (checked when Cesium
//!   parses `layer.json`), `available` is indexed by level with
//!   **object** descriptors `{startX, startY, endX, endY}` (Cesium reads
//!   `range.startX` — bare 4-number arrays yield `undefined`/NaN ranges,
//!   verified against `CesiumTerrainProvider.js`, the STK terrain docs
//!   and cesium-native's `AvailabilityRectangle`), and the `{y}` URL
//!   value is TMS y.
//! - Missing levels/tiles are simply absent from `available` — Cesium then
//!   reports "not available" and never requests them.

use std::collections::VecDeque;

use crate::cog::DemBounds;
use crate::error::DemError;

/// Metres per degree of latitude (same constant main.rs grids with).
const M_PER_DEG_LAT: f64 = 111_132.954;

/// Zoom levels past this would overflow u32 tile indices — meaningless for a
/// planet-scale grid; arithmetic clamps to it so nothing panics or wraps.
const MAX_ARITHMETIC_ZOOM: u8 = 30;

/// Cap for `maxzoom_for_resolution`: only absurd resolutions reach it — the
/// real driver is one mesh vertex (tile/32) per DEM cell.
const MAX_DESIRED_ZOOM: i64 = 24;

/// Quantized-mesh grid: `GRID_SEGMENTS + 1` vertices per tile edge.
pub const GRID_SEGMENTS: usize = 32;

/// One TMS tile coordinate (y counts from the south, matching file paths).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileCoord {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// `(x_tiles, y_tiles)` at level `z` — 2×1 at level 0, doubling per level.
fn level_dims(z: u8) -> (i64, i64) {
    let z = z.min(MAX_ARITHMETIC_ZOOM);
    (1i64 << (z + 1), 1i64 << z)
}

/// Geographic bounds of tile `(z, x, y)` (half-open: east/north exclusive).
///
/// # Panics
///
/// Never; `x`/`y` beyond the level simply yield out-of-world bounds (the
/// caller contract is `x < 2^(z+1)`, `y < 2^z`, as produced by
/// [`tiles_covering`]).
pub fn tile_bounds(z: u8, x: u32, y: u32) -> DemBounds {
    let (nx, ny) = level_dims(z);
    let tw = 360.0 / nx as f64;
    let th = 180.0 / ny as f64;
    let west = -180.0 + f64::from(x) * tw;
    let south = -90.0 + f64::from(y) * th;
    DemBounds {
        north: south + th,
        south,
        east: west + tw,
        west,
    }
}

/// All tiles at level `z` with a non-empty overlap with `bounds`, in row
/// order (y south→north, x west→east). Bounds are normalised (ordered,
/// clamped to the world) so malformed input cannot panic.
pub fn tiles_covering(bounds: &DemBounds, z: u8) -> Vec<TileCoord> {
    let (west, east) = (bounds.west.min(bounds.east), bounds.west.max(bounds.east));
    let (south, north) = (
        bounds.south.min(bounds.north),
        bounds.south.max(bounds.north),
    );
    let (west, east) = (west.clamp(-180.0, 180.0), east.clamp(-180.0, 180.0));
    let (south, north) = (south.clamp(-90.0, 90.0), north.clamp(-90.0, 90.0));

    let (nx, ny) = level_dims(z);
    let tw = 360.0 / nx as f64;
    let th = 180.0 / ny as f64;
    // Half-open tiles: first tile whose east edge passes `west`, last tile
    // whose west edge is still below `east`. dyadic edges keep this exact.
    let x0 = (((west + 180.0) / tw).floor() as i64).clamp(0, nx - 1);
    let x1 = ((((east + 180.0) / tw).ceil() as i64) - 1).clamp(0, nx - 1);
    let y0 = (((south + 90.0) / th).floor() as i64).clamp(0, ny - 1);
    let y1 = ((((north + 90.0) / th).ceil() as i64) - 1).clamp(0, ny - 1);
    let (x1, y1) = (x1.max(x0), y1.max(y0));

    let cols = (x1 - x0 + 1) as usize;
    let rows = (y1 - y0 + 1) as usize;
    let mut out = Vec::with_capacity(cols * rows);
    for y in y0..=y1 {
        for x in x0..=x1 {
            out.push(TileCoord {
                z,
                x: x as u32,
                y: y as u32,
            });
        }
    }
    out
}

/// Deepest zoom whose 33×33 vertex grid roughly matches the DEM resolution
/// (one vertex per cell ⇒ tile edge = 32 × resolution metres).
/// 1 m data → z19 (~38 m tiles), 30 m data → z14; clamped to
/// [`MAX_DESIRED_ZOOM`] so tiny resolutions cannot explode the tile count
/// (a `--maxzoom` override can still raise/lower it per run).
pub fn maxzoom_for_resolution(resolution_m: f64) -> u8 {
    // Junk (NaN/≤0) resolution cannot drive refinement — return 0 rather
    // than guessing; tiny-but-real resolutions clamp to MAX_DESIRED_ZOOM.
    if !resolution_m.is_finite() || resolution_m <= 0.0 {
        return 0;
    }
    let z = ((180.0 * M_PER_DEG_LAT) / (GRID_SEGMENTS as f64 * resolution_m)).log2();
    (z.round() as i64).clamp(0, MAX_DESIRED_ZOOM) as u8
}

/// One availability descriptor in `layer.json` `available` — **must
/// serialise as a JSON object**: Cesium reads `range.startX` /
/// `range.startY` / `range.endX` / `range.endY` properties, so bare
/// 4-number arrays produce `undefined` coordinates and NaN availability
/// ranges (no terrain ever loads).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityRect {
    pub start_x: u32,
    pub start_y: u32,
    pub end_x: u32,
    pub end_y: u32,
}

/// `layer.json` `available`: one descriptor per level `0..=maxzoom`
/// (TMS y, as Cesium flips it itself), or an empty level when nothing
/// intersects (Cesium then treats that level as unavailable).
pub fn availability_ranges(bounds: &DemBounds, maxzoom: u8) -> Vec<Vec<AvailabilityRect>> {
    (0..=maxzoom)
        .map(|z| {
            let tiles = tiles_covering(bounds, z);
            let Some(first) = tiles.first() else {
                return Vec::new();
            };
            let mut rect = AvailabilityRect {
                start_x: first.x,
                end_x: first.x,
                start_y: first.y,
                end_y: first.y,
            };
            for t in &tiles {
                rect.start_x = rect.start_x.min(t.x);
                rect.end_x = rect.end_x.max(t.x);
                rect.start_y = rect.start_y.min(t.y);
                rect.end_y = rect.end_y.max(t.y);
            }
            vec![rect]
        })
        .collect()
}

/// Relative URL of one tile file (`{z}/{x}/{y}.terrain` template values).
pub fn tile_url_path(t: &TileCoord) -> String {
    format!("{}/{}/{}.terrain", t.z, t.x, t.y)
}

/// The `layer.json` manifest Cesium fetches next to a terrain base URL.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LayerJson {
    pub tilejson: &'static str,
    pub name: String,
    pub version: String,
    pub format: &'static str,
    pub scheme: &'static str,
    pub projection: &'static str,
    pub tiles: Vec<String>,
    /// Oct-encoded vertex normals (always emitted — see `mesh`).
    pub extensions: Vec<&'static str>,
    /// `[west, south, east, north]` (GeoJSON order).
    pub bounds: [f64; 4],
    pub minzoom: u8,
    pub maxzoom: u8,
    /// Per-level availability — objects, see [`AvailabilityRect`].
    pub available: Vec<Vec<AvailabilityRect>>,
}

/// Build `layer.json` for a tileset covering `bounds` down to `maxzoom`.
/// `version` is the DEM content hash — cache-busting if a client ever uses
/// the `{version}` template (ours does not).
pub fn layer_json(name: &str, version: &str, bounds: &DemBounds, maxzoom: u8) -> LayerJson {
    LayerJson {
        tilejson: "2.1.0",
        name: name.to_string(),
        version: version.to_string(),
        // Cesium rejects anything not starting with "quantized-mesh-1.".
        format: "quantized-mesh-1.0",
        scheme: "tms",
        projection: "EPSG:4326",
        tiles: vec!["{z}/{x}/{y}.terrain".to_string()],
        extensions: vec!["octvertexnormals"],
        bounds: [bounds.west, bounds.south, bounds.east, bounds.north],
        minzoom: 0,
        maxzoom,
        available: availability_ranges(bounds, maxzoom),
    }
}

/// One DTM raster with geographic placement, ready for mesh sampling.
/// Values are north-up row-major (row 0 = `bounds.north`); voids are
/// filled with the nearest valid cell at construction — a visualization
/// aid only (the drape/clearance reader keeps rejecting voids, ADR-004).
pub struct PlacedRaster {
    bounds: DemBounds,
    width: usize,
    height: usize,
    values: Vec<f32>,
}

impl PlacedRaster {
    /// Wrap raw DTM values. `values` must be `width * height` cells,
    /// row-major north-up, cell centres covering `bounds`.
    ///
    /// # Errors
    ///
    /// [`DemError::Raster`] on empty/mismatched dimensions, inverted bounds,
    /// or non-finite coordinates.
    pub fn try_new(
        bounds: DemBounds,
        width: usize,
        height: usize,
        values: Vec<f32>,
    ) -> Result<Self, DemError> {
        if width == 0 || height == 0 {
            return Err(DemError::Raster("terrain raster has zero cells".into()));
        }
        if values.len() != width * height {
            return Err(DemError::Raster(format!(
                "terrain raster: expected {} samples, got {}",
                width * height,
                values.len()
            )));
        }
        if values.iter().any(|v| v.is_infinite()) {
            return Err(DemError::Raster(
                "terrain raster: infinite sample (voids must be NaN)".into(),
            ));
        }
        let finite = bounds.west.is_finite()
            && bounds.east.is_finite()
            && bounds.south.is_finite()
            && bounds.north.is_finite();
        if !finite || bounds.west >= bounds.east || bounds.south >= bounds.north {
            return Err(DemError::Raster(format!(
                "terrain raster: invalid bounds {bounds:?}"
            )));
        }
        let mut raster = Self {
            bounds,
            width,
            height,
            values,
        };
        if raster.values.iter().any(|v| v.is_nan()) {
            fill_voids_nearest(&mut raster.values, width, height);
        }
        Ok(raster)
    }

    /// DTM height at (lat, lon): bilinear over cell centres; outside the
    /// raster the edge height is returned (coarse ancestor tiles extend the
    /// survey area instead of punching holes), all-void rasters give 0.0.
    pub fn sample(&self, lat: f64, lon: f64) -> f32 {
        let b = &self.bounds;
        let dlat = (b.north - b.south) / self.height as f64;
        let dlon = (b.east - b.west) / self.width as f64;
        if !dlat.is_finite() || dlat <= 0.0 || !dlon.is_finite() || dlon <= 0.0 {
            return 0.0;
        }
        let lat = lat.clamp(b.south, b.north);
        let lon = lon.clamp(b.west, b.east);
        // Fractional position in cell-centre space, clamped to the centres
        // of the border rows/columns (edge clamp).
        let fy = ((b.north - lat) / dlat - 0.5).clamp(0.0, (self.height - 1) as f64);
        let fx = ((lon - b.west) / dlon - 0.5).clamp(0.0, (self.width - 1) as f64);
        let c0 = fx.floor() as usize;
        let r0 = fy.floor() as usize;
        let c1 = (c0 + 1).min(self.width - 1);
        let r1 = (r0 + 1).min(self.height - 1);
        let tc = fx - c0 as f64;
        let tr = fy - r0 as f64;
        let v = |r: usize, c: usize| f64::from(self.values[r * self.width + c]);
        let top = v(r0, c0) * (1.0 - tc) + v(r0, c1) * tc;
        let bottom = v(r1, c0) * (1.0 - tc) + v(r1, c1) * tc;
        let h = top * (1.0 - tr) + bottom * tr;
        // Only an all-void raster survives the fill as NaN.
        if h.is_nan() {
            0.0
        } else {
            h as f32
        }
    }
}

/// Fill every void with the value of the nearest valid cell: a multi-source
/// BFS flood over the grid (8-connected, O(cells)) — plateaus over voids
/// instead of pits, and neighbouring mesh tiles always agree (the flood is
/// deterministic). All-void rasters stay void ([`PlacedRaster::sample`]
/// then falls back to 0.0).
fn fill_voids_nearest(values: &mut [f32], width: usize, height: usize) {
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, v) in values.iter().enumerate() {
        if !v.is_nan() {
            queue.push_back(i);
        }
    }
    let Some(&first) = queue.front() else {
        return; // all void — nothing to flood from
    };
    let _ = first;
    while let Some(i) = queue.pop_front() {
        let value = values[i];
        let (r, c) = (i / width, i % width);
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (nr, nc) = (r as isize + dy, c as isize + dx);
                if nr < 0 || nc < 0 || nr as usize >= height || nc as usize >= width {
                    continue;
                }
                let ni = nr as usize * width + nc as usize;
                if values[ni].is_nan() {
                    values[ni] = value;
                    queue.push_back(ni);
                }
            }
        }
    }
}

/// The DTM source the mesh builder samples: one or more processed tile
/// rasters (a survey area can span several BC sheets).
pub struct TerrainSource {
    rasters: Vec<PlacedRaster>,
}

impl TerrainSource {
    pub fn new(rasters: Vec<PlacedRaster>) -> Self {
        Self { rasters }
    }

    /// Bounding box covering every raster (`None` when empty).
    pub fn union_bounds(&self) -> Option<DemBounds> {
        let mut rasters = self.rasters.iter().map(|r| r.bounds);
        let first = rasters.next()?;
        Some(rasters.fold(first, |acc, b| DemBounds {
            north: acc.north.max(b.north),
            south: acc.south.min(b.south),
            east: acc.east.max(b.east),
            west: acc.west.min(b.west),
        }))
    }

    /// Height at (lat, lon): the raster containing the point, else the
    /// nearest one (slivers between sheets resolve instead of dropping to
    /// 0.0), else 0.0 for an empty source.
    pub fn sample(&self, lat: f64, lon: f64) -> f32 {
        let mut best: Option<(f64, &PlacedRaster)> = None;
        for r in &self.rasters {
            let b = &r.bounds;
            if b.west <= lon && lon < b.east && b.south <= lat && lat < b.north {
                return r.sample(lat, lon);
            }
            let d = {
                let cl_lon = lon.clamp(b.west, b.east);
                let cl_lat = lat.clamp(b.south, b.north);
                (cl_lat - lat).powi(2) + (cl_lon - lon).powi(2)
            };
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, r));
            }
        }
        best.map_or(0.0, |(_, r)| r.sample(lat, lon))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The BC 1:5000 sheet from a real `dem.json`.
    fn bc_bbox() -> DemBounds {
        DemBounds {
            north: 48.6500937,
            south: 48.6374242,
            east: -123.3999894,
            west: -123.4249917,
        }
    }

    #[test]
    fn level0_and_1_tiles() {
        let a = tile_bounds(0, 0, 0);
        assert_eq!(
            (a.west, a.south, a.east, a.north),
            (-180.0, -90.0, 0.0, 90.0)
        );
        let b = tile_bounds(0, 1, 0);
        assert_eq!(
            (b.west, b.south, b.east, b.north),
            (0.0, -90.0, 180.0, 90.0)
        );
        let c = tile_bounds(1, 0, 0);
        assert_eq!(
            (c.west, c.south, c.east, c.north),
            (-180.0, -90.0, -90.0, 0.0)
        );
        let d = tile_bounds(1, 3, 1);
        assert_eq!((d.west, d.south, d.east, d.north), (90.0, 0.0, 180.0, 90.0));
    }

    #[test]
    fn tile_edges_are_exact_and_contiguous() {
        for z in 0..=12u8 {
            for x in 0..5 {
                let a = tile_bounds(z, x, 0);
                let b = tile_bounds(z, x + 1, 0);
                assert_eq!(a.east, b.west, "east/west seam z={z} x={x}");
                assert!(a.west < a.east);
            }
            let c = tile_bounds(z, 0, 0);
            let d = tile_bounds(z, 0, 1);
            assert_eq!(c.north, d.south, "north/south seam z={z}");
            assert!(c.south < c.north);
        }
    }

    #[test]
    fn covering_returns_the_tile_a_point_is_in() {
        let cases = [
            (0u8, 0u32, 0u32),
            (1, 1, 0),
            (5, 17, 12),
            (19, 164776, 403865),
        ];
        for (z, x, y) in cases {
            let b = tile_bounds(z, x, y);
            let cx = (b.west + b.east) / 2.0;
            let cy = (b.south + b.north) / 2.0;
            let point = DemBounds {
                north: cy,
                south: cy,
                east: cx,
                west: cx,
            };
            assert_eq!(
                tiles_covering(&point, z),
                vec![TileCoord { z, x, y }],
                "point-in-tile z={z}"
            );
        }
    }

    #[test]
    fn covering_tiles_intersect_the_bbox_and_form_a_block() {
        let bb = bc_bbox();
        for z in [0u8, 3, 7, 12, 15, 19] {
            let tiles = tiles_covering(&bb, z);
            assert!(!tiles.is_empty(), "z={z}");
            for t in &tiles {
                let tb = tile_bounds(t.z, t.x, t.y);
                assert!(
                    tb.west < bb.east
                        && tb.east > bb.west
                        && tb.south < bb.north
                        && tb.north > bb.south,
                    "z={z} tile {:?} does not intersect {:?}",
                    (t.x, t.y),
                    bb
                );
                assert!((-180.0..=180.0).contains(&tb.west));
                assert!((-90.0..=90.0).contains(&tb.south));
            }
            // Rectangular block: count == x-span × y-span.
            let xs: Vec<u32> = tiles.iter().map(|t| t.x).collect();
            let ys: Vec<u32> = tiles.iter().map(|t| t.y).collect();
            let (xmin, xmax) = (
                xs.iter().min().copied().unwrap(),
                xs.iter().max().copied().unwrap(),
            );
            let (ymin, ymax) = (
                ys.iter().min().copied().unwrap(),
                ys.iter().max().copied().unwrap(),
            );
            assert_eq!(
                tiles.len(),
                ((xmax - xmin + 1) * (ymax - ymin + 1)) as usize,
                "z={z} block shape"
            );
            // The bbox south-west corner lies inside one of them.
            assert!(tiles.iter().any(|t| {
                let tb = tile_bounds(t.z, t.x, t.y);
                tb.west <= bb.west
                    && bb.west < tb.east
                    && tb.south <= bb.south
                    && bb.south < tb.north
            }));
        }
    }

    #[test]
    fn covering_level0_lands_in_the_west_tile() {
        // BC is in the level-0 tile covering lon [-180, 0) / lat [-90, 90).
        let tiles = tiles_covering(&bc_bbox(), 0);
        assert_eq!(tiles, vec![TileCoord { z: 0, x: 0, y: 0 }]);
    }

    #[test]
    fn half_open_edges_do_not_grow_a_phantom_tile() {
        // A bbox whose east edge sits exactly on a tile boundary must not
        // include the next tile (dyadic boundary).
        let b = tile_bounds(4, 5, 6);
        let exact = DemBounds {
            west: b.west,
            east: b.east, // exactly on tile 6's west edge
            south: b.south,
            north: b.north,
        };
        let tiles = tiles_covering(&exact, 4);
        assert_eq!(tiles.len(), 1, "expected only tile x=5, got {tiles:?}");
        assert_eq!(tiles[0].x, 5);
    }

    #[test]
    fn maxzoom_tracks_dem_resolution() {
        assert_eq!(maxzoom_for_resolution(1.0), 19);
        assert_eq!(maxzoom_for_resolution(0.5), 20);
        assert_eq!(maxzoom_for_resolution(5.0), 17);
        assert_eq!(maxzoom_for_resolution(30.0), 14);
        assert_eq!(maxzoom_for_resolution(10_000.0), 6, "10 km cells");
        assert_eq!(maxzoom_for_resolution(1e6), 0, "clamps low");
        assert_eq!(maxzoom_for_resolution(1e-6), 24, "clamps high");
        assert_eq!(maxzoom_for_resolution(f64::NAN), 0, "junk input");
        assert_eq!(maxzoom_for_resolution(0.0), 0, "zero resolution");
        assert_eq!(maxzoom_for_resolution(-5.0), 0, "negative resolution");
    }

    #[test]
    fn availability_rects_match_covering_tiles() {
        let bb = bc_bbox();
        let maxzoom = 6;
        let ranges = availability_ranges(&bb, maxzoom);
        assert_eq!(ranges.len(), usize::from(maxzoom) + 1);
        for z in 0..=maxzoom {
            let tiles = tiles_covering(&bb, z);
            let levels = &ranges[usize::from(z)];
            assert_eq!(levels.len(), 1, "z={z} one rect per level");
            let rect = levels[0];
            assert!(!tiles.is_empty(), "z={z}");
            for t in &tiles {
                assert!(
                    rect.start_x <= t.x
                        && t.x <= rect.end_x
                        && rect.start_y <= t.y
                        && t.y <= rect.end_y,
                    "z={z} tile {:?} outside rect {rect:?}",
                    (t.x, t.y)
                );
            }
            // Rect is exactly the span — no spare rows/columns.
            let xs: Vec<u32> = tiles.iter().map(|t| t.x).collect();
            let ys: Vec<u32> = tiles.iter().map(|t| t.y).collect();
            assert_eq!(rect.start_x, *xs.iter().min().unwrap());
            assert_eq!(rect.end_x, *xs.iter().max().unwrap());
            assert_eq!(rect.start_y, *ys.iter().min().unwrap());
            assert_eq!(rect.end_y, *ys.iter().max().unwrap());
        }
        // Level 0: BC sits in the single west/south tile (TMS y=0).
        assert_eq!(
            ranges[0],
            vec![AvailabilityRect {
                start_x: 0,
                start_y: 0,
                end_x: 0,
                end_y: 0,
            }]
        );
    }

    #[test]
    fn layer_json_is_what_cesium_parses() {
        let bb = bc_bbox();
        let lj = layer_json("MagGCS DEM", "deadbeefdeadbeef", &bb, 19);
        let v: serde_json::Value = serde_json::to_value(&lj).expect("serialise");
        // Cesium rejects formats not starting with "quantized-mesh-1.".
        assert!(v["format"]
            .as_str()
            .unwrap()
            .starts_with("quantized-mesh-1."));
        assert_eq!(v["tilejson"], "2.1.0");
        assert_eq!(v["scheme"], "tms");
        assert_eq!(v["projection"], "EPSG:4326");
        assert_eq!(v["tiles"][0], "{z}/{x}/{y}.terrain");
        assert_eq!(v["extensions"], serde_json::json!(["octvertexnormals"]));
        assert_eq!(v["minzoom"], 0);
        assert_eq!(v["maxzoom"], 19);
        assert_eq!(
            v["bounds"],
            serde_json::json!([-123.4249917, 48.6374242, -123.3999894, 48.6500937])
        );
        // Levels 0..=maxzoom, each with a rect. The descriptor must be a
        // JSON *object* with exactly these keys — Cesium accesses
        // `range.startX` etc. property-style, so arrays silently produce
        // `undefined` and NaN availability ranges.
        let available = v["available"].as_array().expect("available array");
        assert_eq!(available.len(), 20);
        let level0 = available[0]
            .as_array()
            .expect("level 0 rect list")
            .first()
            .expect("level 0 rect");
        assert_eq!(
            level0,
            &serde_json::json!({"startX": 0, "startY": 0, "endX": 0, "endY": 0})
        );
        assert!(level0.is_object(), "descriptor must be an object");
        assert_eq!(level0.as_object().unwrap().len(), 4);
        assert_eq!(v["name"], "MagGCS DEM");
        assert_eq!(v["version"], "deadbeefdeadbeef");
    }

    #[test]
    fn tile_url_path_formats() {
        assert_eq!(
            tile_url_path(&TileCoord {
                z: 19,
                x: 164776,
                y: 403865
            }),
            "19/164776/403865.terrain"
        );
    }

    /// A 4×4 raster whose heights form an affine plane — bilinear must
    /// reproduce the plane exactly at centres and between them.
    fn plane_raster() -> PlacedRaster {
        let bounds = DemBounds {
            west: 10.0,
            south: 20.0,
            east: 14.0,
            north: 24.0,
        };
        let plane = |lat: f64, lon: f64| (100.0 + 2.0 * lon - 5.0 * lat) as f32;
        let mut values = Vec::with_capacity(16);
        for r in 0..4usize {
            for c in 0..4usize {
                values.push(plane(24.0 - (r as f64 + 0.5), 10.0 + c as f64 + 0.5));
            }
        }
        PlacedRaster::try_new(bounds, 4, 4, values).expect("valid raster")
    }

    #[test]
    fn sample_hits_cell_centres_and_interpolates() {
        let r = plane_raster();
        // Cell centre (row 0, col 0): lat 23.5, lon 10.5.
        let at_centre = f64::from(r.sample(23.5, 10.5));
        assert!((at_centre - (100.0 + 21.0 - 117.5)).abs() < 1e-3);
        // Intersection of four centres: bilinear of a plane = the plane.
        let between = f64::from(r.sample(23.0, 11.0));
        assert!((between - (100.0 + 22.0 - 115.0)).abs() < 1e-3);
    }

    #[test]
    fn sample_clamps_outside_to_the_border_centre() {
        let r = plane_raster();
        // Far outside == exactly the border cell-centre sample (edge clamp).
        assert_eq!(r.sample(30.0, 5.0), r.sample(23.5, 10.5));
        assert_eq!(r.sample(10.0, 40.0), r.sample(20.5, 13.5));
    }

    #[test]
    fn voids_fill_from_nearest_valid_cell() {
        let bounds = DemBounds {
            west: 0.0,
            south: 0.0,
            east: 3.0,
            north: 3.0,
        };
        let mut values = vec![7.0f32; 9];
        values[4] = f32::NAN; // centre void (NaN is allowed — it's a void)
        let r = PlacedRaster::try_new(bounds, 3, 3, values).expect("valid");
        assert_eq!(r.sample(1.5, 1.5), 7.0, "centre void filled from ring");
        // A void far from any data still resolves (no NaN escapes).
        let mut sparse = vec![f32::NAN; 9];
        sparse[0] = 42.0;
        let r2 = PlacedRaster::try_new(bounds, 3, 3, sparse).expect("valid");
        assert_eq!(r2.sample(0.5, 2.5), 42.0);
    }

    #[test]
    fn all_void_raster_samples_zero() {
        let bounds = DemBounds {
            west: 0.0,
            south: 0.0,
            east: 1.0,
            north: 1.0,
        };
        let r = PlacedRaster::try_new(bounds, 2, 2, vec![f32::NAN; 4]).expect("valid");
        assert_eq!(r.sample(0.5, 0.5), 0.0);
    }

    #[test]
    fn try_new_rejects_bad_input() {
        let ok = DemBounds {
            west: 0.0,
            south: 0.0,
            east: 1.0,
            north: 1.0,
        };
        assert!(PlacedRaster::try_new(ok, 2, 2, vec![0.0; 3]).is_err());
        assert!(PlacedRaster::try_new(ok, 0, 2, vec![]).is_err());
        let inverted = DemBounds {
            west: 1.0,
            south: 0.0,
            east: 0.0,
            north: 1.0,
        };
        assert!(PlacedRaster::try_new(inverted, 2, 2, vec![0.0; 4]).is_err());
        let nonfinite = DemBounds {
            west: f64::NAN,
            south: 0.0,
            east: 1.0,
            north: 1.0,
        };
        assert!(PlacedRaster::try_new(nonfinite, 2, 2, vec![0.0; 4]).is_err());
        assert!(
            PlacedRaster::try_new(ok, 2, 2, vec![f32::INFINITY; 4]).is_err(),
            "infinite samples rejected"
        );
    }

    #[test]
    fn terrain_source_picks_covering_and_nearest_raster() {
        let a = PlacedRaster::try_new(
            DemBounds {
                west: 0.0,
                south: 0.0,
                east: 1.0,
                north: 1.0,
            },
            2,
            2,
            vec![10.0; 4],
        )
        .expect("a");
        let b = PlacedRaster::try_new(
            DemBounds {
                west: 1.0,
                south: 0.0,
                east: 2.0,
                north: 1.0,
            },
            2,
            2,
            vec![20.0; 4],
        )
        .expect("b");
        let src = TerrainSource::new(vec![a, b]);
        assert_eq!(src.sample(0.5, 0.5), 10.0);
        assert_eq!(src.sample(0.5, 1.5), 20.0);
        // Half-open: lon == 1.0 belongs to the eastern raster.
        assert_eq!(src.sample(0.5, 1.0), 20.0);
        // Outside everything → nearest raster's clamped edge, not 0.0.
        assert_eq!(src.sample(0.5, -5.0), 10.0);
        assert_eq!(src.sample(-5.0, 0.5), 10.0);
        let u = src.union_bounds().expect("union");
        assert_eq!((u.west, u.south, u.east, u.north), (0.0, 0.0, 2.0, 1.0));
    }

    #[test]
    fn empty_terrain_source_is_harmless() {
        let src = TerrainSource::new(vec![]);
        assert!(src.union_bounds().is_none());
        assert_eq!(src.sample(48.6, -123.4), 0.0);
    }
}
