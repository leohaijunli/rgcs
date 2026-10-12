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
