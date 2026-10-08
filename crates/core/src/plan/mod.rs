//! Planning model, separate from the wire model (ADR-013).
//!
//! [`PlannedMission`] stores absolute, datum-tagged AMSL heights. [`compile`]
//! turns it into the `MissionItem`s the MAVLink layer exchanges, choosing the
//! altitude frame at that point: absolute (`GLOBAL_INT`) or relative to HOME
//! (`GLOBAL_RELATIVE_ALT_INT`). The two compilations describe the same
//! geometry; only `z` and `frame` differ.
//!
//! [`compile`]: PlannedMission::compile

pub mod error;
pub mod types;

pub use error::PlanError;
pub use types::{FramePolicy, GeoPoint, PlanBlock, PlanMeta, PlannedMission, PlannedWaypoint};

use crate::height::HeightDatum;
use crate::mission::{MissionFrame, MissionItem};

/// Largest waypoint count expressible as a MAVLink `u16` sequence number.
const MAX_ITEMS: usize = u16::MAX as usize;

impl FramePolicy {
    /// The MAVLink frame this policy compiles to.
    pub fn frame(self) -> MissionFrame {
        match self {
            Self::GlobalInt => MissionFrame::GlobalInt,
            Self::GlobalRelativeAltInt => MissionFrame::GlobalRelativeAltInt,
        }
    }
}

impl PlannedMission {
    /// Compile to wire mission items under `policy`.
    ///
    /// Every waypoint must carry an AMSL altitude; the relative frame is
    /// computed from [`PlannedMission::home`]. Complex-item blocks are
    /// provenance only and do not appear here: their geometry already lives in
    /// `waypoints` (the frontend expands QGC `simpleItems` on import).
    ///
    /// # Errors
    ///
    /// Returns [`PlanError`] if HOME or a waypoint is not AMSL, a coordinate is
    /// out of range, or the plan exceeds [`MAX_ITEMS`].
    pub fn compile(&self, policy: FramePolicy) -> Result<Vec<MissionItem>, PlanError> {
        if self.home.datum() != HeightDatum::AmslEgm96 {
            return Err(PlanError::HomeNotAmsl(self.home.datum()));
        }
        if self.waypoints.len() > MAX_ITEMS {
            return Err(PlanError::TooManyItems {
                index: self.waypoints.len(),
                max: MAX_ITEMS,
            });
        }
        let home_amsl = self.home.meters();
        self.waypoints
            .iter()
            .enumerate()
            .map(|(index, wp)| self.compile_one(policy, index, wp, home_amsl))
            .collect()
    }

