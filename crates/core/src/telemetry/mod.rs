//! Normalized telemetry types exposed to the UI (ADR-002: generated via
//! `ts-rs`). These are plain-data structs that serialize to JSON over the
//! Tauri IPC / server API. Conversion from raw MAVLink messages lives in
//! `mavlink` parsing functions implemented in Phase 0 (task 2) and Phase 2
//! (RTK status).
//!
//! Field units follow the MAVLink conventions (meters, m/s, degrees).

pub mod hub;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::height::{Height, HeightDatum};
use crate::mavlink::message::{MavMessage, MessageEnvelope};

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
    /// Main battery current in milliamps (converted from centiamps).
    pub battery_current_ma: i32,
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
    Ppp,
}

impl From<::mavlink::common::GpsFixType> for GpsFixType {
    fn from(fix: ::mavlink::common::GpsFixType) -> Self {
        match fix {
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_NO_GPS => Self::NoGps,
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_NO_FIX => Self::NoFix,
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_2D_FIX => Self::Fix2d,
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_3D_FIX => Self::Fix3d,
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_DGPS => Self::Dgps,
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_RTK_FLOAT => Self::RtkFloat,
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_RTK_FIXED => Self::RtkFixed,
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_STATIC => Self::Static,
            ::mavlink::common::GpsFixType::GPS_FIX_TYPE_PPP => Self::Ppp,
        }
    }
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

/// Vehicle/component type from `HEARTBEAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum VehicleType {
    Generic,
    FixedWing,
    Quadrotor,
    Coaxial,
    Helicopter,
    Tricopter,
    Hexarotor,
    Octorotor,
    Dodecarotor,
    GenericMultirotor,
    GroundRover,
    SurfaceBoat,
    Submarine,
    VtolTailsitterDuorotor,
    VtolTailsitterQuadrotor,
    VtolTiltrotor,
    VtolFixedrotor,
    VtolTailsitter,
    VtolTiltwing,
    Gcs,
    AntennaTracker,
    OnboardController,
    Gimbal,
    Camera,
    Other,
    Unknown,
}

impl From<::mavlink::common::MavType> for VehicleType {
    fn from(t: ::mavlink::common::MavType) -> Self {
        use ::mavlink::common::MavType as M;
        match t {
            M::MAV_TYPE_FIXED_WING => Self::FixedWing,
            M::MAV_TYPE_QUADROTOR => Self::Quadrotor,
            M::MAV_TYPE_COAXIAL => Self::Coaxial,
            M::MAV_TYPE_HELICOPTER => Self::Helicopter,
            M::MAV_TYPE_TRICOPTER => Self::Tricopter,
            M::MAV_TYPE_HEXAROTOR => Self::Hexarotor,
            M::MAV_TYPE_OCTOROTOR => Self::Octorotor,
            M::MAV_TYPE_DODECAROTOR => Self::Dodecarotor,
            M::MAV_TYPE_GENERIC_MULTIROTOR => Self::GenericMultirotor,
            M::MAV_TYPE_GROUND_ROVER => Self::GroundRover,
            M::MAV_TYPE_SURFACE_BOAT => Self::SurfaceBoat,
            M::MAV_TYPE_SUBMARINE => Self::Submarine,
            M::MAV_TYPE_VTOL_TAILSITTER_DUOROTOR => Self::VtolTailsitterDuorotor,
            M::MAV_TYPE_VTOL_TAILSITTER_QUADROTOR => Self::VtolTailsitterQuadrotor,
            M::MAV_TYPE_VTOL_TILTROTOR => Self::VtolTiltrotor,
            M::MAV_TYPE_VTOL_FIXEDROTOR => Self::VtolFixedrotor,
            M::MAV_TYPE_VTOL_TAILSITTER => Self::VtolTailsitter,
            M::MAV_TYPE_VTOL_TILTWING => Self::VtolTiltwing,
            M::MAV_TYPE_GCS => Self::Gcs,
            M::MAV_TYPE_ANTENNA_TRACKER => Self::AntennaTracker,
            M::MAV_TYPE_ONBOARD_CONTROLLER => Self::OnboardController,
            M::MAV_TYPE_GIMBAL => Self::Gimbal,
            M::MAV_TYPE_CAMERA => Self::Camera,
            M::MAV_TYPE_GENERIC => Self::Generic,
            _ => Self::Other,
        }
    }
}

