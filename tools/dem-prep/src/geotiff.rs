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
    let mut decoder = Decoder::new(BufReader::new(file))
        .map_err(|e| DemError::Raster(e.to_string()))?
        .with_limits({
            // BC LiDAR product tiles reach ~15000x11000 f32 cells (~700 MB);
            // the crate default (256 MiB) rejects those as "limits exceeded".
            // Local, trusted files — raise the buffers, keep everything else
            // at the crate defaults (Limits is #[non_exhaustive]).
            let mut l = tiff::decoder::Limits::default();
            l.decoding_buffer_size = 2 * 1024 * 1024 * 1024;
            l.intermediate_buffer_size = 256 * 1024 * 1024;
            l.ifd_value_size = 16 * 1024 * 1024;
            l
        });
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
    if !(-180.0..=180.0).contains(&west) || north.abs() > 90.0 {
        return Err(DemError::Raster(format!(
            "{}: raster is not geographic (degrees) — regrid with gdalwarp/QGIS first",
            path.display()
        )));
    }

    // Optional nodata sentinel: GDAL writes it as an ASCII tag (e.g.
    // "-32768" — Copernicus DEM ocean pixels); some writers store a
    // numeric tag insteadaint none of ours does.
    let nodata = decoder
        .get_tag_ascii_string(Tag::GdalNodata)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .or_else(|| {
            decoder
                .get_tag_f64_vec(Tag::GdalNodata)
                .ok()
                .and_then(|v| v.first().copied())
        });

    let mut values: Vec<f64> = match decoder
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
    // Replace the nodata sentinel with NaN so voids flow through the
    // pipeline the same way as ours (write NaN → -9999 → read NaN → fill).
    if let Some(n) = nodata {
        values = apply_nodata(values, n);
    }
    if values.len() != width * height {
        return Err(DemError::Raster(format!(
            "expected {} samples, got {}",
            width * height,
            values.len()
        )));
    }
    Ok((values, width, height, west, north, scale[1], scale[0]))
}

/// Turn every sample equal to the nodata sentinel into NaN (void). Exact
/// equality is right: the pixels store the sentinel verbatim, and a near
/// match would swallow real terrain at that value.
fn apply_nodata(values: Vec<f64>, nodata: f64) -> Vec<f64> {
    values
        .into_iter()
        .map(|v| if v == nodata { f64::NAN } else { v })
        .collect()
}

/// Read a single-band float TIFF **without** requiring GeoTIFF tags: the
/// pipeline's own `dtm.tif` carries placement only in `dem.json`, so the
/// tileset generator pairs this with the sidecar's bounds. Samples
/// `<= NODATA` become NaN (voids, later nearest-filled for meshing).
///
/// # Errors
///
/// [`DemError::Raster`] on decode problems or a sample-count mismatch.
pub fn read_band_raw(path: &Path) -> Result<(Vec<f32>, usize, usize), DemError> {
    let file = File::open(path).map_err(|e| DemError::Raster(e.to_string()))?;
    let mut decoder = Decoder::new(BufReader::new(file))
        .map_err(|e| DemError::Raster(e.to_string()))?
        .with_limits({
            // BC LiDAR product tiles reach ~15000x11000 f32 cells (~700 MB);
            // the crate default (256 MiB) rejects those as "limits exceeded".
            // Local, trusted files — raise the buffers, keep everything else
            // at the crate defaults (Limits is #[non_exhaustive]).
            let mut l = tiff::decoder::Limits::default();
            l.decoding_buffer_size = 2 * 1024 * 1024 * 1024;
            l.intermediate_buffer_size = 256 * 1024 * 1024;
            l.ifd_value_size = 16 * 1024 * 1024;
            l
        });
    let (width, height) = decoder
        .dimensions()
        .map_err(|e| DemError::Raster(e.to_string()))?;
    let (width, height) = (width as usize, height as usize);

    let raw: Vec<f32> = match decoder
        .read_image()
        .map_err(|e| DemError::Raster(e.to_string()))?
    {
        tiff::decoder::DecodingResult::F32(v) => v,
        tiff::decoder::DecodingResult::F64(v) => v.into_iter().map(|v| v as f32).collect(),
        tiff::decoder::DecodingResult::U16(v) => v.into_iter().map(f32::from).collect(),
        tiff::decoder::DecodingResult::U32(v) => v.into_iter().map(|v| v as f32).collect(),
        tiff::decoder::DecodingResult::U8(v) => v.into_iter().map(f32::from).collect(),
        other => {
            return Err(DemError::Raster(format!(
                "unsupported sample type: {other:?}"
            )))
        }
    };
    if raw.len() != width * height {
        return Err(DemError::Raster(format!(
            "{}: expected {} samples, got {}",
            path.display(),
            width * height,
            raw.len()
        )));
    }
    let values = raw
        .into_iter()
        .map(|v| if v <= crate::cog::NODATA { f32::NAN } else { v })
        .collect();
    Ok((values, width, height))
}