    fn compile_one(
        &self,
        policy: FramePolicy,
        index: usize,
        wp: &PlannedWaypoint,
        home_amsl: f64,
    ) -> Result<MissionItem, PlanError> {
        let seq = index as u16;
        if !wp.position.is_valid() {
            return Err(PlanError::InvalidCoordinate {
                seq,
                lat: wp.position.latitude_deg,
                lon: wp.position.longitude_deg,
            });
        }
        if wp.altitude.datum() != HeightDatum::AmslEgm96 {
            return Err(PlanError::WaypointNotAmsl {
                seq,
                datum: wp.altitude.datum(),
            });
        }
        let z = match policy {
            FramePolicy::GlobalInt => wp.altitude.meters(),
            FramePolicy::GlobalRelativeAltInt => wp.altitude.meters() - home_amsl,
        };
        Ok(MissionItem {
            seq,
            frame: policy.frame(),
            command: wp.command,
            params: wp.params.to_vec(),
            x: (wp.position.latitude_deg * 1e7).round() as i32,
            y: (wp.position.longitude_deg * 1e7).round() as i32,
            z: z as f32,
            autocontinue: wp.autocontinue,
            current: index == 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::Height;

    fn sample() -> PlannedMission {
        let mut mission = PlannedMission::new(100.0);
        mission.waypoints.push(PlannedWaypoint::waypoint(
            GeoPoint::new(48.6493, -123.3982),
            130.0,
        ));
        mission.waypoints.push(PlannedWaypoint::waypoint(
            GeoPoint::new(48.6500, -123.3970),
            180.0,
        ));
        mission
    }

    #[test]
    fn absolute_and_relative_compiles_have_identical_geometry() {
        let mission = sample();
        let absolute = mission.compile(FramePolicy::GlobalInt).expect("abs");
        let relative = mission
            .compile(FramePolicy::GlobalRelativeAltInt)
            .expect("rel");

        assert_eq!(absolute.len(), relative.len());
        for (a, r) in absolute.iter().zip(relative.iter()) {
            assert_eq!(a.x, r.x, "lat must not depend on the frame");
            assert_eq!(a.y, r.y, "lon must not depend on the frame");
            assert_eq!(a.seq, r.seq);
            assert_eq!(a.frame, MissionFrame::GlobalInt);
            assert_eq!(r.frame, MissionFrame::GlobalRelativeAltInt);
            // Relative altitude = AMSL - home, exactly (home = 100 m).
            assert!((a.z - r.z - 100.0).abs() < 1e-4, "z={} vs {}", a.z, r.z);
        }
        assert_eq!(absolute[0].z, 130.0);
        assert_eq!(absolute[1].z, 180.0);
        assert!((relative[0].z - 30.0).abs() < 1e-4);
        assert!((relative[1].z - 80.0).abs() < 1e-4);
    }

    #[test]
    fn only_the_first_item_is_current() {
        let items = sample().compile(FramePolicy::GlobalInt).expect("compile");
        assert!(items[0].current);
        assert!(!items[1].current);
        assert_eq!(items[0].seq, 0);
        assert_eq!(items[1].seq, 1);
    }

    #[test]
    fn rejects_non_amsl_home() {
        let mut mission = sample();
        mission.home = Height::new(HeightDatum::EllipsoidWgs84, 100.0);
        let err = mission.compile(FramePolicy::GlobalInt).unwrap_err();
        assert_eq!(err, PlanError::HomeNotAmsl(HeightDatum::EllipsoidWgs84));
    }

    #[test]
    fn rejects_non_amsl_waypoint() {
        let mut mission = sample();
        mission.waypoints[1].altitude = Height::new(HeightDatum::OrthometricCgvd2013, 180.0);
        let err = mission.compile(FramePolicy::GlobalInt).unwrap_err();
        assert_eq!(
            err,
            PlanError::WaypointNotAmsl {
                seq: 1,
                datum: HeightDatum::OrthometricCgvd2013,
            }
        );
    }

    #[test]
    fn rejects_out_of_range_coordinate() {
        let mut mission = sample();
        mission.waypoints[0].position = GeoPoint::new(91.0, -123.0);
        let err = mission.compile(FramePolicy::GlobalInt).unwrap_err();
        assert!(matches!(err, PlanError::InvalidCoordinate { seq: 0, .. }));
    }

    #[test]
    fn blocks_and_meta_are_carried_through() {
        let mut mission = sample();
        mission.blocks.push(PlanBlock {
            kind: "Survey".into(),
            raw_json: r#"{"type":"Survey","gridSpacing":20}"#.into(),
        });
        mission.meta.name = "Test survey".into();
        assert_eq!(mission.blocks.len(), 1);
        assert_eq!(mission.meta.name, "Test survey");
        // Blocks never appear on the wire.
        assert_eq!(mission.compile(FramePolicy::GlobalInt).unwrap().len(), 2);
    }

    #[test]
    fn frame_policy_maps_to_mission_frame() {
        assert_eq!(FramePolicy::GlobalInt.frame(), MissionFrame::GlobalInt);
        assert_eq!(
            FramePolicy::GlobalRelativeAltInt.frame(),
            MissionFrame::GlobalRelativeAltInt
        );
    }
}
