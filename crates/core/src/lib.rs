//! Core library for the MagGCS ground control station.
//!
//! This crate is backend-agnostic: the desktop app (`app-tauri`), the headless
//! server (`server`), and the udev helper all consume this crate.
//!
//! # Height discipline
//!
//! Absolute heights are never represented as bare `f64`. Use [`height::Height`],
//! which carries a [`height::HeightDatum`] tag. See ADR-006.

pub mod devices;
pub mod height;
pub mod mission;
pub mod mavlink;
pub mod telemetry;

pub use devices::{DeviceDatabase, DeviceEvent, DeviceManager, DeviceRole, SerialDeviceInfo};
pub use mavlink::connection::{ConnectionConfig, ConnectionEvent, ConnectionHandle};
pub use mavlink::endpoint::Endpoint;
pub use mavlink::error::MavlinkError;
pub use mavlink::heartbeat::{HeartbeatMonitor, HeartbeatStatus};
pub use telemetry::{GlobalPositionInt, TelemetrySnapshot};
