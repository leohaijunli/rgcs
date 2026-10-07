#![allow(deprecated)]

//! Mission data types (Phase 1).
//!
//! Plain-data structs that cross the Tauri boundary (ADR-002: `ts-rs`
//! generated types for the planning view).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::MissionError;

/// Coordinate frame of a mission item.
///
/// Only the integer (`*_INT`) global frames are modelled: a mission item is
/// always exchanged as `MISSION_ITEM_INT`, so keeping both the INT and
/// non-INT spelling made download→upload round trips lossy (issues.md #11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum MissionFrame {
    GlobalInt,
    GlobalRelativeAltInt,
    GlobalTerrainAltInt,
    LocalNed,
    LocalEnu,
    LocalOffsetNed,
    BodyNed,
}

impl MissionFrame {
    /// Map from the MAVLink frame enum.
    ///
    /// The non-INT global frames alias their INT counterparts; anything else
    /// is an error rather than a silent fallback (issues.md #11).
    pub fn from_mav(f: ::mavlink::common::MavFrame) -> Result<Self, MissionError> {
        use ::mavlink::common::MavFrame as F;
        match f {
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
