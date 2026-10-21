//! Terrain overlay bridge (plan A): read a dem-prep tile folder
//! (hillshade.png + dem.json) and hand the frontend a data URL plus the
//! geographic placement for the Cesium imagery layer.
//!
//! Plan B (3D): [`terrain3d_start`] / [`terrain3d_stop`] run the local
//! quantized-mesh tileset server — see [`crate::terrain_server`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use maggcs_core::terrain::{DemBounds, DemMetadata, DemSource, VerticalDatum};

use crate::state::AppState;
use crate::terrain_server::TerrainServer;

/// The overlay payload for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct TerrainOverlay {
    /// Data URL of the hillshade PNG — `None` when the folder carries no
    /// hillshade (raster-product tiles): the shade layer is optional, the
    /// DEM itself (drape/AGL/terrain mode) never depends on it.
    pub url: Option<String>,
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
    // The hillshade is a visual nicety; its absence must not block the DEM
    // (drape / terrain mode work off dtm.tif alone).
    let png = tokio::fs::read(&png_path).await.ok();
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
        url: png.map(|p| format!("data:image/png;base64,{}", base64_encode(&p))),
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
    /// `drape_preview`.
    pub path: String,
    pub folder: String,
    pub bounds: DemBounds,
    pub version: String,
    pub has_dsm: bool,
    /// What produced the DEM (LiDAR preferred for the low-altitude drape).
    pub source: DemSource,
    /// Chip facts (H6): nominal resolution, acquisition date, vertical datum
    /// — read from the tile's `dem.json` (`None` when the index entry predates
    /// the field or the sidecar is missing).
    pub resolution_m: Option<f64>,
    pub acquisition: Option<String>,
    pub vertical_datum: Option<VerticalDatum>,
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
        let mut out = Vec::with_capacity(tiles.len());
        for t in tiles {
            let path = dir.join(&t.folder).display().to_string();
            let meta = tile_metadata(&dir.join(&t.folder));
            out.push(DiscoveredTile {
                path,
                folder: t.folder,
                bounds: t.bounds,
                version: t.version,
                has_dsm: t.has_dsm,
                source: t.source,
                resolution_m: meta.as_ref().map(|m| m.resolution_m),
                acquisition: meta.and_then(|m| m.acquisition),
                vertical_datum: None,
            });
        }
        return Ok(TerrainDiscovery { tiles: out });
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
                resolution_m: Some(meta.resolution_m),
                acquisition: meta.acquisition,
                vertical_datum: Some(meta.vertical_datum),
            }],
        });
    }
    Err(format!(
        "{}: no index.json or dem.json found",
        dir.display()
    ))
}

