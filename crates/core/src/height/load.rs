//! Geoid grid file loading (D2): `testdata/geoid/*.mggeoid` — the ASCII
//! format `tools` writes when extracting a region of a published model
//! (EGM96 5′ global grid, CGG2013 when available).
//!
//! Format (line-oriented, `#` comments):
//!
//! ```text
//! DATUM EGM96
//! LAT0 47.5   LON0 -125.5
//! DLAT 0.0833 DLON 0.0833
//! NLAT 37     NLON 37
//! VALUES
//! <nlat rows of nlon undulations in metres, whitespace-separated>
//! ```
//!
//! `DATUM` maps onto [`HeightDatum::AmslEgm96`] / `CGVD2013`.

use std::path::Path;

use super::grid::{GeoidGrid, GeoidGridModel, GeoidGridSpec};
use super::{HeightDatum, HeightError};

/// Load one `.mggeoid` file and register it under its datum in `model`.
///
/// # Errors
///
/// [`HeightError::InvalidGrid`] on a malformed file (bad datum, shape,
/// counts or values) — a silently mis-parsed grid would bias every height.
pub fn load_geoid_grid(
    path: &Path,
    model: &mut GeoidGridModel,
) -> Result<HeightDatum, HeightError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| HeightError::InvalidGrid(format!("{}: {e}", path.display())))?;
    let grid = parse_grid(&text)?;
    let datum = grid.datum();
    *model = std::mem::take(model).with_grid(grid);
    Ok(datum)
}

/// Build a model from one or more grid files.
///
/// # Errors
///
/// See [`load_geoid_grid`].
pub fn load_geoid_model(paths: &[&Path]) -> Result<GeoidGridModel, HeightError> {
    let mut model = GeoidGridModel::new("loaded");
    for path in paths {
        load_geoid_grid(path, &mut model)?;
    }
    Ok(model)
}

/// The committed EGM96 BC-SW extraction, embedded at compile time and parsed
/// once: a ready-to-use [`GeoidModel`] for the AMSL ↔ ellipsoid step of the
/// height chain in the app (`geoid_sample`), valid inside the extraction's
/// bounds (queries outside fail closed, like every grid).
pub fn egm96_model() -> &'static GeoidGridModel {
    static MODEL: std::sync::OnceLock<GeoidGridModel> = std::sync::OnceLock::new();
    MODEL.get_or_init(|| {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/geoid/egm96_bc_sw.mggeoid"
        ));
        let grid = parse_grid(text).unwrap_or_else(|e| {
            // The committed extraction is a repo fixture; a parse failure is
            // a build-time bug, not a runtime condition. A rejected model
            // would silently un-convert heights, so panic instead (the test
            // below pins the fixture).
            panic!("committed egm96_bc_sw.mggeoid failed to parse: {e}")
        });
        GeoidGridModel::new("egm96-embedded").with_grid(grid)
    })
}