/// A projected (UTM) single-band raster: raw values on their native grid.
#[allow(clippy::type_complexity)]
pub struct ProjectedBand {
    pub values: Vec<f64>,
    pub width: usize,
    pub height: usize,
    /// Placement of pixel (0,0)'s **corner** (GeoTIFF PixelIsArea): west
    /// edge easting, north edge northing, metres.
    pub min_e: f64,
    pub max_n: f64,
    /// Cell size, metres (square cells assumed — checked).
    pub res_m: f64,
    /// UTM zone from the GeoKeys (EPSG 269xx / 3156..=3166 / citation).
    pub zone: u8,
}

/// Read one single-band **UTM-projected** GeoTIFF (BC LiDAR DTM products:
/// EPSG 26910 / 3157, PixelIsArea tiepoints, metre scale).
///
/// # Errors
///
/// [`DemError::Raster`] when the tags are missing, the raster is not UTM
/// (no parsable zone), or the sample type is unsupported.
pub fn read_projected_band(path: &Path) -> Result<ProjectedBand, DemError> {
    let file = File::open(path).map_err(|e| DemError::Raster(e.to_string()))?;
    let mut decoder = Decoder::new(BufReader::new(file))
        .map_err(|e| DemError::Raster(e.to_string()))?
        .with_limits({
            // BC LiDAR product tiles reach ~15000x11000 f32 cells (~700 MB);
            // the crate default (256 MiB) rejects those as "limits exceeded".
            // Local, trusted files — raise the buffers, keep everything else
            // at the crate defaults (Limits is #[non_exhaustive]).
            let mut l = tiff::decoder::Limits::default();
            l.decoding_buffer_size = 2 * 1024 * 1024 * 1024;
            l.intermediate_buffer_size = 256 * 1024 * 1024;
            l.ifd_value_size = 16 * 1024 * 1024;
            l
        });
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
    if scale.len() < 2 || (scale[0] - scale[1]).abs() > 1e-9 || scale[0] <= 0.0 {
        return Err(DemError::Raster(
            "projected raster needs a square metre cell size".into(),
        ));
    }
    if tie.len() < 6 {
        return Err(DemError::Raster("no ModelTiepoint".into()));
    }
    let (min_e, max_n) = (tie[3], tie[4]);

    // Zone: GeoKey 3072 (ProjectedCSType) EPSG codes cover NAD83 UTM
    // 26901..=26960 (CSRS) and 3156..=3166-ish legacy NAD83; the citation
    // (GeoKey 1026 / ASCII 34737) often says "UTM zone 10N" as a fallback.
    let mut zone: Option<u8> = None;
    if let Ok(keys) = decoder.get_tag_u16_vec(Tag::GeoKeyDirectoryTag) {
        if keys.len() >= 4 {
            let num = keys[3] as usize;
            for i in 0..num {
                let base = 4 + i * 4;
                if base + 3 >= keys.len() {
                    break;
                }
                if keys[base] == 3072 {
                    let epsg = u32::from(keys[base + 3]);
                    if (26901..=26960).contains(&epsg) {
                        zone = Some((epsg - 26900) as u8);
                    } else if (3150..=3200).contains(&epsg) {
                        // Legacy NAD83 UTM: EPSG 3154..3157 map zones
                        // alphabetically — unreliable; fall through to the
                        // citation check.
                    }
                }
            }
        }
    }
    if zone.is_none() {
        if let Ok(cit) = decoder.get_tag_ascii_string(Tag::GeoAsciiParamsTag) {
            let lower = cit.to_ascii_lowercase();
            if let Some(idx) = lower.find("zone ") {
                let digits: String = lower[idx + 5..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(z) = digits.parse::<u8>() {
                    if (1..=60).contains(&z) {
                        zone = Some(z);
                    }
                }
            }
        }
    }
    let Some(zone) = zone else {
        return Err(DemError::Raster(format!(
            "{}: no parsable UTM zone (GeoKey 3072 / citation)",
            path.display()
        )));
    };

    // Nodata sentinel (BC DTM uses -32767 or none at all).
    let nodata = decoder
        .get_tag_ascii_string(Tag::GdalNodata)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .or_else(|| {
            decoder
                .get_tag_f64_vec(Tag::GdalNodata)
                .ok()
                .and_then(|v| v.first().copied())
        });

    let mut values: Vec<f64> = match decoder
        .read_image()
        .map_err(|e| DemError::Raster(e.to_string()))?
    {
        tiff::decoder::DecodingResult::F32(v) => v.into_iter().map(f64::from).collect(),
        tiff::decoder::DecodingResult::F64(v) => v,
        tiff::decoder::DecodingResult::U16(v) => v.into_iter().map(f64::from).collect(),
        tiff::decoder::DecodingResult::U32(v) => v.into_iter().map(f64::from).collect(),
        tiff::decoder::DecodingResult::I16(v) => v.into_iter().map(f64::from).collect(),
        tiff::decoder::DecodingResult::I32(v) => v.into_iter().map(f64::from).collect(),
        tiff::decoder::DecodingResult::U8(v) => v.into_iter().map(f64::from).collect(),
        other => {
            return Err(DemError::Raster(format!(
                "unsupported sample type: {other:?}"
            )))
        }
    };
    if let Some(nd) = nodata {
        for v in values.iter_mut() {
            if *v == nd {
                *v = f64::NAN;
            }
        }
    }
    if values.len() != width * height {
        return Err(DemError::Raster(format!(
            "{}: expected {} samples, got {}",
            path.display(),
            width * height,
            values.len()
        )));
    }
    Ok(ProjectedBand {
        values,
        width,
        height,
        min_e,
        max_n,
        res_m: scale[0],
        zone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cog;
    use crate::grid::GeoRaster;

    /// Written voids (NaN → NODATA sentinel) must come back as NaN, finite
    /// samples unchanged — the mesh generator relies on this pairing.
    #[test]
    fn read_band_raw_roundtrips_values_and_voids() {
        let dir = std::env::temp_dir().join(format!("dem-prep-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("dtm.tif");
        let mut values = vec![42.0f32; 16];
        values[2 * 4 + 1] = f32::NAN;
        let raster = GeoRaster {
            width: 4,
            height: 4,
            values,
            west: -123.425,
            north: 48.65,
        };
        cog::write_geo_tiff(&path, &raster).expect("write");
        let (back, w, h) = read_band_raw(&path).expect("read");
        assert_eq!((w, h), (4, 4));
        assert_eq!(back[0], 42.0);
        assert!(back[2 * 4 + 1].is_nan(), "void must survive the round-trip");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nodata_sentinel_becomes_a_void_and_real_values_survive() {
        // Copernicus DEM stores ocean pixels at -32768; the fallback reader
        // must turn them into voids (which the pipeline fills) and leave
        // genuine terrain values — even one numerically close — untouched.
        let values = vec![-32768.0, 12.5, -32767.0, 0.0, 500.25, -32768.0];
        let out = apply_nodata(values, -32768.0);
        assert!(out[0].is_nan(), "ocean pixel void");
        assert!(out[5].is_nan(), "ocean pixel void");
        assert!(!out[1].is_nan() && out[1] == 12.5);
        assert!(
            !out[2].is_nan() && out[2] == -32767.0,
            "near-sentinel terrain kept"
        );
        assert!(!out[3].is_nan() && out[3] == 0.0);
        assert!(!out[4].is_nan() && out[4] == 500.25);
        // No sentinel present: values pass through untouched.
        let kept = apply_nodata(vec![1.0, 2.0, 3.0], -9999.0);
        assert!(kept.iter().all(|v| v.is_finite()));
    }
}
