//! Core library for the MagGCS ground control station.
//!
//! This crate is backend-agnostic: the desktop app (`app-tauri`), the headless
//! server (`server`), and the udev helper all consume this crate.
//!
//! # Height discipline
//!
//! Absolute heights are never represented as bare `f64`. Use [`height::Height`],
//! which carries a [`height::HeightDatum`] tag. See ADR-006.

pub mod calib;
pub mod commands;
pub mod devices;
pub mod dsp;
pub mod height;
pub mod inspector;
pub mod mavlink;
pub mod mission;
pub mod motor_test;
pub mod plan;
pub mod rtk;
pub mod signals;
pub mod survey;
pub mod telemetry;
pub mod ulog;

pub use commands::{CommandError, CommandEvent, CommandResult, CommandService, CommandSession};
pub use devices::{DeviceDatabase, DeviceEvent, DeviceManager, DeviceRole, SerialDeviceInfo};
pub use mavlink::connection::{ConnectionConfig, ConnectionEvent, ConnectionHandle};
pub use mavlink::endpoint::Endpoint;
pub use mavlink::error::MavlinkError;
pub use mavlink::heartbeat::{HeartbeatMonitor, HeartbeatStatus};
pub use mavlink::router::{MessageRoute, RoutedEvents};
pub use mission::{MissionIds, MissionService, MissionServiceError, MissionServiceEvent};
pub use plan::{FramePolicy, PlanError, PlannedMission, PlannedWaypoint};
pub use survey::{CloverleafPattern, PatternPlan, SurveyPattern};
pub use telemetry::{GlobalPositionInt, TelemetrySnapshot};
