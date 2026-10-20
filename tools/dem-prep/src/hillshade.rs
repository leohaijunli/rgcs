//! Hillshade rendering (terrain overlay, plan A): a grayscale PNG from the
//! DTM via the standard horn-form slope + 315° illumination — the look every
//! GIS tool shows, enough for the operator to read ridges and gullies under
//! the survey lines.

use std::path::Path;

use super::error::DemError;
use super::grid::GeoRaster;

/// Sun azimuth (315°, north-west) and altitude (45°) — the classic hillshade.
const SUN_AZIMUTH_DEG: f64 = 315.0;
const SUN_ALTITUDE_DEG: f64 = 45.0;

/// Render the DTM to a grayscale PNG (row-major, north-up like the raster).
///
/// # Errors
///
/// [`DemError::Raster`] on an encoder/write failure.
pub fn write_hillshade_png(
    path: &Path,
    dtm: &GeoRaster,
    dlat_deg: f64,
    dlon_deg: f64,
) -> Result<(), DemError> {
    let shade = render(dtm, dlat_deg, dlon_deg);
    let file = File::create(path).map_err(|e| DemError::Raster(e.to_string()))?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        u32::try_from(dtm.width).map_err(|e| DemError::Raster(e.to_string()))?,
        u32::try_from(dtm.height).map_err(|e| DemError::Raster(e.to_string()))?,
    );
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| DemError::Raster(e.to_string()))?;
    writer
        .write_image_data(&shade)
        .map_err(|e| DemError::Raster(e.to_string()))?;
    Ok(())
}

/// Horn-form hillshade into 8-bit grayscale. Void cells (NaN) render black.
fn render(dtm: &GeoRaster, dlat_deg: f64, dlon_deg: f64) -> Vec<u8> {
    let (w, h) = (dtm.width, dtm.height);
    let mut out = vec![0u8; w * h];
    // Cell size in metres (the DTM is a metre grid before the WGS84 regrid —
    // but this runs on the geographic raster, so recover metres-per-pixel
    // from the placement: use the same conversion the pipeline applies).
    let mid_lat = dtm.north - dlat_deg * h as f64 / 2.0;
    let m_per_px_lat = dlat_deg * 111_132.954;
    let m_per_px_lon = dlon_deg * 111_319.49 * mid_lat.to_radians().cos();

    let az = SUN_AZIMUTH_DEG.to_radians();
    let alt = SUN_ALTITUDE_DEG.to_radians();

    for row in 0..h {
        for col in 0..w {
            let i = row * w + col;
            let z = dtm.values[i];
            if z.is_nan() {
                continue; // black
            }
            let zm = |r: isize, c: isize| -> Option<f64> {
                if r < 0 || c < 0 || r as usize >= h || c as usize >= w {
                    return None;
                }
                let v = dtm.values[r as usize * w + c as usize];
                if v.is_nan() {
                    None
                } else {
                    Some(f64::from(v))
                }
            };
            let (zl, zr) = (
                zm(row as isize, col as isize - 1),
                zm(row as isize, col as isize + 1),
            );
            let (zu, zd) = (
                zm(row as isize - 1, col as isize),
                zm(row as isize + 1, col as isize),
            );
            let Some(zl) = zl else { continue };
            let Some(zr) = zr else { continue };
            let Some(zu) = zu else { continue };
            let Some(zd) = zd else { continue };
            // Horn gradients (cell size metres).
            let dzdx = (zr - zl) / (2.0 * m_per_px_lon);
            let dzdy = (zd - zu) / (2.0 * m_per_px_lat);
            // Surface normal; dot with the sun vector.
            let slope = (1.0 + dzdx * dzdx + dzdy * dzdy).sqrt();
            let nx = -dzdx / slope;
            let ny = -dzdy / slope;
            let nz = 1.0 / slope;
            let sun_x = alt.cos() * az.sin();
            let sun_y = alt.cos() * az.cos();
            let sun_z = alt.sin();
            let lum = (nx * sun_x + ny * sun_y + nz * sun_z).max(0.0);
            // Gamma-lite: lift the midtones so gullies stay readable.
            let v = (lum.powf(0.7) * 255.0).round() as u8;
            out[i] = v;
        }
    }
    out
}

use std::fs::File;
use std::io::BufWriter;
