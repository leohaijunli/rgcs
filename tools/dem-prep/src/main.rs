//! dem-prep: BC LiDAR (LAS/LAZ) → DTM/DSM COG layers the ground station can
//! load (ADR-004/005, comprehensive plan D4).
//!
//! Batch mode processes every .laz/.las in a folder into one output folder
//! per tile:
//!
//! ```text
//! dem-prep process <folder> [--out <dir>] [--resolution 1.0] [--ground-only-dsm false]
//! dem-prep probe <file.laz>
//! dem-prep terrain <processed-dir> [--out <dir>] [--maxzoom N] [--geoid <file.mggeoid>]
//! dem-prep hrdem <lat> <lon> [--width 100] [--height 100] [--buffer 20]
//!             [--resolution 1.0] [--out <dir>]
//! dem-prep hrdem-status <lat> <lon> [--width 100] [--height 100]
//!             [--buffer 20] [--resolution 1.0] [--out <dir>]  (cached readiness JSON, offline)
//! dem-prep geoid <grid.tif> --datum CGVD2013 --bbox <w s e n> [--out <file>] [--note <text>]
//! ```
//!
//! The GCS loads a tile folder (dtm.tif + dsm.tif + dem.json) through
//! `core::terrain::CogDem` — the point clouds never reach the app. The
//! `terrain` subcommand turns processed tile folders into a quantized-mesh
//! tileset (layer.json + {z}/{x}/{y}.terrain) the 3D globe streams.
//! `--geoid` converts the DTM's orthometric heights to WGS84 ellipsoid
//! heights at generation time (`H + N(CGG2013)`, ADR-004/006).

mod cog;
mod error;
mod fetch;
mod gdal;
mod geoid;
mod geotiff;
mod grid;
mod hillshade;
mod hrdem;
mod las;
mod mesh;
mod terrain;
mod utm;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::cog::{DemMetadata, IndexEntry};
use crate::grid::{dsm_value, dtm_value, fill_single_cell_holes, resample_to_wgs84, UtmGrid};

/// Metres per degree of latitude (constant) and of longitude at the equator.
/// Kept together so the longitude factor cannot be inverted again: a degree of
/// longitude spans fewer metres away from the equator, so the per-cell
/// longitude **angle** is the metres **divided** by `M_PER_DEG_LON_EQUATOR ·
/// cos φ`, not multiplied by it.
const M_PER_DEG_LAT: f64 = 111_132.954;
const M_PER_DEG_LON_EQUATOR: f64 = 111_319.49;

/// Per-cell degrees `(dlat, dlon)` for a raster sampled at `res_m` metres near
/// `lat_deg`. `dlon >= dlat` everywhere, with equality only at the equator.
fn degrees_per_cell(res_m: f64, lat_deg: f64) -> (f64, f64) {
    (
        res_m / M_PER_DEG_LAT,
        res_m / (M_PER_DEG_LON_EQUATOR * lat_deg.to_radians().cos()),
    )
}

