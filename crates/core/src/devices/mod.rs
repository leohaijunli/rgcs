//! Serial/device enumeration and identification (Phase 0, task 3).
//!
//! Enumerates serial ports with USB identity metadata (VID/PID/serial
//! number), maps them to suggested roles, and watches for hotplug events.
//! [`discovery`] provides the shared, revisioned inventory with bounded scans
//! and joined shutdown (AUTO-02a). See the module README for migration status.
//! The udev rule generation/installation layer (plan §9) builds on top of
//! this module in a later phase.

pub mod autoconnect;
pub mod database;
pub mod device;
pub mod discovery;
pub mod error;
pub mod lease;
pub mod manager;

pub use autoconnect::{BaudSelection, SerialCancellation, SERIAL_BAUD_RATES};
pub use database::{DeviceDatabase, KnownDevice};
pub use device::{DeviceId, DeviceRole, PortTransport, SerialDeviceInfo};
pub use discovery::{
    spawn_serial_discovery, stable_usb_id, InventoryHealth, InventoryRevision, LeasePhase,
    PortInstanceId, SerialDiscoveryConfig, SerialDiscoveryError, SerialDiscoveryHandle,
    SerialInventory, SerialInventorySource, SerialLeaseId, SerialLeaseStatus, SerialOwner,
    SerialPortRef, SerialPortSnapshot, StableUsbId,
};
pub use error::DeviceError;
pub use lease::{canonical_serial_path, LeaseRequest, SerialLease};
pub use manager::{DeviceEvent, DeviceManager, DeviceManagerHandle};
