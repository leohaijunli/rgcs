//! Pattern → mission compilation (comprehensive plan S1/S2): turn a
//! [`PatternPlan`] into the waypoint sequence that belongs in a
//! [`PlannedMission`], plus the quality stats the Pattern step shows.
//!
//! Design decisions (operator-confirmed 2026-10-11):
//! - The magnetometer samples continuously; survey-line boundaries are only
//!   needed to mark which data is "the real measurement". By default no
//!   mission items are spent on that — the GCS segments live data by the
//!   [`PatternLine`] table + `MISSION_CURRENT`, and post-flight QC uses the
//!   same table. Optionally `DO_SET_CAM_TRIGG_DIST` gates are emitted for a
//!   magnetometer wired to the camera-trigger input.
//! - No takeoff/RTL items: the pattern is the body; the caller decides the
//!   envelope. No inter-line transition items either: NAV waypoints are
//!   flown as straight lines, and the serpentine order comes from the
//!   generator.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::pattern::{LineKind, PatternLine, PatternPlan, MAX_PATTERN_ITEMS};
use super::projection::LocalProjection;
use crate::plan::types::{GeoPoint, PlannedWaypoint};

/// `MAV_CMD_DO_SET_CAM_TRIGG_DIST`.
const CMD_DO_SET_CAM_TRIGG_DIST: u16 = 206;
/// Cruise speed assumed for the ETA when no speed is given (m/s).
pub const DEFAULT_SURVEY_SPEED_MPS: f32 = 5.0;

/// How survey-line boundaries are marked in the mission (S1 decision).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MagTrigger {
    /// No mission items: the GCS segments by the [`PatternLine`] table
    /// (default).
    #[default]
    GcsSide,
    /// `DO_SET_CAM_TRIGG_DIST` on every survey line: `trigger_dist_m` at the
    /// line start, 0 (off) after the line end — for a magnetometer wired to
    /// the camera-trigger input.
    CameraTrigger { trigger_dist_m: f32 },
}

/// Compilation options.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct CompileOptions {
    pub mag_trigger: MagTrigger,
    /// Ground speed for the ETA; `None` uses [`DEFAULT_SURVEY_SPEED_MPS`].
    /// Should match the speed the pattern emitted (DO_CHANGE_SPEED).
    pub cruise_speed_mps: Option<f32>,
    /// The surveyed area (the polygon the sweep was generated from), for the
    /// coverage stat. `None` (e.g. a cloverleaf) omits the stat.
    pub coverage_polygon: Option<Vec<GeoPoint>>,
}

/// Quality metrics for the Pattern step (S2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct PatternStats {
    /// Flown length of the survey lines, metres (line ranges only).
    pub survey_length_m: f64,
    /// Flown length of the tie lines, metres.
    pub tie_length_m: f64,
    /// Flown length of the calibration manoeuvre, metres.
    pub calibration_length_m: f64,
    /// Whole path including lead-in/out and inter-line transitions, metres.
    pub total_length_m: f64,
    pub line_count: u32,
    /// `total_length_m / cruise speed`, seconds.
    pub estimated_time_s: f64,
    /// Shoelace area of the coverage polygon, m². `None` = not provided.
    pub coverage_m2: Option<f64>,
}

/// The compiled pattern: waypoints to append to a `PlannedMission` (the line
/// table re-indexed onto them) plus the stats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct CompiledPattern {
    pub waypoints: Vec<PlannedWaypoint>,
    /// The line table with `start_seq`/`end_seq` re-indexed onto
    /// [`CompiledPattern::waypoints`].
    pub lines: Vec<PatternLine>,
    pub stats: PatternStats,
}

