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
use super::{Height, HeightDatum, HeightError};

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

/// The committed CGG2013 BC-SW extraction (plan H7, issue N2): the real
/// NRCan CGG2013 grid, converted verbatim from the PROJ CDN mirror by
/// `dem-prep geoid` (provenance in the file header). This closes the
/// `H(CGVD2013) + N(CGG2013) = h` step of the height chain; together with
/// [`egm96_model`] the full `H → h → AMSL` conversion is available offline
/// inside the extraction's bounds (queries outside fail closed).
///
/// Sign convention (verified against PROJ `cct` EPSG:9246, 2026-10-17):
/// the values are the geoid undulation `N = h − H` — exactly what
/// [`GeoidModel::undulation_m`](crate::height::GeoidModel) defines.
pub fn cgg2013_model() -> &'static GeoidGridModel {
    static MODEL: std::sync::OnceLock<GeoidGridModel> = std::sync::OnceLock::new();
    MODEL.get_or_init(|| {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/geoid/cgg2013_bc_sw.mggeoid"
        ));
        let grid = parse_grid(text).unwrap_or_else(|e| {
            // The committed extraction is a repo fixture; a parse failure is
            // a build-time bug, not a runtime condition. A rejected model
            // would silently un-convert heights, so panic instead (the test
            // below pins the fixture).
            panic!("committed cgg2013_bc_sw.mggeoid failed to parse: {e}")
        });
        GeoidGridModel::new("cgg2013-embedded").with_grid(grid)
    })
}

/// Whether the full CGVD2013 → WGS84 ellipsoid → AMSL chain is available at a
/// point (H7): both embedded geoid grids must cover it. `false` means a
/// CGVD2013 tile there cannot be converted to AMSL — the drape fails closed
/// and the AMSL export is blocked (see `mission::drape`).
#[must_use]
pub fn vertical_datum_chain_available(latitude_deg: f64, longitude_deg: f64) -> bool {
    use super::GeoidModel;
    let cgg = cgg2013_model().undulation_m(
        latitude_deg,
        longitude_deg,
        HeightDatum::OrthometricCgvd2013,
    );
    let egm = egm96_model().undulation_m(latitude_deg, longitude_deg, HeightDatum::AmslEgm96);
    cgg.is_ok() && egm.is_ok()
}

