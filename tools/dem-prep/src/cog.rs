//! COG GeoTIFF writing + the `dem.json` sidecar.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::DemError;
use crate::grid::GeoRaster;

/// GeoTIFF nodata sentinel the runtime reader recognises.
pub const NODATA: f32 = -9999.0;

/// Write one geographic single-band float raster as a tiled, deflate-
/// compressed GeoTIFF with the placement tags the runtime reader requires.
///
/// # Errors
///
/// [`DemError::Raster`] on an encoder failure.
pub fn write_geo_tiff(path: &Path, raster: &GeoRaster) -> Result<(), DemError> {
    // Placement lives in dem.json (single source of truth); the TIFF itself
    // carries no geo tags (the tiff crate cannot tag the write_image IFD).
    let file = File::create(path).map_err(|e| DemError::Raster(e.to_string()))?;
    let mut encoder = tiff::encoder::TiffEncoder::new(BufWriter::new(file))
        .map_err(|e| DemError::Raster(e.to_string()))?
        .with_compression(tiff::encoder::Compression::Deflate(
            tiff::encoder::DeflateLevel::Balanced,
        ));

    // Replace NaN with the nodata sentinel: TIFF has no NaN notion and the
    // reader treats <= -9999 as void.
    let data: Vec<f32> = raster
        .values
        .iter()
        .map(|v| if v.is_nan() { NODATA } else { *v })
        .collect();

    // Plain single-IFD GeoTIFF: the placement (bounds + resolution) lives in
    // dem.json — the single source of truth the runtime reader consumes. The
    // tiff crate cannot attach custom tags to the IFD write_image builds, so
    // no ModelTiepoint/ModelPixelScale tags are emitted (the layers are
    // produced for MagGCS, which reads them through its own reader).
    encoder
        .write_image::<tiff::encoder::colortype::Gray32Float>(
            u32::try_from(raster.width).map_err(|e| DemError::Raster(e.to_string()))?,
            u32::try_from(raster.height).map_err(|e| DemError::Raster(e.to_string()))?,
            &data,
        )
        .map_err(|e| DemError::Raster(e.to_string()))?;
    Ok(())
}

/// The `dem.json` sidecar (consumed by `core::terrain::CogDem`).
///
/// The `source_file` / `source_points` fields are LAS-process metadata: HRDEM
/// sessions write their own `dem.json` without them, so they are optional here
/// — `dem-prep terrain` must mesh either producer's output.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DemMetadata {
    pub version: String,
    pub raster_crs: String,
    pub resolution_m: f64,
    pub bounds: DemBounds,
    pub vertical_datum: String,
    #[serde(default)]
    pub source_file: String,
    #[serde(default)]
    pub source_points: u64,
    /// False for DTM-only fallback sources (CDEM / Copernicus).
    #[serde(default)]
    pub has_dsm: bool,
    /// What produced the raster: `"lidar"` (BC LiDAR LAS) or `"geotiff"`
    /// (CDEM / Copernicus fallback) — the app prefers LiDAR for the drape.
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct DemBounds {
    pub north: f64,
    pub south: f64,
    pub east: f64,
    pub west: f64,
}

/// sha256(content)[..16] — the DEM version ADR-004 requires for archives.
pub fn version_of(contents: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for c in contents {
        hasher.update(c);
    }
    let digest = hasher.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// One processed tile, for the folder-level `index.json` (N1).
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexEntry {
    /// Tile output folder name (relative to the index).
    pub folder: String,
    pub bounds: DemBounds,
    pub version: String,
    pub has_dsm: bool,
    /// `"lidar"` or `"geotiff"` — drives the app's tile auto-pick priority.
    pub source: String,
}

/// Write `index.json`.
///
/// # Errors
///
/// [`DemError::Raster`] on a write failure.
pub fn write_index(path: &Path, tiles: &[IndexEntry]) -> Result<(), DemError> {
    let json = serde_json::to_string_pretty(tiles).map_err(|e| DemError::Raster(e.to_string()))?;
    std::fs::write(path, json).map_err(|e| DemError::Raster(e.to_string()))
}

/// Write `dem.json`.
///
/// # Errors
///
/// [`DemError::Raster`] on a write failure.
pub fn write_metadata(path: &Path, meta: &DemMetadata) -> Result<(), DemError> {
    let json = serde_json::to_string_pretty(meta).map_err(|e| DemError::Raster(e.to_string()))?;
    std::fs::write(path, json).map_err(|e| DemError::Raster(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `hrdem` session `dem.json` (e.g. the survey-area DEM) has no
    /// `source_file`/`source_points` — those are LAS-process metadata — but
    /// `dem-prep terrain` must still mesh it.
    #[test]
    fn hrdem_dem_json_without_source_fields_parses() {
        let json = r#"{
            "version": "ec830d8b208d4ad2",
            "raster_crs": "EPSG:4326",
            "resolution_m": 1.0,
            "bounds": {
                "north": 48.55653346531145,
                "south": 48.554850796100666,
                "east": -123.37784481853657,
                "west": -123.38038703054745
            },
            "vertical_datum": "orthometric_cgvd2013",
            "has_dsm": true,
            "has_dtm": true,
            "source": "hrdem"
        }"#;
        let meta: DemMetadata = serde_json::from_str(json).expect("hrdem dem.json parses");
        assert_eq!(meta.source, "hrdem");
        assert!(meta.source_file.is_empty());
        assert_eq!(meta.source_points, 0);
        assert!(meta.has_dsm);
    }
}