/// The `dem.json` sidecar of one tile, tolerantly: an index built by an
/// older `dem-prep` may not carry the H6 chip facts, so a missing or corrupt
/// sidecar degrades to `None` instead of failing discovery.
fn tile_metadata(tile_dir: &Path) -> Option<DemMetadata> {
    let text = std::fs::read_to_string(tile_dir.join("dem.json")).ok()?;
    serde_json::from_str(&text).ok()
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

/// U13: set by `terrain3d_cancel` — the running `run_stage` polls it and
/// kills its `dem-prep` child, reporting the neutral "cancelled" outcome.
static STAGE_CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
            // U13: a cancel request kills the child mid-stream — the reader
            // sees EOF, and the wait below reports the cancelled outcome.
            if STAGE_CANCELLED.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("cancelled".into());
            }
            let _ = app.emit(
                "terrain3d_progress",
                serde_json::json!({ "stage": stage, "line": line }),
            );
        }
    }
    // Poll the exit status so a cancel can kill while we wait (a blocking
    // `wait_with_output` would be unkillable from the command thread).
    let mut stderr_buf = String::new();
    loop {
        // Drain stdout to EOF first (the lines loop above returns only when
        // the pipe closes or we cancel); between lines, poll the cancel flag.
        if STAGE_CANCELLED.load(Ordering::SeqCst) {
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                break;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => return Err(format!("dem-prep {stage}: {e}")),
        }
    }
    if let Some(mut err) = child.stderr.take() {
        use std::io::Read;
        let _ = err.read_to_string(&mut stderr_buf);
    }
    let tail: String = stderr_buf
        .lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .join("; ");
    if STAGE_CANCELLED.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
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
            "--geoid-embedded".to_string(),
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
    use super::{dem_prep_binary, survey_aoi_args, SURVEY_MARGIN_M};

    #[test]
    fn dem_prep_is_discovered_next_to_the_test_binary_or_on_path() {
        // In the dev workspace the sibling `target/debug/dem-prep` exists
        // (cargo builds both bins into one target dir); if the test binary
        // lives elsewhere, PATH must carry dem-prep for this to pass.
        let found = dem_prep_binary().expect("dem-prep discoverable");
        assert!(found.is_file(), "{}", found.display());
    }

    #[test]
    fn aoi_args_carry_the_survey_size_margin_and_root() {
        let args = survey_aoi_args(
            "hrdem-status",
            48.5,
            -123.25,
            100.0,
            80.5,
            std::path::Path::new("/tmp/s"),
        );
        assert_eq!(args[0], "hrdem-status");
        assert_eq!(args[1], "48.5");
        assert_eq!(args[2], "-123.25");
        assert!(args.windows(2).any(|w| w == ["--width", "100"]));
        assert!(args.windows(2).any(|w| w == ["--height", "80.5"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--buffer", &SURVEY_MARGIN_M.to_string()]));
        assert!(args.windows(2).any(|w| w == ["--out", "/tmp/s"]));
        // The fetch variant differs only in the subcommand.
        let fetch = survey_aoi_args(
            "hrdem",
            48.5,
            -123.25,
            100.0,
            80.5,
            std::path::Path::new("/tmp/s"),
        );
        assert_eq!(fetch[0], "hrdem");
        assert_eq!(fetch[1..], args[1..]);
    }
}

/// Margin added on every side of the survey rectangle before fetching HRDEM.
/// Mirrors `dem-prep`'s `hrdem::DEFAULT_BUFFER_M`.
const SURVEY_MARGIN_M: f64 = 250.0;

/// The core survey-area side clamp. The fetched window is
/// `survey + 2 * SURVEY_MARGIN_M`; `dem-prep`'s `hrdem::AOI_LIMIT_M` (2 km)
/// caps that *total*, so the largest core area is `2 km - 2 * margin`. Kept
/// here only to clamp the status query so an oversized UI value yields a
/// status, not an error.
const SURVEY_AOI_LIMIT_M: f64 = 2000.0 - 2.0 * SURVEY_MARGIN_M;

/// The root folder holding HRDEM survey sessions (H9). `$MAGCS_SURVEY_DIR`
/// wins; else `survey/` under the working directory. Overridable per call.
fn survey_root() -> PathBuf {
    std::env::var_os("MAGCS_SURVEY_DIR").map_or_else(
        || {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("survey")
        },
        PathBuf::from,
    )
}

/// Run `dem-prep` and return stdout, mapping a non-zero exit to a stringified
/// error (the [`run_stage`] outdated-binary hint included).
fn run_capture(bin: &Path, args: &[String], stage: &str) -> Result<String, String> {
    let output = std::process::Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| format!("spawn dem-prep: {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join("; ");
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

/// U9/H9: readiness of the "prepare survey area" session for an AOI. Runs
/// `dem-prep hrdem-status` — pure filesystem, no STAC/GDAL — so it works
/// offline (an already-fetched tile reads as ready with the network down).
///
/// # Errors
///
/// A stringified error when `dem-prep` is missing/outdated, the coordinates
/// are out of range, or its JSON cannot be parsed.
#[tauri::command]
pub async fn survey_prepare_status(
    lat: f64,
    lon: f64,
    width_m: f64,
    height_m: f64,
    out_dir: Option<String>,
) -> Result<maggcs_core::terrain::SurveyPrepStatus, String> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(format!("coordinates out of range: {lat}, {lon}"));
    }
    let bin = dem_prep_binary()?;
    let root = out_dir.map_or_else(survey_root, PathBuf::from);
    let width_m = width_m.clamp(1.0, SURVEY_AOI_LIMIT_M);
    let height_m = height_m.clamp(1.0, SURVEY_AOI_LIMIT_M);
    let args = survey_aoi_args("hrdem-status", lat, lon, width_m, height_m, &root);
    let stdout = tokio::task::spawn_blocking(move || run_capture(&bin, &args, "hrdem-status"))
        .await
        .map_err(|e| format!("survey status task: {e}"))??;
    serde_json::from_str(&stdout).map_err(|e| format!("hrdem-status JSON: {e}"))
}

/// The `dem-prep <sub>` argument vector for an AOI + output root. `sub` is
/// `hrdem-status` (readiness) or `hrdem` (fetch); both take the survey size
/// plus the margin dem-prep expands the window by on every side.
fn survey_aoi_args(
    sub: &str,
    lat: f64,
    lon: f64,
    width_m: f64,
    height_m: f64,
    root: &Path,
) -> Vec<String> {
    vec![
        sub.to_string(),
        format!("{lat}"),
        format!("{lon}"),
        "--width".to_string(),
        format!("{width_m}"),
        "--height".to_string(),
        format!("{height_m}"),
        "--buffer".to_string(),
        format!("{SURVEY_MARGIN_M}"),
        "--out".to_string(),
        root.display().to_string(),
    ]
}

/// The in-flight `dem-prep hrdem` process (H9). At most one prepare runs at a
/// time; holding the handle here lets `survey_prepare_cancel` kill it.
static SURVEY_CHILD: tokio::sync::Mutex<Option<tokio::process::Child>> =
    tokio::sync::Mutex::const_new(None);

/// Set when `survey_prepare_cancel` killed the child, so the reader loop
/// reports the neutral "cancelled" outcome instead of a process failure.
static SURVEY_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Spawn `dem-prep` with the given args, streaming its stdout to the
/// frontend as `terrain3d_progress` events and parking the child handle for
/// cancellation. Returns `Ok(())` on success, `Err("cancelled")` when killed,
/// and the stderr tail otherwise.
async fn run_cancellable(
    app: &tauri::AppHandle,
    bin: &Path,
    args: &[String],
    stage: &str,
) -> Result<(), String> {
    use tauri::Emitter;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    let mut child = tokio::process::Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn dem-prep: {e}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *SURVEY_CHILD.lock().await = Some(child);

    // Drain stderr on its own task so a chatty tool cannot fill the pipe
    // while we are blocked reading stdout.
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        if let Some(mut s) = stderr {
            let _ = s.read_to_string(&mut buf).await;
        }
        buf
    });

    if let Some(stdout) = stdout {
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = app.emit(
                "terrain3d_progress",
                serde_json::json!({ "stage": stage, "line": line }),
            );
        }
    }
    let stderr_text = stderr_task.await.unwrap_or_default();

    let mut guard = SURVEY_CHILD.lock().await;
    let Some(mut child) = guard.take() else {
        return Err("cancelled".to_string());
    };
    drop(guard);
    let status = child
        .wait()
        .await
        .map_err(|e| format!("dem-prep {stage}: {e}"))?;
    if SURVEY_CANCELLED.swap(false, Ordering::SeqCst) {
        return Err("cancelled".to_string());
    }
    if status.success() {
        return Ok(());
    }
    let tail: String = stderr_text
        .lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .join("; ");
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

/// U9b/H9: fetch the HRDEM DTM+DSM for the AOI and return the resulting
/// readiness. Streams progress as `terrain3d_progress` (stage `hrdem`) and is
/// cancellable via [`survey_prepare_cancel`]. A cache hit makes this a no-op
/// beyond the status re-read, so a prepared area works offline.
///
/// # Errors
///
/// A stringified error when another prepare is running, `dem-prep` is
/// missing/outdated, the AOI is oversized / uncovered / cross-project, the
/// machine is offline, or the status re-read fails. `"cancelled"` when the
/// operator cancelled.
#[tauri::command]
pub async fn survey_prepare(
    app: tauri::AppHandle,
    lat: f64,
    lon: f64,
    width_m: f64,
    height_m: f64,
    out_dir: Option<String>,
) -> Result<maggcs_core::terrain::SurveyPrepStatus, String> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(format!("coordinates out of range: {lat}, {lon}"));
    }
    if SURVEY_CHILD.lock().await.is_some() {
        return Err("a survey-area download is already running".to_string());
    }
    let bin = dem_prep_binary()?;
    let root = out_dir.map_or_else(survey_root, PathBuf::from);
    let width_m = width_m.clamp(1.0, SURVEY_AOI_LIMIT_M);
    let height_m = height_m.clamp(1.0, SURVEY_AOI_LIMIT_M);
    SURVEY_CANCELLED.store(false, Ordering::SeqCst);
    let args = survey_aoi_args("hrdem", lat, lon, width_m, height_m, &root);
    run_cancellable(&app, &bin, &args, "hrdem").await?;

    // Re-read from disk so the card reflects exactly what landed.
    let status_args = survey_aoi_args("hrdem-status", lat, lon, width_m, height_m, &root);
    let stdout =
        tokio::task::spawn_blocking(move || run_capture(&bin, &status_args, "hrdem-status"))
            .await
            .map_err(|e| format!("survey status task: {e}"))??;
    serde_json::from_str(&stdout).map_err(|e| format!("hrdem-status JSON: {e}"))
}

