//! Serial/device enumeration and identification (Phase 0, task 3).
//!
//! Enumerates serial ports with USB identity metadata (VID/PID/serial
//! number), maps them to suggested roles, and watches for hotplug events.
//! The udev rule generation/installation layer (plan §9) builds on top of
//! this module in a later phase.

pub mod database;
pub mod device;
pub mod error;
pub mod manager;

pub use database::{DeviceDatabase, KnownDevice};
pub use device::{DeviceId, DeviceRole, PortTransport, SerialDeviceInfo};
pub use error::DeviceError;
pub use manager::{DeviceEvent, DeviceManager, DeviceManagerHandle};
