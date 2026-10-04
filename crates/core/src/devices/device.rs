//! Device identity types.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// USB/PCI device identity as reported by the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeviceId {
    pub vendor_id: u16,
    pub product_id: u16,
}

impl DeviceId {
    /// New device id from VID/PID hex strings (e.g. `"1546"`, `"01a9"`).
    pub fn from_hex(vendor_id: &str, product_id: &str) -> Option<Self> {
        Some(Self {
            vendor_id: u16::from_str_radix(vendor_id, 16).ok()?,
            product_id: u16::from_str_radix(product_id, 16).ok()?,
        })
    }

    /// Hexadecimal VID as used in udev rules (`"1546"`).
    pub fn vid_hex(&self) -> String {
        format!("{:04x}", self.vendor_id)
    }

    /// Hexadecimal PID as used in udev rules (`"01a9"`).
    pub fn pid_hex(&self) -> String {
        format!("{:04x}", self.product_id)
    }
}

/// How a serial port is attached to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PortTransport {
    Usb,
    Pci,
    Bluetooth,
    Virtual,
    Unknown,
}

/// A serial port discovered on the host, with identity metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SerialDeviceInfo {
    /// OS port name, e.g. `/dev/ttyUSB0` or `COM3`.
    pub port_name: String,
    /// How the port is attached.
    pub transport: PortTransport,
    /// VID/PID when available (USB).
    pub device_id: Option<DeviceId>,
    /// USB serial number when the device provides one.
    pub serial_number: Option<String>,
    /// Manufacturer string.
    pub manufacturer: Option<String>,
    /// Product string.
    pub product: Option<String>,
}

/// Suggested role for a recognized device (plan §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    FlightController,
    RtkBaseStation,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_ids_roundtrip() {
        let id = DeviceId::from_hex("1546", "01a9").expect("u-blox ZED-F9P");
        assert_eq!(id.vendor_id, 0x1546);
        assert_eq!(id.product_id, 0x01a9);
        assert_eq!(id.vid_hex(), "1546");
        assert_eq!(id.pid_hex(), "01a9");
    }

    #[test]
    fn invalid_hex_rejected() {
        assert!(DeviceId::from_hex("zz", "01a9").is_none());
        assert!(DeviceId::from_hex("1546", "xyz").is_none());
    }
}