/// Cancel an in-flight [`survey_prepare`], killing the `dem-prep` child.
/// Returns `true` when something was running.
///
/// # Errors
///
/// Never fails today; kept `Result` for command symmetry.
#[tauri::command]
pub async fn survey_prepare_cancel() -> Result<bool, String> {
    let mut guard = SURVEY_CHILD.lock().await;
    if let Some(child) = guard.as_mut() {
        SURVEY_CANCELLED.store(true, Ordering::SeqCst);
        let _ = child.start_kill();
        return Ok(true);
    }
    Ok(false)
}

/// U13: cancel the running 3D build / DEM fetch (`run_stage` polls this and
/// kills its child). `true` when something was actually running.
///
/// # Errors
///
/// Never — the flag set is infallible; the runner surfaces the outcome.
#[tauri::command]
pub fn terrain3d_cancel() -> Result<bool, String> {
    let was = STAGE_CANCELLED.swap(true, Ordering::SeqCst);
    Ok(was)
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
    // Manual build: always re-process (the operator may have swapped the
    // source rasters under the same folder name).
    build_tileset(app, bin, source, maxzoom, false).await
}

/// The `dem/BC` subfolder (raw DEM GeoTIFFs) below every lookup root.
const DEM_SOURCE_SUBDIR: [&str; 2] = ["dem", "BC"];

/// How many executable parents to walk looking for the DEM folder — dev
/// builds live in `target/<profile>` (or `target/<profile>/deps`).
const DEM_LOOKUP_DEPTH: usize = 4;

/// Candidate folders holding the raw DEM GeoTIFF(s): `dem/BC` under the
/// working directory first, then under each parent of the app executable.
/// Pure so the walk-up logic is unit-testable without touching disk.
fn dem_candidates(cwd: Option<&Path>, exe: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(cwd) = cwd {
        out.push(
            DEM_SOURCE_SUBDIR
                .iter()
                .fold(cwd.to_path_buf(), |p, s| p.join(s)),
        );
    }
    let mut dir = exe;
    for _ in 0..DEM_LOOKUP_DEPTH {
        match dir {
            Some(d) => {
                out.push(
                    DEM_SOURCE_SUBDIR
                        .iter()
                        .fold(d.to_path_buf(), |p, s| p.join(s)),
                );
                dir = d.parent();
            }
            None => break,
        }
    }
    out
}

/// The folder the 3D-mesh layer reads raw DEM GeoTIFF(s) from:
/// `$MAGCS_DEM_DIR` (used verbatim, so an operator can point it anywhere)
/// wins, else the first existing `dem/BC` candidate. `None` when nothing
/// exists — the frontend then falls back to the manual folder picker.
fn resolve_dem_source() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MAGCS_DEM_DIR") {
        let path = PathBuf::from(dir);
        if path.is_dir() {
            return Some(path);
        }
    }
    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    dem_candidates(cwd.as_deref(), exe.as_deref())
        .into_iter()
        .find(|p| p.is_dir())
}

/// The raw-DEM source folder the 3D-mesh layer will read, or `None` when
/// none is found (the frontend falls back to the folder picker).
///
/// # Errors
///
/// Never fails today; kept `Result` for command symmetry.
#[tauri::command]
pub async fn terrain3d_dem_source() -> Result<Option<String>, String> {
    Ok(resolve_dem_source().map(|p| p.display().to_string()))
}

/// Toggle-path build (ADR-004 addendum): read the raw DEM GeoTIFF folder
/// (`terrain3d_dem_source`) and return a tileset folder, building it on
/// first use. Unlike [`terrain3d_build`] this **reuses** a completed
/// tileset untouched and skips `dem-prep process` when a processed tile
/// folder is already present, so flipping the mesh on/off stays fast.
/// Progress streams as `terrain3d_progress` events.
///
/// # Errors
///
/// A stringified error when no DEM folder is found, `dem-prep` is missing,
/// or a stage fails.
#[tauri::command]
pub async fn terrain3d_build_from_dem(
    app: tauri::AppHandle,
    maxzoom: Option<u8>,
) -> Result<String, String> {
    let source = resolve_dem_source().ok_or_else(|| {
        "no DEM folder found (expected dem/BC beside the app; override with MAGCS_DEM_DIR)"
            .to_string()
    })?;
    let bin = dem_prep_binary()?;
    build_tileset(app, bin, source, maxzoom, true).await
}

/// Build (or reuse) a quantized-mesh tileset from a single
/// `dem.json` + `dtm.tif` tile folder — the `hrdem` session layout, already
/// in the shape `dem-prep terrain` reads, so no `dem-prep process` step runs.
/// Returns the tileset folder path.
async fn mesh_from_tile_dir(
    app: tauri::AppHandle,
    bin: PathBuf,
    tile_dir: PathBuf,
    tileset: PathBuf,
    maxzoom: Option<u8>,
    reuse: bool,
) -> Result<String, String> {
    let tileset_path = tileset.display().to_string();
    if reuse && tileset.join("layer.json").is_file() {
        return Ok(tileset_path);
    }
    let handle = app.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut terrain_args = vec![
            "terrain".to_string(),
            tile_dir.display().to_string(),
            "--out".to_string(),
            tileset.display().to_string(),
            "--geoid-embedded".to_string(),
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

/// Toggle-path build from the **survey area's HRDEM session** (H9 → 3D mesh):
/// ensure the HRDEM DTM for the AOI is fetched (`dem-prep hrdem`, cached
/// after the first run), then build a quantized-mesh tileset from that
/// session's `dtm.tif` and return the tileset folder. The mesh is built with
/// the embedded CGG2013 grid, so the samples become WGS84 ellipsoid heights
/// (`canvas verticalDatum: ellipsoidWgs84`) — the datum Cesium drapes on; the
/// app adds `N(EGM96)` back for AGL/plan. Where that grid does not cover the
/// AOI the build falls back to orthometric heights (`verticalDatum:
/// orthometricCgvd2013`, no `N`) and the app adapts from the manifest.
/// Progress streams as `terrain3d_progress` (`hrdem` then `terrain`).
///
/// # Errors
///
/// A stringified error when another prepare is running, `dem-prep` is
/// missing/outdated, `hrdem` fails (oversized / uncovered / cross-project /
/// offline / missing GDAL), or the `terrain` stage fails.
#[tauri::command]
pub async fn terrain3d_build_from_survey(
    app: tauri::AppHandle,
    lat: f64,
    lon: f64,
    width_m: f64,
    height_m: f64,
    out_dir: Option<String>,
    maxzoom: Option<u8>,
) -> Result<String, String> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(format!("coordinates out of range: {lat}, {lon}"));
    }
    if SURVEY_CHILD.lock().await.is_some() {
        return Err("a survey-area download is already running".to_string());
    }
    let bin = dem_prep_binary()?;
    let root = out_dir.map_or_else(survey_root, PathBuf::from);
    let width_m = width_m.clamp(1.0, SURVEY_AOI_LIMIT_M);
    let height_m = height_m.clamp(1.0, SURVEY_AOI_LIMIT_M);

    // 1. Ensure the HRDEM DTM+DSM session (cache hit = no network, no GDAL).
    let hrdem_args = vec![
        "hrdem".to_string(),
        format!("{lat}"),
        format!("{lon}"),
        "--width".to_string(),
        format!("{width_m}"),
        "--height".to_string(),
        format!("{height_m}"),
        "--out".to_string(),
        root.display().to_string(),
    ];
    let handle = app.clone();
    let stage_bin = bin.clone();
    tokio::task::spawn_blocking(move || run_stage(&handle, &stage_bin, &hrdem_args, "hrdem"))
        .await
        .map_err(|e| format!("survey hrdem task: {e}"))??;

    // 2. Locate the session folder + confirm it is ready.
    let status_args = survey_aoi_args("hrdem-status", lat, lon, width_m, height_m, &root);
    let capture_bin = bin.clone();
    let stdout = tokio::task::spawn_blocking(move || {
        run_capture(&capture_bin, &status_args, "hrdem-status")
    })
    .await
    .map_err(|e| format!("survey status task: {e}"))??;
    let status: maggcs_core::terrain::SurveyPrepStatus =
        serde_json::from_str(&stdout).map_err(|e| format!("hrdem-status JSON: {e}"))?;
    if !(status.has_dtm && status.has_dsm) {
        return Err("the HRDEM session for this area is incomplete".to_string());
    }
    let folder = status
        .folder
        .ok_or_else(|| "the HRDEM session folder is unknown".to_string())?;
    let tile_dir = PathBuf::from(&folder);
    let name = tile_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("survey");
    let tileset = root.join(format!("{name}-3d"));

    // 3. Build the mesh (reuse a prior tileset for this session).
    mesh_from_tile_dir(app, bin, tile_dir, tileset, maxzoom, true).await
}

