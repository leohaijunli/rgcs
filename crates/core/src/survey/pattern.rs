//! Parameterised flight patterns (survey sweep, calibration cloverleaf).
//!
//! A pattern is a pure function of its parameters: generate it, then treat the
//! resulting waypoints as ordinary planned (AMSL) waypoints. The generators
//! live in [`super::generate`].

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::plan::{GeoPoint, PlannedWaypoint};

/// Largest waypoint count expressible as a MAVLink `u16` sequence number.
pub const MAX_PATTERN_ITEMS: usize = u16::MAX as usize;

/// What a group of pattern waypoints represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// A primary survey (sweep) line.
    Survey,
    /// A tie line, flown perpendicular to the sweep to close the grid.
    Tie,
    /// A calibration manoeuvre (cloverleaf, figure-eight, ...).
    Calibration,
}

/// One flown line: a contiguous `seq` range plus its length.
///
/// The post-flight tools segment the ULog mission-seq topic with this table, so
/// no hardware line markers are needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PatternLine {
    /// 1-based line number within its kind.
    pub id: u32,
    pub kind: LineKind,
    /// First waypoint sequence, inclusive.
    pub start_seq: u16,
    /// Last waypoint sequence, inclusive.
    pub end_seq: u16,
    pub length_m: f64,
}

/// Generated waypoints plus the line table that indexes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PatternPlan {
    pub waypoints: Vec<PlannedWaypoint>,
    pub lines: Vec<PatternLine>,
}

impl PatternPlan {
    /// Number of waypoints.
    pub fn len(&self) -> usize {
        self.waypoints.len()
    }

    /// Whether the pattern produced no waypoints.
    pub fn is_empty(&self) -> bool {
        self.waypoints.is_empty()
    }
}

/// A parallel-line survey sweep, optionally with perpendicular tie lines.
///
/// Lines are clipped to the polygon, so the flown extent is the polygon itself.
/// `alternate` flies a serpentine (each line reversed) instead of returning to
/// the start of every line; for magnetic data a unidirectional sweep can be
/// preferable, so both are supported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SurveyPattern {
    /// Survey area; must be convex and simple (MVP line clipper).
    pub polygon: Vec<GeoPoint>,
    /// Sweep direction, degrees clockwise from north.
    pub line_azimuth_deg: f64,
    /// Distance between adjacent sweep lines, metres.
    pub line_spacing_m: f64,
    /// Distance between tie lines, metres. `None` = no tie lines.
    pub tie_spacing_m: Option<f64>,
    /// Tie-line direction, degrees clockwise from north (usually perpendicular).
    pub tie_azimuth_deg: f64,
    /// Straight run added before the first waypoint of each line, metres.
    pub lead_in_m: f64,
    /// Straight run added after the last waypoint of each line, metres.
    pub lead_out_m: f64,
    /// Constant planned altitude, metres AMSL (draping is WS-D).
    pub altitude_amsl_m: f64,
    /// Serpentine (alternating line direction) when true.
    pub alternate: bool,
    /// Ground speed change emitted once at the start, if any.
    pub speed_mps: Option<f32>,
}

/// A cloverleaf (or N-petal rose) calibration manoeuvre.
///
/// Sampling a rose curve gives a smooth closed path that presents many headings
/// and roll angles to the magnetometer. Used for calibration flights.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CloverleafPattern {
    /// Centre of the manoeuvre.
    pub center: GeoPoint,
    /// Petal reach from the centre, metres.
    pub radius_m: f64,
    /// Petal count; 4 is a cloverleaf, must be even and >= 4.
    pub petals: u32,
    /// Waypoints sampled per petal.
    pub samples_per_petal: u32,
    /// Constant planned altitude, metres AMSL.
    pub altitude_amsl_m: f64,
    /// Rotation of the whole pattern, degrees clockwise from north.
    pub start_heading_deg: f64,
    /// Ground speed change emitted once at the start, if any.
    pub speed_mps: Option<f32>,
}
