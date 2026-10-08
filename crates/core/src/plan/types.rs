//! Planning-side mission model (ADR-013).
//!
//! A planned mission is stored in absolute AMSL heights
//! ([`HeightDatum::AmslEgm96`]) and compiled to wire `MissionItem`s on demand.
//! That keeps "what the survey should fly" separate from "what MAVLink carries":
//! the same plan compiles to an absolute (`GLOBAL_INT`) or home-relative
//! (`GLOBAL_RELATIVE_ALT_INT`) frame without changing its geometry. We never
//! emit `GLOBAL_TERRAIN_ALT`: ADR-005 puts terrain following in the ground
//! station, not the vehicle.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::height::{Height, HeightDatum};

/// Horizontal position in WGS84 degrees.
///
/// Height is deliberately absent: an absolute height is always
/// [`crate::height::Height`], carried by the waypoint's `altitude`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GeoPoint {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
}

impl GeoPoint {
    /// Create a point from WGS84 degrees.
    pub fn new(latitude_deg: f64, longitude_deg: f64) -> Self {
        Self {
            latitude_deg,
            longitude_deg,
        }
    }

    /// Whether the point is finite and inside the usual lat/lon bounds.
    pub fn is_valid(&self) -> bool {
        self.latitude_deg.is_finite()
            && self.longitude_deg.is_finite()
            && (-90.0..=90.0).contains(&self.latitude_deg)
            && (-180.0..=180.0).contains(&self.longitude_deg)
    }
}

/// The frame a plan is compiled to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FramePolicy {
    /// `MAV_FRAME_GLOBAL_INT`: `z` is AMSL.
    GlobalInt,
    /// `MAV_FRAME_GLOBAL_RELATIVE_ALT_INT`: `z` is relative to HOME.
    GlobalRelativeAltInt,
}

/// One planned waypoint: a horizontal position plus an absolute AMSL altitude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlannedWaypoint {
    pub position: GeoPoint,
    /// Absolute altitude; must be [`HeightDatum::AmslEgm96`] to compile.
    pub altitude: Height,
    /// MAV_CMD value (e.g. 16 = `NAV_WAYPOINT`).
    pub command: u16,
    /// Command parameters P1..P4.
    pub params: [f32; 4],
    pub autocontinue: bool,
}

impl PlannedWaypoint {
    /// A `NAV_WAYPOINT` at `position` with an AMSL altitude in meters.
    pub fn waypoint(position: GeoPoint, altitude_amsl_m: f64) -> Self {
        Self {
            position,
            altitude: Height::new(HeightDatum::AmslEgm96, altitude_amsl_m),
            command: 16, // MAV_CMD_NAV_WAYPOINT
            params: [0.0; 4],
            autocontinue: true,
        }
    }
}

/// An opaque QGC complex item (Survey, Corridor Scan, ...).
///
/// The generated waypoints are compiled into [`PlannedMission::waypoints`];
/// the block itself is kept verbatim so a QGC plan survives a round trip
/// (ADR-003, issues.md #12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanBlock {
    /// QGC complex-item type, e.g. `Survey`.
    pub kind: String,
    /// Original JSON fragment, preserved byte-for-byte.
    pub raw_json: String,
}

/// Non-flying plan metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanMeta {
    pub name: String,
    pub notes: String,
}

/// A mission as planned, before it is compiled to the wire frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlannedMission {
    /// HOME reference in AMSL; the origin of the relative frame.
    pub home: Height,
    /// Planned waypoints in flyable order.
    pub waypoints: Vec<PlannedWaypoint>,
    /// Opaque QGC complex items kept for round-trip provenance.
    pub blocks: Vec<PlanBlock>,
    pub meta: PlanMeta,
}

impl PlannedMission {
    /// An empty plan with the given AMSL home altitude.
    pub fn new(home_amsl_m: f64) -> Self {
        Self {
            home: Height::new(HeightDatum::AmslEgm96, home_amsl_m),
            waypoints: Vec::new(),
            blocks: Vec::new(),
            meta: PlanMeta::default(),
        }
    }
}
