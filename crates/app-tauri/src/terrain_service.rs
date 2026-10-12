//! Terrain overlay bridge (plan A): read a dem-prep tile folder
//! (hillshade.png + dem.json) and hand the frontend a data URL plus the
//! geographic placement for the Cesium imagery layer.

use std::path::Path;

use serde::Serialize;

use maggcs_core::terrain::{DemBounds, DemMetadata, VerticalDatum};

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
