//! Normalized telemetry types exposed to the UI (ADR-002: generated via
//! `ts-rs`). These are plain-data structs that serialize to JSON over the
//! Tauri IPC / server API. Conversion from raw MAVLink messages lives in
//! `mavlink` parsing functions implemented in Phase 0 (task 2) and Phase 2
//! (RTK status).
//!
//! Field units follow the MAVLink conventions (meters, m/s, degrees).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::height::Height;

/// MAVLink scaling helpers (protocol integer → physical units).
#[allow(dead_code)] // consumed by MAVLink parse functions (next phase)
pub(crate) const fn deg_scaled_to_deg(v: i32) -> f64 {
    v as f64 / 10_000_000.0
}

#[allow(dead_code)] // consumed by MAVLink parse functions (next phase)
pub(crate) const fn mm_to_m(v: i32) -> f64 {
    v as f64 / 1000.0
}

#[allow(dead_code)] // consumed by MAVLink parse functions (next phase)
pub(crate) const fn cm_to_m(v: i32) -> f64 {
    v as f64 / 100.0
}

#[allow(dead_code)] // consumed by MAVLink parse functions (next phase)
pub(crate) const fn cdeg_to_deg(v: u16) -> f64 {
    v as f64 / 100.0
}

/// 3D velocity in local NED frame, m/s.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VelocityNed {
    pub x_m_s: f64,
    pub y_m_s: f64,
    pub z_m_s: f64,
}

/// Normalized `GLOBAL_POSITION_INT`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GlobalPositionInt {
    /// Milliseconds since boot of the sending system.
    pub time_boot_ms: u32,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    /// AMSL altitude as reported by PX4 (datum-tagged per ADR-006).
    pub altitude: Height,
    /// Height above home, meters (a delta, not a datum height).
    pub relative_alt_m: f64,
    pub velocity: VelocityNed,
    /// Yaw in degrees (0 = north, clockwise).
    pub heading_deg: f64,
}

/// Normalized `ATTITUDE`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Attitude {
    pub time_boot_ms: u32,
    pub roll_deg: f64,
    pub pitch_deg: f64,
    pub yaw_deg: f64,
    pub roll_speed_deg_s: f64,
    pub pitch_speed_deg_s: f64,
    pub yaw_speed_deg_s: f64,
}

/// Sensor present/enabled/health bitmasks from `SYS_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SensorHealthMasks {
    pub present: u32,
    pub enabled: u32,
    pub health: u32,
}

/// Normalized `SYS_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SysStatus {
    pub sensors: SensorHealthMasks,
    /// Main battery voltage in millivolts.
    pub battery_voltage_mv: u16,
    /// Main battery current in milliamps (signed).
    pub battery_current_ma: i16,
    /// Battery remaining percent (0–100).
    pub battery_remaining_percent: i8,
}

/// Normalized `BATTERY_STATUS`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatteryStatus {
    pub battery_id: u8,
    /// Per-cell voltages in millivolts.
    pub voltage_cells_mv: Vec<u16>,
    pub current_ma: i32,
    /// 0–100, `None` when unknown.
    pub remaining_percent: Option<u8>,
    /// Degrees Celsius; `None` when the FC reports unknown.
    pub temperature_deg_c: Option<i16>,
}

/// GPS fix quality, driving the RTK status bar (plan §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GpsFixType {
    NoGps,
    NoFix,
    Fix2d,
    Fix3d,
    Dgps,
    RtkFloat,
    RtkFixed,
    Static,
}

/// Normalized `GPS_RAW_INT`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GpsRawInt {
    pub fix_type: GpsFixType,
    pub satellites_visible: u8,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    /// AMSL altitude (datum-tagged per ADR-006).
    pub altitude: Height,
    pub hdop: f64,
    pub vdop: f64,
    pub velocity_m_s: f64,
    pub course_over_ground_deg: f64,
}

/// Combined telemetry snapshot published to the UI (e.g. at 10–30 Hz).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TelemetrySnapshot {
    /// Monotonic ms timestamp of the latest HEARTBEAT from the FC.
    pub last_heartbeat_at_ms: Option<u64>,
    pub global_position: Option<GlobalPositionInt>,
    pub attitude: Option<Attitude>,
    pub sys_status: Option<SysStatus>,
    pub battery: Option<BatteryStatus>,
    pub gps: Option<GpsRawInt>,
}

impl TelemetrySnapshot {
    /// Empty snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when a heartbeat was seen and at least one telemetry message
    /// arrived.
    pub fn is_alive(&self) -> bool {
        self.last_heartbeat_at_ms.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightDatum;

    #[test]
    fn scaling_constants() {
        assert!((deg_scaled_to_deg(49_000_000) - 4.9).abs() < 1e-12);
        assert!((mm_to_m(1234) - 1.234).abs() < 1e-12);
        assert!((cm_to_m(125) - 1.25).abs() < 1e-12);
        assert!((cdeg_to_deg(45) - 0.45).abs() < 1e-12);
    }

    #[test]
    fn snapshot_defaults() {
        let s = TelemetrySnapshot::new();
        assert!(!s.is_alive());
        assert!(s.global_position.is_none());
    }

    #[test]
    fn height_tagged_altitude() {
        let pos = GlobalPositionInt {
            time_boot_ms: 0,
            latitude_deg: 49.25,
            longitude_deg: -123.1,
            altitude: Height::new(HeightDatum::AmslEgm96, 100.5),
            relative_alt_m: 20.0,
            velocity: VelocityNed {
                x_m_s: 0.0,
                y_m_s: 0.0,
                z_m_s: -0.5,
            },
            heading_deg: 90.0,
        };
        assert_eq!(pos.altitude.datum(), HeightDatum::AmslEgm96);
    }
}
