//! Calibration-parameter application (plan §9, A4a): the GCS applies
//! loaded JSON parameters — it does not calibrate (operator decision
//! 2026-10-10).

pub mod mag;

pub use mag::{CalibrationDomain, MagCalibration, MagCalibrationError};