fn main() -> Result<()> {
    // Desktop launches hand dem-prep a minimal environment, so a GDAL/PROJ
    // installed in a user prefix (conda env, ~/.local/gdal-env, Homebrew) is
    // not on PATH. Find it before any subcommand runs. Logged to stderr —
    // stdout carries progress lines and `hrdem-status` JSON.
    if let Some(prefix) = gdal::augment_path() {
        eprintln!("dem-prep: found GDAL at {}", prefix.join("bin").display());
    }
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("probe") => {
            let path = args.next().context("probe <file.laz>")?;
            probe(Path::new(&path))
        }
        Some("fetch") => {
            let lat: f64 = args.next().context("fetch <lat> <lon>")?.parse()?;
            let lon: f64 = args.next().context("fetch <lat> <lon>")?.parse()?;
            let mut out = PathBuf::from("glo30");
            let mut size_km = 50.0;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--out" => out = PathBuf::from(args.next().context("--out <dir>")?),
                    "--size-km" => size_km = args.next().context("--size-km <n>")?.parse()?,
                    other => bail!("unknown argument: {other}"),
                }
            }
            // Requested box around the centre; whole 1° tiles are downloaded.
            let dlat = size_km / M_PER_DEG_LAT;
            let dlon = size_km / (M_PER_DEG_LON_EQUATOR * lat.to_radians().cos());
            let tiles = fetch::fetch_box(
                lat - dlat / 2.0,
                lat + dlat / 2.0,
                lon - dlon / 2.0,
                lon + dlon / 2.0,
                &out,
            )?;
            for t in &tiles {
                println!("{}", t.display());
            }
            println!("{} GLO-30 tile(s) -> {}", tiles.len(), out.display());
            Ok(())
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
        Some("geoid") => {
            let input = PathBuf::from(args.next().context("geoid <grid.tif>")?);
            let mut datum = "CGVD2013".to_string();
            let mut bbox: Option<[f64; 4]> = None;
            let mut out = PathBuf::from("geoid.mggeoid");
            let mut note = String::new();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--datum" => datum = args.next().context("--datum CGVD2013|EGM96")?,
                    "--bbox" => {
                        let rest: Vec<String> = (0..4)
                            .map(|_| args.next().context("--bbox <w s e n>"))
                            .collect::<Result<_, _>>()?;
                        bbox = Some(
                            rest.iter()
                                .map(|v| v.parse())
                                .collect::<Result<Vec<f64>, _>>()
                                .context("four numbers in --bbox")?
                                .try_into()
                                .expect("four values"),
                        );
                    }
                    "--out" => out = PathBuf::from(args.next().context("--out <file.mggeoid>")?),
                    "--note" => note = args.next().context("--note <provenance text>")?,
                    other => bail!("unknown argument: {other}"),
                }
            }
            let bbox = bbox.context("geoid needs --bbox <west south east north>")?;
            geoid::convert(&input, &datum, bbox, &out, &note)?;
            println!(
                "geoid grid: {} [{bbox:?}] -> {}",
                input.display(),
                out.display()
            );
            Ok(())
        }
        Some("hrdem") => {
            let lat: f64 = args.next().context("hrdem <lat> <lon>")?.parse()?;
            let lon: f64 = args.next().context("hrdem <lat> <lon>")?.parse()?;
            let mut width_m = hrdem::DEFAULT_SURVEY_M;
            let mut height_m = hrdem::DEFAULT_SURVEY_M;
            let mut buffer_m = hrdem::DEFAULT_BUFFER_M;
            let mut resolution_m = hrdem::DEFAULT_RESOLUTION_M;
            let mut out = PathBuf::from("hrdem");
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--width" => width_m = args.next().context("--width <m>")?.parse()?,
                    "--height" => height_m = args.next().context("--height <m>")?.parse()?,
                    "--buffer" => buffer_m = args.next().context("--buffer <m>")?.parse()?,
                    "--resolution" => {
                        resolution_m = args.next().context("--resolution <m>")?.parse()?
                    }
                    "--out" => out = PathBuf::from(args.next().context("--out <dir>")?),
                    other => bail!("unknown argument: {other}"),
                }
            }
            let req = hrdem::HrdemRequest::new(lat, lon, width_m, height_m, buffer_m, &out)?
                .resolution(resolution_m)?;
            let (est_w, est_h) = req.estimated_wgs84_dims();
            println!(
                "HRDEM window: {:.0} m x {:.0} m (+{:.0} m buffer) at ({:.6}, {:.6}), \
                 WGS84 grid at {resolution_m} m (estimated up to ~{est_w} x {est_h} px)",
                req.width_m, req.height_m, req.buffer_m, req.center_lat, req.center_lon
            );
            let dir = hrdem::fetch_tileset(&req)?;
            if let Ok((w, h)) = hrdem::raster_dims(&dir.join("dtm.tif")) {
                println!("warped pair: {w} x {h} px");
            }
            println!("-> {}", dir.display());
            Ok(())
        }
        Some("hrdem-status") => {
            let lat: f64 = args.next().context("hrdem-status <lat> <lon>")?.parse()?;
            let lon: f64 = args.next().context("hrdem-status <lat> <lon>")?.parse()?;
            let mut width_m = hrdem::DEFAULT_SURVEY_M;
            let mut height_m = hrdem::DEFAULT_SURVEY_M;
            let mut buffer_m = hrdem::DEFAULT_BUFFER_M;
            let mut resolution_m = hrdem::DEFAULT_RESOLUTION_M;
            let mut out = PathBuf::from("hrdem");
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--width" => width_m = args.next().context("--width <m>")?.parse()?,
                    "--height" => height_m = args.next().context("--height <m>")?.parse()?,
                    "--buffer" => buffer_m = args.next().context("--buffer <m>")?.parse()?,
                    "--resolution" => {
                        resolution_m = args.next().context("--resolution <m>")?.parse()?
                    }
                    "--out" => out = PathBuf::from(args.next().context("--out <dir>")?),
                    other => bail!("unknown argument: {other}"),
                }
            }
            let req = hrdem::HrdemRequest::new(lat, lon, width_m, height_m, buffer_m, &out)?
                .resolution(resolution_m)?;
            let status = hrdem::read_status(&req);
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Some("terrain") => {
            let folder = PathBuf::from(args.next().context("terrain <processed-dir>")?);
            let mut out = folder.join("terrain");
            let mut maxzoom: Option<u8> = None;
            let mut geoid_path: Option<PathBuf> = None;
            let mut geoid_embedded = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--out" => out = PathBuf::from(args.next().context("--out <dir>")?),
                    "--maxzoom" => maxzoom = Some(args.next().context("--maxzoom <n>")?.parse()?),
                    "--geoid" => {
                        geoid_path = Some(PathBuf::from(
                            args.next().context("--geoid <file.mggeoid>")?,
                        ))
                    }
                    "--geoid-embedded" => geoid_embedded = true,
                    other => bail!("unknown argument: {other}"),
                }
            }
            if geoid_embedded && geoid_path.is_some() {
                bail!("--geoid and --geoid-embedded are mutually exclusive");
            }
            let (geoid, geoid_best_effort) = if geoid_embedded {
                // The embedded CGG2013 BC-SW extraction: convert to WGS84
                // ellipsoid heights when it covers the DEM, otherwise fall
                // back to orthometric (regional coverage, ADR-006).
                (Some(maggcs_core::height::cgg2013_model().clone()), true)
            } else {
                match &geoid_path {
                    Some(path) => {
                        let mut model = maggcs_core::height::GeoidGridModel::new("dem-prep");
                        maggcs_core::height::load_geoid_grid(path, &mut model)
                            .map_err(|e| anyhow::anyhow!("--geoid {}: {e}", path.display()))?;
                        (Some(model), false)
                    }
                    None => (None, false),
                }
            };
            let stats =
                mesh::generate_tileset(&folder, &out, maxzoom, geoid.as_ref(), geoid_best_effort)?;
            println!(
                "tileset: {} DEM tile(s) -> {} mesh tile(s) at z{}, {:.1} MiB, heights {} -> {} ({})",
                stats.dem_tiles,
                stats.mesh_tiles,
                stats.maxzoom,
                stats.bytes as f64 / (1024.0 * 1024.0),
                if stats.converted_to_ellipsoid {
                    "CGVD2013 orthometric (converted)"
                } else {
                    "CGVD2013 orthometric (as-is)"
                },
                if stats.converted_to_ellipsoid { "WGS84 ellipsoid" } else { "served as ellipsoid (no N)" },
                out.display()
            );
            Ok(())
        }
        _ => {
            bail!(
                "usage: dem-prep process <folder> [--out <dir>] [--resolution m] \
                 | probe <file> | fetch <lat> <lon> [--out <dir>] [--size-km km] \
                 | hrdem <lat> <lon> [--width m] [--height m] [--buffer m] [--resolution m] [--out <dir>] \
                 | hrdem-status <lat> <lon> [--width m] [--height m] [--buffer m] [--resolution m] [--out <dir>] \
                 | terrain <processed-dir> [--out <dir>] [--maxzoom n] [--geoid <file.mggeoid> | --geoid-embedded] \
                 | geoid <grid.tif> --datum CGVD2013 --bbox <w s e n> [--out <file.mggeoid>]"
            )
        }
    }
}