/// Compile a generated pattern into mission-ready waypoints + stats.
///
/// The waypoints carry the same geometry as the generator produced (AMSL
/// altitudes untouched); only optional trigger items are inserted, which is
/// why the line table is re-indexed rather than regenerated.
///
/// # Errors
///
/// [`super::error::SurveyError`] if the trigger gates push the item count
/// past the MAVLink `u16` sequence space.
pub fn compile_pattern(
    plan: &PatternPlan,
    options: &CompileOptions,
) -> Result<CompiledPattern, super::error::SurveyError> {
    // Line membership by source waypoint index (ranges are disjoint).
    let kind_of = |index: u16| -> Option<LineKind> {
        plan.lines
            .iter()
            .find(|l| index >= l.start_seq && index <= l.end_seq)
            .map(|l| l.kind)
    };

    let mut waypoints: Vec<PlannedWaypoint> = Vec::with_capacity(plan.waypoints.len());
    let mut shifts: Vec<(u16, i32)> = Vec::new(); // (source index, shift at it)
    let mut shift: i32 = 0;
    for (index, wp) in plan.waypoints.iter().enumerate() {
        let index = index as u16;
        if let MagTrigger::CameraTrigger { trigger_dist_m } = &options.mag_trigger {
            if kind_of(index) == Some(LineKind::Survey)
                && plan
                    .lines
                    .iter()
                    .any(|l| l.kind == LineKind::Survey && l.start_seq == index)
            {
                // Trigger on: distance gates per metre flown on this line.
                let mut item = PlannedWaypoint::waypoint(wp.position, 0.0);
                item.command = CMD_DO_SET_CAM_TRIGG_DIST;
                item.params = [*trigger_dist_m, 0.0, 0.0, 0.0];
                waypoints.push(item);
                shift += 1;
            }
        }
        shifts.push((index, shift));
        waypoints.push(wp.clone());
        if let MagTrigger::CameraTrigger { .. } = &options.mag_trigger {
            if kind_of(index) == Some(LineKind::Survey)
                && plan
                    .lines
                    .iter()
                    .any(|l| l.kind == LineKind::Survey && l.end_seq == index)
            {
                // Trigger off for the transit to the next line.
                let mut item = PlannedWaypoint::waypoint(wp.position, 0.0);
                item.command = CMD_DO_SET_CAM_TRIGG_DIST;
                item.params = [0.0, 0.0, 0.0, 0.0];
                waypoints.push(item);
                shift += 1;
            }
        }
    }
    if waypoints.len() > MAX_PATTERN_ITEMS {
        return Err(super::error::SurveyError::TooManyWaypoints(
            waypoints.len(),
            MAX_PATTERN_ITEMS,
        ));
    }

    let lines = plan
        .lines
        .iter()
        .map(|line| {
            // Shift is constant across a line: triggers only precede a start
            // or follow an end, never inside a range.
            let shift_at_start = shifts
                .iter()
                .find(|(idx, _)| *idx == line.start_seq)
                .map(|(_, s)| *s)
                .unwrap_or(0);
            PatternLine {
                id: line.id,
                kind: line.kind,
                start_seq: (i32::from(line.start_seq) + shift_at_start) as u16,
                end_seq: (i32::from(line.end_seq) + shift_at_start) as u16,
                length_m: line.length_m,
            }
        })
        .collect();

    let stats = compute_stats(plan, options);
    Ok(CompiledPattern {
        waypoints,
        lines,
        stats,
    })
}

/// Path length, coverage and ETA (S2). Lengths come from the line table for
/// the in-line parts; transitions (lead-in/out + line changes) are the
/// remainder of the whole flown path.
fn compute_stats(plan: &PatternPlan, options: &CompileOptions) -> PatternStats {
    // Whole flown path over the located waypoints (the same walk the FC
    // flies; command items carry no coordinate and are skipped).
    let projection = LocalProjection::new(GeoPoint::new(0.0, 0.0));
    let local: Vec<(f64, f64)> = plan
        .waypoints
        .iter()
        .map(|wp| projection.to_local(wp.position))
        .collect();
    let total_length_m: f64 = local.windows(2).map(|w| dist(w[0], w[1])).sum();

    let sum = |kind: LineKind| -> f64 {
        plan.lines
            .iter()
            .filter(|l| l.kind == kind)
            .map(|l| l.length_m)
            .sum()
    };
    let speed = options
        .cruise_speed_mps
        .unwrap_or(DEFAULT_SURVEY_SPEED_MPS)
        .max(f32::EPSILON) as f64;

    PatternStats {
        survey_length_m: sum(LineKind::Survey),
        tie_length_m: sum(LineKind::Tie),
        calibration_length_m: sum(LineKind::Calibration),
        total_length_m,
        line_count: plan.lines.len() as u32,
        estimated_time_s: total_length_m / speed,
        coverage_m2: options.coverage_polygon.as_deref().map(shoelace_m2),
    }
}

/// Local-metre distance between two projected points.
fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