/// Convert an absolute height to the AMSL datum accepted by PX4 global
/// commands, using the embedded project geoid grids.
///
/// The conversion is deliberately centralized here: `CGVD2013 H` follows
/// `H + N(CGG2013) = h`, then `h - N(EGM96) = AMSL`; ellipsoid heights use
/// only the second step. A missing grid sample is an error, so callers cannot
/// silently command a guessed absolute altitude.
pub fn to_px4_amsl(
    height: Height,
    latitude_deg: f64,
    longitude_deg: f64,
) -> Result<Height, HeightError> {
    if height.datum() == HeightDatum::AmslEgm96 {
        return Ok(height);
    }
    let ellipsoid = height.convert(
        HeightDatum::EllipsoidWgs84,
        latitude_deg,
        longitude_deg,
        if height.datum() == HeightDatum::OrthometricCgvd2013 {
            cgg2013_model()
        } else {
            egm96_model()
        },
    )?;
    ellipsoid.convert(
        HeightDatum::AmslEgm96,
        latitude_deg,
        longitude_deg,
        egm96_model(),
    )
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

    /// H7 (*verify* N2, control points measured against PROJ `cct` with the
    /// CDN grids on 2026-10-17): the committed CGG2013 extraction must
    /// reproduce the reference undulations `N = h − H`. Both use bilinear
    /// interpolation over the same source grid, so the tolerance is tight.
    #[test]
    fn cgg2013_extraction_matches_proj_reference() {
        let model = cgg2013_model();
        // (lat, lon) -> N(EGM96-style reference from PROJ, metres)
        let pinned = [
            (48.556_06, -123.381_36, -18.8346),
            (48.4, -123.4, -18.5345),
            (48.7, -123.0, -20.3162),
            (49.5, -125.0, -14.8855),
        ];
        for (lat, lon, expected) in pinned {
            let n = model
                .undulation_m(lat, lon, HeightDatum::OrthometricCgvd2013)
                .expect("inside the extraction");
            assert!(
                (n - expected).abs() <= 0.02,
                "N({lat}, {lon}) = {n}, PROJ reference {expected}"
            );
            // Sign guard: a flipped sign would bias every height by ~2N
            // (≈37 m here) — the single most dangerous failure of H7.
            assert!(n < 0.0, "BC-SW undulations are negative, got {n}");
        }
    }

    /// H7 stage 2: the committed EGM96 extraction must reproduce PROJ's
    /// 15-arc-second reference within the coarser 5-arc-minute resolution.
    #[test]
    fn egm96_extraction_matches_proj_reference() {
        let model = egm96_model();
        let pinned = [
            (48.556_06, -123.381_36, -19.4904),
            (48.4, -123.4, -19.3281),
            (48.7, -123.0, -20.8394),
            (49.5, -125.0, -16.1566),
        ];
        for (lat, lon, expected) in pinned {
            let n = model
                .undulation_m(lat, lon, HeightDatum::AmslEgm96)
                .expect("inside the extraction");
            assert!(
                (n - expected).abs() <= 0.2,
                "N({lat}, {lon}) = {n}, PROJ reference {expected}"
            );
        }
    }

    /// H7: the full `H + N(CGG2013) = h`, `h − N(EGM96) = AMSL` chain
    /// through `Height::convert`, with both committed grids in one model.
    #[test]
    fn full_height_chain_matches_two_stage_reference() {
        let model = GeoidGridModel::new("h7-chain")
            .with_grid(
                parse_grid(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/testdata/geoid/cgg2013_bc_sw.mggeoid"
                )))
                .expect("cgg fixture"),
            )
            .with_grid(
                parse_grid(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/testdata/geoid/egm96_bc_sw.mggeoid"
                )))
                .expect("egm fixture"),
            );

        // The AOI DTM's orthometric height (CGVD2013 H ≈ 19.15 m at the
        // fixture centre) to AMSL:
        // AMSL = H + N(CGG2013) − N(EGM96) = 19.15 − 18.8346 + 19.4904.
        let h = Height::new(HeightDatum::OrthometricCgvd2013, 19.15);
        let amsl = h
            .convert(HeightDatum::AmslEgm96, 48.556_06, -123.381_36, &model)
            .expect("chain converts");
        let expected = 19.15 + (-18.8346) - (-19.4904);
        assert!(
            (amsl.meters() - expected).abs() <= 0.25,
            "AMSL = {}, expected ≈ {expected}",
            amsl.meters()
        );
        assert_eq!(amsl.datum(), HeightDatum::AmslEgm96);
        // Round trip back through both grids is exact.
        let back = amsl
            .convert(
                HeightDatum::OrthometricCgvd2013,
                48.556_06,
                -123.381_36,
                &model,
            )
            .expect("chain converts back");
        assert!(
            (back.meters() - 19.15).abs() <= 1e-9,
            "round trip drift {}",
            back.meters()
        );
    }

    #[test]
    fn px4_amsl_helper_handles_all_absolute_datums_and_fails_closed() {
        let (lat, lon) = (48.556_06, -123.381_36);
        let amsl = to_px4_amsl(Height::new(HeightDatum::AmslEgm96, 42.0), lat, lon)
            .expect("AMSL identity");
        assert_eq!(amsl, Height::new(HeightDatum::AmslEgm96, 42.0));

        let n_cgg = cgg2013_model()
            .undulation_m(lat, lon, HeightDatum::OrthometricCgvd2013)
            .expect("CGG2013 grid");
        let n_egm = egm96_model()
            .undulation_m(lat, lon, HeightDatum::AmslEgm96)
            .expect("EGM96 grid");
        let cgvd = to_px4_amsl(
            Height::new(HeightDatum::OrthometricCgvd2013, 42.0),
            lat,
            lon,
        )
        .expect("full chain");
        assert!((cgvd.meters() - (42.0 + n_cgg - n_egm)).abs() < 1e-9);

        let ellipsoid = to_px4_amsl(Height::new(HeightDatum::EllipsoidWgs84, 60.0), lat, lon)
            .expect("ellipsoid to AMSL");
        assert!((ellipsoid.meters() - (60.0 - n_egm)).abs() < 1e-9);

        assert!(to_px4_amsl(
            Height::new(HeightDatum::OrthometricCgvd2013, 42.0),
            51.05,
            -114.07,
        )
        .is_err());
    }

    /// H7 rationale made measurable: the pre-H7 "H ≈ AMSL" shortcut is off
    /// by |N(CGG2013) − N(EGM96)| — which exceeds the 0.5 m acceptance
    /// budget at the AOI. This is the number that justified the AMSL
    /// export gate; if it ever shrinks below budget the gate note can be
    /// revisited, not before.
    #[test]
    fn h_approx_amsl_deviation_exceeds_the_half_metre_budget() {
        let lat = 48.556_06;
        let lon = -123.381_36;
        let n_cgg = cgg2013_model()
            .undulation_m(lat, lon, HeightDatum::OrthometricCgvd2013)
            .expect("cgg");
        let n_egm = egm96_model()
            .undulation_m(lat, lon, HeightDatum::AmslEgm96)
            .expect("egm");
        let deviation = (n_cgg - n_egm).abs();
        assert!(
            deviation > 0.5,
            "the H≈AMSL shortcut is off by only {deviation} m — revisit the gate note"
        );
    }

    /// U9/H7: the chain-availability probe used by the survey-prep card is
    /// true over the survey area and false where either grid is out of
    /// coverage (Calgary is outside the BC-SW extractions).
    #[test]
    fn chain_availability_follows_the_extraction_bounds() {
        assert!(
            vertical_datum_chain_available(48.556_06, -123.381_36),
            "the AOI is inside both extractions"
        );
        assert!(
            !vertical_datum_chain_available(51.05, -114.07),
            "Calgary is outside the BC-SW extractions"
        );
    }

    /// Both committed extractions fail closed outside their bounds.
    #[test]
    fn chain_grids_fail_closed_outside_the_extractions() {
        for (model, datum) in [
            (cgg2013_model(), HeightDatum::OrthometricCgvd2013),
            (egm96_model(), HeightDatum::AmslEgm96),
        ] {
            // Well outside BC-SW (Calgary).
            assert!(
                model.undulation_m(51.05, -114.07, datum).is_err(),
                "grids must never extrapolate"
            );
        }
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