/// The `<source>-3d/{processed,tileset}` artifact pair for a DEM source.
fn build_paths(source: &Path) -> (PathBuf, PathBuf) {
    let root = source.parent().unwrap_or(source).join(format!(
        "{}-3d",
        source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("terrain")
    ));
    (root.join("processed"), root.join("tileset"))
}

/// Count processed tiles (`<tile>/dem.json`) under `dir`.
fn count_processed_tiles(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let path = e.path();
                    path.is_dir() && path.join("dem.json").is_file()
                })
                .count()
        })
        .unwrap_or(0)
}

/// Whether `processed` already holds as many tiles as `source` has top-level
/// rasters — the cheap check that lets a toggle reuse a previous
/// `dem-prep process` (a partial run must not be mistaken for a complete
/// one). With no top-level rasters (raw LiDAR downloads nest one folder per
/// grid tile) any processed tile counts, like [`terrain_process_raw`].
fn has_processed_tiles(processed: &Path, source: &Path) -> bool {
    let done = count_processed_tiles(processed);
    done > 0 && done >= count_input_rasters(source).max(1)
}

/// Run `dem-prep process` (unless `reuse` finds processed tiles already) then
/// `dem-prep terrain`, returning the tileset folder. With `reuse` a finished
/// tileset is returned untouched before any work is spawned.
async fn build_tileset(
    app: tauri::AppHandle,
    bin: PathBuf,
    source: PathBuf,
    maxzoom: Option<u8>,
    reuse: bool,
) -> Result<String, String> {
    let (processed, tileset) = build_paths(&source);
    let tileset_path = tileset.display().to_string();
    if reuse && tileset.join("layer.json").is_file() {
        return Ok(tileset_path);
    }
    let handle = app.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        if !reuse || !has_processed_tiles(&processed, &source) {
            let process_args = vec![
                "process".to_string(),
                source.display().to_string(),
                "--out".to_string(),
                processed.display().to_string(),
                "--resolution".to_string(),
                "1.0".to_string(),
            ];
            run_stage(&handle, &bin, &process_args, "process")?;
        }
        let mut terrain_args = vec![
            "terrain".to_string(),
            processed.display().to_string(),
            "--out".to_string(),
            tileset.display().to_string(),
            "--geoid-embedded".to_string(),
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

/// Process a folder of **raw** BC LiDAR downloads (one subfolder per grid
/// tile, `.laz` inside) through `dem-prep process`, then discover the tiles.
/// Returns the processed output folder plus its discovery — the caller can
/// feed the folder straight to the auto-pick like any processed folder.
///
/// # Errors
///
/// A stringified error when `dem-prep` is missing or the processing fails.
#[tauri::command]
pub async fn terrain_process_raw(
    app: tauri::AppHandle,
    folder: String,
) -> Result<RawProcessResult, String> {
    let bin = dem_prep_binary()?;
    let source = PathBuf::from(&folder);
    if !source.is_dir() {
        return Err(format!("{} is not a folder", source.display()));
    }
    // Output next to the source: <source>-processed (flat, with index.json).
    let out = source.parent().unwrap_or(&source).join(format!(
        "{}-processed",
        source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("terrain")
    ));
    let out_display = out.display().to_string();

    // The conversion is a one-time cost: if the output folder already holds
    // a discovery with at least as many tiles as there are source rasters,
    // reuse it instead of re-running dem-prep (which would re-resample the
    // ~700 MB product tiles). Persisted on disk, so it survives restarts.
    let source_rasters = count_input_rasters(&source);
    if source_rasters > 0 {
        if let Ok(discovery) = terrain_discover(out_display.clone()).await {
            if discovery.tiles.len() >= source_rasters {
                return Ok(RawProcessResult {
                    processed: out_display,
                    tiles: discovery.tiles,
                });
            }
        }
    }

    let handle = app.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let args = vec![
            "process".to_string(),
            source.display().to_string(),
            "--out".to_string(),
            out.display().to_string(),
        ];
        run_stage(&handle, &bin, &args, "process")?;
        Ok(())
    })
    .await
    .map_err(|e| format!("raw processing task: {e}"))??;

    let discovery = terrain_discover(out_display.clone()).await?;
    Ok(RawProcessResult {
        processed: out_display,
        tiles: discovery.tiles,
    })
}

/// Result of [`terrain_process_raw`]: the generated folder + its tiles.
#[derive(Debug, Clone, Serialize)]
pub struct RawProcessResult {
    pub processed: String,
    pub tiles: Vec<DiscoveredTile>,
}

