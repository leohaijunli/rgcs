#![allow(deprecated)]

//! Mission data types (Phase 1).
//!
//! Plain-data structs that cross the Tauri boundary (ADR-002: `ts-rs`
//! generated types for the planning view).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::MissionError;

pub const MAX_MISSION_ITEMS: usize = u16::MAX as usize;
const MISSION_PARAMETER_COUNT: usize = 4;
const MAX_LATITUDE_INT: i32 = 900_000_000;
const MAX_LONGITUDE_INT: i32 = 1_800_000_000;

/// Validate the exact wire snapshot before the protocol can narrow or encode it.
pub fn validate_upload(items: &[MissionItem]) -> Result<(), MissionError> {
    if items.is_empty() {
        return Err(MissionError::NoItems);
    }
    if items.len() > MAX_MISSION_ITEMS {
        return Err(MissionError::InvalidRequest {
            message: "mission item count exceeds the u16 wire limit".into(),
        });
    }
    for (index, item) in items.iter().enumerate() {
        if usize::from(item.seq) != index {
            return Err(MissionError::InvalidRequest {
                message: "mission sequences must be contiguous and zero-based".into(),
            });
        }
        if item.params.len() > MISSION_PARAMETER_COUNT
            || item.params.iter().any(|value| value.is_infinite())
            || !item.z.is_finite()
        {
            return Err(MissionError::InvalidRequest {
                message: "mission parameters/altitude are not encodable".into(),
            });
        }
        if <::mavlink::common::MavCmd as ::num_traits::FromPrimitive>::from_u16(item.command)
            .is_none()
        {
            return Err(MissionError::InvalidRequest {
                message: format!("unknown MAV_CMD {}", item.command),
            });
        }
        let coordinate = command_uses_coordinate(item.command);
        if coordinate == (item.frame == MissionFrame::Mission) {
            return Err(MissionError::InvalidRequest {
                message: "mission command and coordinate frame do not agree".into(),
            });
        }
        if matches!(
            item.frame,
            MissionFrame::GlobalInt
                | MissionFrame::GlobalRelativeAltInt
                | MissionFrame::GlobalTerrainAltInt
        ) && (!(-MAX_LATITUDE_INT..=MAX_LATITUDE_INT).contains(&item.x)
            || !(-MAX_LONGITUDE_INT..=MAX_LONGITUDE_INT).contains(&item.y))
        {
            return Err(MissionError::InvalidRequest {
                message: "mission position is outside WGS84 coordinate bounds".into(),
            });
        }
    }
    Ok(())
}

/// Coordinate frame of a mission item.
///
/// Only the integer (`*_INT`) global frames are modelled: a mission item is
/// always exchanged as `MISSION_ITEM_INT`, so keeping both the INT and
/// non-INT spelling made download→upload round trips lossy (issues.md #11).
///
/// [`MissionFrame::Mission`] (`MAV_FRAME_MISSION`) is not a coordinate system:
/// it marks a *command* item, whose arguments are `param1..param4` (issues.md
/// #34). Autopilots only accept a global frame for the commands listed in
/// [`command_uses_coordinate`]; a `DO_CHANGE_SPEED` sent on
/// `GLOBAL_RELATIVE_ALT_INT` is answered with `MAV_MISSION_UNSUPPORTED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum MissionFrame {
    Mission,
    GlobalInt,
    GlobalRelativeAltInt,
    GlobalTerrainAltInt,
    LocalNed,
    LocalEnu,
    LocalOffsetNed,
    BodyNed,
}

/// Whether `command` is flown as a coordinate item, or as a
/// [`MissionFrame::Mission`] command item.
///
/// Mirrors the split in PX4's `mavlink_mission.cpp::parse_mavlink_mission_item`
/// (v1.17), which is also how QGroundControl writes `.plan` files: a command
/// outside this list must be sent with `MAV_FRAME_MISSION`, or the flight
/// controller rejects the whole upload with `MAV_MISSION_UNSUPPORTED`
/// (issues.md #34). Values are `MAV_CMD_*`; keep in step with
/// `frontend/src/mission/compile.ts::commandUsesCoordinate`.
pub fn command_uses_coordinate(command: u16) -> bool {
    /// `MAV_CMD_*` values accepted with a global frame.
    const COORDINATE_COMMANDS: &[u16] = &[
        16,   // NAV_WAYPOINT
        17,   // NAV_LOITER_UNLIM
        18,   // NAV_LOITER_TURNS
        19,   // NAV_LOITER_TIME
        21,   // NAV_LAND
        22,   // NAV_TAKEOFF
        31,   // NAV_LOITER_TO_ALT
        82,   // NAV_SPLINE_WAYPOINT
        84,   // NAV_VTOL_TAKEOFF
        85,   // NAV_VTOL_LAND
        80,   // NAV_ROI
        113,  // CONDITION_GATE
        179,  // DO_SET_HOME
        195,  // DO_SET_ROI_LOCATION
        201,  // DO_SET_ROI
        400,  // COMPONENT_ARM_DISARM
        4501, // NAV_FENCE_RETURN_POINT
        5001, // NAV_FENCE_POLYGON_VERTEX_INCLUSION
        5002, // NAV_FENCE_POLYGON_VERTEX_EXCLUSION
        5003, // NAV_FENCE_CIRCLE_INCLUSION
        5004, // NAV_FENCE_CIRCLE_EXCLUSION
        5100, // NAV_RALLY_POINT
    ];
    COORDINATE_COMMANDS.contains(&command)
}

