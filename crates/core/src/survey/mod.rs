//! Survey and calibration pattern generation (WS-C).
//!
//! Preset, parameterised trajectories so the planning view can drop a complete
//! flight plan instead of hand-placing waypoints:
//!
//! - [`SurveyPattern`] — a parallel-line sweep, optionally with perpendicular
//!   tie lines, clipped to a convex polygon, with lead-in/lead-out and an
//!   alternating (serpentine) option.
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
    fn non_convex_polygon_is_rejected() {
        use crate::plan::GeoPoint;
        let dart = vec![
            GeoPoint::new(48.0, -123.0),
            GeoPoint::new(48.01, -123.0),
            GeoPoint::new(48.005, -123.005), // inward notch -> reflex
            GeoPoint::new(48.01, -123.01),
            GeoPoint::new(48.0, -123.01),
        ];
        let mut pattern = sweep(false, None);
        pattern.polygon = dart;
        assert_eq!(
            pattern.generate().unwrap_err(),
            SurveyError::NonConvexPolygon
        );
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
