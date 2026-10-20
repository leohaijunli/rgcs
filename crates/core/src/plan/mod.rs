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
use crate::mission::{command_uses_coordinate, MissionFrame, MissionItem};

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
    /// Commands that are not flown as a coordinate (a `DO_*`, `CONDITION_*`,
    /// `NAV_RETURN_TO_LAUNCH`, ...) compile to [`MissionFrame::Mission`]
    /// whatever `policy` says, because flight controllers only accept a global
    /// frame for the commands in [`command_uses_coordinate`] (issues.md #34).
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
        // A command item carries its arguments in `param1..param4`; its `z` is
        // a command argument, not an altitude, so it is never re-datumed
        // (DO_SET_HOME, the one command here that uses the coordinate, takes it
        // as an absolute position, matching QGroundControl).
        let coordinate = command_uses_coordinate(wp.command);
        let frame = if coordinate {
            policy.frame()
        } else {
            MissionFrame::Mission
        };
        let z = if !coordinate {
            wp.altitude.meters()
        } else {
            match policy {
                FramePolicy::GlobalInt => wp.altitude.meters(),
                FramePolicy::GlobalRelativeAltInt => wp.altitude.meters() - home_amsl,
            }
        };
        Ok(MissionItem {
            seq,
            frame,
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

    /// A command item must never carry a global frame: PX4 answers
    /// `MAV_MISSION_UNSUPPORTED` (issues.md #34).
    #[test]
    fn command_items_compile_to_the_mission_frame() {
        let mut mission = sample();
        let mut speed = PlannedWaypoint::waypoint(GeoPoint::new(48.6493, -123.3982), 0.0);
        speed.command = 178; // MAV_CMD_DO_CHANGE_SPEED
        speed.params = [1.0, 5.0, -1.0, 0.0];
        mission.waypoints.insert(0, speed);
        let mut loiter = PlannedWaypoint::waypoint(GeoPoint::new(48.65, -123.397), 180.0);
        loiter.command = 20; // MAV_CMD_NAV_RETURN_TO_LAUNCH
        mission.waypoints.push(loiter);

        for policy in [FramePolicy::GlobalInt, FramePolicy::GlobalRelativeAltInt] {
            let items = mission.compile(policy).expect("compile");
            assert_eq!(items[0].frame, MissionFrame::Mission, "speed item");
            assert_eq!(items[0].z, 0.0, "command z is not re-datumed");
            assert_eq!(items[0].command, 178);
            assert_eq!(
                items.last().expect("last").frame,
                MissionFrame::Mission,
                "rtl"
            );
            assert_eq!(items[1].frame, policy.frame(), "waypoint keeps the policy");
            let expected = match policy {
                FramePolicy::GlobalInt => 130.0,
                FramePolicy::GlobalRelativeAltInt => 30.0, // 130 - home
            };
            assert_eq!(items[1].z, expected, "{policy:?} waypoint z");
        }
    }

    #[test]
    fn coordinate_commands_are_classified_like_px4() {
        for command in [16, 21, 22, 17, 19, 82, 84, 85, 5001, 5100] {
            assert!(
                command_uses_coordinate(command),
                "{command} carries a point"
            );
        }
        for command in [20, 93, 178, 179 + 1, 112, 113 + 1, 300, 400 + 1] {
            assert!(!command_uses_coordinate(command), "{command} is a command");
        }
    }
}
