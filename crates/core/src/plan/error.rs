//! Errors from planning-model compilation.

use thiserror::Error;

use crate::height::HeightDatum;

/// Errors produced when a [`super::PlannedMission`] is compiled to wire items.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum PlanError {
    /// A waypoint's horizontal position is out of range or not finite.
    #[error("waypoint {seq}: invalid coordinate ({lat}, {lon})")]
    InvalidCoordinate { seq: u16, lat: f64, lon: f64 },

    /// Absolute heights must be tagged AMSL (ADR-006/013).
    #[error("waypoint {seq}: planned altitude must be AMSL (EGM96), got {datum:?}")]
    WaypointNotAmsl { seq: u16, datum: HeightDatum },

    /// The HOME reference must be an absolute AMSL height.
    #[error("home altitude must be AMSL (EGM96), got {0:?}")]
    HomeNotAmsl(HeightDatum),

    /// A mission item sequence number does not fit the MAVLink `u16`.
    #[error("plan has more than {max} waypoints (item {index})")]
    TooManyItems { index: usize, max: usize },
}