/// Autopilot type from `HEARTBEAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Autopilot {
    Generic,
    ArduPilot,
    Px4,
    OpenPilot,
    AutoQuad,
    Smartap,
    GenericWaypointsOnly,
    GenericWaypointsAndSimpleNavigationOnly,
    GenericMissionFull,
    Invalid,
    Other,
    Unknown,
}

impl From<::mavlink::common::MavAutopilot> for Autopilot {
    fn from(a: ::mavlink::common::MavAutopilot) -> Self {
        use ::mavlink::common::MavAutopilot as M;
        match a {
            M::MAV_AUTOPILOT_ARDUPILOTMEGA => Self::ArduPilot,
            M::MAV_AUTOPILOT_PX4 => Self::Px4,
            M::MAV_AUTOPILOT_OPENPILOT => Self::OpenPilot,
            M::MAV_AUTOPILOT_AUTOQUAD => Self::AutoQuad,
            M::MAV_AUTOPILOT_SMARTAP => Self::Smartap,
            M::MAV_AUTOPILOT_GENERIC_WAYPOINTS_ONLY => Self::GenericWaypointsOnly,
            M::MAV_AUTOPILOT_GENERIC_WAYPOINTS_AND_SIMPLE_NAVIGATION_ONLY => {
                Self::GenericWaypointsAndSimpleNavigationOnly
            }
            M::MAV_AUTOPILOT_GENERIC_MISSION_FULL => Self::GenericMissionFull,
            M::MAV_AUTOPILOT_INVALID => Self::Invalid,
            M::MAV_AUTOPILOT_GENERIC => Self::Generic,
            _ => Self::Other,
        }
    }
}

/// System status flag from `HEARTBEAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FlightState {
    Uninit,
    Boot,
    Calibrating,
    Standby,
    Active,
    Critical,
    Emergency,
    Poweroff,
    FlightTermination,
    Unknown,
}

impl From<::mavlink::common::MavState> for FlightState {
    fn from(s: ::mavlink::common::MavState) -> Self {
        use ::mavlink::common::MavState as M;
        match s {
            M::MAV_STATE_UNINIT => Self::Uninit,
            M::MAV_STATE_BOOT => Self::Boot,
            M::MAV_STATE_CALIBRATING => Self::Calibrating,
            M::MAV_STATE_STANDBY => Self::Standby,
            M::MAV_STATE_ACTIVE => Self::Active,
            M::MAV_STATE_CRITICAL => Self::Critical,
            M::MAV_STATE_EMERGENCY => Self::Emergency,
            M::MAV_STATE_POWEROFF => Self::Poweroff,
            M::MAV_STATE_FLIGHT_TERMINATION => Self::FlightTermination,
        }
    }
}

/// Decoded `MAV_MODE_FLAG` bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BaseMode {
    pub custom_mode_enabled: bool,
    pub test_enabled: bool,
    pub auto_enabled: bool,
    pub guided_enabled: bool,
    pub stabilize_enabled: bool,
    pub hil_enabled: bool,
    pub manual_input_enabled: bool,
    pub safety_armed: bool,
}

impl BaseMode {
    /// Bit positions per MAVLink `MAV_MODE_FLAG`.
    pub const BIT_CUSTOM_MODE: u8 = 1;
    pub const BIT_TEST: u8 = 2;
    pub const BIT_AUTO: u8 = 4;
    pub const BIT_GUIDED: u8 = 8;
    pub const BIT_STABILIZE: u8 = 16;
    pub const BIT_HIL: u8 = 32;
    pub const BIT_MANUAL_INPUT: u8 = 64;
    pub const BIT_SAFETY_ARMED: u8 = 128;

    /// Decode a raw mode byte.
    pub fn from_raw(raw: u8) -> Self {
        Self {
            custom_mode_enabled: raw & Self::BIT_CUSTOM_MODE != 0,
            test_enabled: raw & Self::BIT_TEST != 0,
            auto_enabled: raw & Self::BIT_AUTO != 0,
            guided_enabled: raw & Self::BIT_GUIDED != 0,
            stabilize_enabled: raw & Self::BIT_STABILIZE != 0,
            hil_enabled: raw & Self::BIT_HIL != 0,
            manual_input_enabled: raw & Self::BIT_MANUAL_INPUT != 0,
            safety_armed: raw & Self::BIT_SAFETY_ARMED != 0,
        }
    }

