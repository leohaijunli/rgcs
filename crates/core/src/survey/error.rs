//! Errors from pattern generation.

use thiserror::Error;

/// Errors produced while generating a survey or calibration pattern.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SurveyError {
    /// A polygon needs at least three vertices to enclose an area.
    #[error("polygon needs at least 3 vertices, got {0}")]
    PolygonTooFew(usize),

    /// The MVP line clipper only handles convex, simple polygons.
    #[error("polygon must be convex and simple")]
    NonConvexPolygon,

    /// A vertex is not finite or is outside the valid latitude/longitude range.
    #[error("invalid polygon vertex ({lat}, {lon})")]
    InvalidVertex { lat: f64, lon: f64 },

    /// Line or tie spacing must be a positive, finite number of metres.
    #[error("spacing must be a positive finite number of metres, got {0}")]
    InvalidSpacing(f64),

    /// An azimuth must be a finite number of degrees.
    #[error("azimuth must be finite, got {0}")]
    InvalidAzimuth(f64),

    /// Lead-in/lead-out must be finite and non-negative.
    #[error("lead-in/out must be finite and non-negative, got {0}")]
    InvalidLead(f64),

    /// The altitude or radius parameter is not usable.
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    /// The pattern is too large to express as `u16` mission sequences.
    #[error("pattern has {0} waypoints, above the {1} item limit")]
    TooManyWaypoints(usize, usize),
}
