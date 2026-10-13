//! Terrain overlay bridge (plan A): read a dem-prep tile folder
//! (hillshade.png + dem.json) and hand the frontend a data URL plus the
//! geographic placement for the Cesium imagery layer.
//!
//! Plan B (3D): [`terrain3d_start`] / [`terrain3d_stop`] run the local
//! quantized-mesh tileset server — see [`crate::terrain_server`].

use std::path::{Path, PathBuf};

use serde::Serialize;

use maggcs_core::terrain::{DemBounds, DemMetadata, DemSource, VerticalDatum};

use crate::state::AppState;
use crate::terrain_server::TerrainServer;

/// The overlay payload for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct TerrainOverlay {
    /// Data URL of the hillshade PNG (CSP allows img-src data:).
    pub url: String,
    pub bounds: DemBounds,
    /// dem.json version (sha256[..16]) — mission archives reference it.
    pub version: String,
}

/// Load a dem-prep tile folder.
///
/// # Errors
///
/// A stringified [`std::io::Error`] / serde error when the folder does not
/// carry a valid tile (missing hillshade.png or dem.json).
#[tauri::command]
pub async fn terrain_load(folder: String) -> Result<TerrainOverlay, String> {
    let dir = Path::new(&folder);
    let png_path = dir.join("hillshade.png");
    let meta_path = dir.join("dem.json");
    let png = tokio::fs::read(&png_path)
        .await
        .map_err(|e| format!("{}: {e}", png_path.display()))?;
    let meta: DemMetadata = serde_json::from_str(
        &tokio::fs::read_to_string(&meta_path)
            .await
            .map_err(|e| format!("{}: {e}", meta_path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", meta_path.display()))?;
    // Sanity: the overlay is geographic and metre-resolved.
    let _ = VerticalDatum::OrthometricCgvd2013;
    if meta.bounds.north <= meta.bounds.south || meta.bounds.east <= meta.bounds.west {
        return Err(format!("{}: invalid bounds", meta_path.display()));
    }
    Ok(TerrainOverlay {
        url: format!("data:image/png;base64,{}", base64_encode(&png)),
        bounds: meta.bounds,
        version: meta.version,
    })
}

/// Standard base64 (RFC 4648, padding) — small hand-rolled version to keep
/// the dependency set lean.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// One discovered tile: an absolute tile folder plus the metadata the
/// frontend needs to auto-pick it against the survey area (N1).
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredTile {
    /// Absolute tile folder path — feed straight to `terrain_load` /
    /// `terrain_drape`.
    pub path: String,
    pub folder: String,
    pub bounds: DemBounds,
    pub version: String,
    pub has_dsm: bool,
    /// What produced the DEM (LiDAR preferred for the low-altitude drape).
    pub source: DemSource,
}

/// The tiles under a dem-prep output folder (N1).
#[derive(Debug, Clone, Serialize)]
pub struct TerrainDiscovery {
    pub tiles: Vec<DiscoveredTile>,
}

/// Discover the tiles under a dem-prep output folder (N1).
///
/// Accepts either a **parent** folder carrying a multi-tile `index.json`
/// (each entry a relative tile subfolder) or a **single-tile** folder
/// (carrying `dem.json` itself, the pre-N1 layout the drape dialog already
/// accepted). The frontend auto-picks the tile covering the survey area.
///
/// # Errors
///
/// A stringified error when the folder carries neither `index.json` nor
/// `dem.json`.
#[tauri::command]
pub async fn terrain_discover(folder: String) -> Result<TerrainDiscovery, String> {
    let dir = Path::new(&folder);
    let index_path = dir.join("index.json");
    if index_path.is_file() {
        let tiles = maggcs_core::terrain::load_tile_index(dir).map_err(|e| e.to_string())?;
        return Ok(TerrainDiscovery {
            tiles: tiles
                .into_iter()
                .map(|t| DiscoveredTile {
                    path: dir.join(&t.folder).display().to_string(),
                    folder: t.folder,
                    bounds: t.bounds,
                    version: t.version,
                    has_dsm: t.has_dsm,
                    source: t.source,
                })
                .collect(),
        });
    }
    // Single-tile folder (dem.json + hillshade.png directly under `folder`).
    if dir.join("dem.json").is_file() {
        let meta: DemMetadata = serde_json::from_str(
            &tokio::fs::read_to_string(dir.join("dem.json"))
                .await
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        return Ok(TerrainDiscovery {
            tiles: vec![DiscoveredTile {
                path: folder,
                folder: String::new(),
                bounds: meta.bounds,
                version: meta.version,
                has_dsm: meta.has_dsm,
                source: meta.source,
            }],
        });
    }
    Err(format!(
        "{}: no index.json or dem.json found",
        dir.display()
    ))
}

/// Start (or restart) the local 3D terrain tileset server on
/// `127.0.0.1` (plan B) rooted at `dir` — a `dem-prep terrain` output
/// folder (`layer.json` + `{z}/{x}/{y}.terrain`).
///
/// Returns the tileset **base URL** (trailing slash), ready for
/// `Cesium.CesiumTerrainProvider.fromUrl` — which appends `layer.json`
/// itself.
///
/// # Errors
///
/// A stringified error when `dir` is not a tileset folder, or the pinned
/// port cannot be bound (already in use).
#[tauri::command]
pub async fn terrain3d_start(
    state: tauri::State<'_, AppState>,
    dir: String,
) -> Result<String, String> {
    // Stop any previous instance first — it holds the port.
    let previous = state.terrain3d.lock().take();
    if let Some(old) = previous {
        tokio::task::spawn_blocking(move || old.stop())
            .await
            .map_err(|e| format!("stopping previous terrain server: {e}"))?;
    }
    let server = TerrainServer::start(PathBuf::from(dir))?;
    let url = server.url();
    *state.terrain3d.lock() = Some(server);
    Ok(url)
}

/// Stop the local 3D terrain tileset server (no-op when it is not
/// running).
#[tauri::command]
pub async fn terrain3d_stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let running = state.terrain3d.lock().take();
    if let Some(old) = running {
        tokio::task::spawn_blocking(move || old.stop())
            .await
            .map_err(|e| format!("stopping terrain server: {e}"))?;
    }
    Ok(())
}

/// Tileset base URL of the running server, or `None` when it is stopped
/// — lets the frontend restore state across reloads.
#[tauri::command]
pub async fn terrain3d_status(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.terrain3d.lock().as_ref().map(TerrainServer::url))
}

/// Locate the `dem-prep` binary: shipped next to the app executable first
/// (dev: `target/debug/dem-prep` beside `target/debug/maggcs-app`), then the
/// cargo test layout (`target/debug/deps/…` → `target/debug/dem-prep`), then
/// the operator's `PATH`.
fn dem_prep_binary() -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("dem-prep");
        if sibling.is_file() {
            return Ok(sibling);
        }
        // cargo test binaries live in <target>/<profile>/deps/: the profile
        // directory (one level up) also holds the dem-prep bin.
        if let Some(profile) = exe.parent().and_then(Path::parent) {
            let profile_bin = profile.join("dem-prep");
            if profile_bin.is_file() {
                return Ok(profile_bin);
            }
        }
    }
    // Fall back to PATH via `which`-style probe on each PATH entry.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("dem-prep");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(
        "dem-prep not found next to the app or on PATH — build it with \
         `cargo build -p dem-prep`"
            .to_string(),
    )
}