/// `MAV_CMD_DO_CHANGE_SPEED`.
pub const DO_CHANGE_SPEED: u16 = 178;

/// `MAV_CMD_DO_CHANGE_SPEED` params for PX4 v1.17.
///
/// PX4's `navigator_main.cpp` (`VEHICLE_CMD_DO_CHANGE_SPEED`) reads the speed
/// from `param2` and only applies `param3` as a throttle when it is `> 0`, so
/// the payload is `[speed type, speed m/s, throttle %, reserved]` with
/// `param1 = 1` (groundspeed; PX4 does not differentiate yet) and
/// `param3 = -1` (leave the throttle alone). Mission items and live commands
/// share this encoder so an uploaded speed change and a commanded one mean the
/// same thing.
pub fn do_change_speed_params(ground_speed_mps: f32) -> [f32; 4] {
    [1.0, ground_speed_mps, -1.0, 0.0]
}

impl MissionFrame {
    /// Map from the MAVLink frame enum.
    ///
    /// The non-INT global frames alias their INT counterparts; anything else
    /// is an error rather than a silent fallback (issues.md #11).
    pub fn from_mav(f: ::mavlink::common::MavFrame) -> Result<Self, MissionError> {
        use ::mavlink::common::MavFrame as F;
        match f {
            F::MAV_FRAME_MISSION => Ok(Self::Mission),
            F::MAV_FRAME_GLOBAL | F::MAV_FRAME_GLOBAL_INT => Ok(Self::GlobalInt),
            F::MAV_FRAME_GLOBAL_RELATIVE_ALT | F::MAV_FRAME_GLOBAL_RELATIVE_ALT_INT => {
                Ok(Self::GlobalRelativeAltInt)
            }
            F::MAV_FRAME_GLOBAL_TERRAIN_ALT | F::MAV_FRAME_GLOBAL_TERRAIN_ALT_INT => {
                Ok(Self::GlobalTerrainAltInt)
            }
            F::MAV_FRAME_LOCAL_NED => Ok(Self::LocalNed),
            F::MAV_FRAME_LOCAL_ENU => Ok(Self::LocalEnu),
            F::MAV_FRAME_LOCAL_OFFSET_NED => Ok(Self::LocalOffsetNed),
            F::MAV_FRAME_BODY_NED => Ok(Self::BodyNed),
            other => Err(MissionError::UnsupportedFrame(other as u8)),
        }
    }

    /// Map to the MAVLink frame enum.
    pub fn to_mav(self) -> ::mavlink::common::MavFrame {
        use ::mavlink::common::MavFrame as F;
        match self {
            Self::Mission => F::MAV_FRAME_MISSION,
            Self::GlobalInt => F::MAV_FRAME_GLOBAL_INT,
            Self::GlobalRelativeAltInt => F::MAV_FRAME_GLOBAL_RELATIVE_ALT_INT,
            Self::GlobalTerrainAltInt => F::MAV_FRAME_GLOBAL_TERRAIN_ALT_INT,
            Self::LocalNed => F::MAV_FRAME_LOCAL_NED,
            Self::LocalEnu => F::MAV_FRAME_LOCAL_ENU,
            Self::LocalOffsetNed => F::MAV_FRAME_LOCAL_OFFSET_NED,
            Self::BodyNed => F::MAV_FRAME_BODY_NED,
        }
    }
}

/// A single mission item (MISSION_ITEM_INT payload).
///
/// `x`/`y` are the raw MAVLink integers (lat/lon ×1e7 for global frames, local
/// meters ×1e4 for local frames); `z` is the altitude in meters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MissionItem {
    /// Waypoint sequence number, zero-based.
    pub seq: u16,
    pub frame: MissionFrame,
    /// MAV_CMD value (e.g. 16 = NAV_WAYPOINT).
    pub command: u16,
    /// Command parameters P1..P4.
    ///
    /// For an integer mission item P5/P6/P7 are the coordinate fields, which
    /// live in `x`/`y`/`z`; duplicating them here would lose precision when the
    /// `i32` lat/lon is narrowed to `f32` (issues.md #10).
    pub x: i32,
    pub y: i32,
    pub z: f32,
    pub autocontinue: bool,
    /// Whether this item is the active one (1 for the first item on upload).
    pub current: bool,
    #[serde(with = "nullable_parameters")]
    #[ts(type = "Array<number | null>")]
    pub params: Vec<f32>,
}

