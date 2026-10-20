//! Golden-file tests for the survey → mission pipeline (comprehensive plan
//! S4): a fixed polygon plus fixed parameters must produce byte-identical
//! mission items, line tables and stats. Regenerate the goldens with
//! `UPDATE_GOLDEN=1 cargo test -p maggcs-core --test survey_golden`.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use maggcs_core::plan::GeoPoint;
use maggcs_core::survey::compile::{compile_pattern, CompileOptions, MagTrigger};
use maggcs_core::survey::pattern::{CloverleafPattern, SurveyPattern};

static REGENERATE: AtomicBool = AtomicBool::new(false);

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn assert_golden(name: &str, value: &impl serde::Serialize) {
    let json = serde_json::to_string_pretty(value).expect("serialize golden");
    let path = golden_dir().join(format!("{name}.json"));
    if REGENERATE.load(Ordering::Relaxed) {
        fs::create_dir_all(golden_dir()).expect("mkdir");
        fs::write(&path, &json).expect("write golden");
        println!("regenerated {}", path.display());
        return;
    }
    match fs::read_to_string(&path) {
        Ok(expected) => assert_eq!(
            expected, json,
            "golden {name} drifted; regenerate with UPDATE_GOLDEN=1"
        ),
        Err(_) => panic!(
            "golden {} missing; run UPDATE_GOLDEN=1 cargo test -p maggcs-core --test survey_golden",
            path.display()
        ),
    }
}

fn init() {
    REGENERATE.store(
        std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1"),
        Ordering::Relaxed,
    );
}

/// A 500x500 m square centred near 48 N (the tests' standard area).
fn square_500() -> Vec<GeoPoint> {
    let center = GeoPoint::new(48.0, -123.0);
    let projection = maggcs_core::survey::projection::LocalProjection::new(center);
    [
        (-250.0, -250.0),
        (250.0, -250.0),
        (250.0, 250.0),
        (-250.0, 250.0),
    ]
    .iter()
    .map(|&(e, n)| projection.to_geo(e, n))
    .collect()
}

fn sweep() -> SurveyPattern {
    SurveyPattern {
        polygon: square_500(),
        line_azimuth_deg: 0.0,
        line_spacing_m: 50.0,
        tie_spacing_m: Some(250.0),
        tie_azimuth_deg: 90.0,
        lead_in_m: 20.0,
        lead_out_m: 20.0,
        altitude_amsl_m: 150.0,
        alternate: true,
        speed_mps: Some(5.0),
    }
}

#[test]
fn sweep_golden_with_camera_trigger_gates() {
    init();
    let plan = sweep().generate().expect("generate");
    let compiled = compile_pattern(
        &plan,
        &CompileOptions {
            mag_trigger: MagTrigger::CameraTrigger {
                trigger_dist_m: 5.0,
            },
            cruise_speed_mps: Some(5.0),
            coverage_polygon: Some(square_500()),
        },
    )
    .expect("compile");
    assert_golden("survey_sweep_500_camera_trigger", &compiled);
}

#[test]
fn sweep_golden_gcs_side() {
    init();
    let plan = sweep().generate().expect("generate");
    let compiled = compile_pattern(
        &plan,
        &CompileOptions {
            mag_trigger: MagTrigger::GcsSide,
            cruise_speed_mps: Some(5.0),
            coverage_polygon: Some(square_500()),
        },
    )
    .expect("compile");
    assert_golden("survey_sweep_500_gcs_side", &compiled);
}

#[test]
fn cloverleaf_golden() {
    init();
    let pattern = CloverleafPattern {
        center: GeoPoint::new(48.0, -123.0),
        radius_m: 75.0,
        petals: 4,
        samples_per_petal: 10,
        altitude_amsl_m: 150.0,
        start_heading_deg: 0.0,
        speed_mps: Some(5.0),
    };
    let plan = pattern.generate().expect("generate");
    let compiled = compile_pattern(
        &plan,
        &CompileOptions {
            mag_trigger: MagTrigger::GcsSide,
            cruise_speed_mps: Some(5.0),
            coverage_polygon: None,
        },
    )
    .expect("compile");
    assert_golden("survey_cloverleaf_75", &compiled);
}
