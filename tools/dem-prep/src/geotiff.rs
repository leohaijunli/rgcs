//! Single-band geographic GeoTIFF reading for the fallback sources (CDEM /
//! Copernicus GLO-30): values + the placement the pipeline passes through.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use tiff::decoder::Decoder;
use tiff::tags::Tag;

use crate::error::DemError;

/// Read one single-band float GeoTIFF with GeoTIFF placement tags.
///
/// # Errors
///
/// [`DemError::Raster`] on any decode/placement problem.
#[allow(clippy::type_complexity)]
pub fn read_single_band(
    path: &Path,
) -> Result<(Vec<f64>, usize, usize, f64, f64, f64, f64), DemError> {
    let file = File::open(path).map_err(|e| DemError::Raster(e.to_string()))?;
    let mut decoder =
        Decoder::new(BufReader::new(file)).map_err(|e| DemError::Raster(e.to_string()))?;
    let (width, height) = decoder
        .dimensions()
        .map_err(|e| DemError::Raster(e.to_string()))?;
    let (width, height) = (width as usize, height as usize);

    let scale = decoder
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .map_err(|e| DemError::Raster(format!("no ModelPixelScale: {e}")))?;
    let tie = decoder
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .map_err(|e| DemError::Raster(format!("no ModelTiepoint: {e}")))?;
    if scale.len() < 2 || scale[0] <= 0.0 || scale[1] <= 0.0 || tie.len() < 6 {
        return Err(DemError::Raster(format!(
            "{}: missing/invalid GeoTIFF placement tags",
            path.display()
        )));
    }
    let (west, north) = (tie[3], tie[4]);
    // Geographic degrees only — projected inputs must be regridded first
    // (gdalwarp/QGIS), keeping this tool's dependency set tiny.
    if !(west >= -180.0 && west <= 180.0 && north.abs() <= 90.0) {
        return Err(DemError::Raster(format!(
            "{}: raster is not geographic (degrees) — regrid with gdalwarp/QGIS first",
            path.display()
        )));
    }

    let values: Vec<f64> = match decoder
        .read_image()
        .map_err(|e| DemError::Raster(e.to_string()))?
    {
        tiff::decoder::DecodingResult::F32(v) => v.into_iter().map(f64::from).collect(),
        tiff::decoder::DecodingResult::F64(v) => v,
        tiff::decoder::DecodingResult::U16(v) => v.into_iter().map(f64::from).collect(),
        tiff::decoder::DecodingResult::U32(v) => v.into_iter().map(f64::from).collect(),
        tiff::decoder::DecodingResult::U8(v) => v.into_iter().map(f64::from).collect(),
        other => {
            return Err(DemError::Raster(format!(
                "unsupported sample type: {other:?}"
            )))
        }
    };
    if values.len() != width * height {
        return Err(DemError::Raster(format!(
            "expected {} samples, got {}",
            width * height,
            values.len()
        )));
    }
    Ok((values, width, height, west, north, scale[1], scale[0]))
}
