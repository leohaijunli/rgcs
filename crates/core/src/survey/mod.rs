//! Survey and calibration pattern generation (WS-C).
//!
//! Preset, parameterised trajectories so the planning view can drop a complete
//! flight plan instead of hand-placing waypoints:
//!
//! - [`SurveyPattern`] — a parallel-line sweep, optionally with perpendicular
//!   tie lines, clipped to a simple polygon (convex or concave), with
//!   lead-in/lead-out and an alternating (serpentine) option.
//! - [`CloverleafPattern`] — an N-petal rose for calibration flights.
//!
//! Generators are pure geometry and return [`PatternPlan`] (planned AMSL
//! waypoints + a `seq -> line` table for post-flight segmentation). Draping to
//! terrain and UTM-accurate projections land in WS-D; the projection seam is
//! [`projection::LocalProjection`].

pub mod error;
pub mod generate;
pub mod pattern;
pub mod projection;

pub use error::SurveyError;
pub use pattern::{
    CloverleafPattern, LineKind, PatternLine, PatternPlan, SurveyPattern, MAX_PATTERN_ITEMS,
};
pub use projection::LocalProjection;

#[cfg(test)]
mod tests {
    use super::*;

    /// A ~1.1 km square near 48 N used by the geometry tests.
    fn square() -> Vec<crate::plan::GeoPoint> {
        use crate::plan::GeoPoint;
        let dlat = 0.01;
        let dlon = 0.01 / 48f64.to_radians().cos();
        vec![
            GeoPoint::new(48.0, -123.0),
            GeoPoint::new(48.0 + dlat, -123.0),
            GeoPoint::new(48.0 + dlat, -123.0 + dlon),
            GeoPoint::new(48.0, -123.0 + dlon),
        ]
    }

    fn sweep(alternate: bool, tie: Option<f64>) -> SurveyPattern {
        SurveyPattern {
            polygon: square(),
            line_azimuth_deg: 0.0,
            line_spacing_m: 200.0,
            tie_spacing_m: tie,
            tie_azimuth_deg: 90.0,
            lead_in_m: 0.0,
            lead_out_m: 0.0,
            altitude_amsl_m: 150.0,
            alternate,
            speed_mps: None,
        }
    }

    #[test]
    fn sweep_clips_lines_to_the_polygon() {
        let pattern = sweep(false, None).generate().expect("generate");
        // ~1113 m across / 200 m spacing -> 6 lines, 2 waypoints each.
        assert_eq!(pattern.waypoints.len(), 12, "waypoints");
        assert_eq!(pattern.lines.len(), 6, "lines");
        for line in &pattern.lines {
            assert_eq!(line.kind, LineKind::Survey);
            assert!(
                (line.length_m - 1113.0).abs() < 5.0,
                "length {}",
                line.length_m
            );
        }
        // Line table indexes the waypoints in order.
        assert_eq!(pattern.lines[0].start_seq, 0);
        assert_eq!(pattern.lines[0].end_seq, 1);
        assert_eq!(pattern.lines[5].start_seq, 10);
    }

