//! quantized-mesh tile building: 33×33 grid → gzip `.terrain` files plus
//! the tileset layout the 3D globe streams (plan B, ADR-004 addendum).
//!
//! Heights are sampled from a [`TerrainSource`] at the **dequantized**
//! vertex positions (`u/32767`), so what Cesium reconstructs is exactly what
//! we measured — neighbouring tiles share edge samples bit-for-bit and their
//! seams match. Vertex normals are unit **ECEF** vectors (Cesium's GlobeVS
//! mixes the oct-decoded normal with ECEF directions — verified against the
//! shader) so `octvertexnormals` lights correctly.

use std::path::{Path, PathBuf};

use quantized_mesh::coords::geodetic_to_ecef;
use quantized_mesh::{
    EdgeIndices, EncodeOptions, QuantizedMeshEncoder, QuantizedMeshHeader, QuantizedVertices,
    TileBounds, QUANTIZED_MAX,
};

use crate::cog::{self, DemMetadata};
use crate::error::DemError;
use crate::geotiff;
use crate::terrain::{
    layer_json, maxzoom_for_resolution, tile_bounds, tile_url_path, tiles_covering, PlacedRaster,
    TerrainSource, TileCoord, GRID_SEGMENTS,
};

/// Gzip level for stored `.terrain` files — the app's server replays them
/// with `Content-Encoding: gzip`, so the browser inflates on the way to
/// Cesium and disk stays ~4× smaller.
const GZIP_LEVEL: u32 = 6;

/// `layer.json` `name` for the local tileset.
const TILESET_NAME: &str = "MagGCS DEM";

/// Vertices per tile edge (`GRID_SEGMENTS + 1` = 33): the quantized-mesh
/// standard grid.
const EDGE_VERTS: usize = GRID_SEGMENTS + 1;

/// Encode one quantized-mesh tile as gzip-compressed `.terrain` bytes.
/// Total on validated inputs (sampling is NaN-free, arithmetic clamped).
pub fn encode_tile(source: &TerrainSource, t: TileCoord) -> Vec<u8> {
    let bounds = tile_bounds(t.z, t.x, t.y);
    let width = bounds.east - bounds.west;
    let height = bounds.north - bounds.south;
    let qmax = f64::from(QUANTIZED_MAX);

    // Quantized coordinate of grid line i: round(i · 32767 / 32) — exactly
    // what Cesium reconstructs (u/32767), so samples land on rendered
    // vertices and interior lines never hit 0/32767 (no phantom edges).
    let quant = |i: usize| {
        (((i as u32) * u32::from(QUANTIZED_MAX) + (GRID_SEGMENTS as u32) / 2)
            / GRID_SEGMENTS as u32) as u16
    };

    // Geodetic grid, row-major: index = j·EDGE_VERTS + i; j = 0 is the
    // SOUTH row (v = 0), i = 0 the west column (u = 0).
    let mut geodetic = Vec::with_capacity(EDGE_VERTS * EDGE_VERTS);
    for j in 0..EDGE_VERTS {
        let v = quant(j);
        let lat = bounds.south + (f64::from(v) / qmax) * height;
        for i in 0..EDGE_VERTS {
            let u = quant(i);
            let lon = bounds.west + (f64::from(u) / qmax) * width;
            let h = f64::from(source.sample(lat, lon));
            geodetic.push([lon, lat, h]);
        }
    }

    let (min_h, max_h) = geodetic
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p[2] as f32), hi.max(p[2] as f32))
        });
    let span = max_h - min_h;

    // Two CCW triangles per cell (viewed from outside the ellipsoid),
    // diagonal b–c — the order the crate's own docs use.
    let mut indices = Vec::with_capacity(GRID_SEGMENTS * GRID_SEGMENTS * 6);
    for j in 0..GRID_SEGMENTS {
        for i in 0..GRID_SEGMENTS {
            let a = (j * EDGE_VERTS + i) as u32;
            let b = a + 1;
            let c = a + EDGE_VERTS as u32;
            let d = c + 1;
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }

    // High-water-mark index coding (used by every quantized-mesh decoder,
    // Cesium included) requires each vertex's *first* reference to arrive in
    // ascending id order — impossible with row-major grid ids. Renumber by
    // first appearance and permute the vertex streams to match
    // (webgl-loader style); edge detection runs on values, so it just works.
    let mut old_of_new: Vec<u32> = Vec::with_capacity(EDGE_VERTS * EDGE_VERTS);
    let mut new_of_old = vec![u32::MAX; EDGE_VERTS * EDGE_VERTS];
    for &idx in &indices {
        let slot = &mut new_of_old[idx as usize];
        if *slot == u32::MAX {
            *slot = old_of_new.len() as u32;
            old_of_new.push(idx);
        }
    }
    for idx in &mut indices {
        *idx = new_of_old[*idx as usize];
    }
    let normals = vertex_normals(&geodetic); // grid order, computed first
    let geodetic: Vec<[f64; 3]> = old_of_new
        .iter()
        .map(|&old| geodetic[old as usize])
        .collect();
    let normals: Vec<[f32; 3]> = old_of_new
        .iter()
        .map(|&old| normals[old as usize])
        .collect();

    let mut vertices = QuantizedVertices::with_capacity(EDGE_VERTS * EDGE_VERTS);
    for (k, p) in geodetic.iter().enumerate() {
        let hq = if span > 0.0 {
            ((f64::from(p[2] as f32) - f64::from(min_h)) / f64::from(span) * qmax)
                .round()
                .clamp(0.0, qmax) as u16
        } else {
            0 // flat tile: every vertex dequantizes back to min_h
        };
        let old = old_of_new[k] as usize;
        vertices.push(quant(old % EDGE_VERTS), quant(old / EDGE_VERTS), hq);
    }

    let edge_indices = EdgeIndices::from_vertices(&vertices);
    let header = QuantizedMeshHeader::from_bounds_with_vertices_iter(
        &TileBounds::new(bounds.west, bounds.south, bounds.east, bounds.north),
        min_h,
        max_h,
        geodetic.iter().copied(),
    );

    QuantizedMeshEncoder::new(header, vertices, indices, edge_indices).encode_with_options(
        &EncodeOptions {
            include_normals: true,
            normals: Some(normals),
            compression_level: GZIP_LEVEL,
            ..EncodeOptions::default()
        },
    )
}