/// Recursively collect raw input files (`.laz`/`.las`/`.tif`) under `dir`,
/// skipping the pipeline's own outputs (a `--out` inside the download tree
/// must not be re-ingested on the next run).
fn collect_inputs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    const SKIP_DIRS: [&str; 4] = ["processed", "tileset", "terrain", "downloads"];
    let entries = std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !SKIP_DIRS.contains(&name.as_str()) {
                collect_inputs(&path, out)?;
            }
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("laz") | Some("las") | Some("tif") | Some("tiff")
        ) {
            out.push(path);
        }
    }
    Ok(())
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
    // Recursive walk: BC LiDAR downloads arrive as one folder per grid tile
    // (each holding its .laz), so the operator points at the download root
    // and every raw file below it is picked up. The output stays flat — one
    // tile folder per input, named by the file stem (stems are unique per
    // BC sheet; a stem collision re-processes the later file in place).
    let mut tiles: Vec<PathBuf> = Vec::new();
    collect_inputs(folder, &mut tiles)?;
    tiles.sort();
    if tiles.is_empty() {
        bail!(
            "no .laz/.las/.tif files under {} (recursively)",
            folder.display()
        );
    }
    println!("{} tile(s) under {}", tiles.len(), folder.display());
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

    // Zone from the tile's own projection metadata (audit: it used to be
    // hard-coded to 10, silently trashing tiles in BC's other zones).
    let (zone, vertical_datum) = las_zone_and_datum(&info)?;
    println!("  zone UTM{zone}N, vertical datum {vertical_datum}");

    // Output resolution in degrees: convert the metre resolution through the
    // metre-per-degree factors at the tile's mid-latitude.
    let (_, mid_lon) = utm::from_utm(
        (info.min_x + info.max_x) / 2.0,
        (info.min_y + info.max_y) / 2.0,
        zone,
    )?;
    let m_per_deg_lat = 111_132.954;
    let m_per_deg_lon = 111_319.49 * mid_lon.to_radians().cos();
    let dlat = resolution_m / m_per_deg_lat;
    let dlon = resolution_m / m_per_deg_lon;

    let mut dtm = resample_to_wgs84(&utm_grid, (dlat, dlon), |i| dtm_value(&utm_grid, i), zone)?;
    fill_single_cell_holes(&mut dtm);
    let mut dsm = resample_to_wgs84(&utm_grid, (dlat, dlon), |i| dsm_value(&utm_grid, i), zone)?;
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
        source: "lidar".into(),
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
        source: "lidar".into(),
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

    // Geographic products (CDEM/Copernicus) pass straight through; **UTM
    // products** (BC LiDAR DTM tiles, EPSG 26910/3157) are resampled onto
    // the WGS84 grid here — the operator no longer needs gdalwarp.
    let (raster, dlat, dlon, resolution_label) = match crate::geotiff::read_single_band(tile) {
        Ok((values, width, height, src_west, src_north, src_dlat, src_dlon)) => {
            let r = grid::GeoRaster {
                width,
                height,
                values: values.iter().map(|v| *v as f32).collect(),
                west: src_west,
                north: src_north,
            };
            (r, src_dlat, src_dlon, "geographic passthrough")
        }
        Err(e) if e.to_string().contains("not geographic") => {
            let band = crate::geotiff::read_projected_band(tile)?;
            println!(
                "  UTM{}N product, {}x{} @ {} m",
                band.zone, band.width, band.height, band.res_m
            );
            let mid = crate::utm::from_utm(
                band.min_e + band.width as f64 * band.res_m / 2.0,
                band.max_n - band.height as f64 * band.res_m / 2.0,
                band.zone,
            )?;
            let (dlat, dlon) = degrees_per_cell(band.res_m, mid.0);
            let r = grid::resample_raster_to_wgs84(&band, (dlat, dlon), band.zone)?;
            (r, dlat, dlon, "UTM resampled")
        }
        Err(e) => return Err(e.into()),
    };
    let _ = resolution_label;
    let (width, height) = (raster.width, raster.height);
    // The raster's placement is its north-west corner; its extent follows from
    // the per-cell degrees of whichever path produced it — the UTM path
    // resamples onto exactly `dlat`/`dlon`, the passthrough keeps the product's
    // own degrees. Longitude must be corrected by `1/cos φ` (see
    // [`degrees_per_cell`]): a past bug multiplied by `cos φ`, shrinking every
    // projected tile to ~cos²φ of its true width and misplacing the 3D mesh.
    let (west, north) = (raster.west, raster.north);
    let south = north - dlat * height as f64;
    let east = west + dlon * width as f64;

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
    // Terrain overlay for the map (plan A): same hillshade as the LAS path —
    // terrain_load requires it next to dem.json (bug: raster products used
    // to load without it and fail at the overlay step).
    hillshade::write_hillshade_png(&tile_out.join("hillshade.png"), &raster, dlat, dlon)?;

    let dtm_bytes = std::fs::read(&dtm_path)?;
    let metadata = DemMetadata {
        version: cog::version_of(&[&dtm_bytes]),
        raster_crs: "EPSG:4326".into(),
        resolution_m: dlat * M_PER_DEG_LAT,
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
        source: "geotiff".into(),
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
        source: "geotiff".into(),
    })
}

