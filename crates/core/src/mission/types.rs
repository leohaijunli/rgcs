#![allow(deprecated)]

//! Mission data types (Phase 1).
//!
//! Plain-data structs that cross the Tauri boundary (ADR-002: `ts-rs`
//! generated types for the planning view).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Coordinate frame of a mission item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum MissionFrame {
    Global,
    LocalNed,
    GlobalRelativeAlt,
    LocalEnu,
    GlobalInt,
    GlobalRelativeAltInt,
    LocalOffsetNed,
    BodyNed,
    GlobalTerrainAlt,
    GlobalTerrainAltInt,
}

impl MissionFrame {
    /// Map from the MAVLink frame enum.
    pub fn from_mav(f: ::mavlink::common::MavFrame) -> Self {
        use ::mavlink::common::MavFrame as F;
        match f {
            F::MAV_FRAME_GLOBAL => Self::Global,
            F::MAV_FRAME_LOCAL_NED => Self::LocalNed,
            F::MAV_FRAME_GLOBAL_RELATIVE_ALT => Self::GlobalRelativeAlt,
            F::MAV_FRAME_LOCAL_ENU => Self::LocalEnu,
            F::MAV_FRAME_GLOBAL_INT => Self::GlobalInt,
            F::MAV_FRAME_GLOBAL_RELATIVE_ALT_INT => Self::GlobalRelativeAltInt,
            F::MAV_FRAME_LOCAL_OFFSET_NED => Self::LocalOffsetNed,
            F::MAV_FRAME_BODY_NED => Self::BodyNed,
            F::MAV_FRAME_GLOBAL_TERRAIN_ALT => Self::GlobalTerrainAlt,
            F::MAV_FRAME_GLOBAL_TERRAIN_ALT_INT => Self::GlobalTerrainAltInt,
            _ => Self::Global,
        }
    }

    /// Map to the MAVLink frame enum.
    pub fn to_mav(self) -> ::mavlink::common::MavFrame {
        use ::mavlink::common::MavFrame as F;
        match self {
            Self::Global => F::MAV_FRAME_GLOBAL_INT,
            Self::LocalNed => F::MAV_FRAME_LOCAL_NED,
            Self::GlobalRelativeAlt => F::MAV_FRAME_GLOBAL_RELATIVE_ALT_INT,
            Self::LocalEnu => F::MAV_FRAME_LOCAL_ENU,
            Self::GlobalInt => F::MAV_FRAME_GLOBAL_INT,
            Self::GlobalRelativeAltInt => F::MAV_FRAME_GLOBAL_RELATIVE_ALT_INT,
            Self::LocalOffsetNed => F::MAV_FRAME_LOCAL_OFFSET_NED,
            Self::BodyNed => F::MAV_FRAME_BODY_NED,
            Self::GlobalTerrainAlt => F::MAV_FRAME_GLOBAL_TERRAIN_ALT_INT,
            Self::GlobalTerrainAltInt => F::MAV_FRAME_GLOBAL_TERRAIN_ALT_INT,
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
    /// Command parameters P1..P7.
    pub params: Vec<f32>,
    pub x: i32,
    pub y: i32,
    pub z: f32,
    pub autocontinue: bool,
    /// Whether this item is the active one (1 for the first item on upload).
    pub current: bool,
}

impl MissionItem {
    /// Create a basic waypoint.
    pub fn waypoint(lat_deg: f64, lon_deg: f64, alt_m: f32, frame: MissionFrame) -> Self {
        Self {
            seq: 0,
            frame,
            command: 16, // MAV_CMD_NAV_WAYPOINT
            params: vec![0.0; 7],
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