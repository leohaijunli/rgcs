//! Normalized telemetry types exposed to the UI (ADR-002: generated via
//! `ts-rs`). These are plain-data structs that serialize to JSON over the
//! Tauri IPC / server API. Conversion from raw MAVLink messages lives in
//! `mavlink` parsing functions implemented in Phase 0 (task 2) and Phase 2
//! (RTK status).
//!
//! Field units follow the MAVLink conventions (meters, m/s, degrees).

pub mod hub;
pub mod px4_mode;
pub mod statustext;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::height::{Height, HeightDatum};
use crate::mavlink::connection::LinkErrorKind;
use crate::mavlink::message::{MavMessage, MessageEnvelope};

/// A link-layer error surfaced to the UI.
///
/// Carries the failure category and a timestamp so the UI can show an error
/// history instead of a single overwritten string (issues.md #17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TelemetryError {
    pub kind: LinkErrorKind,
    pub message: String,
    #[ts(type = "number")]
    pub at_ms: u64,
}

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

/// Identity of a MAVLink node: the `(system_id, component_id)` pair a frame
/// was sent from.
///
/// Telemetry must be attributed to one vehicle: a link shared with QGroundControl,
/// a gimbal, a companion computer or a second airframe carries heartbeats and
/// telemetry from several nodes, and only the target FC's data belongs in the
/// snapshot (issue #6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VehicleId {
    pub system_id: u8,
    pub component_id: u8,
}

impl VehicleId {
    /// Construct a node identity.
    pub const fn new(system_id: u8, component_id: u8) -> Self {
        Self {
            system_id,
            component_id,
        }
    }

    /// Identity of the sender of `env`.
    pub fn from_envelope(env: &MessageEnvelope) -> Self {
        Self {
            system_id: env.header.system_id,
            component_id: env.header.component_id,
        }
    }

    /// True when both ids match.
    pub fn matches(&self, other: Self) -> bool {
        self.system_id == other.system_id && self.component_id == other.component_id
    }
}

/// Normalized `HEARTBEAT`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Heartbeat {
    pub system_id: u8,
    pub component_id: u8,
    pub vehicle_type: VehicleType,
    pub autopilot: Autopilot,
    pub base_mode: BaseMode,
    pub custom_mode: u32,
    /// PX4 custom mode pre-decoded for display (`core::telemetry::px4_mode`,
    /// ADR-017); empty for other autopilots. The frontend must not decode
    /// `custom_mode` itself.
    pub mode_label: String,
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
            MavMessage::HEARTBEAT(hb) => {
                let autopilot = Autopilot::from(hb.autopilot);
                let mode_label = if autopilot == Autopilot::Px4 {
                    px4_mode::decode_custom_mode(hb.custom_mode)
                } else {
                    String::new()
                };
                Ok(Heartbeat {
                    system_id: env.header.system_id,
                    component_id: env.header.component_id,
                    vehicle_type: VehicleType::from(hb.mavtype),
                    autopilot,
                    base_mode: BaseMode::from_raw(hb.base_mode.bits()),
                    custom_mode: hb.custom_mode,
                    mode_label,
                    flight_state: FlightState::from(hb.system_status),
                    mavlink_version: hb.mavlink_version,
                })
            }
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

/// Landed state from `EXTENDED_SYS_STATE`, driving the preflight "on ground"
/// check (L5) and the survey session's airborne detection (M1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LandedState {
    Unknown,
    OnGround,
    InAir,
    Takeoff,
    Landing,
}

impl From<::mavlink::common::MavLandedState> for LandedState {
    fn from(s: ::mavlink::common::MavLandedState) -> Self {
        use ::mavlink::common::MavLandedState as M;
        match s {
            M::MAV_LANDED_STATE_ON_GROUND => Self::OnGround,
            M::MAV_LANDED_STATE_IN_AIR => Self::InAir,
            M::MAV_LANDED_STATE_TAKEOFF => Self::Takeoff,
            M::MAV_LANDED_STATE_LANDING => Self::Landing,
            M::MAV_LANDED_STATE_UNDEFINED => Self::Unknown,
        }
    }
}

/// VTOL state from `EXTENDED_SYS_STATE` (a multirotor survey usually reports
/// `undefined`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum VtolState {
    Undefined,
    TransitionToFw,
    TransitionToMc,
    Mc,
    Fw,
}

impl From<::mavlink::common::MavVtolState> for VtolState {
    fn from(s: ::mavlink::common::MavVtolState) -> Self {
        use ::mavlink::common::MavVtolState as M;
        match s {
            M::MAV_VTOL_STATE_TRANSITION_TO_FW => Self::TransitionToFw,
            M::MAV_VTOL_STATE_TRANSITION_TO_MC => Self::TransitionToMc,
            M::MAV_VTOL_STATE_MC => Self::Mc,
            M::MAV_VTOL_STATE_FW => Self::Fw,
            M::MAV_VTOL_STATE_UNDEFINED => Self::Undefined,
        }
    }
}

