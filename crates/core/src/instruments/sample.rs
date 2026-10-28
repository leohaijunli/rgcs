//! Typed instrument samples for the walk-magnetometer survey (ADR-020).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::height::Height;
use crate::telemetry::GpsFixType;

/// Which instrument a sample came from. The walk profile uses a single
/// magnetometer and a single RTK GNSS (ADR-020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentKind {
    /// DroneCAN magnetometer, read from a MAVLink mag message.
    Magnetometer,
    /// Holybro H-RTK ZED-F9P GNSS/RTK, read from `GPS_RAW_INT`.
    Gnss,
}

/// The MAVLink message / instance a magnetometer sample was read from.
///
/// PX4 publishes the **primary** DroneCAN magnetometer on `HIGHRES_IMU` (105);
/// [`MagInstance::primary`] is the survey default (ADR-020, W0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MagInstance {
    /// MAVLink message id: `HIGHRES_IMU` (105), `RAW_IMU` (27), or one of the
    /// three `SCALED_IMU*` messages (26/116/129).
    pub message_id: u32,
    /// 1-based magnetometer instance as published by PX4.
    pub sensor: u8,
}

impl MagInstance {
    /// Build an instance from a message id and sensor index.
    pub const fn new(message_id: u32, sensor: u8) -> Self {
        Self { message_id, sensor }
    }

    /// PX4 primary magnetometer: `HIGHRES_IMU`, sensor 1. `HIGHRES_IMU` mag is
    /// **post-calibration** (operator-confirmed, ADR-020) and published at
    /// ≈[`crate::instruments::monitor::EXPECTED_MAG_RATE_HZ`].
    pub const fn primary() -> Self {
        Self {
            message_id: crate::signals::HIGHRES_IMU_ID,
            sensor: 1,
        }
    }
}

impl Default for MagInstance {
    fn default() -> Self {
        Self::primary()
    }
}

/// One 3-axis magnetometer sample. Every component and the total are in
/// **nanotesla** (ADR-020); the `HIGHRES_IMU` wire unit (gauss) is already
/// converted by the decoder.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MagSample {
    /// Flight-controller sample time in milliseconds (`time_usec / 1000`),
    /// mapped to UTC downstream (M8). Monotonic per boot; a rollback means the
    /// FC rebooted.
    pub t_ms: f64,
    /// Which message/instance this sample came from.
    pub instance: MagInstance,
    /// Field components in nanotesla.
    pub x_nt: f64,
    pub y_nt: f64,
    pub z_nt: f64,
    /// Total field `√(x² + y² + z²)` in nanotesla.
    pub total_nt: f64,
}

/// One GNSS/RTK sample. The altitude is datum-tagged (AMSL, from
/// `GPS_RAW_INT.alt`) — never a bare `f64` (ADR-006).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GnssSample {
    /// Flight-controller sample time in milliseconds (`time_usec / 1000`).
    pub t_ms: f64,
    /// Fix quality, driving RTK status (reuses the telemetry enum).
    pub fix: GpsFixType,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    /// AMSL altitude (datum-tagged per ADR-006).
    pub altitude: Height,
    /// Horizontal dilution of precision.
    pub hdop: f64,
    pub satellites_visible: u8,
    /// Ground speed in m/s (from `GPS_RAW_INT.vel`).
    pub velocity_m_s: f64,
    /// Course over ground in degrees (from `GPS_RAW_INT.cog`).
    pub course_over_ground_deg: f64,
}

/// A decoded instrument sample. The variant's `t_ms` is the FC sample time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum InstrumentSample {
    Mag(MagSample),
    Gnss(GnssSample),
}

impl InstrumentSample {
    /// Which instrument produced this sample.
    pub fn kind(&self) -> InstrumentKind {
        match self {
            Self::Mag(_) => InstrumentKind::Magnetometer,
            Self::Gnss(_) => InstrumentKind::Gnss,
        }
    }

    /// Flight-controller sample time in milliseconds.
    pub fn t_ms(&self) -> f64 {
        match self {
            Self::Mag(m) => m.t_ms,
            Self::Gnss(g) => g.t_ms,
        }
    }
}
