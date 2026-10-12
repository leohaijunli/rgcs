//! dem-prep: BC LiDAR (LAS/LAZ) → DTM/DSM COG layers the ground station can
//! load (ADR-004/005, comprehensive plan D4).
//!
//! Batch mode processes every .laz/.las in a folder into one output folder
//! per tile:
//!
//! ```text
//! dem-prep process <folder> [--out <dir>] [--resolution 1.0] [--ground-only-dsm false]
//! dem-prep probe <file.laz>
//! dem-prep terrain <processed-dir> [--out <dir>] [--maxzoom N]
//! ```
//!
//! The GCS loads a tile folder (dtm.tif + dsm.tif + dem.json) through
//! `core::terrain::CogDem` — the point clouds never reach the app. The
//! `terrain` subcommand turns processed tile folders into a quantized-mesh
//! tileset (layer.json + {z}/{x}/{y}.terrain) the 3D globe streams.

mod cog;
mod error;
mod geotiff;
mod grid;
mod hillshade;
mod las;
mod mesh;
mod terrain;
mod utm;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::cog::{DemMetadata, IndexEntry};
use crate::grid::{dsm_value, dtm_value, fill_single_cell_holes, resample_to_wgs84, UtmGrid};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("probe") => {
            let path = args.next().context("probe <file.laz>")?;
            probe(Path::new(&path))
        }
        Some("process") => {
            let folder = PathBuf::from(args.next().context("process <folder>")?);
            let mut out = folder.clone();
            let mut resolution_m = 1.0;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--out" => out = PathBuf::from(args.next().context("--out <dir>")?),
                    "--resolution" => {
                        resolution_m = args.next().context("--resolution <m>")?.parse()?
                    }
                    other => bail!("unknown argument: {other}"),
                }
            }
            process_folder(&folder, &out, resolution_m)
        }
        Some("terrain") => {
            let folder = PathBuf::from(args.next().context("terrain <processed-dir>")?);
            let mut out = folder.join("terrain");
            let mut maxzoom: Option<u8> = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--out" => out = PathBuf::from(args.next().context("--out <dir>")?),
                    "--maxzoom" => maxzoom = Some(args.next().context("--maxzoom <n>")?.parse()?),
                    other => bail!("unknown argument: {other}"),
                }
            }
            let stats = mesh::generate_tileset(&folder, &out, maxzoom)?;
            println!(
                "tileset: {} DEM tile(s) -> {} mesh tile(s) at z{}, {:.1} MiB -> {}",
                stats.dem_tiles,
                stats.mesh_tiles,
                stats.maxzoom,
                stats.bytes as f64 / (1024.0 * 1024.0),
                out.display()
            );
            Ok(())
        }
        _ => {
            bail!(
                "usage: dem-prep process <folder> [--out <dir>] [--resolution m] \
                 | probe <file> | terrain <processed-dir> [--out <dir>] [--maxzoom n]"
            )
        }
    }
}

fn probe(path: &Path) -> Result<()> {
    let info = las::probe(path)?;
    println!("version: {}.{}", info.version.0, info.version.1);
    println!("points: {}", info.count);
    println!(
        "bounds (UTM): x {:.1}..{:.1}  y {:.1}..{:.1}  z {:.1}..{:.1}",
        info.min_x, info.max_x, info.min_y, info.max_y, info.min_z, info.max_z
    );
    let area_m2 = (info.max_x - info.min_x) * (info.max_y - info.min_y);
    println!("density ~{:.1} pts/m2", info.count as f64 / area_m2);
    if let Some(p) = &info.projection {
        println!("crs: {}", &p[..p.len().min(120)]);
    }
    Ok(())
}