/// Normalized `EXTENDED_SYS_STATE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtendedSysState {
    pub landed_state: LandedState,
    pub vtol_state: VtolState,
}

impl TryFrom<&MavMessage> for ExtendedSysState {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::EXTENDED_SYS_STATE(m) => Ok(ExtendedSysState {
                landed_state: LandedState::from(m.landed_state),
                vtol_state: VtolState::from(m.vtol_state),
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "EXTENDED_SYS_STATE",
            }),
        }
    }
}

/// Normalized `HOME_POSITION`: the takeoff/home point the FC reports.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HomePosition {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    /// AMSL altitude (datum-tagged per ADR-006).
    pub altitude: Height,
}

impl TryFrom<&MavMessage> for HomePosition {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::HOME_POSITION(m) => Ok(HomePosition {
                latitude_deg: deg_scaled_to_deg(m.latitude),
                longitude_deg: deg_scaled_to_deg(m.longitude),
                altitude: Height::new(HeightDatum::AmslEgm96, mm_to_m(m.altitude)),
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "HOME_POSITION",
            }),
        }
    }
}

/// Decoded `ESTIMATOR_STATUS` validity flags, plus the raw bitmask for
/// forward-compatibility with flags this build does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstimatorStatusFlags {
    pub raw: u16,
    pub attitude: bool,
    pub velocity_horiz: bool,
    pub velocity_vert: bool,
    pub pos_horiz_rel: bool,
    pub pos_horiz_abs: bool,
    pub pos_vert_abs: bool,
    pub pos_vert_agl: bool,
    pub const_pos_mode: bool,
    pub pred_pos_horiz_rel: bool,
    pub pred_pos_horiz_abs: bool,
    pub gps_glitch: bool,
    pub accel_error: bool,
}

impl EstimatorStatusFlags {
    fn from_flags(flags: ::mavlink::common::EstimatorStatusFlags) -> Self {
        use ::mavlink::common::EstimatorStatusFlags as F;
        Self {
            raw: flags.bits(),
            attitude: flags.contains(F::ESTIMATOR_ATTITUDE),
            velocity_horiz: flags.contains(F::ESTIMATOR_VELOCITY_HORIZ),
            velocity_vert: flags.contains(F::ESTIMATOR_VELOCITY_VERT),
            pos_horiz_rel: flags.contains(F::ESTIMATOR_POS_HORIZ_REL),
            pos_horiz_abs: flags.contains(F::ESTIMATOR_POS_HORIZ_ABS),
            pos_vert_abs: flags.contains(F::ESTIMATOR_POS_VERT_ABS),
            pos_vert_agl: flags.contains(F::ESTIMATOR_POS_VERT_AGL),
            const_pos_mode: flags.contains(F::ESTIMATOR_CONST_POS_MODE),
            pred_pos_horiz_rel: flags.contains(F::ESTIMATOR_PRED_POS_HORIZ_REL),
            pred_pos_horiz_abs: flags.contains(F::ESTIMATOR_PRED_POS_HORIZ_ABS),
            gps_glitch: flags.contains(F::ESTIMATOR_GPS_GLITCH),
            accel_error: flags.contains(F::ESTIMATOR_ACCEL_ERROR),
        }
    }
}

/// Normalized `ESTIMATOR_STATUS` (EKF health; drives the preflight "EKF ready"
/// and the magnetic fault checks in M2).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstimatorStatus {
    /// Horizontal position 1-σ accuracy relative to the EKF origin, meters.
    pub pos_horiz_accuracy_m: f32,
    /// Vertical position 1-σ accuracy relative to the EKF origin, meters.
    pub pos_vert_accuracy_m: f32,
    pub flags: EstimatorStatusFlags,
}

impl TryFrom<&MavMessage> for EstimatorStatus {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::ESTIMATOR_STATUS(m) => Ok(EstimatorStatus {
                pos_horiz_accuracy_m: m.pos_horiz_accuracy,
                pos_vert_accuracy_m: m.pos_vert_accuracy,
                flags: EstimatorStatusFlags::from_flags(m.flags),
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "ESTIMATOR_STATUS",
            }),
        }
    }
}

/// Normalized `DISTANCE_SENSOR` (plan R1/DR5: the vehicle's rangefinder /
/// lidar AGL input). Distances convert from centimetres to metres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DistanceSensor {
    /// Report id (multi-sensor vehicles).
    pub sensor_id: u8,
    /// `MAV_SENSOR_ORIENTATION` the beam points in (0 = forward for
    /// rotation-body; downward-aimed units report 25 / `ROTATION_PITCH_270`).
    pub orientation: u8,
    /// Minimum/maximum reportable range, metres.
    pub min_m: f32,
    pub max_m: f32,
    /// Latest range, metres.
    pub current_m: f32,
}