// MAVLink uses NaN for unspecified parameters (notably waypoint heading).
// JSON uses null for that sentinel; preserve it on download -> upload.
mod nullable_parameters {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(values: &[f32], serializer: S) -> Result<S::Ok, S::Error> {
        values
            .iter()
            .map(|value| if value.is_nan() { None } else { Some(*value) })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<f32>, D::Error> {
        Ok(Vec::<Option<f32>>::deserialize(deserializer)?
            .into_iter()
            .map(|value| value.unwrap_or(f32::NAN))
            .collect())
    }
}

impl MissionItem {
    /// Create a basic waypoint.
    pub fn waypoint(lat_deg: f64, lon_deg: f64, alt_m: f32, frame: MissionFrame) -> Self {
        Self {
            seq: 0,
            frame,
            command: 16, // MAV_CMD_NAV_WAYPOINT
            params: vec![0.0; 4],
            x: (lat_deg * 1e7) as i32,
            y: (lon_deg * 1e7) as i32,
            z: alt_m,
            autocontinue: true,
            current: false,
        }
    }
}

/// An in-memory mission.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Mission {
    pub items: Vec<MissionItem>,
    /// Currently active waypoint sequence, if any.
    pub current_seq: Option<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::mavlink::common::MavFrame;

    #[test]
    fn mission_frame_round_trips() {
        assert_eq!(
            MissionFrame::from_mav(MavFrame::MAV_FRAME_MISSION),
            Ok(MissionFrame::Mission)
        );
        assert_eq!(MissionFrame::Mission.to_mav(), MavFrame::MAV_FRAME_MISSION);
    }

    #[test]
    fn a_command_item_is_not_a_coordinate_item() {
        assert!(command_uses_coordinate(16)); // NAV_WAYPOINT
        assert!(!command_uses_coordinate(178)); // DO_CHANGE_SPEED
        assert!(!command_uses_coordinate(93)); // NAV_DELAY
        assert!(!command_uses_coordinate(20)); // NAV_RETURN_TO_LAUNCH
    }

    /// PX4 v1.17 reads the speed from `param2`, not `param1`: an item written
    /// with the speed in `param1` only changes the speed type and is a no-op
    /// (issues.md #34 neighbours, AF-17a).
    #[test]
    fn speed_change_params_put_the_speed_in_param2() {
        assert_eq!(do_change_speed_params(7.5), [1.0, 7.5, -1.0, 0.0]);
        assert_eq!(DO_CHANGE_SPEED, 178);
    }

    #[test]
    fn nullable_heading_round_trips_without_becoming_zero() {
        let mut item = MissionItem::waypoint(49.0, -123.0, 50.0, MissionFrame::GlobalInt);
        item.params[3] = f32::NAN;
        let json = serde_json::to_value(&item).expect("JSON mission");
        assert!(json["params"][3].is_null());
        let restored: MissionItem = serde_json::from_value(json).expect("null is the NaN sentinel");
        assert!(restored.params[3].is_nan());
        assert_eq!(restored.x, item.x);
        assert!(validate_upload(&[restored]).is_ok());
    }

    #[test]
    fn invalid_uploads_fail_before_narrowing_or_encoding() {
        let good = MissionItem::waypoint(49.0, -123.0, 50.0, MissionFrame::GlobalInt);
        let mut invalid = Vec::new();
        let mut item = good.clone();
        item.seq = 1;
        invalid.push(item);
        let mut item = good.clone();
        item.command = u16::MAX;
        invalid.push(item);
        let mut item = good.clone();
        item.frame = MissionFrame::Mission;
        invalid.push(item);
        let mut item = good.clone();
        item.z = f32::NAN;
        invalid.push(item);
        let mut item = good.clone();
        item.x = MAX_LATITUDE_INT + 1;
        invalid.push(item);
        let mut item = good.clone();
        item.params.push(0.0);
        invalid.push(item);
        let mut item = good.clone();
        item.params[0] = f32::INFINITY;
        invalid.push(item);
        for item in invalid {
            assert!(validate_upload(&[item]).is_err());
        }
        assert!(validate_upload(&vec![good; MAX_MISSION_ITEMS + 1]).is_err());
    }
}
