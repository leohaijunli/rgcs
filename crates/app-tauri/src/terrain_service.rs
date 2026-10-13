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