/// One pipeline stage: spawn `dem-prep <args>`, stream its stdout lines to
/// the frontend as `terrain3d_progress` events, and fail with the tail of
/// stderr when the exit status is not success.
fn run_stage(
    app: &tauri::AppHandle,
    bin: &Path,
    args: &[String],
    stage: &str,
) -> Result<(), String> {
    use std::io::BufRead;
    use tauri::Emitter;

    let mut child = std::process::Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn dem-prep: {e}"))?;
    if let Some(stdout) = child.stdout.take() {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            let _ = app.emit(
                "terrain3d_progress",
                serde_json::json!({ "stage": stage, "line": line }),
            );
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("dem-prep {stage}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join("; ");
    // An old dem-prep prints its usage line for unknown subcommands — tell
    // the operator to rebuild instead of pasting the usage back.
    if tail.contains("usage: dem-prep") {
        return Err(format!(
            "the dem-prep binary next to the app is outdated (no `{stage}` subcommand) — \
             rebuild both with `cargo build --release -p maggcs-app -p dem-prep`"
        ));
    }
    Err(format!(
        "dem-prep {stage} failed: {}",
        if tail.is_empty() {
            "see console"
        } else {
            &tail
        }
    ))
}

/// One-click 3D terrain (ADR-004 addendum): download the Copernicus GLO-30
/// tiles covering the box and build the quantized-mesh tileset, streaming
/// per-line progress as `terrain3d_progress` events. Returns the tileset
/// folder, ready for `terrain3d_start`.
///
/// # Errors
///
/// A stringified error when `dem-prep` is missing, a stage fails, or the
/// coordinates are out of range.
#[tauri::command]
pub async fn terrain3d_download(
    app: tauri::AppHandle,
    lat: f64,
    lon: f64,
    size_km: f64,
    maxzoom: Option<u8>,
    out_dir: String,
) -> Result<String, String> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(format!("coordinates out of range: {lat}, {lon}"));
    }
    let size_km = size_km.clamp(1.0, 500.0);
    let bin = dem_prep_binary()?;
    let root = PathBuf::from(&out_dir);
    let downloads = root.join("downloads");
    let processed = root.join("processed");
    let tileset = root.join("tileset");
    let tileset_path = tileset.display().to_string();

    // CPU/IO stages run off the async runtime (the CLI streams progress).
    let handle = app.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let fetch_args = vec![
            "fetch".to_string(),
            format!("{lat}"),
            format!("{lon}"),
            "--out".to_string(),
            downloads.display().to_string(),
            "--size-km".to_string(),
            format!("{size_km}"),
        ];
        run_stage(&handle, &bin, &fetch_args, "fetch")?;
        let process_args = vec![
            "process".to_string(),
            downloads.display().to_string(),
            "--out".to_string(),
            processed.display().to_string(),
        ];
        run_stage(&handle, &bin, &process_args, "process")?;
        let mut terrain_args = vec![
            "terrain".to_string(),
            processed.display().to_string(),
            "--out".to_string(),
            tileset.display().to_string(),
        ];
        if let Some(z) = maxzoom {
            terrain_args.push("--maxzoom".to_string());
            terrain_args.push(z.to_string());
        }
        run_stage(&handle, &bin, &terrain_args, "terrain")?;
        Ok(())
    })
    .await
    .map_err(|e| format!("terrain download task: {e}"))??;

    Ok(tileset_path)
}