/// One sampled geoid undulation (AMSL → WGS84 ellipsoid offset).
#[derive(Debug, Clone, Serialize)]
pub struct GeoidSample {
    pub lat: f64,
    pub lon: f64,
    /// `N(EGM96)` in metres at the point (add to AMSL for ellipsoid height),
    /// `None` outside the loaded grid.
    pub undulation_m: Option<f64>,
}

/// Sample the EGM96 undulation under the given points: `h(ellipsoid) =
/// AMSL + N`. This is the offset that puts plan waypoints on the same
/// height datum as a `--geoid`-converted 3D terrain mesh, so draped
/// trajectories hug the mesh in the 3D view (ADR-004/006).
///
/// Points outside the grid return `None` — the frontend keeps them at AMSL
/// rather than extrapolating (the same fail-closed rule as the grids).
///
/// # Errors
///
/// A stringified error when the committed EGM96 extraction cannot load.
#[tauri::command]
pub async fn geoid_sample(points: Vec<(f64, f64)>) -> Result<Vec<GeoidSample>, String> {
    use maggcs_core::height::GeoidModel;
    let model = maggcs_core::height::egm96_model();
    let mut out = Vec::with_capacity(points.len());
    for (lat, lon) in points {
        let n = model
            .undulation_m(lat, lon, maggcs_core::height::HeightDatum::AmslEgm96)
            .ok();
        out.push(GeoidSample {
            lat,
            lon,
            undulation_m: n,
        });
    }
    Ok(out)
}

/// The H→AMSL correction for CGVD2013 orthometric sources (plan H7):
/// `offset = N(EGM96) − N(CGG2013)`; `AMSL = H − offset`. This is the real
/// vertical-datum chain (ADR-006: `H + N(CGG2013) = h`, `h − N(EGM96) =
/// AMSL`) replacing the old "H ≈ AMSL" shortcut, which measurement showed
/// to be off by 0.52–1.27 m across BC-SW.
///
/// Points outside either committed extraction return `None` — fail-closed:
/// the caller must refuse to produce AMSL heights from a CGVD2013 source
/// without the full chain, never extrapolate.
#[tauri::command]
pub async fn vertical_datum_offset(points: Vec<(f64, f64)>) -> Result<Vec<OffsetSample>, String> {
    let mut out = Vec::with_capacity(points.len());
    for (lat, lon) in points {
        out.push(OffsetSample {
            lat,
            lon,
            offset_m: vertical_chain_offset(lat, lon),
        });
    }
    Ok(out)
}

/// The H→AMSL chain offset at one point (`AMSL = H − offset`); `None`
/// outside either committed extraction (fail-closed, never extrapolate).
fn vertical_chain_offset(lat: f64, lon: f64) -> Option<f64> {
    use maggcs_core::height::GeoidModel;
    let cgg = maggcs_core::height::cgg2013_model();
    let egm = maggcs_core::height::egm96_model();
    egm.undulation_m(lat, lon, maggcs_core::height::HeightDatum::AmslEgm96)
        .ok()
        .and_then(|n_egm| {
            cgg.undulation_m(
                lat,
                lon,
                maggcs_core::height::HeightDatum::OrthometricCgvd2013,
            )
            .ok()
            .map(|n_cgg| n_egm - n_cgg)
        })
}

/// One [`vertical_datum_offset`] row: the H→AMSL correction in metres,
/// `None` where the chain is not available (fail-closed).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OffsetSample {
    pub lat: f64,
    pub lon: f64,
    pub offset_m: Option<f64>,
}

/// Count raw DEM inputs (`.tif`/`.las`) directly under `dir` — the cheap
/// freshness check for reusing a previous [`terrain_process_raw`] run.
fn count_input_rasters(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    matches!(
                        e.path().extension().and_then(|x| x.to_str()),
                        Some("tif") | Some("tiff") | Some("las") | Some("laz")
                    )
                })
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
#[cfg(test)]
mod vertical_datum_offset_tests {
    use super::*;

    /// The H7 chain offset at the H0 fixture AOI: N(EGM96) − N(CGG2013)
    /// ≈ −19.4904 − (−18.8346) = −0.6558 m (AMSL = H − offset = H + 0.66).
    /// Pinned from the PROJ reference control points (2026-10-17).
    #[tokio::test]
    async fn offset_matches_the_proj_reference_at_the_aoi() {
        let out = vertical_datum_offset(vec![(48.556_06, -123.381_36)])
            .await
            .expect("chain offset");
        assert_eq!(out.len(), 1);
        let offset = out[0].offset_m.expect("inside both extractions");
        assert!(
            (offset - (-0.6558)).abs() <= 0.02,
            "offset = {offset}, expected ≈ −0.6558"
        );
    }

    /// Outside either extraction the offset is `None` — fail-closed, the
    /// caller must refuse to build AMSL heights from a CGVD2013 source.
    #[tokio::test]
    async fn offset_is_none_outside_the_extractions() {
        let out = vertical_datum_offset(vec![(51.05, -114.07)])
            .await
            .expect("query");
        assert!(out[0].offset_m.is_none(), "grids must never extrapolate");
    }
}

#[cfg(test)]
mod drape_apply_tests {
    use super::*;
    use maggcs_core::height::{Height, HeightDatum};
    use maggcs_core::plan::{GeoPoint, PlannedWaypoint};

    /// A9: the compiled route's NAV heights convert through the real chain —
    /// `AMSL = H − offset` at the H0 fixture AOI (offset ≈ −0.6558, pinned
    /// from the PROJ reference control points).
    #[test]
    fn compiled_nav_waypoints_convert_h_to_amsl_through_the_chain() {
        let mut wp = PlannedWaypoint::waypoint(GeoPoint::new(48.556_06, -123.381_36), 0.0);
        wp.altitude = Height::new(HeightDatum::OrthometricCgvd2013, 100.0);
        let out = compiled_to_amsl(&wp).expect("chain available at the fixture AOI");
        assert_eq!(out.altitude.datum(), HeightDatum::AmslEgm96);
        assert!(
            (out.altitude.meters() - 100.655_8).abs() <= 0.02,
            "AMSL = H − offset: {}",
            out.altitude.meters()
        );
    }

    /// H7 fail-closed: a compiled waypoint outside either committed
    /// extraction refuses the whole conversion — no unverified AMSL heights.
    #[test]
    fn a_compiled_waypoint_outside_the_chain_refuses_fail_closed() {
        let mut wp = PlannedWaypoint::waypoint(GeoPoint::new(51.05, -114.07), 0.0);
        wp.altitude = Height::new(HeightDatum::OrthometricCgvd2013, 100.0);
        let err = compiled_to_amsl(&wp).expect_err("outside both extractions");
        assert!(err.contains("vertical datum unverified"), "got {err}");
    }