impl TryFrom<&MavMessage> for DistanceSensor {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::DISTANCE_SENSOR(m) => Ok(DistanceSensor {
                sensor_id: m.id,
                orientation: m.orientation as u8,
                min_m: f32::from(m.min_distance) / 100.0,
                max_m: f32::from(m.max_distance) / 100.0,
                current_m: f32::from(m.current_distance) / 100.0,
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "DISTANCE_SENSOR",
            }),
        }
    }
}

/// Normalized `VIBRATION` (accelerometer vibration levels and clipping counts).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Vibration {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub clipping_0: u32,
    pub clipping_1: u32,
    pub clipping_2: u32,
}

impl TryFrom<&MavMessage> for Vibration {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::VIBRATION(m) => Ok(Vibration {
                x: m.vibration_x,
                y: m.vibration_y,
                z: m.vibration_z,
                clipping_0: m.clipping_0,
                clipping_1: m.clipping_1,
                clipping_2: m.clipping_2,
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "VIBRATION",
            }),
        }
    }
}

/// `UINT8_MAX` is the MAVLink "unknown value" sentinel for RSSI/noise fields.
const RSSI_UNKNOWN: u8 = u8::MAX;

/// Normalized `RC_CHANNELS`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RcChannels {
    /// Number of valid entries in `channels` (the FC may send up to 18).
    pub chancount: u8,
    /// Raw channel values, truncated to `chancount`. Empty when no RC is
    /// available, never padded with zeros.
    pub channels: Vec<u16>,
    /// RC link RSSI, `None` when the FC reports the unknown sentinel.
    pub rssi: Option<u8>,
}

impl TryFrom<&MavMessage> for RcChannels {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::RC_CHANNELS(m) => {
                let raw = [
                    m.chan1_raw,
                    m.chan2_raw,
                    m.chan3_raw,
                    m.chan4_raw,
                    m.chan5_raw,
                    m.chan6_raw,
                    m.chan7_raw,
                    m.chan8_raw,
                    m.chan9_raw,
                    m.chan10_raw,
                    m.chan11_raw,
                    m.chan12_raw,
                    m.chan13_raw,
                    m.chan14_raw,
                    m.chan15_raw,
                    m.chan16_raw,
                    m.chan17_raw,
                    m.chan18_raw,
                ];
                let count = usize::from(m.chancount).min(raw.len());
                Ok(RcChannels {
                    chancount: count as u8,
                    channels: raw[..count].to_vec(),
                    rssi: (m.rssi != RSSI_UNKNOWN).then_some(m.rssi),
                })
            }
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "RC_CHANNELS",
            }),
        }
    }
}

/// Normalized `RADIO_STATUS` (telemetry link quality; feeds L7's link alerts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RadioStatus {
    pub rxerrors: u16,
    pub fixed: u16,
    /// Local (sender) RSSI, `None` when unknown.
    pub rssi: Option<u8>,
    /// Remote RSSI, `None` when unknown.
    pub remrssi: Option<u8>,
    /// Remaining free transmit buffer space (0–100).
    pub txbuf: u8,
    /// Local background noise, `None` when unknown.
    pub noise: Option<u8>,
    /// Remote background noise, `None` when unknown.
    pub remnoise: Option<u8>,
}

impl TryFrom<&MavMessage> for RadioStatus {
    type Error = TelemetryParseError;

    fn try_from(msg: &MavMessage) -> Result<Self, Self::Error> {
        match msg {
            MavMessage::RADIO_STATUS(m) => Ok(RadioStatus {
                rxerrors: m.rxerrors,
                fixed: m.fixed,
                rssi: (m.rssi != RSSI_UNKNOWN).then_some(m.rssi),
                remrssi: (m.remrssi != RSSI_UNKNOWN).then_some(m.remrssi),
                txbuf: m.txbuf,
                noise: (m.noise != RSSI_UNKNOWN).then_some(m.noise),
                remnoise: (m.remnoise != RSSI_UNKNOWN).then_some(m.remnoise),
            }),
            _ => Err(TelemetryParseError::WrongMessageType {
                expected: "RADIO_STATUS",
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
    ExtendedSysState(ExtendedSysState),
    HomePosition(HomePosition),
    Estimator(EstimatorStatus),
    Vibration(Vibration),
    DistanceSensor(DistanceSensor),
    RcChannels(RcChannels),
    Radio(RadioStatus),
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
            MavMessage::EXTENDED_SYS_STATE(_) => ExtendedSysState::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::ExtendedSysState),
            MavMessage::HOME_POSITION(_) => HomePosition::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::HomePosition),
            MavMessage::ESTIMATOR_STATUS(_) => EstimatorStatus::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::Estimator),
            MavMessage::VIBRATION(_) => Vibration::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::Vibration),
            MavMessage::DISTANCE_SENSOR(_) => DistanceSensor::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::DistanceSensor),
            MavMessage::RC_CHANNELS(_) => RcChannels::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::RcChannels),
            MavMessage::RADIO_STATUS(_) => RadioStatus::try_from(&env.message)
                .ok()
                .map(TelemetryUpdate::Radio),
            _ => None,
        }
    }
}