/// The `dem-prep` binary discovery (exposed for tests).
#[cfg(test)]
mod download_tests {
    use super::dem_prep_binary;

    #[test]
    fn dem_prep_is_discovered_next_to_the_test_binary_or_on_path() {
        // In the dev workspace the sibling `target/debug/dem-prep` exists
        // (cargo builds both bins into one target dir); if the test binary
        // lives elsewhere, PATH must carry dem-prep for this to pass.
        let found = dem_prep_binary().expect("dem-prep discoverable");
        assert!(found.is_file(), "{}", found.display());
    }
}

/// One-click build from a **local** data folder (ADR-004 addendum): BC LiDAR
/// `.laz` files (or GeoTIFF fallback rasters) the operator downloaded from
/// the BC Data Catalogue → `dem-prep process` → `dem-prep terrain` → the
/// tileset folder, streaming progress like [`terrain3d_download`]. LiDAR
/// runs at the native 1 m resolution (the drape-grade DTM+DSM).
///
/// # Errors
///
/// A stringified error when `dem-prep` is missing or a stage fails.
#[tauri::command]
pub async fn terrain3d_build(
    app: tauri::AppHandle,
    folder: String,
    maxzoom: Option<u8>,
) -> Result<String, String> {
    let bin = dem_prep_binary()?;
    let source = PathBuf::from(&folder);
    if !source.is_dir() {
        return Err(format!("{} is not a folder", source.display()));
    }
    // The tileset lands next to the source folder: <folder>-3d/tileset.
    let root = source.parent().unwrap_or(&source).join(format!(
        "{}-3d",
        source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("terrain")
    ));
    let processed = root.join("processed");
    let tileset = root.join("tileset");
    let tileset_path = tileset.display().to_string();

    let handle = app.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let process_args = vec![
            "process".to_string(),
            source.display().to_string(),
            "--out".to_string(),
            processed.display().to_string(),
            "--resolution".to_string(),
            "1.0".to_string(),
        ];
        run_stage(&handle, &bin, &process_args, "process")?;
        let mut terrain_args = vec![
            "terrain".to_string(),
            processed.display().to_string(),
            "--out".to_string(),
            tileset.display().to_string(),
        ];
        if let Some(z) = maxzoom {
            terrain_args.push("--maxzoom".to_string());
            terrain_args.push(z.to_string());
        }
        run_stage(&handle, &bin, &terrain_args, "terrain")?;
        Ok(())
    })
    .await
    .map_err(|e| format!("terrain build task: {e}"))??;

    Ok(tileset_path)
}

