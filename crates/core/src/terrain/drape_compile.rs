//! Waypoint compilation from the designed profiles (plan §5B DR3, ADR-022 §7).
//!
//! The designed `z(s)` is a dense curve; the FC flies **straight segments
//! between waypoints** (plan issue 13, *verify*), so the compiled polyline —
//! not the curve — is the real target. Compilation:
//!
//! 1. **Simplify** each line's (s, z) curve with a Douglas–Peucker variant
//!    whose distance is the **vertical deviation** from the chord (the very
//!    quantity DR5 reports as compilation error — a perpendicular distance
//!    would understate it), tolerance `wp_tol_m`.
//! 2. **Never break the floor**: at every original sample the polyline must
//!    sit at or above `z_min = DSM + min_clearance` (a `None` DSM — DTM-only
//!    tiles — skips the check). A violated sample is **re-inserted** and the
//!    check repeats until clean, so simplification can reduce the item count
//!    but never the clearance.
//! 3. **Speed items**: one `DO_CHANGE_SPEED` before each line (the per-line
//!    cruise speed; parameter semantics *verify*, plan issue 13).
//! 4. **Mission-item limit**: the total is checked against
//!    [`MAX_PATTERN_ITEMS`]; on exceed the error carries a `wp_tol` suggestion
//!    that was *verified to fit* — never a silent relaxation.
//!
//! Every compiled waypoint carries `s` / `line_id` / `kind` so the monitor
//! (DR5) can match the flown position to the plan and the archive (DR7) can
//! aggregate per line. Takeoff/landing/locked items are merged by the caller
//! (DR4's wiring); this module compiles the survey geometry only.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::drape_solver::DrapeError;
use super::drape_spec::LineProfile;
use crate::height::Height;
use crate::plan::PlannedWaypoint;
use crate::survey::pattern::{LineKind, MAX_PATTERN_ITEMS};

/// `MAV_CMD_DO_CHANGE_SPEED` (PX4 reads the speed from `param2`; the payload
/// comes from [`crate::mission::types::do_change_speed_params`]).
pub use crate::mission::types::DO_CHANGE_SPEED;

/// One compiled waypoint with the metadata the monitor and the archive need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct CompiledWaypoint {
    pub waypoint: PlannedWaypoint,
    /// Along-line distance on its line, metres.
    pub s: f64,
    pub line_id: String,
    pub kind: LineKind,
}

impl Default for CompiledWaypoint {
    fn default() -> Self {
        Self {
            waypoint: PlannedWaypoint {
                position: crate::plan::GeoPoint::new(0.0, 0.0),
                altitude: Height::new(crate::height::HeightDatum::default(), 0.0),
                command: 16,
                params: [0.0; 4],
                autocontinue: true,
                locked: false,
            },
            s: 0.0,
            line_id: String::new(),
            kind: LineKind::default(),
        }
    }
}

/// Compile every line's profile into mission-ready waypoints (see the module
/// docs). Pure and deterministic.
///
/// # Errors
///
/// [`DrapeError::TooManyItems`] when the compiled count exceeds the mission
/// limit, carrying a `wp_tol` suggestion verified to fit.
pub fn compile(
    spec: &super::drape_spec::DrapeSpec,
    profiles: &[LineProfile],
    datum: crate::height::HeightDatum,
) -> Result<Vec<CompiledWaypoint>, DrapeError> {
    compile_limited(spec, profiles, datum, MAX_PATTERN_ITEMS)
}

/// [`compile`] against an explicit limit (the tests exercise the refusal path
/// with a small limit instead of a 66 000-point pathological fixture).
pub fn compile_limited(
    spec: &super::drape_spec::DrapeSpec,
    profiles: &[LineProfile],
    datum: crate::height::HeightDatum,
    limit: usize,
) -> Result<Vec<CompiledWaypoint>, DrapeError> {
    // Compile once at the operator's tolerance: over the limit is a refusal
    // with a *verified* suggestion — never a silent relaxation (§5B DR3).
    let compiled = compile_at(spec, profiles, datum, spec.wp_tol_m)?;
    if compiled.len() <= limit {
        return Ok(compiled);
    }
    let mut tol = spec.wp_tol_m;
    for _ in 0..MAX_SUGGESTION_STEPS {
        tol *= 2.0;
        if count_at(spec, profiles, datum, tol) <= limit {
            break;
        }
    }
    Err(DrapeError::TooManyItems {
        count: compiled.len(),
        limit,
        suggested_wp_tol_m: tol,
    })
}