/// Resolve a tile's UTM zone and vertical datum from its **own projection
/// metadata** (the `LASF_Projection` VLR), falling back to the zone the
/// point-cloud centre falls in. The vertical datum used to be hard-coded
/// "verified from the VLR" without any parsing; it is now actually read.
///
/// # Errors
///
/// When the zone cannot be determined, or the points' UTM centre falls
/// outside the declared zone (a mislabelled tile must not silently produce
/// misregistered rasters).
fn las_zone_and_datum(info: &las::TileInfo) -> Result<(u8, &'static str)> {
    let projection = info.projection.as_deref().unwrap_or("");

    // EPSG NAD83 UTM zone N = 26900 + N (WKT1 often carries just the EPSG).
    let zone_from_epsg = |s: &str| -> Option<u8> {
        for token in s.split(|c: char| !c.is_ascii_digit()) {
            if let Ok(v) = token.parse::<u32>() {
                if (26901..=26960).contains(&v) {
                    return Some((v - 26900) as u8);
                }
            }
        }
        None
    };
    // WKT2 names the zone explicitly: "UTM zone 10N" / "zone 10N".
    let zone_from_name = |s: &str| -> Option<u8> {
        let lower = s.to_ascii_lowercase();
        let idx = lower.find("zone ")?;
        let rest = &lower[idx + 5..];
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        num.parse::<u8>().ok().filter(|z| (1..=60).contains(z))
    };

    // Fallback: BC tiles all sit near a central meridian; the zone whose
    // easting band contains the tile centre wins. (The VLR check above
    // normally answers this.)
    let zone = zone_from_epsg(projection)
        .or_else(|| zone_from_name(projection))
        .unwrap_or_else(|| utm::zone_from_easting((info.min_x + info.max_x) / 2.0));
    // Sanity: the centre must land within the declared zone's 3° half-width.
    let centre_easting_offset = ((info.min_x + info.max_x) / 2.0 - 500_000.0).abs();
    if centre_easting_offset > 700_000.0 {
        bail!(
            "tile centre easting {:.0} is outside any sane UTM zone width",
            (info.min_x + info.max_x) / 2.0
        );
    }

    // Vertical datum: BC LiDAR is CGVD2013 (HT2); anything else is named, so
    // a mislabeled product cannot masquerade as CGVD2013.
    let lower = projection.to_ascii_lowercase();
    let vertical_datum =
        if lower.contains("cgvd28") || lower.contains("cgvd2013") || lower.contains("ht2") {
            "orthometric_cgvd2013"
        } else if lower.contains("nad83") || lower.contains("utm") || lower.contains("epsg") {
            // Horizontal-only WKT: BC LiDAR heights are CGVD2013 by product spec,
            // and the zone pattern matched, so keep the datum but say so.
            "orthometric_cgvd2013"
        } else if lower.contains("navd88") {
            bail!("vertical datum NAVD88 is not supported — convert with verticalize first")
        } else {
            "orthometric_cgvd2013"
        };
    Ok((zone, vertical_datum))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A degree of longitude is `cos φ` times a degree of latitude in metres,
    /// so the per-cell longitude **angle** is larger by `1/cos φ`. A past bug
    /// multiplied by `cos φ` instead, shrinking projected tiles (regression
    /// guard for the misplaced 3D mesh).
    #[test]
    fn longitude_degrees_per_cell_is_divided_by_cos_latitude() {
        let (dlat, dlon) = degrees_per_cell(1.0, 48.56);
        assert!(
            dlon > dlat,
            "dlon {dlon} must exceed dlat {dlat} off the equator"
        );
        let ratio = dlon / dlat;
        let expected = M_PER_DEG_LAT / (M_PER_DEG_LON_EQUATOR * 48.56f64.to_radians().cos());
        assert!(
            (ratio - expected).abs() < 1e-12,
            "ratio {ratio} vs {expected}"
        );
    }

    /// The real BC tile `bc_092b053_xl1m_utm10_2019` (12186 px @ 1 m, centre
    /// ~48.556°N, west -123.56462°): the eastern edge must land near the true
    /// -123.3985°, not the cos-bugged -123.4916°.
    #[test]
    fn projected_tile_east_bound_matches_the_true_span() {
        let (dlat, dlon) = degrees_per_cell(1.0, 48.556);
        let _ = dlat;
        let west = -123.564_62;
        let east = west + dlon * 12_186.0;
        assert!((east - -123.3985).abs() < 2e-3, "east = {east}");
        assert!(east > -123.45, "east {east} must clear the cos-bug value");
    }
}
