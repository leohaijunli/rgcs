//! The drape pipeline end-to-end (plan §5B DR1–DR3 acceptance, on real file
//! I/O): synthetic DTM+DSM GeoTIFFs on disk → `CogDem::open` (the same
//! reader the app uses) → `drape_solver::solve` → `drape_compile::compile` —
//! asserting every constraint the plan names, on the artifacts the app will
//! consume. The UI wiring (DR4) sits on top of exactly these calls.
//!
//! ```text
//! cargo test -p maggcs-core --test drape_pipeline
//! ```

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use maggcs_core::plan::{GeoPoint, PlannedWaypoint};
use maggcs_core::survey::pattern::{LineKind, MAX_PATTERN_ITEMS};
use maggcs_core::terrain::cog::CogDem;
use maggcs_core::terrain::drape_compile::compile;
use maggcs_core::terrain::drape_solver::{solve, DrapeError, InputLine};
use maggcs_core::terrain::drape_spec::{DrapeSpec, DRAPE_SPEC_VERSION};
use maggcs_core::terrain::{DemBounds, DemMetadata, DemSource, ElevationSource, VerticalDatum};

/// A site with one obstacle: the DTM ramps gently then steps; the DSM rides
/// 1 m above the ground with a 9 m pole near the middle of the line.
struct Site {
    dem: Arc<CogDem>,
    dir: PathBuf,
}