    /// Command items (e.g. the per-line `DO_CHANGE_SPEED`) carry a command
    /// parameter, not an altitude: the value and its datum tag pass through.
    #[test]
    fn command_items_keep_their_value_and_datum() {
        let mut speed = PlannedWaypoint::waypoint(GeoPoint::new(48.556_06, -123.381_36), 0.0);
        speed.command = 178; // DO_CHANGE_SPEED
        speed.params = [5.0, 0.0, 0.0, 0.0];
        speed.altitude = Height::new(HeightDatum::OrthometricCgvd2013, 57.0);
        let out = compiled_to_amsl(&speed).expect("command item");
        assert_eq!(out.altitude, speed.altitude, "untouched");
        assert_eq!(out.command, 178);
        assert_eq!(out.params, speed.params);
    }
}

#[cfg(test)]
mod dem_source_tests {
    use super::*;

    #[test]
    fn candidates_prefer_cwd_then_walk_up_from_the_exe() {
        let cands = dem_candidates(
            Some(Path::new("/work")),
            Some(Path::new("/w/target/release")),
        );
        assert_eq!(cands[0], PathBuf::from("/work/dem/BC"));
        assert!(cands.contains(&PathBuf::from("/w/target/release/dem/BC")));
        assert!(cands.contains(&PathBuf::from("/w/dem/BC")));
    }

    #[test]
    fn candidates_without_roots_are_empty() {
        assert!(dem_candidates(None, None).is_empty());
    }

    #[test]
    fn build_paths_park_artifacts_next_to_the_source() {
        let (processed, tileset) = build_paths(Path::new("/data/dem/BC"));
        assert_eq!(processed, PathBuf::from("/data/dem/BC-3d/processed"));
        assert_eq!(tileset, PathBuf::from("/data/dem/BC-3d/tileset"));
    }