/// Shoelace area in m² for a simple polygon given in local ENU metres.
fn shoelace_m2(polygon: &[GeoPoint]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    // Project around the first vertex so the metre values are exact enough
    // for survey-sized areas (the projection is local, so no distortion).
    let origin = polygon[0];
    let projection = LocalProjection::new(origin);
    let pts: Vec<(f64, f64)> = polygon.iter().map(|p| projection.to_local(*p)).collect();
    let mut area = 0.0;
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        area += pts[i].0 * pts[j].1 - pts[j].0 * pts[i].1;
    }
    (area / 2.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::Height;

    /// A two-line pattern: one survey line (3 points), one tie line (2
    /// points), hand-built so the expectations are exact.
    fn sample_plan() -> PatternPlan {
        let amsl = Height::new(crate::height::HeightDatum::AmslEgm96, 100.0);
        let wp = |lat: f64, lon: f64| {
            let mut w = PlannedWaypoint::waypoint(GeoPoint::new(lat, lon), 0.0);
            w.altitude = amsl;
            w
        };
        // ~111 m per degree of latitude; keep the geometry axis-aligned.
        let waypoints = vec![
            wp(0.0000, 0.0000), // survey line 1: 0,1,2 (N→S along lon 0)
            wp(0.0000, 0.0010),
            wp(0.0000, 0.0020),
            wp(0.0005, 0.0020), // tie line: 3,4 (E→W)
            wp(0.0005, 0.0000),
        ];
        let lines = vec![
            PatternLine {
                id: 1,
                kind: LineKind::Survey,
                start_seq: 0,
                end_seq: 2,
                length_m: 222.6,
            },
            PatternLine {
                id: 1,
                kind: LineKind::Tie,
                start_seq: 3,
                end_seq: 4,
                length_m: 111.3,
            },
        ];
        PatternPlan { waypoints, lines }
    }

    #[test]
    fn gcs_side_keeps_waypoints_and_indices() {
        let compiled =
            compile_pattern(&sample_plan(), &CompileOptions::default()).expect("compile");
        assert_eq!(compiled.waypoints.len(), 5);
        assert_eq!(compiled.lines[0].start_seq, 0);
        assert_eq!(compiled.lines[0].end_seq, 2);
        assert_eq!(compiled.lines[1].start_seq, 3);
        assert!(
            compiled.waypoints.iter().all(|wp| wp.command == 16),
            "no trigger items"
        );
    }

    #[test]
    fn camera_trigger_gates_reindex_the_line_table() {
        let options = CompileOptions {
            mag_trigger: MagTrigger::CameraTrigger {
                trigger_dist_m: 5.0,
            },
            ..Default::default()
        };
        let compiled = compile_pattern(&sample_plan(), &options).expect("compile");
        // +1 gate before the survey start, +1 after its end; the tie line is
        // ungated.
        assert_eq!(compiled.waypoints.len(), 7);
        let trig = |i: usize| compiled.waypoints[i].command;
        assert_eq!(trig(0), CMD_DO_SET_CAM_TRIGG_DIST, "on-gate first");
        assert_eq!(compiled.waypoints[0].params[0], 5.0);
        assert_eq!(trig(1), 16); // survey start
        assert_eq!(trig(3), 16); // survey end
        assert_eq!(trig(4), CMD_DO_SET_CAM_TRIGG_DIST, "off-gate");
        assert_eq!(compiled.waypoints[4].params[0], 0.0);
        assert_eq!(trig(5), 16); // tie line untouched

        // Line table shifted by exactly one for the survey line, two for the
        // tie line (the survey off-gate precedes it).
        assert_eq!(compiled.lines[0].start_seq, 1);
        assert_eq!(compiled.lines[0].end_seq, 3);
        assert_eq!(compiled.lines[1].start_seq, 5);
        assert_eq!(compiled.lines[1].end_seq, 6);
    }

    #[test]
    fn stats_sum_line_lengths_and_transitions() {
        let polygon = vec![
            GeoPoint::new(0.0, 0.0),
            GeoPoint::new(0.001, 0.0),
            GeoPoint::new(0.001, 0.002),
            GeoPoint::new(0.0, 0.002),
        ];
        let options = CompileOptions {
            coverage_polygon: Some(polygon),
            cruise_speed_mps: Some(10.0),
            ..Default::default()
        };
        let compiled = compile_pattern(&sample_plan(), &options).expect("compile");
        let stats = &compiled.stats;
        assert_eq!(stats.line_count, 2);
        assert!(stats.survey_length_m > 0.0 && stats.tie_length_m > 0.0);
        assert!(stats.calibration_length_m < f64::EPSILON);
        // The whole flown path is at least the in-line length (transitions
        // add the 0.0005° jump between the lines).
        assert!(stats.total_length_m >= stats.survey_length_m + stats.tie_length_m);
        assert!(
            (stats.total_length_m - (stats.survey_length_m + stats.tie_length_m)) > 10.0,
            "the inter-line transition is included"
        );
        // ETA at 10 m/s.
        assert!((stats.estimated_time_s - stats.total_length_m / 10.0).abs() < 1e-9);
        // Coverage: 0.001° lat ≈ 111.3 m × 0.002° lon ≈ 222.6 m.
        let coverage = stats.coverage_m2.expect("coverage");
        assert!(
            (coverage - 111.32 * 222.64).abs() < 20.0,
            "coverage {coverage}"
        );
    }

    #[test]
    fn default_speed_is_used_when_none_given() {
        let compiled =
            compile_pattern(&sample_plan(), &CompileOptions::default()).expect("compile");
        assert!(
            (compiled.stats.estimated_time_s
                - compiled.stats.total_length_m / f64::from(DEFAULT_SURVEY_SPEED_MPS))
            .abs()
                < 1e-9
        );
    }
}