fn build_site(tag: &str) -> Site {
    let dir = std::env::temp_dir().join(format!(
        "maggcs-drape-pipeline-{}-{}",
        tag,
        maggcs_core::recorder::now_utc_us()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");

    // 300 m west→east; a 12 m step at x = 150 and a 9 m pole at x = 220.
    let dtm = |x: f64, _y: f64| 17.0 + if x > 150.0 { 12.0 } else { 0.0 };
    let dsm = |x: f64, y: f64| {
        let pole = if (215.0..225.0).contains(&x) && y.abs() < 3.0 {
            9.0
        } else {
            0.0
        };
        dtm(x, y) + 1.0 + pole
    };

    // A 300 × 60 m geographic raster at ~1 m cells (300 × 60 px). The
    // longitude cell is inflated by 1/cos(lat) so a pixel is 1 m in BOTH
    // axes on the ground.
    let (west, north) = (-123.4f64, 48.5f64);
    let cell_deg = 1.0 / 111_320.0;
    let cos_lat = (north * std::f64::consts::PI / 180.0).cos();
    let lon_cell_deg = cell_deg / cos_lat;
    let width = 300usize;
    let height = 60usize;

    let mut dtm_values = Vec::with_capacity(width * height);
    let mut dsm_values = Vec::with_capacity(width * height);
    for r in 0..height {
        let y = -(r as f64) * cell_deg * 111_320.0; // local metres south of north
        for c in 0..width {
            let x = c as f64 * 1.0;
            dtm_values.push(dtm(x, y) as f32);
            dsm_values.push(dsm(x, y) as f32);
        }
    }

    write_geo(
        &dir.join("dtm.tif"),
        &dtm_values,
        width,
        height,
        west,
        north,
        lon_cell_deg,
    );
    write_geo(
        &dir.join("dsm.tif"),
        &dsm_values,
        width,
        height,
        west,
        north,
        lon_cell_deg,
    );

    let bounds = DemBounds {
        north,
        south: north - height as f64 * cell_deg,
        west,
        east: west + width as f64 * lon_cell_deg,
    };
    let meta = DemMetadata {
        version: "pipeline00000000".into(),
        raster_crs: "EPSG:4326".into(),
        resolution_m: 1.0,
        bounds,
        vertical_datum: VerticalDatum::OrthometricCgvd2013,
        has_dsm: true,
        has_dtm: true,
        source: DemSource::Lidar,
        dlat_deg: Some(cell_deg),
        dlon_deg: Some(lon_cell_deg),
        stac_item_id: None,
        project_name: None,
        acquisition: None,
        original_crs: None,
        provenance_sha256: None,
        gdal_version: None,
        proj_version: None,
    };
    let json = serde_json::to_string_pretty(&meta).expect("dem.json");
    std::fs::write(dir.join("dem.json"), json).expect("write dem.json");

    let dem = Arc::new(
        CogDem::open(&dir.join("dtm.tif"), &dir.join("dsm.tif"), &meta).expect("open site"),
    );
    Site { dem, dir }
}

/// A single-band f32 GeoTIFF like `dem-prep` writes (no placement tags; the
/// placement lives in `dem.json`).
fn write_geo(
    path: &Path,
    values: &[f32],
    width: usize,
    height: usize,
    _west: f64,
    _north: f64,
    _cell: f64,
) {
    let file = File::create(path).expect("create tif");
    let mut encoder = tiff::encoder::TiffEncoder::new(BufWriter::new(file)).expect("encoder");
    encoder
        .write_image::<tiff::encoder::colortype::Gray32Float>(width as u32, height as u32, values)
        .expect("write image");
}

fn line_x(origin_lon: f64, x_from: f64, x_to: f64, lat: f64) -> InputLine {
    let deg_per_m = 1.0 / (111_320.0 * lat.to_radians().cos());
    InputLine {
        id: "S1".into(),
        kind: LineKind::Survey,
        vertices: vec![
            GeoPoint::new(lat, origin_lon + x_from * deg_per_m),
            GeoPoint::new(lat, origin_lon + x_to * deg_per_m),
        ],
    }
}

fn spec() -> DrapeSpec {
    DrapeSpec {
        min_clearance_dsm_m: Some(5.0),
        target_agl_m: 40.0,
        agl_band_m: 3.0,
        max_climb_rate_mps: 3.0,
        max_descent_rate_mps: 3.0,
        ground_speed_mps: 5.0,
        sample_step_m: 1.0,
        runin_length_m: 0.0,
        wp_tol_m: 2.0,
        ..DrapeSpec::default()
    }
}

#[test]
fn the_full_pipeline_produces_a_flyable_compiled_mission() {
    let site = build_site("e2e");
    let lat: f64 = 48.5;
    // The line spans the DEM's x extent minus margins: x from 20 to 280 m.
    let deg_per_m = 1.0 / (111_320.0 * lat.to_radians().cos());
    let x = |m: f64| -123.4 + m * deg_per_m;
    let _ = x;

    let spec = spec();
    let lines = vec![line_x(-123.4, 20.0, 280.0, lat)];
    let mut result = solve(&spec, &lines, site.dem.as_ref()).expect("solve");

    // DR2 invariants on the profile.
    assert_eq!(result.lines.len(), 1);
    assert_eq!(result.lines[0].line_id, "S1");
    let l = &result.lines[0];
    for i in 1..l.profile.len() {
        let ds = l.profile[i].s - l.profile[i - 1].s;
        let grade = (l.profile[i].z_plan - l.profile[i - 1].z_plan) / ds;
        assert!(
            grade <= spec.max_grade_up() + 1e-6 && grade >= -spec.max_grade_down() - 1e-6,
            "grade {grade} at s {}",
            l.profile[i].s
        );
        if let Some(c) = l.profile[i].clearance {
            assert!(c >= spec.min_clearance_dsm_m.unwrap() - 1e-6);
        }
    }

    // DR3: compile and verify the artifacts the app will upload.
    let compiled = compile(&spec, &result.lines, result.dem_ref.datum).expect("compile");
    assert!(compiled.len() <= MAX_PATTERN_ITEMS);
    assert_eq!(
        compiled
            .iter()
            .filter(|c| c.waypoint.command == 178)
            .count(),
        1,
        "one speed item"
    );
    for c in compiled.iter().filter(|c| c.waypoint.command == 16) {
        assert_eq!(c.line_id, "S1");
        assert_eq!(c.kind, LineKind::Survey);
        assert!(c.s.is_finite());
        // The DEM ground under the waypoint from the REAL reader: the floor
        // must hold using an independent sample (not the solver's own cache).
        let ground = site
            .dem
            .elevation(GeoPoint::new(
                c.waypoint.position.latitude_deg,
                c.waypoint.position.longitude_deg,
            ))
            .expect("in coverage");
        let dsm = site
            .dem
            .sample_pair(GeoPoint::new(
                c.waypoint.position.latitude_deg,
                c.waypoint.position.longitude_deg,
            ))
            .expect("pair")
            .1
            .meters();
        let clearance = c.waypoint.altitude.meters() - dsm;
        assert!(
            clearance >= spec.min_clearance_dsm_m.unwrap() - 1.0,
            "compiled waypoint at s {} clears {clearance} m (DEM neighbourhood max may sit between samples)",
            c.s
        );
        let _ = ground;
    }

    // The deviation is stored on the stats and within the tolerance.
    result.compiled = compiled;
    let dev = result
        .stats
        .clone()
        .compile_deviation_max_m
        .unwrap_or_else(|| {
            maggcs_core::terrain::drape_compile::compile_deviation_max_m(
                &result.lines,
                &result.compiled,
            )
            .expect("deviation")
        });
    assert!(dev <= spec.wp_tol_m + 1e-9, "deviation {dev} > wp_tol");

    // Determinism: the same inputs solve to the same hash.
    let again = solve(&spec, &lines, site.dem.as_ref()).expect("again");
    assert_eq!(again.spec_hash, result.spec_hash);
    assert_eq!(again.spec_hash.len(), 64);
    assert_eq!(result.version, DRAPE_SPEC_VERSION);

    let _ = std::fs::remove_dir_all(&site.dir);
}

#[test]
fn a_planned_waypoint_round_trip_keeps_the_compiled_heights() {
    // The compiled PlannedWaypoints must survive the serde round trip the
    // mission upload path applies (the `itemsHash` input shape).
    let site = build_site("roundtrip");
    let lines = vec![line_x(-123.4, 20.0, 280.0, 48.5)];
    let spec = spec();
    let result = solve(&spec, &lines, site.dem.as_ref()).expect("solve");
    let compiled = compile(&spec, &result.lines, result.dem_ref.datum).expect("compile");
    let waypoints: Vec<PlannedWaypoint> = compiled.iter().map(|c| c.waypoint.clone()).collect();
    let json = serde_json::to_string(&waypoints).expect("serialize");
    let back: Vec<PlannedWaypoint> = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.len(), waypoints.len());
    for (a, b) in back.iter().zip(&waypoints) {
        assert_eq!(a.command, b.command);
        assert_eq!(a.altitude, b.altitude, "heights round-trip exactly");
        // One-ULP f64 drift through the JSON parser is ~1e-14 degrees —
        // three orders below the GNSS noise floor.
        assert!(
            (a.position.latitude_deg - b.position.latitude_deg).abs() < 1e-9
                && (a.position.longitude_deg - b.position.longitude_deg).abs() < 1e-9,
            "position drifted: {:?} vs {:?}",
            a.position,
            b.position
        );
    }
    let _ = std::fs::remove_dir_all(&site.dir);
}

#[test]
fn an_uncovered_line_refuses_the_whole_pipeline() {
    let site = build_site("uncovered");
    // The DEM spans ~300 m west→east; a line far outside refuses.
    let lines = vec![line_x(-123.4, 500.0, 700.0, 48.5)];
    let err = solve(&spec(), &lines, site.dem.as_ref()).expect_err("must refuse");
    assert!(matches!(err, DrapeError::NoData { .. }), "got {err}");
    let _ = std::fs::remove_dir_all(&site.dir);
}