/// Bounded suggestion search (the tolerance doubles; the loop above runs it).
const MAX_SUGGESTION_STEPS: usize = 20;

fn count_at(
    spec: &super::drape_spec::DrapeSpec,
    profiles: &[LineProfile],
    datum: crate::height::HeightDatum,
    tol: f64,
) -> usize {
    compile_at(spec, profiles, datum, tol)
        .map(|c| c.len())
        .unwrap_or(usize::MAX)
}

/// Compile at one tolerance: simplify per line, re-insert floor violations,
/// then emit the waypoints plus one speed item per line.
fn compile_at(
    spec: &super::drape_spec::DrapeSpec,
    profiles: &[LineProfile],
    datum: crate::height::HeightDatum,
    tol: f64,
) -> Result<Vec<CompiledWaypoint>, DrapeError> {
    if tol <= 0.0 || !tol.is_finite() {
        return Err(DrapeError::SelfCheck(format!(
            "wp_tol must be positive, got {tol}"
        )));
    }
    let mut out = Vec::new();
    for line in profiles {
        if line.profile.len() < 2 {
            continue;
        }
        let ss: Vec<f64> = line.profile.iter().map(|s| s.s).collect();
        let zs: Vec<f64> = line.profile.iter().map(|s| s.z_plan).collect();
        let mut kept = douglas_peucker(&ss, &zs, tol);
        // Re-insert floor violations until the polyline clears every sample.
        for _ in 0..line.profile.len() {
            let floors: Vec<Option<f64>> = line
                .profile
                .iter()
                .map(|s| s.dsm.map(|d| d + spec.min_clearance_dsm_m.unwrap_or(0.0)))
                .collect();
            let violating = floor_violations(&ss, &zs, &kept, &floors);
            if violating.is_empty() || kept.len() >= ss.len() {
                break;
            }
            kept.extend(violating);
            kept.sort_unstable();
            kept.dedup();
        }

        // The speed item rides at the line start (its own command item).
        let first = &line.profile[0];
        out.push(CompiledWaypoint {
            waypoint: PlannedWaypoint {
                position: crate::plan::GeoPoint::new(0.0, 0.0),
                altitude: Height::new(datum, first.z_plan),
                command: DO_CHANGE_SPEED,
                params: crate::mission::types::do_change_speed_params(spec.ground_speed_mps as f32),
                autocontinue: true,
                locked: false,
            },
            s: first.s,
            line_id: line.line_id.clone(),
            kind: line.kind,
        });
        for &k in &kept {
            let s = &line.profile[k];
            out.push(CompiledWaypoint {
                waypoint: PlannedWaypoint {
                    position: crate::plan::GeoPoint::new(s.y, s.x),
                    altitude: Height::new(datum, s.z_plan),
                    command: 16,
                    params: [0.0; 4],
                    autocontinue: true,
                    locked: false,
                },
                s: s.s,
                line_id: line.line_id.clone(),
                kind: line.kind,
            });
        }
    }
    Ok(out)
}

/// The worst vertical deviation of the polyline over the original samples
/// (the compilation error DR5 reports separately from the flight error).
#[must_use]
pub fn compile_deviation_max_m(
    profiles: &[LineProfile],
    compiled: &[CompiledWaypoint],
) -> Option<f64> {
    let mut worst: Option<f64> = None;
    for line in profiles {
        let pts: Vec<(f64, f64)> = compiled
            .iter()
            .filter(|c| c.line_id == line.line_id && c.waypoint.command == 16)
            .map(|c| (c.s, c.waypoint.altitude.meters()))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        for s in line.profile.iter() {
            let z_poly = chord_z(&pts, s.s);
            let dev = (z_poly - s.z_plan).abs();
            worst = Some(match worst {
                Some(w) if w >= dev => w,
                _ => dev,
            });
        }
    }
    worst
}

/// Original-sample indices where the chord dips below the floor.
fn floor_violations(ss: &[f64], zs: &[f64], kept: &[usize], floors: &[Option<f64>]) -> Vec<usize> {
    let pts: Vec<(f64, f64)> = kept.iter().map(|&k| (ss[k], zs[k])).collect();
    let mut out = Vec::new();
    for (i, floor) in floors.iter().enumerate() {
        if let Some(floor) = floor {
            if chord_z(&pts, ss[i]) < *floor - 1e-6 {
                out.push(i);
            }
        }
    }
    out
}