/// Unit normals in **ECEF** (the frame Cesium's globe shader combines the
/// decoded normal with its ECEF directions): central differences over the
/// grid, u(east) × v(north) pointing outward, radial fallback where a
/// difference degenerates.
fn vertex_normals(geodetic: &[[f64; 3]]) -> Vec<[f32; 3]> {
    let ecef: Vec<[f64; 3]> = geodetic
        .iter()
        .map(|p| geodetic_to_ecef(p[0], p[1], p[2]))
        .collect();
    let idx = |i: usize, j: usize| j * EDGE_VERTS + i;
    let mut out = Vec::with_capacity(ecef.len());
    for j in 0..EDGE_VERTS {
        for i in 0..EDGE_VERTS {
            let i0 = i.saturating_sub(1);
            let i1 = (i + 1).min(EDGE_VERTS - 1);
            let j0 = j.saturating_sub(1);
            let j1 = (j + 1).min(EDGE_VERTS - 1);
            let du = sub3(ecef[idx(i1, j)], ecef[idx(i0, j)]);
            let dv = sub3(ecef[idx(i, j1)], ecef[idx(i, j0)]);
            let cross = cross3(du, dv);
            let m2 = cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2];
            let dir = if m2 > 0.0 && m2.is_finite() {
                cross
            } else {
                ecef[idx(i, j)]
            };
            let n = normalize3(dir);
            out.push([n[0] as f32, n[1] as f32, n[2] as f32]);
        }
    }
    out
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(a: [f64; 3]) -> [f64; 3] {
    let m = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if m > 0.0 && m.is_finite() {
        [a[0] / m, a[1] / m, a[2] / m]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Tileset generation results (printed by the CLI, asserted by tests).
#[derive(Debug)]
pub struct TilesetStats {
    pub dem_tiles: usize,
    pub mesh_tiles: usize,
    pub maxzoom: u8,
    pub bytes: u64,
}

/// Build a full quantized-mesh tileset from a `dem-prep process` output
/// folder: every `<tile>/dem.json` + `dtm.tif` becomes one [`TerrainSource`]
/// raster, then `layer.json` + `{z}/{x}/{y}.terrain` are written under `out`.
/// `maxzoom_override` replaces the resolution-derived zoom (levels
/// `0..=maxzoom` always include the world tiles Cesium expects).
///
/// # Errors
///
/// [`DemError::Manifest`] when no tile folders exist or a `dem.json` fails
/// to parse; [`DemError::Raster`] on TIFF/placement/write failures.
pub fn generate_tileset(
    processed: &Path,
    out: &Path,
    maxzoom_override: Option<u8>,
) -> Result<TilesetStats, DemError> {
    let mut tile_dirs: Vec<PathBuf> = std::fs::read_dir(processed)
        .map_err(|e| DemError::Manifest(format!("read {}: {e}", processed.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("dem.json").is_file())
        .collect();
    tile_dirs.sort();
    if tile_dirs.is_empty() {
        return Err(DemError::Manifest(format!(
            "no <tile>/dem.json folders under {} — run `dem-prep process` first",
            processed.display()
        )));
    }

    let mut rasters = Vec::new();
    let mut versions = Vec::new();
    let mut maxzoom = 0u8;
    for dir in &tile_dirs {
        let meta_path = dir.join("dem.json");
        let bytes = std::fs::read(&meta_path)
            .map_err(|e| DemError::Manifest(format!("{}: {e}", meta_path.display())))?;
        let meta: DemMetadata = serde_json::from_slice(&bytes)
            .map_err(|e| DemError::Manifest(format!("{}: {e}", meta_path.display())))?;
        let (values, width, height) = geotiff::read_band_raw(&dir.join("dtm.tif"))?;
        rasters.push(PlacedRaster::try_new(meta.bounds, width, height, values)?);
        versions.push(meta.version);
        maxzoom = maxzoom.max(maxzoom_for_resolution(meta.resolution_m));
    }

    let source = TerrainSource::new(rasters);
    let union = source
        .union_bounds()
        .ok_or_else(|| DemError::Manifest("empty tileset".into()))?;
    let maxzoom = maxzoom_override.unwrap_or(maxzoom);
    let version = cog::version_of(&[versions.join("\n").as_bytes()]);

    let manifest = layer_json(TILESET_NAME, &version, &union, maxzoom);
    let json =
        serde_json::to_string_pretty(&manifest).map_err(|e| DemError::Manifest(e.to_string()))?;
    std::fs::create_dir_all(out).map_err(io_err)?;
    std::fs::write(out.join("layer.json"), json).map_err(io_err)?;

    let mut mesh_tiles = 0usize;
    let mut bytes_written = 0u64;
    for z in 0..=maxzoom {
        let coords = tiles_covering(&union, z);
        for t in &coords {
            let blob = encode_tile(&source, *t);
            let path = out.join(tile_url_path(t));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io_err)?;
            }
            bytes_written += blob.len() as u64;
            std::fs::write(&path, &blob).map_err(io_err)?;
            mesh_tiles += 1;
        }
        println!("  z{z}: {} tile(s)", coords.len());
    }

    Ok(TilesetStats {
        dem_tiles: tile_dirs.len(),
        mesh_tiles,
        maxzoom,
        bytes: bytes_written,
    })
}

fn io_err(e: std::io::Error) -> DemError {
    DemError::Raster(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quantized_mesh::{is_gzip, DecodedMesh};

    use crate::cog::DemBounds;

    const TEST_TILE: TileCoord = TileCoord {
        z: 10,
        x: 600,
        y: 400,
    };

    /// Plane `h = 12 + 2.5·lat − 1.75·lon` over the tile plus a margin —
    /// bilinear sampling reproduces planes exactly, so roundtrip assertions
    /// are sharp.
    fn plane_source(bounds: &DemBounds) -> TerrainSource {
        let m = 0.02;
        let b = DemBounds {
            west: bounds.west - m,
            east: bounds.east + m,
            south: bounds.south - m,
            north: bounds.north + m,
        };
        let (w, h) = (65usize, 65usize);
        let dlat = (b.north - b.south) / h as f64;
        let dlon = (b.east - b.west) / w as f64;
        let mut values = Vec::with_capacity(w * h);
        for r in 0..h {
            for c in 0..w {
                let lat = b.north - (r as f64 + 0.5) * dlat;
                let lon = b.west + (c as f64 + 0.5) * dlon;
                values.push(plane(lat, lon));
            }
        }
        TerrainSource::new(vec![
            PlacedRaster::try_new(b, w, h, values).expect("valid plane raster")
        ])
    }

    fn plane(lat: f64, lon: f64) -> f32 {
        (12.0 + 2.5 * lat - 1.75 * lon) as f32
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dem-prep-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn encode_tile_roundtrips_the_plane() {
        let t = TEST_TILE;
        let tb = tile_bounds(t.z, t.x, t.y);
        let src = plane_source(&tb);
        let bytes = encode_tile(&src, t);
        assert!(is_gzip(&bytes), "tiles are stored gzip-compressed");

        let mesh = DecodedMesh::decode(&bytes).expect("decode");
        let n = EDGE_VERTS * EDGE_VERTS;
        assert_eq!(mesh.vertices.len(), n);
        assert_eq!(mesh.indices.len(), GRID_SEGMENTS * GRID_SEGMENTS * 6);

        // Exactly one grid line per boundary, everything else interior —
        // true regardless of the HWM renumbering that permuted the stream.
        assert_eq!(
            mesh.vertices.u.iter().filter(|&&u| u == 0).count(),
            EDGE_VERTS
        );
        assert_eq!(
            mesh.vertices
                .u
                .iter()
                .filter(|&&u| u == QUANTIZED_MAX)
                .count(),
            EDGE_VERTS
        );
        assert_eq!(
            mesh.vertices.v.iter().filter(|&&v| v == 0).count(),
            EDGE_VERTS
        );
        assert_eq!(
            mesh.vertices
                .v
                .iter()
                .filter(|&&v| v == QUANTIZED_MAX)
                .count(),
            EDGE_VERTS
        );
        assert!(mesh
            .vertices
            .u
            .iter()
            .zip(&mesh.vertices.v)
            .all(|(&u, &v)| u <= QUANTIZED_MAX && v <= QUANTIZED_MAX));

        // Indices are in range (u16 grid, 1089 vertices).
        assert!(mesh.indices.iter().all(|&i| (i as usize) < n));

        // One index per grid line on every edge, sorted along the edge.
        assert_eq!(mesh.edge_indices.west.len(), EDGE_VERTS);
        assert_eq!(mesh.edge_indices.south.len(), EDGE_VERTS);
        assert_eq!(mesh.edge_indices.east.len(), EDGE_VERTS);
        assert_eq!(mesh.edge_indices.north.len(), EDGE_VERTS);
        let west_v: Vec<u16> = mesh
            .edge_indices
            .west
            .iter()
            .map(|&i| mesh.vertices.v[i as usize])
            .collect();
        assert!(west_v.windows(2).all(|w| w[0] < w[1]), "west sorted by v");
        let south_u: Vec<u16> = mesh
            .edge_indices
            .south
            .iter()
            .map(|&i| mesh.vertices.u[i as usize])
            .collect();
        assert!(south_u.windows(2).all(|w| w[0] < w[1]), "south sorted by u");

        // Heights dequantize back onto the plane within one quantisation step.
        let (min_h, max_h) = (mesh.header.min_height, mesh.header.max_height);
        assert!(max_h > min_h, "plane is not flat");
        let step = f64::from(max_h - min_h) / f64::from(QUANTIZED_MAX);
        for k in 0..n {
            let u = f64::from(mesh.vertices.u[k]) / f64::from(QUANTIZED_MAX);
            let v = f64::from(mesh.vertices.v[k]) / f64::from(QUANTIZED_MAX);
            let lon = tb.west + u * (tb.east - tb.west);
            let lat = tb.south + v * (tb.north - tb.south);
            let expected = f64::from(plane(lat, lon));
            let got = f64::from(min_h)
                + (f64::from(mesh.vertices.height[k]) / f64::from(QUANTIZED_MAX))
                    * f64::from(max_h - min_h);
            assert!(
                (got - expected).abs() <= step + 1e-4,
                "vertex {k}: {got} vs plane {expected}"
            );
        }

        // Header range covers the sampled heights.
        assert!(mesh.header.min_height.is_finite());
        assert!(mesh.header.max_height.is_finite());

        // Normals: present, unit-length after the oct round-trip, and
        // roughly radial over this near-flat plane (0.0013° tilt; oct loss
        // stays under ~0.5°).
        let normals = mesh
            .extensions
            .normals
            .as_ref()
            .expect("octvertexnormals extension");
        assert_eq!(normals.len(), n);
        for (k, nz) in normals.iter().enumerate() {
            let m = (f64::from(nz[0]) * f64::from(nz[0])
                + f64::from(nz[1]) * f64::from(nz[1])
                + f64::from(nz[2]) * f64::from(nz[2]))
            .sqrt();
            assert!((m - 1.0).abs() < 0.02, "normal {k} unit length, got {m}");
        }
        let u0 = f64::from(mesh.vertices.u[0]) / f64::from(QUANTIZED_MAX);
        let v0 = f64::from(mesh.vertices.v[0]) / f64::from(QUANTIZED_MAX);
        let lon0 = tb.west + u0 * (tb.east - tb.west);
        let lat0 = tb.south + v0 * (tb.north - tb.south);
        let radial = normalize3(geodetic_to_ecef(lon0, lat0, f64::from(min_h)));
        let dot = f64::from(normals[0][0]) * radial[0]
            + f64::from(normals[0][1]) * radial[1]
            + f64::from(normals[0][2]) * radial[2];
        assert!(dot > 0.995, "normal points outward (dot {dot})");

        // Winding: the first triangle is CCW seen from outside (outward
        // normal of the face points away from Earth's centre).
        let deq = |k: usize| -> [f64; 3] {
            let u = f64::from(mesh.vertices.u[k]) / f64::from(QUANTIZED_MAX);
            let v = f64::from(mesh.vertices.v[k]) / f64::from(QUANTIZED_MAX);
            let h = f64::from(min_h)
                + (f64::from(mesh.vertices.height[k]) / f64::from(QUANTIZED_MAX))
                    * f64::from(max_h - min_h);
            geodetic_to_ecef(
                tb.west + u * (tb.east - tb.west),
                tb.south + v * (tb.north - tb.south),
                h,
            )
        };
        let (a, b, c) = (
            deq(mesh.indices[0] as usize),
            deq(mesh.indices[1] as usize),
            deq(mesh.indices[2] as usize),
        );
        let face = cross3(sub3(b, a), sub3(c, a));
        let center = geodetic_to_ecef(
            (tb.west + tb.east) / 2.0,
            (tb.south + tb.north) / 2.0,
            f64::from(min_h),
        );
        let outward = face[0] * center[0] + face[1] * center[1] + face[2] * center[2];
        assert!(outward > 0.0, "front face points outward");
    }

    #[test]
    fn flat_tile_encodes_and_decodes() {
        let t = TileCoord {
            z: 10,
            x: 601,
            y: 400,
        };
        let tb = tile_bounds(t.z, t.x, t.y);
        let m = 0.02;
        let b = DemBounds {
            west: tb.west - m,
            east: tb.east + m,
            south: tb.south - m,
            north: tb.north + m,
        };
        let src = TerrainSource::new(vec![
            PlacedRaster::try_new(b, 16, 16, vec![42.5; 256]).expect("flat raster")
        ]);
        let mesh = DecodedMesh::decode(&encode_tile(&src, t)).expect("decode");
        assert_eq!(mesh.header.min_height, 42.5);
        assert_eq!(mesh.header.max_height, 42.5, "flat span stays empty");
        assert!(
            mesh.vertices
                .height
                .iter()
                .all(|&h| f64::from(min_deq(h, 42.5, 42.5)) == 42.5),
            "every vertex dequantizes to the flat height"
        );
        assert!(mesh.extensions.normals.is_some(), "normals always present");
    }

    /// Height dequantisation: `min + q/32767 · (max − min)`.
    fn min_deq(q: u16, min: f32, max: f32) -> f32 {
        min + (f32::from(q) / f32::from(QUANTIZED_MAX)) * (max - min)
    }

    #[test]
    fn generate_writes_manifest_and_decodable_tiles() {
        let root = temp_dir("tileset");
        let proc = root.join("proc");
        let sheet = proc.join("sheet");
        std::fs::create_dir_all(&sheet).expect("mkdir");

        // 8×8 DTM at 0.00025° cells, placement exactly as `process` writes
        // it: bounds = edge-to-edge of the grid, sidecar-only geo info.
        let (west, north, cell) = (-123.425f64, 48.65f64, 0.00025f64);
        let bounds = DemBounds {
            north,
            south: north - 8.0 * cell,
            east: west + 8.0 * cell,
            west,
        };
        let raster = crate::grid::GeoRaster {
            width: 8,
            height: 8,
            values: vec![42.0; 64],
            west,
            north,
        };
        cog::write_geo_tiff(&sheet.join("dtm.tif"), &raster).expect("tif");
        let meta = DemMetadata {
            version: "cafebabecafebabe".into(),
            raster_crs: "EPSG:4326".into(),
            resolution_m: 1.0,
            bounds,
            vertical_datum: "orthometric_cgvd2013".into(),
            source_file: "sheet.laz".into(),
            source_points: 64,
            has_dsm: false,
        };
        cog::write_metadata(&sheet.join("dem.json"), &meta).expect("dem.json");

        let out = root.join("tileset");
        let stats = generate_tileset(&proc, &out, Some(8)).expect("generate");
        assert_eq!(stats.dem_tiles, 1);
        assert_eq!(stats.maxzoom, 8);
        // Every level contributes 1..4 tiles for a tiny bbox: 9..36 total.
        assert!(
            (9..=36).contains(&stats.mesh_tiles),
            "mesh tile count {} out of expected 9..=36",
            stats.mesh_tiles
        );
        assert!(stats.bytes > 0);

        // layer.json is what Cesium parses.
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("layer.json")).expect("layer.json"))
                .expect("json");
        assert_eq!(manifest["format"], "quantized-mesh-1.0");
        assert_eq!(manifest["maxzoom"], 8);
        assert_eq!(manifest["minzoom"], 0);
        assert_eq!(manifest["scheme"], "tms");
        assert_eq!(manifest["projection"], "EPSG:4326");
        assert_eq!(
            manifest["extensions"],
            serde_json::json!(["octvertexnormals"])
        );
        assert_eq!(manifest["available"].as_array().expect("avail").len(), 9);
        // The layer version is derived from the per-tile DEM versions.
        assert_eq!(manifest["version"], cog::version_of(&[b"cafebabecafebabe"]));

        // On disk: exactly one file per generated tile, deepest decodes.
        let mut files = Vec::new();
        let mut stack = vec![out.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("walk") {
                let p = entry.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|e| e == "terrain") {
                    files.push(p);
                }
            }
        }
        assert_eq!(files.len(), stats.mesh_tiles, "one file per tile");

        let deepest = tiles_covering(&bounds, 8);
        assert!(!deepest.is_empty(), "bbox always has tiles at z8");
        let tile_path = out.join(tile_url_path(&deepest[0]));
        let mesh = DecodedMesh::decode(&std::fs::read(&tile_path).expect("tile bytes"))
            .expect("decode deepest");
        assert_eq!(mesh.vertices.len(), EDGE_VERTS * EDGE_VERTS);
        assert_eq!(mesh.header.min_height, 42.0);
        assert_eq!(mesh.header.max_height, 42.0);
        assert!(
            mesh.vertices
                .height
                .iter()
                .all(|&h| min_deq(h, 42.0, 42.0) == 42.0),
            "flat DTM survives the full pipeline"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generate_rejects_an_empty_folder() {
        let root = temp_dir("empty");
        let err = generate_tileset(&root, &root.join("out"), None).expect_err("no dem.json");
        assert!(matches!(err, DemError::Manifest(_)), "got {err:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
