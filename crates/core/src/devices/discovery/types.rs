//! Inventory wire types. Runtime identities are losslessly string-encoded.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::devices::{DeviceId, DeviceRole, PortTransport, SerialDeviceInfo};

macro_rules! opaque_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
        #[ts(export, type = "string")]
        pub struct $name(pub(crate) u64);

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                let id = value.parse::<u64>().map_err(serde::de::Error::custom)?;
                if id == 0 {
                    return Err(serde::de::Error::custom("serial identity must be nonzero"));
                }
                Ok(Self(id))
            }
        }
    };
}

opaque_id!(
    InventoryRevision,
    "Process-monotonic inventory publication identity."
);
opaque_id!(
    PortInstanceId,
    "Identity of one observed serial attachment, not a saved device."
);
opaque_id!(
    SerialLeaseId,
    "Opaque identity of a serial worker's port reservation."
);

/// USB matching evidence; uniqueness is checked against the complete inventory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StableUsbId {
    pub device_id: DeviceId,
    pub serial_number: String,
}

/// Selection echoed by callers so a stale path cannot select a new attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SerialPortRef {
    pub instance_id: PortInstanceId,
    pub device: SerialDeviceInfo,
}

/// Logical owner of an application serial reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SerialOwner {
    PrimaryFc,
    MotorTestTarget,
    AuxiliarySource,
    RtkBase,
}

/// A worker retains ownership through its actual I/O teardown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LeasePhase {
    Reserved,
    Probing,
    Active,
    Stopping,
}

/// Reservation metadata for inventory consumers; this is not a lease token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SerialLeaseStatus {
    pub id: SerialLeaseId,
    pub owner: SerialOwner,
    pub phase: LeasePhase,
}

/// One attached serial port and its evidence. A role hint is not protocol proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SerialPortSnapshot {
    pub port: SerialPortRef,
    pub stable_id: Option<StableUsbId>,
    pub suggested_role: DeviceRole,
    /// Populated by the reservation layer when it is integrated (AUTO-02a).
    /// `None` alone cannot establish that the OS port is available.
    pub lease: Option<SerialLeaseStatus>,
}

/// An unavailable snapshot retains the most recent successfully scanned ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InventoryHealth {
    Starting,
    Ready,
    Unavailable { message: String },
}

/// Complete retained state; revisions change only when the visible state does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SerialInventory {
    pub revision: InventoryRevision,
    pub health: InventoryHealth,
    pub ports: Vec<SerialPortSnapshot>,
}

/// Extract USB matching evidence without inventing identity from a port name.
///
/// Exact serial strings are retained, including case. Duplicate keys remain in
/// the inventory so selection can report ambiguity instead of choosing a port.
pub fn stable_usb_id(device: &SerialDeviceInfo) -> Option<StableUsbId> {
    if device.transport != PortTransport::Usb {
        return None;
    }
    let device_id = device.device_id?;
    let serial_number = device.serial_number.as_ref()?;
    if serial_number.trim().is_empty() {
        return None;
    }
    Some(StableUsbId {
        device_id,
        serial_number: serial_number.clone(),
    })
}