    #[test]
    fn sweep_adds_perpendicular_tie_lines() {
        let pattern = sweep(true, Some(400.0)).generate().expect("generate");
        let survey = pattern
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Survey)
            .count();
        let ties = pattern
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Tie)
            .count();
        assert_eq!(survey, 6, "survey lines");
        assert_eq!(ties, 3, "tie lines"); // 1113 / 400 -> offsets 0,400,800
        assert_eq!(pattern.waypoints.len(), (survey + ties) * 2);
    }

    #[test]
    fn alternate_serpentine_reverses_odd_lines() {
        let straight = sweep(false, None).generate().expect("straight");
        let serpentine = sweep(true, None).generate().expect("serpentine");
        // Same geometry, but the second line is flown the other way round.
        assert_eq!(
            straight.waypoints[2].position,
            serpentine.waypoints[3].position
        );
        assert_eq!(
            straight.waypoints[3].position,
            serpentine.waypoints[2].position
        );
    }

    #[test]
    fn speed_change_is_prepended_and_shifts_the_line_table() {
        let mut pattern = sweep(false, None);
        pattern.speed_mps = Some(5.0);
        let plan = pattern.generate().expect("generate");
        assert_eq!(plan.waypoints.len(), 13);
        assert_eq!(plan.waypoints[0].command, 178, "DO_CHANGE_SPEED");
        assert_eq!(plan.waypoints[0].params[1], 5.0);
        assert_eq!(plan.lines[0].start_seq, 1);
        assert_eq!(plan.lines[0].end_seq, 2);
    }

    #[test]
    fn concave_polygon_now_generates() {
        use crate::plan::GeoPoint;
        let dart = vec![
            GeoPoint::new(48.0, -123.0),
            GeoPoint::new(48.01, -123.0),
            GeoPoint::new(48.005, -123.005), // inward notch -> reflex vertex
            GeoPoint::new(48.01, -123.01),
            GeoPoint::new(48.0, -123.01),
        ];
        let mut pattern = sweep(false, None);
        pattern.polygon = dart;
        // B1: non-convex polygons are no longer rejected — the clipper splits
        // each crossing line at the reflex vertex.
        let plan = pattern.generate().expect("concave polygons generate");
        assert!(plan.waypoints.len() >= 4, "waypoints");
        assert!(plan.lines.len() >= 2, "lines");
    }

    #[test]
    fn self_intersecting_polygon_is_rejected() {
        use crate::plan::GeoPoint;
        // A bowtie: edges 0-1 and 2-3 cross.
        let bowtie = vec![
            GeoPoint::new(48.0, -123.0),
            GeoPoint::new(48.01, -123.01),
            GeoPoint::new(48.0, -123.01),
            GeoPoint::new(48.01, -123.0),
        ];
        let mut pattern = sweep(false, None);
        pattern.polygon = bowtie;
        assert_eq!(
            pattern.generate().unwrap_err(),
            SurveyError::SelfIntersectingPolygon
        );
    }

    #[test]
    fn degenerate_polygon_is_rejected() {
        use crate::plan::GeoPoint;
        let collinear = vec![
            GeoPoint::new(48.0, -123.0),
            GeoPoint::new(48.01, -123.0),
            GeoPoint::new(48.02, -123.0),
        ];
        let mut pattern = sweep(false, None);
        pattern.polygon = collinear;
        assert_eq!(
            pattern.generate().unwrap_err(),
            SurveyError::DegeneratePolygon
        );
    }

    /// Build a polygon from local (east, north) metre offsets around `origin`,
    /// so the golden geometry tests are written in metres.
    fn local_polygon(
        origin: crate::plan::GeoPoint,
        offsets: &[(f64, f64)],
    ) -> Vec<crate::plan::GeoPoint> {
        let projection = LocalProjection::new(origin);
        offsets
            .iter()
            .map(|&(e, n)| projection.to_geo(e, n))
            .collect()
    }

    #[test]
    fn u_shape_vertical_sweep_skips_the_notch() {
        // A 600x600 m square with a 200..400 m, 200..600 m notch cut from the
        // top. Vertical lines at x = 0, 250, 500: x = 0 lies on the boundary
        // (the left edge, flown), x = 250 sits in the notch (only the bottom
        // strip), x = 500 spans the full height of the right wing.
        let u = [
            (0.0, 0.0),
            (600.0, 0.0),
            (600.0, 600.0),
            (400.0, 600.0),
            (400.0, 200.0),
            (200.0, 200.0),
            (200.0, 600.0),
            (0.0, 600.0),
        ];
        let pattern = SurveyPattern {
            polygon: local_polygon(crate::plan::GeoPoint::new(48.0, -123.0), &u),
            line_azimuth_deg: 0.0,
            line_spacing_m: 250.0,
            tie_spacing_m: None,
            tie_azimuth_deg: 90.0,
            lead_in_m: 0.0,
            lead_out_m: 0.0,
            altitude_amsl_m: 150.0,
            alternate: false,
            speed_mps: None,
        };
        let plan = pattern.generate().expect("generate");
        assert_eq!(plan.waypoints.len(), 6, "three lines x two endpoints");
        assert_eq!(plan.lines.len(), 3, "lines");
        assert_eq!(plan.lines[0].start_seq, 0, "fly order");
        assert_eq!(plan.lines[1].start_seq, 2, "fly order");
        assert_eq!(plan.lines[2].start_seq, 4, "fly order");
        let mut lengths: Vec<f64> = plan.lines.iter().map(|l| l.length_m).collect();
        lengths.sort_by(f64::total_cmp);
        for (got, want) in lengths.iter().zip([200.0, 600.0, 600.0]) {
            assert!((got - want).abs() < 1.0, "length {got} vs {want}");
        }
    }

    #[test]
    fn u_shape_horizontal_sweep_splits_the_notch_line() {
        // The same U, swept horizontally (azimuth 90). The lines through the
        // notch (y = 600 and y = 350) are cut into two disjoint segments — the
        // left and right wings — each its own PatternLine; the line under the
        // notch (y = 100) is one full-width segment.
        let u = [
            (0.0, 0.0),
            (600.0, 0.0),
            (600.0, 600.0),
            (400.0, 600.0),
            (400.0, 200.0),
            (200.0, 200.0),
            (200.0, 600.0),
            (0.0, 600.0),
        ];
        let pattern = SurveyPattern {
            polygon: local_polygon(crate::plan::GeoPoint::new(48.0, -123.0), &u),
            line_azimuth_deg: 90.0,
            line_spacing_m: 250.0,
            tie_spacing_m: None,
            tie_azimuth_deg: 0.0,
            lead_in_m: 0.0,
            lead_out_m: 0.0,
            altitude_amsl_m: 150.0,
            alternate: false,
            speed_mps: None,
        };
        let plan = pattern.generate().expect("generate");
        assert_eq!(plan.waypoints.len(), 10, "five lines x two endpoints");
        assert_eq!(plan.lines.len(), 5, "the notch lines split into wings");
        let mut lengths: Vec<f64> = plan.lines.iter().map(|l| l.length_m).collect();
        lengths.sort_by(f64::total_cmp);
        for (got, want) in lengths.iter().zip([200.0, 200.0, 200.0, 200.0, 600.0]) {
            assert!((got - want).abs() < 1.0, "length {got} vs {want}");
        }
    }

    #[test]
    fn invalid_spacing_and_azimuth_are_rejected() {
        let mut pattern = sweep(false, None);
        pattern.line_spacing_m = 0.0;
        assert!(matches!(
            pattern.generate().unwrap_err(),
            SurveyError::InvalidSpacing(_)
        ));
        let mut pattern = sweep(false, None);
        pattern.line_azimuth_deg = f64::NAN;
        assert!(matches!(
            pattern.generate().unwrap_err(),
            SurveyError::InvalidAzimuth(_)
        ));
    }

    #[test]
    fn cloverleaf_is_closed_and_bounded_by_the_radius() {
        let pattern = CloverleafPattern {
            center: crate::plan::GeoPoint::new(48.0, -123.0),
            radius_m: 100.0,
            petals: 4,
            samples_per_petal: 8,
            altitude_amsl_m: 120.0,
            start_heading_deg: 0.0,
            speed_mps: None,
        };
        let plan = pattern.generate().expect("generate");
        assert_eq!(
            plan.waypoints.len(),
            33,
            "4 petals x 8 samples + closing point"
        );
        assert_eq!(plan.lines.len(), 1);
        assert_eq!(plan.lines[0].kind, LineKind::Calibration);
        assert_eq!(plan.lines[0].end_seq, 32);
        assert!(plan.lines[0].length_m > 0.0);

        // Closed path: first and last coincide.
        assert_eq!(plan.waypoints[0].position, plan.waypoints[32].position);
        // Every point is within the petals' reach of the centre.
        let projection = LocalProjection::new(crate::plan::GeoPoint::new(48.0, -123.0));
        for wp in &plan.waypoints {
            let (east, north) = projection.to_local(wp.position);
            let r = (east * east + north * north).sqrt();
            assert!(r <= 100.0 + 1e-6, "r = {r}");
            assert!((wp.altitude.meters - 120.0).abs() < 1e-9, "altitude");
        }
    }

    #[test]
    fn cloverleaf_rejects_odd_petals() {
        let pattern = CloverleafPattern {
            center: crate::plan::GeoPoint::new(48.0, -123.0),
            radius_m: 100.0,
            petals: 3,
            samples_per_petal: 8,
            altitude_amsl_m: 120.0,
            start_heading_deg: 0.0,
            speed_mps: None,
        };
        assert!(matches!(
            pattern.generate().unwrap_err(),
            SurveyError::InvalidParameter(_)
        ));
    }

    #[test]
    fn local_projection_round_trips() {
        use crate::plan::GeoPoint;
        let origin = GeoPoint::new(48.5, -123.4);
        let projection = LocalProjection::new(origin);
        let p = GeoPoint::new(48.51, -123.39);
        let local = projection.to_local(p);
        let back = projection.to_geo(local.0, local.1);
        assert!((back.latitude_deg - p.latitude_deg).abs() < 1e-9);
        assert!((back.longitude_deg - p.longitude_deg).abs() < 1e-9);
    }
}