/// Every BC LiDAR tile in `folder` becomes `<out>/<stem>/{dtm.tif,dsm.tif,dem.json}`.
fn process_folder(folder: &Path, out: &Path, resolution_m: f64) -> Result<()> {
    let mut tiles: Vec<PathBuf> = std::fs::read_dir(folder)
        .with_context(|| format!("read {}", folder.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("laz") | Some("las") | Some("tif") | Some("tiff")
            )
        })
        .collect();
    tiles.sort();
    if tiles.is_empty() {
        bail!("no .laz/.las files in {}", folder.display());
    }
    println!("{} tile(s) in {}", tiles.len(), folder.display());
    let mut index: Vec<IndexEntry> = Vec::new();
    for tile in &tiles {
        let is_raster = matches!(
            tile.extension().and_then(|e| e.to_str()),
            Some("tif") | Some("tiff")
        );
        let result: Result<IndexEntry> = if is_raster {
            process_raster_tile(tile, out)
        } else {
            process_tile(tile, out, resolution_m)
        };
        match result {
            Ok(entry) => index.push(entry),
            Err(e) => eprintln!("FAILED {}: {e}", tile.display()),
        }
    }
    // Tile index (N1): the GCS picks the tile covering a survey area from
    // this list instead of the operator guessing folder names.
    let index_path = out.join("index.json");
    cog::write_index(&index_path, &index)?;
    println!("index: {} tile(s) -> {}", index.len(), index_path.display());
    Ok(())
}

fn process_tile(tile: &Path, out: &Path, resolution_m: f64) -> Result<cog::IndexEntry> {
    let stem = tile
        .file_stem()
        .and_then(|s| s.to_str())
        .context("tile name")?
        .to_string();
    let tile_out = out.join(&stem);
    std::fs::create_dir_all(&tile_out)?;

    let info = las::probe(tile)?;
    println!(
        "{}: {} pts, {:.1}x{:.1} m",
        stem,
        info.count,
        info.max_x - info.min_x,
        info.max_y - info.min_y
    );

    // Grid in UTM (native), then regrid to WGS84 for the runtime reader.
    let mut utm_grid = UtmGrid::new(info.min_x, info.min_y, info.max_x, info.max_y, resolution_m);
    las::for_each_point(tile, |p| utm_grid.add(p))?;
    if utm_grid.dtm_n.iter().all(|&n| n == 0) {
        bail!(crate::error::DemError::NoGroundPoints(tile.to_path_buf()));
    }

    // Output resolution in degrees: convert the metre resolution through the
    // metre-per-degree factors at the tile's mid-latitude.
    let mid_y = (info.min_y + info.max_y) / 2.0;
    let (mid_lat, _, _) = {
        let (lat, _, zone) = utm_zone_of(&info);
        (lat, (), zone)
    };
    let _ = mid_y;
    let m_per_deg_lat = 111_132.954;
    let m_per_deg_lon = 111_319.49 * mid_lat.to_radians().cos();
    let dlat = resolution_m / m_per_deg_lat;
    let dlon = resolution_m / m_per_deg_lon;

    let mut dtm = resample_to_wgs84(&utm_grid, (dlat, dlon), |i| dtm_value(&utm_grid, i))?;
    fill_single_cell_holes(&mut dtm);
    let mut dsm = resample_to_wgs84(&utm_grid, (dlat, dlon), |i| dsm_value(&utm_grid, i))?;
    fill_single_cell_holes(&mut dsm);

    // The resampler stamps the geographic placement onto the raster; the
    // reader reconstructs the extent from placement + scale, so the tags and
    // dem.json share one source of truth.
    let (west, north) = (dtm.west, dtm.north);
    let (south, east) = (
        north - dlat * dtm.height as f64,
        west + dlon * dtm.width as f64,
    );

    let dtm_path = tile_out.join("dtm.tif");
    let dsm_path = tile_out.join("dsm.tif");
    cog::write_geo_tiff(&dtm_path, &dtm)?;
    cog::write_geo_tiff(&dsm_path, &dsm)?;
    // Terrain overlay for the map (plan A): grayscale hillshade PNG the
    // Layers menu drapes over the imagery.
    hillshade::write_hillshade_png(&tile_out.join("hillshade.png"), &dtm, dlat, dlon)?;

    // Version = sha256 over both rasters.
    let dtm_bytes = std::fs::read(&dtm_path)?;
    let dsm_bytes = std::fs::read(&dsm_path)?;
    let metadata = DemMetadata {
        version: cog::version_of(&[&dtm_bytes, &dsm_bytes]),
        raster_crs: "EPSG:4326".into(),
        resolution_m,
        bounds: cog::DemBounds {
            north,
            south,
            east,
            west,
        },
        // BC LiDAR: CGVD2013 orthometric (verified from the projection VLR).
        vertical_datum: "orthometric_cgvd2013".into(),
        source_file: tile
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .into(),
        source_points: info.count,
        has_dsm: true,
    };
    cog::write_metadata(&tile_out.join("dem.json"), &metadata)?;
    println!(
        "  -> {} ({}x{} px, {} ground cells, version {})",
        tile_out.display(),
        dtm.width,
        dtm.height,
        utm_grid.dtm_n.iter().filter(|&&n| n > 0).count(),
        metadata.version
    );
    Ok(cog::IndexEntry {
        folder: stem,
        bounds: metadata.bounds,
        version: metadata.version,
        has_dsm: metadata.has_dsm,
    })
}

