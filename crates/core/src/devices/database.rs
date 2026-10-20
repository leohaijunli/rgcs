//! Known-device database for role suggestion and udev rules (plan §9).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::device::{DeviceId, DeviceRole, SerialDeviceInfo};

/// A known device entry used to suggest a role for a plugged-in device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownDevice {
    pub device_id: DeviceId,
    /// Human-readable product name.
    pub product_name: String,
    /// Suggested role.
    pub role: DeviceRole,
}

/// Read-only lookup table of known devices.
#[derive(Debug, Clone, Default)]
pub struct DeviceDatabase {
    entries: BTreeSet<KnownDevice>,
}

impl PartialOrd for KnownDevice {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KnownDevice {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (
            self.device_id.vendor_id,
            self.device_id.product_id,
            &self.product_name,
        )
            .cmp(&(
                other.device_id.vendor_id,
                other.device_id.product_id,
                &other.product_name,
            ))
    }
}

impl DeviceDatabase {
    /// Built-in database of common survey/autopilot devices.
    ///
    /// VID/PID values are cross-checked against vendor documentation before
    /// use (ADR-003 / plan §9).
    pub fn builtin() -> Self {
        let mut db = Self::default();
        db.entries.extend([
            // PX4/Pixhawk (PX4 bootloader + autopilot boards).
            known(
                0x26AC,
                0x0011,
                "PX4 Pixhawk 1",
                DeviceRole::FlightController,
            ),
            known(
                0x26AC,
                0x0012,
                "PX4 Pixhawk 2",
                DeviceRole::FlightController,
            ),
            known(
                0x26AC,
                0x0013,
                "PX4 Pixhawk 3",
                DeviceRole::FlightController,
            ),
            known(
                0x26AC,
                0x0014,
                "PX4 Pixhawk 4",
                DeviceRole::FlightController,
            ),
            known(
                0x26AC,
                0x0015,
                "PX4 Pixhawk Mini",
                DeviceRole::FlightController,
            ),
            known(
                0x26AC,
                0x0039,
                "PX4 Holybro Pixhawk 6C",
                DeviceRole::FlightController,
            ),
            // u-blox ZED-F9P RTK receiver (base station).
            known(0x1546, 0x01A9, "u-blox ZED-F9P", DeviceRole::RtkBaseStation),
            known(0x1546, 0x01A8, "u-blox ZED-F9R", DeviceRole::RtkBaseStation),
            known(0x1546, 0x01A2, "u-blox NEO-M8P", DeviceRole::RtkBaseStation),
            // USB-serial bridges commonly found on FCs and radios.
            known(0x0403, 0x6001, "FTDI FT232R", DeviceRole::Other),
            known(0x0403, 0x6010, "FTDI FT2232", DeviceRole::Other),
            known(0x0403, 0x6014, "FTDI FT232H", DeviceRole::Other),
            known(0x0403, 0x6015, "FTDI FT231X", DeviceRole::Other),
            known(0x10C4, 0xEA60, "Silicon Labs CP210x", DeviceRole::Other),
            known(0x1A86, 0x7523, "WCH CH340", DeviceRole::Other),
            known(0x0483, 0x5740, "STMicroelectronics VCP", DeviceRole::Other),
            // Holybro radio modules.
            known(0x12A8, 0x0001, "Holybro Telemetry Radio", DeviceRole::Other),
        ]);
        db
    }

    /// Look up a device by VID/PID.
    pub fn lookup(&self, device_id: &DeviceId) -> Option<&KnownDevice> {
        self.entries.iter().find(|e| e.device_id == *device_id)
    }

    /// Suggested role for a serial device, falling back to [`DeviceRole::Other`].
    pub fn suggested_role(&self, info: &SerialDeviceInfo) -> DeviceRole {
        match info.device_id {
            Some(id) => self
                .lookup(&id)
                .map(|e| e.role)
                .unwrap_or(DeviceRole::Other),
            None => DeviceRole::Other,
        }
    }

    /// Add or replace an entry. Returns `true` when a new entry was inserted.
    pub fn add(&mut self, entry: KnownDevice) -> bool {
        self.entries.insert(entry)
    }

    /// Remove an entry. Returns `true` when it was present.
    pub fn remove(&mut self, device_id: &DeviceId, product_name: &str) -> bool {
        self.entries.remove(&KnownDevice {
            device_id: *device_id,
            product_name: product_name.to_string(),
            role: DeviceRole::Other,
        })
    }

    /// All entries, sorted by VID/PID.
    pub fn entries(&self) -> impl Iterator<Item = &KnownDevice> {
        self.entries.iter()
    }
}

fn known(vid: u16, pid: u16, product_name: &str, role: DeviceRole) -> KnownDevice {
    KnownDevice {
        device_id: DeviceId {
            vendor_id: vid,
            product_id: pid,
        },
        product_name: product_name.to_string(),
        role,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_identifies_zed_f9p() {
        let db = DeviceDatabase::builtin();
        let id = DeviceId {
            vendor_id: 0x1546,
            product_id: 0x01A9,
        };
        assert_eq!(
            db.lookup(&id).map(|e| e.role),
            Some(DeviceRole::RtkBaseStation)
        );
        let info = SerialDeviceInfo {
            port_name: "/dev/ttyACM0".into(),
            transport: crate::devices::device::PortTransport::Usb,
            device_id: Some(id),
            serial_number: Some("SERIAL-1".into()),
            manufacturer: None,
            product: None,
        };
        assert_eq!(db.suggested_role(&info), DeviceRole::RtkBaseStation);
    }

    #[test]
    fn unknown_device_is_other() {
        let db = DeviceDatabase::builtin();
        let info = SerialDeviceInfo {
            port_name: "/dev/ttyS0".into(),
            transport: crate::devices::device::PortTransport::Unknown,
            device_id: None,
            serial_number: None,
            manufacturer: None,
            product: None,
        };
        assert_eq!(db.suggested_role(&info), DeviceRole::Other);
    }
}