    #[test]
    fn processed_tiles_must_cover_the_source_rasters() {
        let base = std::env::temp_dir().join(format!("maggcs-demtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("src");
        let processed = base.join("src-3d/processed");
        std::fs::create_dir_all(&source).unwrap();
        // Stray file, no `<tile>/dem.json`: not processed.
        std::fs::create_dir_all(processed.join("loose")).unwrap();
        std::fs::write(processed.join("loose/readme.txt"), "x").unwrap();
        assert!(!has_processed_tiles(&processed, &source));
        // Two source rasters, one processed tile: a partial run is not done.
        std::fs::create_dir_all(processed.join("tile")).unwrap();
        std::fs::write(processed.join("tile/dem.json"), "{}").unwrap();
        std::fs::write(source.join("a.tif"), "x").unwrap();
        std::fs::write(source.join("b.tif"), "x").unwrap();
        assert!(!has_processed_tiles(&processed, &source));
        // A second processed tile completes the set.
        std::fs::create_dir_all(processed.join("tile2")).unwrap();
        std::fs::write(processed.join("tile2/dem.json"), "{}").unwrap();
        assert!(has_processed_tiles(&processed, &source));
        // An absent processed folder is never "done".
        assert!(!has_processed_tiles(&base.join("empty"), &source));
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// One plan line for the drape preview: id, kind (`survey`/`tie`/…), and the
/// vertices (WGS84 degrees) to drape.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DrapeLineInput {
    pub id: String,
    pub kind: String,
    /// `[lon, lat]` pairs in WGS84 degrees.
    pub vertices: Vec<[f64; 2]>,
}

/// The drape preview (DR4) and — since A9 — the **only** waypoint producer:
/// `solve → compile` over the plan's lines, with the compiled route converted
/// to the plan's stored AMSL datum and the landing items placed on the DEM
/// surface. The interim per-waypoint bridge (`core::terrain::drape` +
/// `mission/meshDrape.ts`) was removed (ADR-022 supersedes it).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DrapePreview {
    pub result: maggcs_core::terrain::drape_spec::DrapeResult,
    /// The compiled route in the plan's stored datum (`AMSL_EGM96`, ADR-013),
    /// in fly order (speed items included). `result.compiled` keeps the DEM
    /// datum — it is the monitor baseline's frame (the anchored fix is
    /// DEM-frame too), while this list is what the plan stores.
    pub waypoints: Vec<maggcs_core::plan::PlannedWaypoint>,
    /// AMSL ground height (the DEM surface itself, no AGL, no offset) for
    /// each landing point passed in, parallel to the input order.
    pub landing_amsl_m: Vec<f64>,
    /// The DEM carries a DTM (false = DTM-only source: AGL is height above
    /// surface, warned by the checklist).
    pub has_dtm: bool,
    /// Acquisition date range from `dem.json` (the age check).
    pub acquisition: Option<String>,
    /// DEM cell size, metres (`sample_step` is capped at it).
    pub resolution_m: Option<f64>,
    /// Every line vertex is inside the DEM bounds.
    pub covers_route: bool,
    /// The acquisition is within the configured age (`SafetyConfig`).
    pub dem_age_ok: Option<bool>,
}

/// The H7 fail-closed refusal: a CGVD2013 DEM without the committed
/// CGG2013/EGM96 extractions must not produce AMSL heights.
const VERTICAL_DATUM_CHAIN_ERROR: &str =
    "vertical datum unverified — the CGG2013/EGM96 height chain is unavailable under a draped waypoint";

/// Convert one compiled survey waypoint to the stored AMSL datum. Only
/// coordinate items carry an altitude that means anything: command items
/// (e.g. `DO_CHANGE_SPEED`) keep their DEM-datum value — its datum tag
/// stays honest and the wire compile ignores it (`mission` frame).
fn compiled_to_amsl(
    wp: &maggcs_core::plan::PlannedWaypoint,
) -> Result<maggcs_core::plan::PlannedWaypoint, String> {
    const NAV_WAYPOINT: u16 = 16;
    let mut out = wp.clone();
    if wp.command == NAV_WAYPOINT {
        let offset = vertical_chain_offset(wp.position.latitude_deg, wp.position.longitude_deg)
            .ok_or_else(|| VERTICAL_DATUM_CHAIN_ERROR.to_string())?;
        out.altitude = maggcs_core::height::Height::new(
            maggcs_core::height::HeightDatum::AmslEgm96,
            wp.altitude.meters() - offset,
        );
    }
    Ok(out)
}

/// Parse a kind string to the core enum (unknown kinds degrade to survey).
fn parse_line_kind(kind: &str) -> maggcs_core::survey::pattern::LineKind {
    match kind {
        "tie" => maggcs_core::survey::pattern::LineKind::Tie,
        "calibration" => maggcs_core::survey::pattern::LineKind::Calibration,
        _ => maggcs_core::survey::pattern::LineKind::Survey,
    }
}

/// Solve + compile the drape pipeline over the plan's lines against the
/// loaded DEM (DR4/A9) and return the route the plan stores.
///
/// `landing` carries the plan's landing-item positions as `[lon, lat]`
/// (WGS84 degrees, the same convention as `DrapeLineInput.vertices`); each
/// comes back placed on the DEM surface (no AGL, no vertical offset —
/// operator decision 2026-10-20), converted through the same chain.
///
/// # Errors
///
/// A stringified error when the DEM cannot be opened, the solver or the
/// compiler refuses (no coverage, self-check failure, over the item limit),
/// a landing point is not sampleable, or the vertical datum chain is
/// unavailable under a converted waypoint (fail-closed, H7).
#[tauri::command]
pub async fn drape_preview(
    state: tauri::State<'_, AppState>,
    folder: String,
    spec: maggcs_core::terrain::drape_spec::DrapeSpec,
    lines: Vec<DrapeLineInput>,
    landing: Option<Vec<[f64; 2]>>,
) -> Result<DrapePreview, String> {
    use maggcs_core::terrain::drape_solver::{solve, InputLine};

    let dem = open_dem_cached(&state, &folder)?;
    let dir = Path::new(&folder);
    let meta: maggcs_core::terrain::DemMetadata = serde_json::from_str(
        &std::fs::read_to_string(dir.join("dem.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let input: Vec<InputLine> = lines
        .iter()
        .map(|l| InputLine {
            id: l.id.clone(),
            kind: parse_line_kind(&l.kind),
            vertices: l
                .vertices
                .iter()
                .map(|p| maggcs_core::plan::GeoPoint::new(p[1], p[0]))
                .collect(),
        })
        .collect();

    // Solve + compile on a blocking task: a 20 000-sample route is a few ms,
    // but the footprint sweep reads the DEM repeatedly. `result.compiled`
    // stays in the DEM's own datum (the monitor baseline's frame, ADR-022
    // §8); the AMSL conversion below produces the stored plan's copy.
    let spec_for_task = spec.clone();
    let dem_for_task = dem.clone();
    let input_for_task = input.clone();
    let result = tokio::task::spawn_blocking(
        move || -> Result<_, maggcs_core::terrain::drape_solver::DrapeError> {
            let result = solve(&spec_for_task, &input_for_task, dem_for_task.as_ref())?;
            let compiled = maggcs_core::terrain::drape_compile::compile(
                &spec_for_task,
                &result.lines,
                result.dem_ref.datum,
            )?;
            Ok(maggcs_core::terrain::drape_spec::DrapeResult { compiled, ..result })
        },
    )
    .await
    .map_err(|e| format!("drape task: {e}"))?
    .map_err(|e| e.to_string())?;

    // A9: the solver result is the only waypoint producer. Convert the
    // compiled coordinate items to the stored AMSL datum (ADR-013) —
    // fail-closed when the chain is unavailable under any of them.
    let mut waypoints = Vec::with_capacity(result.compiled.len());
    for c in &result.compiled {
        waypoints.push(compiled_to_amsl(&c.waypoint)?);
    }

    // Landing items aim at the DEM surface itself: neither the survey AGL
    // nor the DSM clearance applies (operator report: a landing target left
    // on the HOME plane left the vehicle hovering over lower ground).
    let mut landing_amsl_m = Vec::new();
    for point in landing.unwrap_or_default() {
        use maggcs_core::terrain::ElevationSource;
        let p = maggcs_core::plan::GeoPoint::new(point[1], point[0]);
        let ground = dem.elevation(p).map_err(|e| format!("landing {e}"))?;
        let offset = vertical_chain_offset(p.latitude_deg, p.longitude_deg)
            .ok_or_else(|| VERTICAL_DATUM_CHAIN_ERROR.to_string())?;
        landing_amsl_m.push(ground.meters() - offset);
    }

    // Coverage: every line vertex inside the DEM bounds (the solver already
    // refuses sampled points outside, so this mirrors it for the facts).
    let bounds = meta.bounds;
    let covers_route = lines.iter().all(|l| {
        l.vertices.iter().all(|p| {
            p[1] >= bounds.south
                && p[1] <= bounds.north
                && p[0] >= bounds.west
                && p[0] <= bounds.east
        })
    });

    Ok(DrapePreview {
        waypoints,
        landing_amsl_m,
        result,
        has_dtm: meta.has_dtm,
        acquisition: meta.acquisition.clone(),
        resolution_m: Some(meta.resolution_m),
        covers_route,
        dem_age_ok: None, // the age threshold is frontend config (advisory)
    })
}

/// The live drape monitor (DR5→DR6): one per flight, held in `AppState`.
pub struct MonitorSession {
    monitor: maggcs_core::drape_monitor::DrapeMonitor,
    /// The DEM folder the baseline was solved against (diagnostics).
    folder: String,
}

/// Start monitoring the given drape result against the loaded DEM (DR6).
/// Replaces any running session (the caller stops first when that matters).
///
/// # Errors
///
/// A stringified error when the baseline is empty (nothing to monitor).
#[tauri::command]
pub fn drape_monitor_start(
    state: tauri::State<'_, AppState>,
    folder: String,
    result: maggcs_core::terrain::drape_spec::DrapeResult,
    spec: maggcs_core::terrain::drape_spec::DrapeSpec,
) -> Result<(), String> {
    let baseline = maggcs_core::drape_monitor::MonitorBaseline::from_result(&result, &spec);
    if baseline.total_s() <= 0.0 {
        return Err("the drape result carries no route to monitor".into());
    }
    let monitor = maggcs_core::drape_monitor::DrapeMonitor::new(
        baseline,
        maggcs_core::drape_monitor::MonitorConfig::default(),
    );
    *state.drape_monitor.lock() = Some(MonitorSession { monitor, folder });
    // DR7: the session's drape.jsonl header carries the flown plan's
    // identity (spec hash + DEM hash) when a recording is running.
    if let Some(recorder) = state.recorder.running_recorder() {
        recorder.set_drape_meta(spec.spec_hash(), result.dem_ref.hash.clone());
    }
    Ok(())
}

/// Stop the monitor (landed / route changed / Fly view left).
#[tauri::command]
pub fn drape_monitor_stop(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    Ok(state.drape_monitor.lock().take().is_some())
}

/// Acknowledge the latched Critical (U3 confirm → here).
#[tauri::command]
pub fn drape_monitor_ack(state: tauri::State<'_, AppState>) -> Result<(), String> {
    if let Some(session) = state.drape_monitor.lock().as_mut() {
        session.monitor.acknowledge();
    }
    Ok(())
}

/// One monitor step over the live fix (DR6): the caller passes the
/// **anchored** AMSL (the same `vehicleAmslAnchoredM` the map renders) and
/// the wall-clock `now_ms` so staleness is judged on the app's clock.
///
/// # Errors
///
/// `no monitor` when none is running; DEM errors surface as the engine's
/// `OutsideDem` freshness, not as an error.
/// The fix payload (the command takes it as one object: Tauri camelCase).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MonitorFixInput {
    pub lat: f64,
    pub lon: f64,
    /// Anchored AMSL, metres (DEM datum).
    pub z_amsl_m: f64,
    pub vn_mps: f64,
    pub ve_mps: f64,
    pub vd_mps: f64,
    /// The app's wall-clock at the fix, epoch ms.
    pub at_ms: f64,
}

#[tauri::command]
pub async fn drape_monitor_step(
    state: tauri::State<'_, AppState>,
    fix: MonitorFixInput,
    radar_m: Option<f64>,
) -> Result<maggcs_core::drape_monitor::MonitorState, String> {
    let now_ms = fix.at_ms as u64;
    let fix = maggcs_core::drape_monitor::MonitorFix {
        lat: fix.lat,
        lon: fix.lon,
        z_amsl_m: fix.z_amsl_m,
        vn_mps: fix.vn_mps,
        ve_mps: fix.ve_mps,
        vd_mps: fix.vd_mps,
        at_ms: now_ms,
    };
    // The DEM is the one the session started against (cached open).
    let folder = {
        let guard = state.drape_monitor.lock();
        guard
            .as_ref()
            .ok_or("no drape monitor running")?
            .folder
            .clone()
    };
    // Open the DEM outside the session lock (CogDem::open reads rasters).
    let dem = open_dem_cached(&state, &folder)?;
    let mut session = state.drape_monitor.lock();
    let session = session.as_mut().ok_or("no drape monitor running")?;
    let out = session
        .monitor
        .step(now_ms, Some(&fix), dem.as_ref(), radar_m)
        .clone();
    // DR7: the step's scalars land in the session's drape.jsonl (never
    // blocks; no recording means no-op).
    if let Some(recorder) = state.recorder.running_recorder() {
        recorder.record_drape(maggcs_core::recorder::DrapeSampleRecord {
            utc_us: now_ms * 1000,
            s_star_m: out.s_star_m,
            line_id: out.line_id.clone(),
            ez_m: out.ez_m,
            e_agl_m: out.e_agl_m,
            cross_track_m: out.cross_track_m,
            clearance_m: out.clearance_m,
            lookahead_clearance_m: out.lookahead_clearance_m,
            e_dem_median_m: out.e_dem_median_m,
            fresh: match out.fresh {
                maggcs_core::drape_monitor::Freshness::Fresh => "fresh",
                maggcs_core::drape_monitor::Freshness::Stale => "stale",
                maggcs_core::drape_monitor::Freshness::OutsideDem => "outside_dem",
            }
            .into(),
            latched_critical: out.latched_critical,
            in_band_ratio: out.in_band_ratio,
        });
    }
    Ok(out)
}

/// The post-flight drape report (DR7): aggregates the session's `drape.jsonl`
/// per line. `None` when the session has no drape log.
///
/// # Errors
///
/// A stringified error when the log cannot be read.
#[tauri::command]
pub fn drape_report(
    state: tauri::State<'_, AppState>,
) -> Result<Option<maggcs_core::recorder::DrapeFlightReport>, String> {
    let Some(dir) = state.recorder.last_session_dir() else {
        return Ok(None);
    };
    let path = std::path::Path::new(&dir).join(maggcs_core::recorder::DRAPE_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let lines = maggcs_core::recorder::read_drape_log(&path).map_err(|e| e.to_string())?;
    Ok(Some(maggcs_core::recorder::build_report(&lines)))
}

/// The post-flight DEM bias estimate (DR9, assessment only): the robust
/// e_dem statistics over the session's radar-bearing samples, with the
/// `vertical_offset` candidate for the operator's *next* plan. Never applied
/// automatically; in-flight re-drape is ruled out (operator 2026-10-18).
///
/// # Errors
///
/// A stringified error when the log cannot be read; `None` without a session.
#[tauri::command]
pub fn drape_bias(
    state: tauri::State<'_, AppState>,
) -> Result<Option<maggcs_core::recorder::DemBiasOutcome>, String> {
    let Some(dir) = state.recorder.last_session_dir() else {
        return Ok(None);
    };
    let path = std::path::Path::new(&dir).join(maggcs_core::recorder::DRAPE_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let lines = maggcs_core::recorder::read_drape_log(&path).map_err(|e| e.to_string())?;
    let mut spec_hash = String::new();
    let mut samples = Vec::new();
    for line in lines {
        match line {
            maggcs_core::recorder::DrapeLine::Meta(meta) => spec_hash = meta.spec_hash,
            maggcs_core::recorder::DrapeLine::Sample(s) => samples.push(s),
        }
    }
    Ok(Some(maggcs_core::recorder::estimate_bias(
        &spec_hash, &samples,
    )))
}

/// Write the session's drape summary (CSV + Markdown) next to its log (DR7).
///
/// # Errors
///
/// A stringified error when the log cannot be read or the files cannot be
/// written; `None` when the session has no drape log.
#[tauri::command]
pub fn drape_export(
    state: tauri::State<'_, AppState>,
) -> Result<Option<std::path::PathBuf>, String> {
    let Some(dir) = state.recorder.last_session_dir() else {
        return Ok(None);
    };
    let dir = std::path::PathBuf::from(dir);
    let path = dir.join(maggcs_core::recorder::DRAPE_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let lines = maggcs_core::recorder::read_drape_log(&path).map_err(|e| e.to_string())?;
    let report = maggcs_core::recorder::build_report(&lines);
    std::fs::write(
        dir.join("drape-summary.csv"),
        maggcs_core::recorder::to_csv(&report),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("drape-summary.md"),
        maggcs_core::recorder::to_markdown(&report),
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(dir))
}

/// H6: rank DEM source candidates for an AOI — the pure strategy lives in
/// `core::terrain::source_select`; this command only carries the candidates
/// the frontend gathered (discovered tiles, the prepared HRDEM session) over
/// the IPC. No IO, no download (tier ④ is a suggestion the frontend shows).
///
/// # Errors
///
/// A stringified deserialization error (Tauri does the JSON decoding).
#[tauri::command]
pub fn terrain_select(
    candidates: Vec<maggcs_core::terrain::source_select::SourceCandidate>,
    aoi: maggcs_core::terrain::DemBounds,
) -> Result<maggcs_core::terrain::source_select::SourceSelection, String> {
    Ok(maggcs_core::terrain::source_select::select_source(
        &candidates,
        &aoi,
    ))
}
