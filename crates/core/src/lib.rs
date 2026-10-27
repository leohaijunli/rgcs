//! Core library for the MagGCS ground control station.
//!
//! This crate is backend-agnostic: the desktop app (`app-tauri`), the headless
//! server (`server`), and the udev helper all consume this crate.
//!
//! # Height discipline
//!
//! Absolute heights are never represented as bare `f64`. Use [`height::Height`],
//! which carries a [`height::HeightDatum`] tag. See ADR-006.

pub mod alerts;
pub mod archive;
pub mod calib;
pub mod commands;
pub mod config;
pub mod devices;
pub mod drape_monitor;
pub mod dsp;
pub mod geomag;
pub mod height;
pub mod inspector;
pub mod instruments;
pub mod lifecycle;
pub mod logs;
pub mod mavlink;
pub mod mission;
pub mod motor_test;
pub mod params;
pub mod plan;
pub mod project;
pub mod recorder;
pub mod rtk;
pub mod runtime;
pub mod safety;
pub mod shared_state;
pub mod signals;
pub mod survey;
pub mod survey_profile;
pub mod telemetry;
pub mod terrain;
pub mod ulog;

pub use alerts::{
    Alert, AlertCenter, AlertEvent, AlertEventKind, AlertInput, AlertSource, Severity,
};
pub use commands::{
    command_label, CommandError, CommandEvent, CommandResult, CommandService, CommandSession,
};
pub use config::{AppConfig, ConfigError, ModelOrientation, CONFIG_VERSION};
pub use devices::{DeviceDatabase, DeviceEvent, DeviceManager, DeviceRole, SerialDeviceInfo};
pub use lifecycle::{
    Lifecycle, LifecyclePolicy, ManagedService, ShutdownError, ShutdownReport, Stage, StopOutcome,
    StopReason, VehicleState,
};
pub use mavlink::connection::{ConnectionConfig, ConnectionEvent, ConnectionHandle};
pub use mavlink::endpoint::Endpoint;
pub use mavlink::error::MavlinkError;
pub use mavlink::heartbeat::{HeartbeatMonitor, HeartbeatStatus};
pub use mavlink::router::{MessageRoute, RoutedEvents};
pub use mission::{MissionIds, MissionService, MissionServiceError, MissionServiceEvent};
pub use plan::{FramePolicy, PlanError, PlannedMission, PlannedWaypoint};
pub use recorder::{
    pump_raw as pump_recorder_raw, read_events as read_recorded_events, ForceArmAudit, LoggedEvent,
    RecordEvent, Recorder, RecorderConfig, RecorderError, RecorderStats, SessionManifest,
};
pub use safety::health::{HealthMonitor, HealthReading};
pub use safety::{
    arm_gate, arm_gate_with_ack, evaluate_force_arm, Checklist, ChecklistGroup, ChecklistItem,
    ChecklistItemId, ChecklistStatus, ForceArmOutcome, ForceArmReason, ForceArmRequest, PlanState,
    PreflightRefusal, RefusalKind, SafetyConfig, FORCE_ARM_KEYWORD,
};
pub use survey::{CloverleafPattern, PatternPlan, SurveyPattern};
pub use telemetry::{GlobalPositionInt, TelemetrySnapshot};