    /// Encode back to a raw mode byte.
    pub fn raw(&self) -> u8 {
        let mut raw = 0u8;
        if self.custom_mode_enabled {
            raw |= Self::BIT_CUSTOM_MODE;
        }
        if self.test_enabled {
            raw |= Self::BIT_TEST;
        }
        if self.auto_enabled {
            raw |= Self::BIT_AUTO;
        }
        if self.guided_enabled {
            raw |= Self::BIT_GUIDED;
        }
        if self.stabilize_enabled {
            raw |= Self::BIT_STABILIZE;
        }
        if self.hil_enabled {
            raw |= Self::BIT_HIL;
        }
        if self.manual_input_enabled {
            raw |= Self::BIT_MANUAL_INPUT;
        }
        if self.safety_armed {
            raw |= Self::BIT_SAFETY_ARMED;
        }
        raw
    }

    /// Motors enabled / ready to fly.
    pub fn is_armed(&self) -> bool {
        self.safety_armed
    }
}

/// Normalized `HEARTBEAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Heartbeat {
    pub system_id: u8,
    pub component_id: u8,
    pub vehicle_type: VehicleType,
    pub autopilot: Autopilot,
    pub base_mode: BaseMode,
    pub custom_mode: u32,
    pub flight_state: FlightState,
    pub mavlink_version: u8,
}

/// Error raised when a MAVLink message does not carry the expected payload.
#[derive(Debug, Error)]
pub enum TelemetryParseError {
    #[error("expected message {expected}, got a different MAVLink message")]
    WrongMessageType { expected: &'static str },
}

impl TryFrom<&MessageEnvelope> for Heartbeat {
    type Error = TelemetryParseError;

    fn try_from(env: &MessageEnvelope) -> Result<Self, Self::Error> {
        match &env.message {
            MavMessage::HEARTBEAT(hb) => Ok(Heartbeat {
                system_id: env.header.system_id,
                component_id: env.header.component_id,
                vehicle_type: VehicleType::from(hb.mavtype),
                autopilot: Autopilot::from(hb.autopilot),
                base_mode: BaseMode::from_raw(hb.base_mode.bits()),
                custom_mode: hb.custom_mode,
                flight_state: FlightState::from(hb.system_status),
                mavlink_version: hb.mavlink_version,
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "HEARTBEAT",
            }),
        }
    }
}

impl TryFrom<&MavMessage> for GlobalPositionInt {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::GLOBAL_POSITION_INT(m) => Ok(GlobalPositionInt {
                time_boot_ms: m.time_boot_ms,
                latitude_deg: deg_scaled_to_deg(m.lat),
                longitude_deg: deg_scaled_to_deg(m.lon),
                altitude: Height::new(HeightDatum::AmslEgm96, mm_to_m(m.alt)),
                relative_alt_m: mm_to_m(m.relative_alt),
                velocity: VelocityNed {
                    x_m_s: cm_to_m(m.vx as i32),
                    y_m_s: cm_to_m(m.vy as i32),
                    z_m_s: cm_to_m(m.vz as i32),
                },
                heading_deg: cdeg_to_deg(m.hdg),
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "GLOBAL_POSITION_INT",
            }),
        }
    }
}

impl TryFrom<&MavMessage> for Attitude {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::ATTITUDE(m) => Ok(Attitude {
                time_boot_ms: m.time_boot_ms,
                roll_deg: (m.roll as f64).to_degrees(),
                pitch_deg: (m.pitch as f64).to_degrees(),
                yaw_deg: (m.yaw as f64).to_degrees(),
                roll_speed_deg_s: (m.rollspeed as f64).to_degrees(),
                pitch_speed_deg_s: (m.pitchspeed as f64).to_degrees(),
                yaw_speed_deg_s: (m.yawspeed as f64).to_degrees(),
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "ATTITUDE",
            }),
        }
    }
}