fn parse_grid(text: &str) -> Result<GeoidGrid, HeightError> {
    let mut datum = None;
    let mut lat0 = None;
    let mut lon0 = None;
    let mut dlat = None;
    let mut dlon = None;
    let mut nlat = None;
    let mut nlon = None;
    let mut values: Vec<f64> = Vec::new();
    let mut in_values = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if in_values {
            for token in line.split_whitespace() {
                let v: f64 = token
                    .parse()
                    .map_err(|_| HeightError::InvalidGrid(format!("bad value {token:?}")))?;
                values.push(v);
            }
            continue;
        }
        let mut tokens = line.split_whitespace();
        match tokens.next() {
            Some("DATUM") => {
                datum = Some(match tokens.next() {
                    Some("EGM96") => HeightDatum::AmslEgm96,
                    Some("CGVD2013") => HeightDatum::OrthometricCgvd2013,
                    Some(other) => {
                        return Err(HeightError::InvalidGrid(format!("unknown datum {other:?}")))
                    }
                    None => return Err(HeightError::InvalidGrid("DATUM needs a value".into())),
                });
            }
            Some("LAT0") => lat0 = tokens.next().and_then(|t| t.parse().ok()),
            Some("LON0") => lon0 = tokens.next().and_then(|t| t.parse().ok()),
            Some("DLAT") => dlat = tokens.next().and_then(|t| t.parse().ok()),
            Some("DLON") => dlon = tokens.next().and_then(|t| t.parse().ok()),
            Some("NLAT") => nlat = tokens.next().and_then(|t| t.parse::<usize>().ok()),
            Some("NLON") => nlon = tokens.next().and_then(|t| t.parse::<usize>().ok()),
            Some("VALUES") => in_values = true,
            other => {
                return Err(HeightError::InvalidGrid(format!(
                    "unexpected line {other:?}"
                )))
            }
        }
    }

    let datum = datum.ok_or_else(|| HeightError::InvalidGrid("no DATUM".into()))?;
    let spec = GeoidGridSpec {
        datum,
        lat0_deg: lat0.ok_or_else(|| HeightError::InvalidGrid("no LAT0".into()))?,
        lon0_deg: lon0.ok_or_else(|| HeightError::InvalidGrid("no LON0".into()))?,
        lat_step_deg: dlat.ok_or_else(|| HeightError::InvalidGrid("no DLAT".into()))?,
        lon_step_deg: dlon.ok_or_else(|| HeightError::InvalidGrid("no DLON".into()))?,
        nlat: nlat.ok_or_else(|| HeightError::InvalidGrid("no NLAT".into()))?,
        nlon: nlon.ok_or_else(|| HeightError::InvalidGrid("no NLON".into()))?,
    };
    GeoidGrid::new(spec, values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::height::{GeoidModel, Height};

    fn grid_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/geoid/egm96_bc_sw.mggeoid")
    }

    /// The committed BC-SW EGM96 extraction loads and interpolates the real
    /// published values (Sidney: N ≈ −19.8 m).
    #[test]
    fn loads_the_bc_sw_egm96_extraction() {
        let mut model = GeoidGridModel::new("egm96-bc");
        let datum = load_geoid_grid(&grid_path(), &mut model).expect("load");
        assert_eq!(datum, HeightDatum::AmslEgm96);

        // Sidney: the node grid holds −19.79 m near (48.6667, −123.3333).
        let n = model
            .undulation_m(48.6667, -123.3333, HeightDatum::AmslEgm96)
            .expect("undulation");
        assert!((n - (-19.79)).abs() < 0.15, "N = {n}");
    }

    /// The AGENTS.md acceptance chain: `H + N(CGG2013) = h`,
    /// `h − N(EGM96) = AMSL` — exercised through `Height::convert` with the
    /// loaded model (EGM96 grid only; the CGVD2013 N comes from the same
    /// loader once a CGG2013 extraction is committed).
    #[test]
    fn height_chain_converts_orthometric_to_amsl() {
        let mut model = GeoidGridModel::new("egm96-bc");
        load_geoid_grid(&grid_path(), &mut model).expect("load");

        // A BC point: orthometric H = 50 m (CGVD2013). To AMSL:
        // h = H + N(CGG2013); AMSL = h − N(EGM96). With the EGM96 grid only
        // we cannot do the CGG2013 step, so assert the EGM96 half: an AMSL
        // height converts to ellipsoid and back exactly.
        let amsl = Height::new(HeightDatum::AmslEgm96, 50.0);
        let ell = amsl
            .convert(HeightDatum::EllipsoidWgs84, 48.6667, -123.3333, &model)
            .expect("to ellipsoid");
        // h = AMSL + N(EGM96) = 50 + (−19.79) = 30.21
        assert!((ell.meters() - 30.21).abs() < 0.15, "h = {}", ell.meters());
        let back = ell
            .convert(HeightDatum::AmslEgm96, 48.6667, -123.3333, &model)
            .expect("back");
        assert!((back.meters() - 50.0).abs() < 1e-9, "round trip");
    }

    #[test]
    fn malformed_files_are_rejected() {
        let cases = [
            "GARBAGE\n",
            "DATUM UNKNOWN2013\nLAT0 0\nLON0 0\nDLAT 1\nDLON 1\nNLAT 2\nNLON 2\nVALUES\n0 0\n0 0\n",
            "DATUM EGM96\nLAT0 0\nLON0 0\nDLAT 1\nDLON 1\nNLAT 2\nNLON 2\nVALUES\n0 0\n0\n",
        ];
        for (i, case) in cases.iter().enumerate() {
            let dir = std::env::temp_dir().join("maggcs-geoid-test");
            std::fs::create_dir_all(&dir).expect("mkdir");
            let path = dir.join(format!("bad{i}.mggeoid"));
            std::fs::write(&path, case).expect("write");
            let mut model = GeoidGridModel::new("bad");
            assert!(
                load_geoid_grid(&path, &mut model).is_err(),
                "case {i} should be rejected"
            );
        }
    }

    /// The embedded EGM96 model (used by the app's `geoid_sample`) parses
    /// and covers the survey area: Sidney N ≈ −19.8 m, and a query far
    /// outside the extraction fails closed instead of extrapolating.
    #[test]
    fn embedded_egm96_model_covers_the_survey_area() {
        let model = super::egm96_model();
        let n = model
            .undulation_m(48.6667, -123.3333, HeightDatum::AmslEgm96)
            .expect("Sidney inside the extraction");
        assert!((n - (-19.79)).abs() < 0.15, "N = {n}");

        assert!(
            model
                .undulation_m(20.0, 20.0, HeightDatum::AmslEgm96)
                .is_err(),
            "outside the extraction must fail closed"
        );
    }
}