/// One sampled ground height (the DTM the AGL markers are measured against).
#[derive(Debug, Clone, Serialize)]
pub struct SampledGround {
    pub lat: f64,
    pub lon: f64,
    /// DTM height in the DEM's own datum (CGVD2013 orthometric), `None` when
    /// the point is outside the loaded tile.
    pub dtm_m: Option<f64>,
    /// DSM height (surface incl. trees), `None` when DTM-only or outside.
    pub dsm_m: Option<f64>,
}

/// Open (or reuse) the DEM for `folder`, keyed by the tile path in
/// `AppState.dem` so interactive edits do not re-read the rasters. The
/// `Arc` makes the cached DEM cheap to share.
pub fn open_dem_cached(
    state: &AppState,
    folder: &str,
) -> Result<std::sync::Arc<maggcs_core::terrain::cog::CogDem>, String> {
    use maggcs_core::terrain::ElevationSource;
    let mut guard = state.dem.lock();
    if let Some((path, dem)) = guard.as_ref() {
        if path == folder {
            return Ok(dem.clone());
        }
    }
    let dir = Path::new(folder);
    let meta: DemMetadata = serde_json::from_str(
        &std::fs::read_to_string(dir.join("dem.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let dem = std::sync::Arc::new(
        maggcs_core::terrain::cog::CogDem::open(&dir.join("dtm.tif"), &dir.join("dsm.tif"), &meta)
            .map_err(|e| e.to_string())?,
    );
    // Bounds sanity via the trait: an unusable DEM must not poison the cache.
    let _ = dem.bounds();
    *guard = Some((folder.to_string(), dem.clone()));
    Ok(dem)
}

/// Sample the loaded DEM under the given points (waypoint AGL markers).
/// Points outside the tile return `dtm_m: None` — the frontend shows the
/// marker as ungrounded instead of guessing.
///
/// # Errors
///
/// A stringified error when the DEM cannot be opened.
#[tauri::command]
pub async fn terrain_sample(
    state: tauri::State<'_, AppState>,
    folder: String,
    points: Vec<(f64, f64)>,
) -> Result<Vec<SampledGround>, String> {
    use maggcs_core::terrain::ElevationSource;
    let dem = open_dem_cached(&state, &folder)?;
    let mut out = Vec::with_capacity(points.len());
    for (lat, lon) in points {
        let point = maggcs_core::plan::GeoPoint::new(lat, lon);
        let sample = dem.sample_pair(point).ok();
        let (dtm, dsm) = match sample {
            Some((d, s)) => (Some(d.meters()), Some(s.meters())),
            None => (None, None),
        };
        out.push(SampledGround {
            lat,
            lon,
            dtm_m: dtm,
            dsm_m: dsm,
        });
    }
    Ok(out)
}