impl TryFrom<&MavMessage> for SysStatus {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::SYS_STATUS(m) => Ok(SysStatus {
                sensors: SensorHealthMasks {
                    present: m.onboard_control_sensors_present.bits(),
                    enabled: m.onboard_control_sensors_enabled.bits(),
                    health: m.onboard_control_sensors_health.bits(),
                },
                battery_voltage_mv: m.voltage_battery,
                battery_current_ma: m.current_battery as i32 * 10,
                battery_remaining_percent: m.battery_remaining,
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "SYS_STATUS",
            }),
        }
    }
}

impl TryFrom<&MavMessage> for BatteryStatus {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::BATTERY_STATUS(m) => Ok(BatteryStatus {
                battery_id: m.id,
                voltage_cells_mv: m
                    .voltages
                    .iter()
                    .filter(|v| **v != u16::MAX)
                    .copied()
                    .collect(),
                current_ma: m.current_battery as i32 * 10,
                remaining_percent: (m.battery_remaining >= 0).then_some(m.battery_remaining as u8),
                temperature_deg_c: (m.temperature != i16::MAX).then_some(m.temperature),
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "BATTERY_STATUS",
            }),
        }
    }
}

impl TryFrom<&MavMessage> for GpsRawInt {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::GPS_RAW_INT(m) => Ok(GpsRawInt {
                fix_type: GpsFixType::from(m.fix_type),
                satellites_visible: m.satellites_visible,
                latitude_deg: deg_scaled_to_deg(m.lat),
                longitude_deg: deg_scaled_to_deg(m.lon),
                altitude: Height::new(HeightDatum::AmslEgm96, mm_to_m(m.alt)),
                hdop: cm_to_m(m.eph as i32),
                vdop: cm_to_m(m.epv as i32),
                velocity_m_s: cm_to_m(m.vel as i32),
                course_over_ground_deg: cdeg_to_deg(m.cog),
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "GPS_RAW_INT",
            }),
        }
    }
}

/// One decoded telemetry update derived from an inbound MAVLink frame.
#[derive(Debug, Clone, PartialEq)]
pub enum TelemetryUpdate {
    Heartbeat(Heartbeat),
    GlobalPosition(GlobalPositionInt),
    Attitude(Attitude),
    SysStatus(SysStatus),
    Battery(BatteryStatus),
    Gps(GpsRawInt),
}

impl TelemetryUpdate {
    /// Decode an envelope; returns `None` for non-telemetry messages.
    pub fn try_from_envelope(env: &MessageEnvelope) -> Option<Self> {
        match &env.message {
            MavMessage::HEARTBEAT(_) => Heartbeat::try_from(env)
                .ok()
                .map(TelemetryUpdate::Heartbeat),
            MavMessage::GLOBAL_POSITION_INT(_) => GlobalPositionInt::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::GlobalPosition),
            MavMessage::ATTITUDE(_) => Attitude::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::Attitude),
            MavMessage::SYS_STATUS(_) => SysStatus::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::SysStatus),
            MavMessage::BATTERY_STATUS(_) => BatteryStatus::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::Battery),
            MavMessage::GPS_RAW_INT(_) => GpsRawInt::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::Gps),
            _ => None,
        }
    }
}

/// Folds [`TelemetryUpdate`]s into a single [`TelemetrySnapshot`].
#[derive(Debug, Clone, Default)]
pub struct TelemetryAggregator {
    snapshot: TelemetrySnapshot,
}

impl TelemetryAggregator {
    /// Empty aggregator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one update. `now_ms` is a caller-provided monotonic or epoch
    /// millisecond timestamp used for `last_heartbeat_at_ms`.
    pub fn apply(&mut self, update: TelemetryUpdate, now_ms: u64) {
        match update {
            TelemetryUpdate::Heartbeat(hb) => {
                self.snapshot.last_heartbeat_at_ms = Some(now_ms);
                self.snapshot.heartbeat = Some(hb);
            }
            TelemetryUpdate::GlobalPosition(p) => self.snapshot.global_position = Some(p),
            TelemetryUpdate::Attitude(a) => self.snapshot.attitude = Some(a),
            TelemetryUpdate::SysStatus(s) => self.snapshot.sys_status = Some(s),
            TelemetryUpdate::Battery(b) => self.snapshot.battery = Some(b),
            TelemetryUpdate::Gps(g) => self.snapshot.gps = Some(g),
        }
    }