/// Folds [`TelemetryUpdate`]s into a single [`TelemetrySnapshot`].
///
/// When a target [`VehicleId`] is set, updates from any other node are
/// dropped so a shared link cannot overwrite the snapshot with another
/// node's heartbeat, position or attitude (issue #6).
#[derive(Debug, Clone, Default)]
pub struct TelemetryAggregator {
    target: Option<VehicleId>,
    snapshot: TelemetrySnapshot,
}

impl TelemetryAggregator {
    /// Empty aggregator that accepts updates from every node.
    ///
    /// Use [`TelemetryAggregator::for_vehicle`] in production so foreign
    /// traffic is filtered out.
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty aggregator that only accepts updates from `target`.
    pub fn for_vehicle(target: VehicleId) -> Self {
        Self {
            target: Some(target),
            snapshot: TelemetrySnapshot::default(),
        }
    }

    /// True when an update from `source` belongs in this snapshot.
    pub fn accepts(&self, source: VehicleId) -> bool {
        match self.target {
            Some(target) => target.matches(source),
            None => true,
        }
    }

    /// Apply one update. `now_ms` is a caller-provided monotonic or epoch
    /// millisecond timestamp used for `last_heartbeat_at_ms`.
    ///
    /// Updates whose `source` is not the target vehicle are ignored.
    pub fn apply(&mut self, source: VehicleId, update: TelemetryUpdate, now_ms: u64) {
        if !self.accepts(source) {
            return;
        }
        match update {
            TelemetryUpdate::Heartbeat(hb) => {
                self.snapshot.last_heartbeat_at_ms = Some(now_ms);
                self.snapshot.field_ages.heartbeat_at_ms = Some(now_ms);
                self.snapshot.heartbeat = Some(hb);
            }
            TelemetryUpdate::GlobalPosition(p) => {
                self.snapshot.field_ages.global_position_at_ms = Some(now_ms);
                self.snapshot.global_position = Some(p);
            }
            TelemetryUpdate::Attitude(a) => {
                self.snapshot.field_ages.attitude_at_ms = Some(now_ms);
                self.snapshot.attitude = Some(a);
            }
            TelemetryUpdate::SysStatus(s) => {
                self.snapshot.field_ages.sys_status_at_ms = Some(now_ms);
                self.snapshot.sys_status = Some(s);
            }
            TelemetryUpdate::Battery(b) => {
                self.snapshot.field_ages.battery_at_ms = Some(now_ms);
                self.snapshot.battery = Some(b);
            }
            TelemetryUpdate::Gps(g) => {
                self.snapshot.field_ages.gps_at_ms = Some(now_ms);
                self.snapshot.gps = Some(g);
            }
            TelemetryUpdate::ExtendedSysState(s) => {
                self.snapshot.field_ages.extended_sys_state_at_ms = Some(now_ms);
                self.snapshot.extended_sys_state = Some(s);
            }
            TelemetryUpdate::HomePosition(h) => {
                self.snapshot.field_ages.home_at_ms = Some(now_ms);
                self.snapshot.home = Some(h);
            }
            TelemetryUpdate::Estimator(e) => {
                self.snapshot.field_ages.estimator_at_ms = Some(now_ms);
                self.snapshot.estimator = Some(e);
            }
            TelemetryUpdate::Vibration(v) => {
                self.snapshot.field_ages.vibration_at_ms = Some(now_ms);
                self.snapshot.vibration = Some(v);
            }
            TelemetryUpdate::DistanceSensor(d) => {
                self.snapshot.field_ages.distance_sensor_at_ms = Some(now_ms);
                self.snapshot.distance_sensor = Some(d);
            }
            TelemetryUpdate::RcChannels(rc) => {
                self.snapshot.field_ages.rc_channels_at_ms = Some(now_ms);
                self.snapshot.rc_channels = Some(rc);
            }
            TelemetryUpdate::Radio(r) => {
                self.snapshot.field_ages.radio_at_ms = Some(now_ms);
                self.snapshot.radio = Some(r);
            }
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
    pub extended_sys_state: Option<ExtendedSysState>,
    pub home: Option<HomePosition>,
    pub estimator: Option<EstimatorStatus>,
    pub vibration: Option<Vibration>,
    pub distance_sensor: Option<DistanceSensor>,
    pub rc_channels: Option<RcChannels>,
    pub radio: Option<RadioStatus>,
    /// Per-field freshness, so a field that stopped updating while others keep
    /// streaming can be greyed out (issues.md #16).
    pub field_ages: TelemetryFieldAges,
}

/// Epoch-ms timestamp of the last update per telemetry field.
///
/// `None` means the field has not been seen since connecting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TelemetryFieldAges {
    #[ts(type = "number | null")]
    pub heartbeat_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub global_position_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub attitude_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub sys_status_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub battery_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub gps_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub extended_sys_state_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub home_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub estimator_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub vibration_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub distance_sensor_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub rc_channels_at_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub radio_at_ms: Option<u64>,
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

/// MAVLink ids of the supplementary telemetry messages (L6).
mod message_id {
    /// `HOME_POSITION`.
    pub const HOME_POSITION: u32 = 242;
    /// `EXTENDED_SYS_STATE`.
    pub const EXTENDED_SYS_STATE: u32 = 245;
    /// `ESTIMATOR_STATUS`.
    pub const ESTIMATOR_STATUS: u32 = 230;
    /// `VIBRATION`.
    pub const VIBRATION: u32 = 241;
    /// `DISTANCE_SENSOR`.
    pub const DISTANCE_SENSOR: u32 = 132;
    /// `RC_CHANNELS`.
    pub const RC_CHANNELS: u32 = 65;
    /// `RADIO_STATUS`.
    pub const RADIO_STATUS: u32 = 109;
}

/// A stream-rate request to send after connecting (L6).
///
/// PX4 omits several of these messages (or streams them slowly) by default, so
/// the GCS asks for an explicit rate. The rate request only changes the live
/// stream; a reboot reverts it (nothing persistent is written).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamRateRequest {
    pub message_id: u32,
    /// Requested interval in microseconds (positive).
    pub interval_us: i32,
}

/// Requested post-connect rates for the supplementary telemetry (L6).
///
/// `EXTENDED_SYS_STATE` (landed state) and `ESTIMATOR_STATUS` (EKF health) are
/// the fastest because the preflight checklist reads them; `HOME_POSITION` and
/// `RADIO_STATUS` change rarely.
pub const SUPPLEMENTARY_STREAM_RATES: &[StreamRateRequest] = &[
    StreamRateRequest {
        message_id: message_id::EXTENDED_SYS_STATE,
        interval_us: 500_000, // 2 Hz
    },
    StreamRateRequest {
        message_id: message_id::ESTIMATOR_STATUS,
        interval_us: 500_000, // 2 Hz
    },
    StreamRateRequest {
        message_id: message_id::VIBRATION,
        interval_us: 500_000, // 2 Hz
    },
    StreamRateRequest {
        message_id: message_id::DISTANCE_SENSOR,
        interval_us: 200_000, // 5 Hz (R1/DR5: the profile's lidar input)
    },
    StreamRateRequest {
        message_id: message_id::RC_CHANNELS,
        interval_us: 200_000, // 5 Hz
    },
    StreamRateRequest {
        message_id: message_id::HOME_POSITION,
        interval_us: 1_000_000, // 1 Hz
    },
    StreamRateRequest {
        message_id: message_id::RADIO_STATUS,
        interval_us: 1_000_000, // 1 Hz
    },
];

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
        // MAVLink reports radians; the snapshot is degrees with the sign kept,
        // so the map marker can apply roll/pitch/yaw verbatim.
        assert!((a.roll_deg - 0.1_f64.to_degrees()).abs() < 1e-6);
        assert!((a.pitch_deg - (-0.2_f64).to_degrees()).abs() < 1e-6);
        assert!((a.yaw_deg - 1.5_f64.to_degrees()).abs() < 1e-6);
        assert!((a.roll_speed_deg_s - 0.01_f64.to_degrees()).abs() < 1e-6);
        assert!((a.pitch_speed_deg_s - 0.02_f64.to_degrees()).abs() < 1e-6);
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
        agg.apply(
            VehicleId::from_envelope(&hb),
            TelemetryUpdate::try_from_envelope(&hb).unwrap(),
            1_000,
        );

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
        agg.apply(
            VehicleId::from_envelope(&pos),
            TelemetryUpdate::try_from_envelope(&pos).unwrap(),
            1_000,
        );

