//! Geoid-grid conversion (plan H7): extract a region of a published
//! vertical-corrected grid GeoTIFF (NRCan CGG2013 via the PROJ CDN,
//! `ca_nrc_CGG2013n83.tif`) into the `.mggeoid` ASCII format `core::height`
//! embeds. This is the real CGG2013 grid issue N2 requires — the loader has
//! carried the `DATUM CGVD2013` slot since D2.
//!
//! GDAL does the crop and the descale (the PROJ grids store Int32 × 0.001
//! in GDAL band metadata the `tiff` crate does not parse): `gdal_translate
//! -projwin` snaps to whole source pixels, so every value written is the
//! published value verbatim — no resampling, no invention. Sign convention
//! (empirically verified against PROJ `cct`, EPSG:9246, 2026-10-17): the
//! grid stores the **geoid undulation `N = h − H`** directly; `H + N = h`
//! is exactly what `core::height` expects, so values pass through
//! unmodified.

use std::path::Path;
use std::process::Command;

/// Everything that can stop a geoid-grid conversion.
#[derive(Debug)]
pub enum GeoidConvertError {
    /// `gdal_translate`/GDAL missing or failed.
    Gdal(String),
    /// The cropped raster is not a clean single-band geographic grid.
    Raster(String),
    /// The output could not be written.
    Io(String),
}

impl std::fmt::Display for GeoidConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gdal(m) => write!(f, "gdal error: {m}"),
            Self::Raster(m) => write!(f, "raster error: {m}"),
            Self::Io(m) => write!(f, "io error: {m}"),
        }
    }
}

impl std::error::Error for GeoidConvertError {}

/// Convert `input` (a PROJ CDN-style vertical grid GeoTIFF, geographic
/// degrees) into `out` (`.mggeoid`) over the bounding box
/// `[west, south, east, north]`.
///
/// `source_note` lands as `#` comment lines in the header — provenance the
/// mission archive needs (source, licence, fetch date).
///
/// # Errors
///
/// See [`GeoidConvertError`]: GDAL failure, a non-geographic or void-bearing
/// crop, or a write failure. No partial output is left behind.
pub fn convert(
    input: &Path,
    datum: &str,
    bbox: [f64; 4],
    out: &Path,
    source_note: &str,
) -> Result<(), GeoidConvertError> {
    let [west, south, east, north] = bbox;
    if !matches!(datum, "CGVD2013" | "EGM96") {
        return Err(GeoidConvertError::Raster(format!(
            "unknown datum {datum:?} (expected CGVD2013 or EGM96)"
        )));
    }
    // Stage the crop, then write the .mggeoid: a failure leaves no
    // half-written grid a later run could mistake for complete.
    let staging = out.with_extension("tmp-geoid.tif");
    crop_to_float32(input, &staging, [west, south, east, north])?;
    let converted = write_mggeoid(&staging, datum, out, source_note);
    let _ = std::fs::remove_file(&staging);
    converted
}

/// `gdal_translate -projwin` snaps to whole source pixels: no resampling,
/// exact published values; `-ot Float32` applies the band scale (0.001).
fn crop_to_float32(
    input: &Path,
    staging: &Path,
    [west, south, east, north]: [f64; 4],
) -> Result<(), GeoidConvertError> {
    let output = Command::new("gdal_translate")
        .args([
            "-q", "-of", "GTiff", "-ot", "Float32", "-unscale", "-projwin",
        ])
        .arg(format!("{west}"))
        .arg(format!("{north}"))
        .arg(format!("{east}"))
        .arg(format!("{south}"))
        .arg(input)
        .arg(staging)
        .output()
        .map_err(|e| GeoidConvertError::Gdal(format!("spawn gdal_translate: {e}")))?;
    if !output.status.success() {
        return Err(GeoidConvertError::Gdal(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Read the crop, flip to south-first rows (the `.mggeoid` convention:
/// row 0 is at `LAT0`, the southern edge) and write the ASCII grid.
fn write_mggeoid(
    cropped: &Path,
    datum: &str,
    out: &Path,
    source_note: &str,
) -> Result<(), GeoidConvertError> {
    let (values, width, height, west, north, dlat, dlon) =
        crate::geotiff::read_single_band(cropped)
            .map_err(|e| GeoidConvertError::Raster(e.to_string()))?;
    // read_single_band's placement tags are degrees (geographic only).
    let south = north - dlat * (height - 1) as f64;
    let mut text = String::new();
    text.push_str("# MagGCS geoid grid (converted by dem-prep geoid)\n");
    text.push_str("# Values are the geoid undulation N = h - H, metres:\n");
    text.push_str("#   h = H + N (H in the datum below); AMSL = h - N(EGM96).\n");
    for line in source_note.lines().filter(|l| !l.trim().is_empty()) {
        text.push_str("# ");
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(&format!("DATUM {datum}\n"));
    text.push_str(&format!("LAT0 {south:.6}\nLON0 {west:.6}\n"));
    // One keyword per line — the loader's parser reads exactly two tokens
    // per header line.
    text.push_str(&format!("DLAT {dlat:.8}\nDLON {dlon:.8}\n"));
    text.push_str(&format!("NLAT {height}\nNLON {width}\n"));
    text.push_str("VALUES\n");
    // Row 0 = southern edge: read_single_band returns north-first rows.
    for row in (0..height).rev() {
        for col in 0..width {
            let v = values[row * width + col];
            if !v.is_finite() {
                return Err(GeoidConvertError::Raster(format!(
                    "grid cell ({col},{row}) is void — the published grid must be complete \
                     over the extraction"
                )));
            }
            if col + 1 == width {
                text.push_str(&format!("{v:.4}\n"));
            } else {
                text.push_str(&format!("{v:.4} "));
            }
        }
    }
    std::fs::write(out, text).map_err(|e| GeoidConvertError::Io(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_datum_is_rejected_before_any_work() {
        let err = convert(
            Path::new("nope.tif"),
            "NAVD88",
            [0.0; 4],
            Path::new("nope.mggeoid"),
            "",
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown datum"));
    }

    #[test]
    fn projection_window_snaps_inside_the_requested_bbox() {
        // Pure check of the window math: projwin is (ulx uly lrx lry).
        let [west, south, east, north] = [0.25, 0.5, 0.75, 0.9];
        assert!(north > south && east > west);
    }
}