    /// Current snapshot.
    pub fn snapshot(&self) -> &TelemetrySnapshot {
        &self.snapshot
    }
}

/// Combined telemetry snapshot published to the UI (e.g. at 10–30 Hz).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TelemetrySnapshot {
    /// Monotonic ms timestamp of the latest HEARTBEAT from the FC.
    #[ts(type = "number")]
    pub last_heartbeat_at_ms: Option<u64>,
    pub heartbeat: Option<Heartbeat>,
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

    fn envelope(msg: MavMessage) -> MessageEnvelope {
        MessageEnvelope {
            header: ::mavlink::MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            message: msg,
            received_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn parse_heartbeat() {
        use ::mavlink::common::{MavAutopilot, MavModeFlag, MavState, MavType};
        let msg = MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 3,
            mavtype: MavType::MAV_TYPE_QUADROTOR,
            autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: MavModeFlag::from_bits_truncate(0x80 | 0x04 | 0x01),
            system_status: MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        });
        let hb = Heartbeat::try_from(&envelope(msg)).expect("parse");
        assert_eq!(hb.system_id, 1);
        assert_eq!(hb.vehicle_type, VehicleType::Quadrotor);
        assert_eq!(hb.autopilot, Autopilot::Px4);
        assert_eq!(hb.flight_state, FlightState::Active);
        assert!(hb.base_mode.is_armed());
        assert!(hb.base_mode.auto_enabled);
        assert_eq!(hb.base_mode.raw(), 0x85);
    }

    #[test]
    fn parse_global_position_int() {
        let msg = MavMessage::GLOBAL_POSITION_INT(::mavlink::common::GLOBAL_POSITION_INT_DATA {
            time_boot_ms: 1234,
            lat: 49_250_000,
            lon: -123_100_000,
            alt: 123_456,
            relative_alt: 20_000,
            vx: 100,
            vy: -50,
            vz: 25,
            hdg: 9000,
        });
        let p = GlobalPositionInt::try_from(&msg).expect("parse");
        assert!((p.latitude_deg - 4.925).abs() < 1e-9);
        assert!((p.longitude_deg + 12.31).abs() < 1e-9);
        assert!((p.altitude.meters() - 123.456).abs() < 1e-9);
        assert_eq!(p.altitude.datum(), HeightDatum::AmslEgm96);
        assert!((p.relative_alt_m - 20.0).abs() < 1e-9);
        assert!((p.velocity.x_m_s - 1.0).abs() < 1e-9);
        assert!((p.heading_deg - 90.0).abs() < 1e-9);
    }

    #[test]
    fn parse_attitude() {
        let msg = MavMessage::ATTITUDE(::mavlink::common::ATTITUDE_DATA {
            time_boot_ms: 0,
            roll: 0.1,
            pitch: -0.2,
            yaw: 1.5,
            rollspeed: 0.01,
            pitchspeed: 0.02,
            yawspeed: 0.03,
        });
        let a = Attitude::try_from(&msg).expect("parse");
        assert!((a.roll_deg - 0.1_f64.to_degrees()).abs() < 1e-6);
        assert!((a.yaw_speed_deg_s - 0.03_f64.to_degrees()).abs() < 1e-6);
    }

    #[test]
    fn parse_sys_status() {
        let msg = MavMessage::SYS_STATUS(::mavlink::common::SYS_STATUS_DATA {
            onboard_control_sensors_present: ::mavlink::common::MavSysStatusSensor::empty(),
            onboard_control_sensors_enabled: ::mavlink::common::MavSysStatusSensor::empty(),
            onboard_control_sensors_health: ::mavlink::common::MavSysStatusSensor::empty(),
            load: 250,
            voltage_battery: 16_800,
            current_battery: -1234,
            drop_rate_comm: 0,
            errors_comm: 0,
            errors_count1: 0,
            errors_count2: 0,
            errors_count3: 0,
            errors_count4: 0,
            battery_remaining: 76,
        });
        let s = SysStatus::try_from(&msg).expect("parse");
        assert_eq!(s.battery_voltage_mv, 16_800);
        assert_eq!(s.battery_current_ma, -12_340);
        assert_eq!(s.battery_remaining_percent, 76);
    }

    #[test]
    fn parse_battery_status() {
        let msg = MavMessage::BATTERY_STATUS(::mavlink::common::BATTERY_STATUS_DATA {
            current_consumed: -1,
            energy_consumed: -1,
            temperature: 25,
            voltages: [
                4100,
                4090,
                4080,
                u16::MAX,
                u16::MAX,
                u16::MAX,
                u16::MAX,
                u16::MAX,
                u16::MAX,
                u16::MAX,
            ],
            current_battery: 750,
            id: 0,
            battery_function: ::mavlink::common::MavBatteryFunction::MAV_BATTERY_FUNCTION_ALL,
            mavtype: ::mavlink::common::MavBatteryType::MAV_BATTERY_TYPE_LIPO,
            battery_remaining: 80,
        });
        let b = BatteryStatus::try_from(&msg).expect("parse");
        assert_eq!(b.voltage_cells_mv, vec![4100, 4090, 4080]);
        assert_eq!(b.current_ma, 7500);
        assert_eq!(b.remaining_percent, Some(80));
        assert_eq!(b.temperature_deg_c, Some(25));
    }

    #[test]
    fn parse_gps_raw_int_rtk_fixed() {
        let msg = MavMessage::GPS_RAW_INT(::mavlink::common::GPS_RAW_INT_DATA {
            time_usec: 0,
            lat: 49_250_000,
            lon: -123_100_000,
            alt: 100_000,
            eph: 12,
            epv: 25,
            vel: 300,
            cog: 45_000,
            fix_type: ::mavlink::common::GpsFixType::GPS_FIX_TYPE_RTK_FIXED,
            satellites_visible: 18,
        });
        let g = GpsRawInt::try_from(&msg).expect("parse");
        assert_eq!(g.fix_type, GpsFixType::RtkFixed);
        assert_eq!(g.satellites_visible, 18);
        assert!((g.hdop - 0.12).abs() < 1e-9);
        assert!((g.velocity_m_s - 3.0).abs() < 1e-9);
    }

    #[test]
    fn parse_wrong_message_is_error() {
        let msg = MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: ::mavlink::common::MavType::MAV_TYPE_GENERIC,
            autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_GENERIC,
            base_mode: ::mavlink::common::MavModeFlag::empty(),
            system_status: ::mavlink::common::MavState::MAV_STATE_UNINIT,
            mavlink_version: 3,
        });
        let r = GlobalPositionInt::try_from(&msg);
        assert!(matches!(
            r,
            Err(TelemetryParseError::WrongMessageType {
                expected: "GLOBAL_POSITION_INT"
            })
        ));
    }

    #[test]
    fn aggregator_folds_updates() {
        use ::mavlink::common::{MavAutopilot, MavModeFlag, MavState, MavType};
        let mut agg = TelemetryAggregator::new();
        assert!(!agg.snapshot().is_alive());

        let hb = envelope(MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: MavType::MAV_TYPE_QUADROTOR,
            autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: MavModeFlag::empty(),
            system_status: MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        }));
        agg.apply(TelemetryUpdate::try_from_envelope(&hb).unwrap(), 1_000);

        let pos = envelope(MavMessage::GLOBAL_POSITION_INT(
            ::mavlink::common::GLOBAL_POSITION_INT_DATA {
                time_boot_ms: 0,
                lat: 0,
                lon: 0,
                alt: 0,
                relative_alt: 0,
                vx: 0,
                vy: 0,
                vz: 0,
                hdg: 0,
            },
        ));
        agg.apply(TelemetryUpdate::try_from_envelope(&pos).unwrap(), 1_000);

        let snap = agg.snapshot();
        assert!(snap.is_alive());
        assert_eq!(snap.last_heartbeat_at_ms, Some(1_000));
        assert!(snap.global_position.is_some());
        assert!(snap.gps.is_none());
    }

    #[test]
    fn non_telemetry_message_is_none() {
        let env = envelope(MavMessage::ATTITUDE_QUATERNION(
            ::mavlink::common::ATTITUDE_QUATERNION_DATA {
                time_boot_ms: 0,
                q1: 1.0,
                q2: 0.0,
                q3: 0.0,
                q4: 0.0,
                rollspeed: 0.0,
                pitchspeed: 0.0,
                yawspeed: 0.0,
            },
        ));
        assert!(TelemetryUpdate::try_from_envelope(&env).is_none());
    }
}