        let snap = agg.snapshot();
        assert!(snap.is_alive());
        assert_eq!(snap.last_heartbeat_at_ms, Some(1_000));
        assert!(snap.global_position.is_some());
        assert!(snap.gps.is_none());
    }

    #[test]
    fn aggregator_tracks_per_field_ages() {
        // Issue #16: a field that stops updating keeps its own timestamp while
        // the others advance.
        use ::mavlink::common::{MavAutopilot, MavModeFlag, MavState, MavType};
        let mut agg = TelemetryAggregator::new();

        let hb = envelope(MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: MavType::MAV_TYPE_QUADROTOR,
            autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: MavModeFlag::empty(),
            system_status: MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        }));
        agg.apply(
            VehicleId::from_envelope(&hb),
            TelemetryUpdate::try_from_envelope(&hb).unwrap(),
            1_000,
        );

        let gps = envelope(MavMessage::GPS_RAW_INT(
            ::mavlink::common::GPS_RAW_INT_DATA {
                time_usec: 0,
                lat: 0,
                lon: 0,
                alt: 0,
                eph: 0,
                epv: 0,
                vel: 0,
                cog: 0,
                fix_type: ::mavlink::common::GpsFixType::GPS_FIX_TYPE_3D_FIX,
                satellites_visible: 10,
                ..Default::default()
            },
        ));
        agg.apply(
            VehicleId::from_envelope(&gps),
            TelemetryUpdate::try_from_envelope(&gps).unwrap(),
            1_000,
        );

        let att = envelope(MavMessage::ATTITUDE(::mavlink::common::ATTITUDE_DATA {
            time_boot_ms: 0,
            roll: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            rollspeed: 0.0,
            pitchspeed: 0.0,
            yawspeed: 0.0,
        }));
        agg.apply(
            VehicleId::from_envelope(&att),
            TelemetryUpdate::try_from_envelope(&att).unwrap(),
            2_000,
        );

        let ages = &agg.snapshot().field_ages;
        assert_eq!(ages.attitude_at_ms, Some(2_000));
        assert_eq!(ages.gps_at_ms, Some(1_000));
        assert_eq!(ages.heartbeat_at_ms, Some(1_000));
        assert_eq!(ages.global_position_at_ms, None);
    }

    #[test]
    fn aggregator_rejects_foreign_sources() {
        use ::mavlink::common::{
            MavAutopilot, MavModeFlag, MavState, MavType, GLOBAL_POSITION_INT_DATA, HEARTBEAT_DATA,
        };

        // Target FC is system 1 / component 1; the other nodes are a QGC-like
        // GCS (255/190) and a companion computer on the same link (42/1).
        let target = VehicleId::new(1, 1);
        let mut agg = TelemetryAggregator::for_vehicle(target);

        let foreign_hb = MessageEnvelope {
            header: ::mavlink::MavHeader {
                system_id: 255,
                component_id: 190,
                sequence: 0,
            },
            message: MavMessage::HEARTBEAT(HEARTBEAT_DATA {
                custom_mode: 0x0504_0000, // foreign node claims Auto·RTL
                mavtype: MavType::MAV_TYPE_QUADROTOR,
                autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
                base_mode: MavModeFlag::empty(),
                system_status: MavState::MAV_STATE_ACTIVE,
                mavlink_version: 3,
            }),
            received_at: std::time::Instant::now(),
        };
        agg.apply(
            VehicleId::from_envelope(&foreign_hb),
            TelemetryUpdate::try_from_envelope(&foreign_hb).unwrap(),
            1_000,
        );
        assert!(
            !agg.snapshot().is_alive(),
            "a foreign heartbeat must not mark the FC alive"
        );
        assert!(agg.snapshot().heartbeat.is_none());

        let target_hb = MessageEnvelope {
            header: ::mavlink::MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 1,
            },
            message: MavMessage::HEARTBEAT(HEARTBEAT_DATA {
                custom_mode: 0x0004_0000, // target is in Auto·Mission
                mavtype: MavType::MAV_TYPE_QUADROTOR,
                autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
                base_mode: MavModeFlag::empty(),
                system_status: MavState::MAV_STATE_ACTIVE,
                mavlink_version: 3,
            }),
            received_at: std::time::Instant::now(),
        };
        agg.apply(
            VehicleId::from_envelope(&target_hb),
            TelemetryUpdate::try_from_envelope(&target_hb).unwrap(),
            2_000,
        );
        let snap = agg.snapshot();
        assert_eq!(snap.last_heartbeat_at_ms, Some(2_000));
        assert_eq!(snap.heartbeat.as_ref().unwrap().custom_mode, 0x0004_0000);

        // A foreign position fix must not overwrite the target's.
        let foreign_pos = MessageEnvelope {
            header: ::mavlink::MavHeader {
                system_id: 42,
                component_id: 1,
                sequence: 2,
            },
            message: MavMessage::GLOBAL_POSITION_INT(GLOBAL_POSITION_INT_DATA {
                time_boot_ms: 0,
                lat: 10_000_000,
                lon: 10_000_000,
                alt: 0,
                relative_alt: 0,
                vx: 0,
                vy: 0,
                vz: 0,
                hdg: 0,
            }),
            received_at: std::time::Instant::now(),
        };
        agg.apply(
            VehicleId::from_envelope(&foreign_pos),
            TelemetryUpdate::try_from_envelope(&foreign_pos).unwrap(),
            3_000,
        );
        assert!(agg.snapshot().global_position.is_none());
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
                ..Default::default()
            },
        ));
        assert!(TelemetryUpdate::try_from_envelope(&env).is_none());
    }

    #[test]
    fn parse_extended_sys_state() {
        use ::mavlink::common::{MavLandedState, MavVtolState};
        let msg = MavMessage::EXTENDED_SYS_STATE(::mavlink::common::EXTENDED_SYS_STATE_DATA {
            vtol_state: MavVtolState::MAV_VTOL_STATE_UNDEFINED,
            landed_state: MavLandedState::MAV_LANDED_STATE_ON_GROUND,
        });
        let s = ExtendedSysState::try_from(&msg).expect("parse");
        assert_eq!(s.landed_state, LandedState::OnGround);
        assert_eq!(s.vtol_state, VtolState::Undefined);

        let air = MavMessage::EXTENDED_SYS_STATE(::mavlink::common::EXTENDED_SYS_STATE_DATA {
            vtol_state: MavVtolState::MAV_VTOL_STATE_MC,
            landed_state: MavLandedState::MAV_LANDED_STATE_IN_AIR,
        });
        let s = ExtendedSysState::try_from(&air).expect("parse");
        assert_eq!(s.landed_state, LandedState::InAir);
        assert_eq!(s.vtol_state, VtolState::Mc);
    }

    #[test]
    fn parse_home_position() {
        let msg = MavMessage::HOME_POSITION(::mavlink::common::HOME_POSITION_DATA {
            latitude: 486_490_000,
            longitude: -1_233_982_000,
            altitude: 583_250, // mm
            ..Default::default()
        });
        let h = HomePosition::try_from(&msg).expect("parse");
        assert!((h.latitude_deg - 48.649).abs() < 1e-9);
        assert!((h.longitude_deg + 123.3982).abs() < 1e-9);
        assert!((h.altitude.meters() - 583.25).abs() < 1e-9);
        assert_eq!(h.altitude.datum(), HeightDatum::AmslEgm96);
    }

    #[test]
    fn parse_estimator_status_flags() {
        use ::mavlink::common::EstimatorStatusFlags as F;
        let flags = F::ESTIMATOR_ATTITUDE | F::ESTIMATOR_POS_HORIZ_ABS | F::ESTIMATOR_GPS_GLITCH;
        let msg = MavMessage::ESTIMATOR_STATUS(::mavlink::common::ESTIMATOR_STATUS_DATA {
            time_usec: 0,
            vel_ratio: 0.1,
            pos_horiz_ratio: 0.2,
            pos_vert_ratio: 0.3,
            mag_ratio: 0.4,
            hagl_ratio: 0.0,
            tas_ratio: 0.0,
            pos_horiz_accuracy: 0.85,
            pos_vert_accuracy: 1.25,
            flags,
        });
        let e = EstimatorStatus::try_from(&msg).expect("parse");
        assert!((e.pos_horiz_accuracy_m - 0.85).abs() < 1e-6);
        assert!((e.pos_vert_accuracy_m - 1.25).abs() < 1e-6);
        assert!(e.flags.attitude);
        assert!(e.flags.pos_horiz_abs);
        assert!(e.flags.gps_glitch);
        assert!(!e.flags.accel_error, "unset flags stay false");
        assert_eq!(e.flags.raw, flags.bits());
    }

    #[test]
    fn parse_vibration() {
        let msg = MavMessage::VIBRATION(::mavlink::common::VIBRATION_DATA {
            time_usec: 0,
            vibration_x: 12.5,
            vibration_y: 8.0,
            vibration_z: 20.25,
            clipping_0: 1,
            clipping_1: 0,
            clipping_2: 3,
        });
        let v = Vibration::try_from(&msg).expect("parse");
        assert!((v.x - 12.5).abs() < 1e-6);
        assert!((v.z - 20.25).abs() < 1e-6);
        assert_eq!(v.clipping_0, 1);
        assert_eq!(v.clipping_2, 3);
    }

    #[test]
    fn parse_rc_channels_truncates_and_nulls_unknown_rssi() {
        let data = ::mavlink::common::RC_CHANNELS_DATA {
            chancount: 4,
            chan1_raw: 1000,
            chan2_raw: 1500,
            chan3_raw: 2000,
            chan4_raw: 999,
            chan5_raw: 1234, // beyond chancount, must not leak in
            rssi: u8::MAX,   // unknown sentinel
            ..Default::default()
        };
        let msg = MavMessage::RC_CHANNELS(data);
        let rc = RcChannels::try_from(&msg).expect("parse");
        assert_eq!(rc.chancount, 4);
        assert_eq!(rc.channels, vec![1000, 1500, 2000, 999]);
        assert_eq!(rc.rssi, None, "0xFF rssi means unknown, not 255");

        // No RC available: empty, never a zero-padded 18-slot vector.
        let none = RcChannels::try_from(&MavMessage::RC_CHANNELS(
            ::mavlink::common::RC_CHANNELS_DATA::default(),
        ))
        .expect("parse");
        assert!(none.channels.is_empty());
        assert_eq!(none.chancount, 0);
    }

    #[test]
    fn parse_radio_status_maps_unknown_sentinels_to_none() {
        let msg = MavMessage::RADIO_STATUS(::mavlink::common::RADIO_STATUS_DATA {
            rxerrors: 3,
            fixed: 1,
            rssi: 200,
            remrssi: u8::MAX,
            txbuf: 100,
            noise: 12,
            remnoise: u8::MAX,
        });
        let r = RadioStatus::try_from(&msg).expect("parse");
        assert_eq!(r.rssi, Some(200));
        assert_eq!(r.remrssi, None);
        assert_eq!(r.remnoise, None);
        assert_eq!(r.noise, Some(12));
        assert_eq!(r.txbuf, 100);
        assert_eq!(r.rxerrors, 3);
    }

    #[test]
    fn supplementary_fields_are_none_until_a_frame_arrives() {
        // Acceptance (L6): a field with no frame is `None`, never a default of
        // 0/false.
        let snap = TelemetrySnapshot::new();
        assert!(snap.home.is_none());
        assert!(snap.extended_sys_state.is_none());
        assert!(snap.estimator.is_none());
        assert!(snap.vibration.is_none());
        assert!(snap.rc_channels.is_none());
        assert!(snap.radio.is_none());
        assert!(snap.field_ages.home_at_ms.is_none());
        assert!(snap.field_ages.radio_at_ms.is_none());

        let mut agg = TelemetryAggregator::new();
        let home = envelope(MavMessage::HOME_POSITION(
            ::mavlink::common::HOME_POSITION_DATA::default(),
        ));
        agg.apply(
            VehicleId::from_envelope(&home),
            TelemetryUpdate::try_from_envelope(&home).unwrap(),
            5_000,
        );
        let snap = agg.snapshot();
        assert!(snap.home.is_some());
        assert_eq!(snap.field_ages.home_at_ms, Some(5_000));
        assert!(
            snap.field_ages.estimator_at_ms.is_none(),
            "a field that never arrived keeps a None age"
        );
    }

    #[test]
    fn supplementary_stream_rates_are_well_formed() {
        assert!(
            !SUPPLEMENTARY_STREAM_RATES.is_empty(),
            "the connect sequence must request the supplementary streams"
        );
        for req in SUPPLEMENTARY_STREAM_RATES {
            assert!(req.interval_us > 0, "every interval is a positive period");
        }
        // The table keys on unique message ids.
        for (i, a) in SUPPLEMENTARY_STREAM_RATES.iter().enumerate() {
            for b in &SUPPLEMENTARY_STREAM_RATES[i + 1..] {
                assert_ne!(a.message_id, b.message_id, "duplicate message id");
            }
        }
    }

    /// R1/DR5: the rangefinder decodes to metres with the sensor identity,
    /// lands in the snapshot and gets its own field age.
    #[test]
    fn distance_sensor_decodes_and_ages() {
        let mut agg = TelemetryAggregator::new();
        let msg = MavMessage::DISTANCE_SENSOR(::mavlink::common::DISTANCE_SENSOR_DATA {
            time_boot_ms: 1_000,
            min_distance: 5, // cm
            max_distance: 4_000,
            current_distance: 3_500,
            mavtype: ::mavlink::common::MavDistanceSensor::MAV_DISTANCE_SENSOR_LASER,
            id: 7,
            orientation: ::mavlink::common::MavSensorOrientation::MAV_SENSOR_ROTATION_PITCH_270,
            covariance: 0,
            signal_quality: 0,
            horizontal_fov: 0.0,
            vertical_fov: 0.0,
            quaternion: [0.0; 4],
        });
        let env = envelope(msg);
        let update = TelemetryUpdate::try_from_envelope(&env).expect("decode");
        let TelemetryUpdate::DistanceSensor(d) = update else {
            panic!("wrong variant {update:?}")
        };
        assert!(
            (d.current_m - 35.0).abs() < 1e-6,
            "cm -> m: {}",
            d.current_m
        );
        assert!((d.min_m - 0.05).abs() < 1e-6);
        assert_eq!(d.sensor_id, 7);
        agg.apply(VehicleId::from_envelope(&env), update, 9_000);
        let snap = agg.snapshot();
        assert!(snap.distance_sensor.is_some());
        assert_eq!(snap.field_ages.distance_sensor_at_ms, Some(9_000));
    }
}
