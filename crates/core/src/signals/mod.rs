//! Signal collection for the Signal Inspector (plan §4, ADR-015/016).
//!
//! The inspector cannot reuse the 20 Hz telemetry snapshot: it needs arbitrary
//! message fields at their native rate for FFT work. This module taps the raw
//! inbound MAVLink stream (`MessageRoute::all()`) and turns every message into
//! a set of numeric samples:
//!
//! - [`SignalId`] identifies one numeric field of one message type from one
//!   node (system/component ids reserve multi-vehicle use).
//! - [`extract::extract_fields`] walks a message with a custom `serde`
//!   serializer, so a new message type needs no hand-written mapping.
//! - [`catalog::SignalCatalog`] tracks the last value and arrival rate of every
//!   known `(msg, field)`.
//! - [`SampleSource`] is the source abstraction (plan decision B): live MAVLink
//!   now, a ULog replay later.
//!
//! The tap task itself (`tap::SignalTap`, P2c) subscribes to a connection route
//! with a reference count, so no work happens while the inspector is closed.

pub mod catalog;
pub mod derived;
pub mod extract;
pub mod tap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Gauss → nanotesla: `1 G = 1e5 nT`. MAVLink `HIGHRES_IMU` (105) mag fields
/// are in gauss; every MagGCS mag value (components, total, calibration,
/// alarm) is nanotesla.
pub const GAUSS_TO_NANOTESLA: f64 = 1e5;
/// Milligauss → nanotesla: `1 mG = 1e-3 G = 100 nT`.
pub const MILLIGAUSS_TO_NANOTESLA: f64 = 100.0;

/// MAVLink message ids carrying a magnetometer triple.
pub const HIGHRES_IMU_ID: u32 = 105;
pub const RAW_IMU_ID: u32 = 27;
pub const SCALED_IMU_ID: u32 = 26;
pub const SCALED_IMU2_ID: u32 = 116;
pub const SCALED_IMU3_ID: u32 = 129;

/// Normalize one magnetometer field value to nanotesla, keyed by the
/// message id it came from: `HIGHRES_IMU` mag is gauss; `RAW_IMU` and the
/// three `SCALED_IMU*` messages are milligauss. Other messages/fields pass
/// through unchanged.
pub fn normalize_mag_fields(message_id: u32, field: &str, value: f64) -> f64 {
    let factor = match message_id {
        HIGHRES_IMU_ID => GAUSS_TO_NANOTESLA,
        RAW_IMU_ID | SCALED_IMU_ID | SCALED_IMU2_ID | SCALED_IMU3_ID => MILLIGAUSS_TO_NANOTESLA,
        _ => return value,
    };
    match field {
        "xmag" | "ymag" | "zmag" | "mag_total" => value * factor,
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highres_imu_gauss_is_converted_to_nanotesla() {
        assert_eq!(normalize_mag_fields(HIGHRES_IMU_ID, "xmag", 3.0), 300_000.0);
        assert_eq!(
            normalize_mag_fields(HIGHRES_IMU_ID, "mag_total", 5.0),
            500_000.0
        );
        // Non-mag fields pass through untouched.
        assert_eq!(normalize_mag_fields(HIGHRES_IMU_ID, "xacc", 9.81), 9.81);
    }

    #[test]
    fn raw_imu_milligauss_is_converted_to_nanotesla() {
        assert_eq!(normalize_mag_fields(RAW_IMU_ID, "xmag", 250.0), 25_000.0);
    }

    #[test]
    fn every_scaled_imu_milligauss_source_is_converted_independently() {
        for (message_id, value, expected) in [
            (SCALED_IMU_ID, 100.0, 10_000.0),
            (SCALED_IMU2_ID, 200.0, 20_000.0),
            (SCALED_IMU3_ID, 300.0, 30_000.0),
        ] {
            assert_eq!(normalize_mag_fields(message_id, "xmag", value), expected);
            assert_eq!(normalize_mag_fields(message_id, "xacc", value), value);
        }
    }

    #[test]
    fn non_mag_messages_pass_through() {
        assert_eq!(normalize_mag_fields(30, "roll", 0.1), 0.1);
        assert_eq!(normalize_mag_fields(105, "temperature", 25.0), 25.0);
    }
}

/// Identifies one numeric signal: a field of a message type from one node.
///
/// Array fields are expanded to one signal per element, `field[i]`; messages
/// with a string `name` field (`NAMED_VALUE_FLOAT`, `DEBUG_*`) use the name as
/// the field (`NAMED_VALUE_FLOAT/<name>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SignalId {
    pub system_id: u8,
    pub component_id: u8,
    /// MAVLink message id.
    pub message_id: u32,
    /// Field path within the message, e.g. `"roll"` or `"accel[0]"`.
    pub field: String,
}

impl SignalId {
    pub fn new(system_id: u8, component_id: u8, message_id: u32, field: impl Into<String>) -> Self {
        Self {
            system_id,
            component_id,
            message_id,
            field: field.into(),
        }
    }
}

/// One collected numeric sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SignalSample {
    pub id: SignalId,
    /// Host-aligned time in milliseconds (see `tap::TimestampPolicy`).
    pub t_ms: f64,
    pub value: f64,
}

/// A source of samples. Live MAVLink is the first implementation; ULog replay
/// is the planned second (plan decision B).
pub trait SampleSource: Send {
    /// The current catalog of known `(msg, field)` entries.
    fn catalog(&self) -> catalog::SignalCatalog;
}
