//! Terrain elevation sources (ADR-004, comprehensive plan D1): a local COG
//! GeoTIFF DEM is the default and the only required source — field sites
//! often have no internet, and survey-grade planning needs deterministic,
//! versioned terrain data (Cesium ion streaming is explicitly out of scope
//! here).
//!
//! The surface follows ADR-004's `ElevationSource` sketch; sampling never
//! extrapolates: a point outside the raster — or inside a void — is an
//! error (the same principle as the geoid grid in `core::height::grid`).
//!
//! Both rasters of a site are opened together: the DTM (bare earth, the AGL
//! datum for S3 draping) and the DSM (surface incl. vegetation, the min-
//! clearance check of AGENTS.md / ADR-005).

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::height::Height;
use crate::plan::GeoPoint;

pub mod cog;
pub mod drape;

pub use cog::CogDem;

/// The `dem.json` sidecar `tools/dem-prep` writes next to the rasters. It is
/// the version chain ADR-004 requires (mission archives reference the DEM
/// they were planned against) plus the placement the reader needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct DemMetadata {
    /// Content hash (sha256, first 16 hex chars) of the raster pair.
    pub version: String,
    /// CRS the rasters are gridded in (e.g. `"EPSG:26910"`).
    pub raster_crs: String,
    /// Nominal grid spacing, metres.
    pub resolution_m: f64,
    /// WGS84 coverage of the raster pair.
    pub bounds: DemBounds,
    /// Vertical datum of the pixel values (BC LiDAR: CGVD2013 orthometric).
    pub vertical_datum: VerticalDatum,
    /// Whether the pair carries a real DSM. Fallback sources (CDEM /
    /// Copernicus) are DTM-only: the drape then skips the clearance check.
    #[serde(default)]
    pub has_dsm: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct DemBounds {
    pub north: f64,
    pub south: f64,
    pub east: f64,
    pub west: f64,
}

/// Vertical datums `tools/dem-prep` emits. The BC LiDAR product is
/// orthometric CGVD2013 (`H`); the runtime converts to the datum a caller
/// needs through `core::height` (the `H + N(CGG2013) = h` chain, ADR-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum VerticalDatum {
    OrthometricCgvd2013,
    AmslEgm96,
}

/// Errors from terrain sources (ADR-004: no extrapolation, no void fill).
#[derive(Debug, thiserror::Error)]
pub enum TerrainError {
    #[error("point outside the DEM coverage")]
    OutsideCoverage(GeoPoint),
    #[error("DEM void")]
    Void(GeoPoint),
    #[error("DEM metadata mismatch: {0}")]
    Metadata(String),
    #[error("DEM file error: {0}")]
    Io(String),
    #[error("GeoTIFF error: {0}")]
    Tiff(String),
}

/// ADR-004's elevation surface. Implemented by [`CogDem`]; a tile cache /
/// network source would implement the same trait later.
pub trait ElevationSource: Send + Sync {
    /// Elevation at the point, in the raster's own vertical datum (tagged).
    ///
    /// # Errors
    ///
    /// [`TerrainError::OutsideCoverage`] / [`TerrainError::Void`] rather
    /// than extrapolating or silently filling.
    fn elevation(&self, point: GeoPoint) -> Result<Height, TerrainError>;

    /// WGS84 coverage of the source.
    fn bounds(&self) -> DemBounds;

    /// The DEM version (the `dem.json` content hash) for mission archives.
    fn version(&self) -> &str;

    /// DTM and DSM at one point (both datum-tagged). The pair is what
    /// draping needs: the DTM sets the AGL datum, the DSM the clearance.
    ///
    /// # Errors
    ///
    /// Same contract as [`ElevationSource::elevation`].
    fn sample_pair(&self, point: GeoPoint) -> Result<(Height, Height), TerrainError>;
}

/// Validate a metadata/raster pair before sampling anything.
pub(crate) fn check_paths(dtm: &Path, dsm: &Path) -> Result<(), TerrainError> {
    for path in [dtm, dsm] {
        if !path.is_file() {
            return Err(TerrainError::Io(format!(
                "missing DEM raster {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// One tile in the folder-level `index.json` `tools/dem-prep` writes (N1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct TileIndexEntry {
    /// Tile output folder name, relative to the index folder.
    pub folder: String,
    pub bounds: DemBounds,
    /// `dem.json` version (sha256[..16]).
    pub version: String,
    /// Whether the tile pair carries a real DSM (fallback sources: DTM-only).
    #[serde(default)]
    pub has_dsm: bool,
}

/// Read the folder-level `index.json` a multi-tile dem-prep run writes (N1).
///
/// # Errors
///
/// [`TerrainError::Io`] when the file is missing, [`TerrainError::Metadata`]
/// when it is not a valid tile list.
pub fn load_tile_index(dir: &Path) -> Result<Vec<TileIndexEntry>, TerrainError> {
    let path = dir.join("index.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| TerrainError::Io(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| TerrainError::Metadata(format!("{}: {e}", path.display())))
}

/// Pick the tile whose bounds contain the centre of the query area (N1
/// auto-select). Returns `None` when the area is outside every tile — the
/// caller then falls back (CDEM/Copernicus) and surfaces a hint.
pub fn pick_tile<'a>(
    tiles: &'a [TileIndexEntry],
    query: &DemBounds,
) -> Option<&'a TileIndexEntry> {
    let cx = (query.north + query.south) / 2.0;
    let cy = (query.east + query.west) / 2.0;
    tiles
        .iter()
        .find(|t| t.bounds.north >= cx && cx >= t.bounds.south && t.bounds.east >= cy && cy >= t.bounds.west)
}

#[cfg(test)]
mod tile_index_tests {
    use super::*;

    fn tile(folder: &str, n: f64, s: f64, e: f64, w: f64) -> TileIndexEntry {
        TileIndexEntry {
            folder: folder.to_string(),
            bounds: DemBounds { north: n, south: s, east: e, west: w },
            version: "v1".to_string(),
            has_dsm: true,
        }
    }

    #[test]
    fn picks_covering_tile() {
        let tiles = vec![
            tile("a", 48.7, 48.6, -123.3, -123.5),
            tile("b", 48.6, 48.5, -123.3, -123.5),
        ];
        let q = DemBounds { north: 48.55, south: 48.52, east: -123.4, west: -123.45 };
        assert_eq!(pick_tile(&tiles, &q).unwrap().folder, "b");
    }

    #[test]
    fn outside_all_tiles_is_none() {
        let tiles = vec![tile("a", 48.7, 48.6, -123.3, -123.5)];
        let q = DemBounds { north: 50.1, south: 50.0, east: -120.0, west: -120.1 };
        assert!(pick_tile(&tiles, &q).is_none());
    }

    #[test]
    fn empty_index_is_none() {
        let q = DemBounds { north: 48.6, south: 48.5, east: -123.4, west: -123.5 };
        assert!(pick_tile(&[], &q).is_none());
    }

    #[test]
    fn loads_written_index() {
        let dir = std::env::temp_dir().join(format!("maggcs-n1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tiles = vec![tile("sidney", 48.7, 48.6, -123.2, -123.5)];
        let path = dir.join("index.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&tiles).unwrap(),
        )
        .unwrap();
        let loaded = load_tile_index(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].folder, "sidney");
        assert_eq!(loaded[0].version, "v1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_index_is_io_error() {
        let dir = std::env::temp_dir().join("maggcs-n1-missing");
        let err = load_tile_index(&dir).unwrap_err();
        assert!(matches!(err, TerrainError::Io(_)));
    }
}