/// Process a GeoTIFF input (CDEM / Copernicus GLO-30 fallback sources): the
/// raster is taken as the DTM as-is when geographic (WGS84 degrees); no DSM
/// exists in such products, so dem.json marks `has_dsm: false` and the
/// runtime drape skips the clearance check with a warning in the report.
///
/// # Errors
///
/// Non-geographic inputs (projected CRS) are rejected with a hint — regrid
/// them once with gdalwarp/QGIS before running dem-prep.
fn process_raster_tile(tile: &Path, out: &Path) -> Result<cog::IndexEntry> {
    let stem = tile
        .file_stem()
        .and_then(|s| s.to_str())
        .context("tile name")?
        .to_string();
    let tile_out = out.join(&stem);
    std::fs::create_dir_all(&tile_out)?;

    // Read the source grid (values + its own geographic placement).
    let (values, width, height, src_west, src_north, src_dlat, src_dlon) =
        crate::geotiff::read_single_band(tile)?;

    // Output placement: pass the source placement through (already
    // geographic — a straight copy, no resampling).
    let raster = grid::GeoRaster {
        width,
        height,
        values: values.iter().map(|v| *v as f32).collect(),
        west: src_west,
        north: src_north,
    };
    let (west, north) = (raster.west, raster.north);
    let (south, east) = (
        north - src_dlat * height as f64,
        west + src_dlon * width as f64,
    );

    let dtm_path = tile_out.join("dtm.tif");
    let dsm_path = tile_out.join("dsm.tif");
    cog::write_geo_tiff(&dtm_path, &raster)?;
    // No DSM source: write an all-nodata DSM so the folder shape stays
    // uniform and the runtime reader sees an all-void DSM (the drape then
    // reports zero clearance and the UI warns — see terrain_drape).
    let empty = grid::GeoRaster {
        width,
        height,
        values: vec![f32::NAN; width * height],
        west,
        north,
    };
    cog::write_geo_tiff(&dsm_path, &empty)?;

    let dtm_bytes = std::fs::read(&dtm_path)?;
    let metadata = DemMetadata {
        version: cog::version_of(&[&dtm_bytes]),
        raster_crs: "EPSG:4326".into(),
        resolution_m: src_dlat * 111_132.954,
        bounds: cog::DemBounds {
            north,
            south,
            east,
            west,
        },
        vertical_datum: "orthometric_cgvd2013".into(),
        source_file: tile
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .into(),
        source_points: 0,
        has_dsm: false,
    };
    cog::write_metadata(&tile_out.join("dem.json"), &metadata)?;
    println!(
        "  -> {} ({}x{} px, no DSM — clearance check unavailable, version {})",
        tile_out.display(),
        width,
        height,
        metadata.version
    );
    Ok(cog::IndexEntry {
        folder: stem,
        bounds: metadata.bounds,
        version: metadata.version,
        has_dsm: metadata.has_dsm,
    })
}

/// The UTM zone and mid-latitude of a tile (from its probe).
fn utm_zone_of(info: &las::TileInfo) -> (f64, f64, u8) {
    let (lat, lon) = utm::from_utm(
        (info.min_x + info.max_x) / 2.0,
        (info.min_y + info.max_y) / 2.0,
        10,
    )
    .expect("zone 10 tile");
    (lat, lon, 10)
}