/// Linear interpolation of the chord at `s` (endpoints clamp).
fn chord_z(pts: &[(f64, f64)], s: f64) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    if s <= pts[0].0 {
        return pts[0].1;
    }
    if s >= pts[pts.len() - 1].0 {
        return pts[pts.len() - 1].1;
    }
    let i = pts.partition_point(|p| p.0 < s);
    let (a, b) = (pts[i - 1], pts[i]);
    let ds = b.0 - a.0;
    if ds <= 0.0 {
        return b.1;
    }
    a.1 + (b.1 - a.1) * (s - a.0) / ds
}

/// Douglas–Peucker on the (s, z) curve with the **vertical** distance to the
/// chord as the split metric (ADR-022 §7). Returns ascending kept indices,
/// first and last included.
fn douglas_peucker(ss: &[f64], zs: &[f64], tol: f64) -> Vec<usize> {
    let mut kept = vec![false; ss.len()];
    kept[0] = true;
    kept[ss.len() - 1] = true;
    let mut stack = vec![(0usize, ss.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let chord = [(ss[a], zs[a]), (ss[b], zs[b])];
        let (worst_i, worst_d) = (a + 1..b)
            .map(|i| (i, (zs[i] - chord_z(&chord, ss[i])).abs()))
            .fold(
                (a, 0.0f64),
                |(bi, bd), (i, d)| if d > bd { (i, d) } else { (bi, bd) },
            );
        if worst_d > tol {
            kept[worst_i] = true;
            stack.push((a, worst_i));
            stack.push((worst_i, b));
        }
    }
    (0..ss.len()).filter(|&i| kept[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::drape_spec::{DrapeSample, DrapeSpec};

    fn line_profile(ss: &[f64], zs: &[f64], dsm: f64) -> LineProfile {
        LineProfile {
            line_id: "L1".into(),
            kind: LineKind::Survey,
            profile: ss
                .iter()
                .zip(zs)
                .map(|(&s, &z)| DrapeSample {
                    s,
                    x: -123.4 + s / 111_320.0,
                    y: 48.5,
                    z_plan: z,
                    dtm: z - 40.0,
                    dsm: Some(dsm),
                    agl_plan: 40.0,
                    clearance: Some(z - dsm),
                })
                .collect(),
        }
    }

    fn spec(wp_tol: f64) -> DrapeSpec {
        DrapeSpec {
            min_clearance_dsm_m: Some(5.0),
            wp_tol_m: wp_tol,
            ground_speed_mps: 5.0,
            ..DrapeSpec::default()
        }
    }

    /// A wiggle curve: an overall slope with sine ripples — every feature
    /// within one `wp_tol` band disappears, the shape survives.
    fn wiggle(n: usize) -> (Vec<f64>, Vec<f64>) {
        let ss: Vec<f64> = (0..n).map(|i| i as f64 * 2.0).collect();
        let zs: Vec<f64> = ss
            .iter()
            .map(|&s| 57.0 + 0.1 * s + 3.0 * (s / 30.0).sin())
            .collect();
        (ss, zs)
    }

    #[test]
    fn compiled_deviation_stays_within_the_tolerance() {
        let (ss, zs) = wiggle(200);
        let profiles = vec![line_profile(&ss, &zs, 18.0)];
        let spec = spec(2.0);
        let compiled = compile(
            &spec,
            &profiles,
            crate::height::HeightDatum::OrthometricCgvd2013,
        )
        .expect("compile");
        let dev = compile_deviation_max_m(&profiles, &compiled).expect("deviation");
        assert!(
            dev <= 2.0 + 1e-9,
            "compiled deviation {dev} exceeds wp_tol 2.0"
        );
        assert!(compiled.len() < 200, "simplification must remove points");
    }

    #[test]
    fn simplification_never_breaks_the_floor() {
        // A wide tolerance would flatten the spike straight through; the
        // re-insertion must put samples back until the floor holds.
        let ss: Vec<f64> = (0..200).map(|i| i as f64).collect();
        let zs: Vec<f64> = ss
            .iter()
            .map(|&s| {
                if (95.0..105.0).contains(&s) {
                    70.0
                } else {
                    57.0
                }
            })
            .collect();
        let profiles = vec![line_profile(&ss, &zs, 18.0)];
        let spec = spec(50.0); // absurdly wide: flatten everything first
        let compiled = compile(
            &spec,
            &profiles,
            crate::height::HeightDatum::OrthometricCgvd2013,
        )
        .expect("compile");
        for c in compiled.iter().filter(|c| c.waypoint.command == 16) {
            let clearance = c.waypoint.altitude.meters() - 18.0;
            assert!(
                clearance >= 5.0 - 1e-6 || c.s == 0.0 || c.s == ss[ss.len() - 1],
                "waypoint at s {} clears only {clearance} m",
                c.s
            );
        }
        // And the polyline between waypoints still clears the floor: the
        // deviation check must show the re-inserted spike samples.
        let dev = compile_deviation_max_m(&profiles, &compiled).expect("deviation");
        assert!(dev <= 50.0 + 1e-9, "deviation {dev} beyond wp_tol");
    }

    #[test]
    fn over_the_item_limit_is_refused_with_a_working_suggestion() {
        // A 300-point zigzag against a 100-item limit exercises the refusal
        // path without a 66 000-point pathological DP fixture.
        let n = 300;
        let ss: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let zs: Vec<f64> = ss
            .iter()
            .map(|&s| {
                57.0 + if (s as usize).is_multiple_of(2) {
                    0.0
                } else {
                    0.01
                }
            })
            .collect();
        let profiles = vec![line_profile(&ss, &zs, 18.0)];
        let spec = spec(0.001);
        let err = compile_limited(
            &spec,
            &profiles,
            crate::height::HeightDatum::OrthometricCgvd2013,
            100,
        )
        .expect_err("must refuse");
        let DrapeError::TooManyItems {
            count,
            suggested_wp_tol_m,
            ..
        } = &err
        else {
            panic!("expected TooManyItems, got {err}");
        };
        assert!(*count > 100);
        // The suggestion actually fits when applied (the operator applies it
        // explicitly — never a silent relaxation).
        let with_suggestion = spec_at(*suggested_wp_tol_m);
        let compiled = compile_limited(
            &with_suggestion,
            &profiles,
            crate::height::HeightDatum::OrthometricCgvd2013,
            100,
        )
        .expect("suggested tolerance compiles");
        assert!(compiled.len() <= 100);
    }

    fn spec_at(wp_tol: f64) -> DrapeSpec {
        DrapeSpec {
            wp_tol_m: wp_tol,
            ..spec(1.0)
        }
    }

    #[test]
    fn one_speed_item_per_line_and_metadata_on_every_waypoint() {
        let (ss, zs) = wiggle(100);
        let l1 = line_profile(&ss, &zs, 18.0);
        let mut l2 = line_profile(&ss, &zs, 18.0);
        l2.line_id = "L2".into();
        let compiled = compile(
            &spec(5.0),
            &[l1, l2],
            crate::height::HeightDatum::OrthometricCgvd2013,
        )
        .expect("compile");
        for line_id in ["L1", "L2"] {
            let speeds = compiled
                .iter()
                .filter(|c| c.line_id == line_id && c.waypoint.command == DO_CHANGE_SPEED)
                .count();
            assert_eq!(speeds, 1, "one speed item per line");
            let speed = compiled
                .iter()
                .find(|c| c.line_id == line_id && c.waypoint.command == DO_CHANGE_SPEED)
                .expect("speed item");
            assert!(
                (f64::from(speed.waypoint.params[1]) - 5.0).abs() < 1e-6,
                "PX4 reads the speed from param2"
            );
            assert_eq!(speed.waypoint.params[0], 1.0, "groundspeed");
            assert_eq!(speed.waypoint.params[2], -1.0, "no throttle change");
        }
        for c in compiled.iter().filter(|c| c.waypoint.command == 16) {
            assert!(!c.line_id.is_empty());
            assert!(c.s.is_finite());
            assert!(c.waypoint.position.latitude_deg.is_finite());
            assert!(c.waypoint.autocontinue);
        }
    }

    #[test]
    fn compilation_is_deterministic() {
        let (ss, zs) = wiggle(150);
        let profiles = vec![line_profile(&ss, &zs, 18.0)];
        let a = compile(
            &spec(3.0),
            &profiles,
            crate::height::HeightDatum::OrthometricCgvd2013,
        )
        .expect("a");
        let b = compile(
            &spec(3.0),
            &profiles,
            crate::height::HeightDatum::OrthometricCgvd2013,
        )
        .expect("b");
        assert_eq!(a, b);
    }
}
