//! Native serial profiles, automatic intent snapshots and pure selection policy.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::devices::{
    stable_usb_id, BaudSelection, DeviceId, DeviceRole, InventoryHealth, SerialDeviceInfo,
    SerialDiscoveryError, SerialInventory, SerialOwner, SerialPortRef, StableUsbId,
};
use crate::mavlink::serial::FcIdentity;
use crate::rtk::base::{BaseMode, BaseSerialRequest, BaseSessionId, SurveyInConfig};

pub const SERIAL_SETTINGS_VERSION: u32 = 1;
pub const SERIAL_SETTINGS_FILE: &str = "serial-connections.json";
const RETRY_SECONDS: [u64; 6] = [1, 2, 4, 8, 16, 30];
const F9P_USB: DeviceId = DeviceId {
    vendor_id: 0x1546,
    product_id: 0x01a9,
};
static NEXT_INTENT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AutoConnectRole {
    PrimaryFc,
    RtkBase,
}

impl AutoConnectRole {
    pub fn owner(self) -> SerialOwner {
        match self {
            Self::PrimaryFc => SerialOwner::PrimaryFc,
            Self::RtkBase => SerialOwner::RtkBase,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoConnectIntentRef {
    pub role: AutoConnectRole,
    pub id: String,
}

impl AutoConnectIntentRef {
    pub fn next(role: AutoConnectRole) -> Result<Self, SerialDiscoveryError> {
        let id = NEXT_INTENT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| SerialDiscoveryError::IdExhausted)?;
        Ok(Self {
            role,
            id: id.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SavedSerialDevice {
    pub stable_id: Option<StableUsbId>,
    pub last_device: SerialDeviceInfo,
}

impl SavedSerialDevice {
    pub fn from_port(port: &SerialPortRef) -> Self {
        Self {
            stable_id: stable_usb_id(&port.device),
            last_device: port.device.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct AutoConnectPolicy {
    pub enabled: bool,
    pub paused_by_user: bool,
    pub saved: Option<SavedSerialDevice>,
}

impl Default for AutoConnectPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            paused_by_user: false,
            saved: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct FcAutoConnectSettings {
    pub policy: AutoConnectPolicy,
    pub baud: BaudSelection,
    pub expected_target: Option<FcIdentity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct RtkAutoConnectSettings {
    pub policy: AutoConnectPolicy,
    pub receiver: BaseSerialRequest,
}

impl Default for RtkAutoConnectSettings {
    fn default() -> Self {
        Self {
            policy: AutoConnectPolicy::default(),
            receiver: BaseSerialRequest {
                baud: BaudSelection::Auto,
                mode: BaseMode::SurveyIn(SurveyInConfig::default()),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct SerialAutoConnectSettings {
    pub version: u32,
    pub fc: FcAutoConnectSettings,
    pub rtk: RtkAutoConnectSettings,
}

impl Default for SerialAutoConnectSettings {
    fn default() -> Self {
        Self {
            version: SERIAL_SETTINGS_VERSION,
            fc: FcAutoConnectSettings::default(),
            rtk: RtkAutoConnectSettings::default(),
        }
    }
}

impl SerialAutoConnectSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != SERIAL_SETTINGS_VERSION {
            return Err(format!(
                "unsupported serial settings version {}",
                self.version
            ));
        }
        self.fc.baud.validate().map_err(|e| e.to_string())?;
        self.rtk
            .receiver
            .baud
            .validate()
            .map_err(|e| e.to_string())?;
        match self.rtk.receiver.mode {
            BaseMode::SurveyIn(config) => config.validate().map_err(|e| e.to_string())?,
            BaseMode::Preconfigured if self.rtk.receiver.baud == BaudSelection::Auto => {
                return Err("preconfigured RTCM requires a fixed baud rate".into())
            }
            BaseMode::Preconfigured => {}
        }
        if self
            .fc
            .expected_target
            .as_ref()
            .is_some_and(|target| target.system_id == 0 || target.component_id != 1)
        {
            return Err("saved autopilot IDs must use a nonzero system and component 1".into());
        }
        Ok(())
    }

    pub fn policy(&self, role: AutoConnectRole) -> &AutoConnectPolicy {
        match role {
            AutoConnectRole::PrimaryFc => &self.fc.policy,
            AutoConnectRole::RtkBase => &self.rtk.policy,
        }
    }

    pub fn policy_mut(&mut self, role: AutoConnectRole) -> &mut AutoConnectPolicy {
        match role {
            AutoConnectRole::PrimaryFc => &mut self.fc.policy,
            AutoConnectRole::RtkBase => &mut self.rtk.policy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AutoConnectPhase {
    Disabled,
    Paused,
    WaitingForDevice,
    NeedsSelection,
    WaitingForPort,
    Probing,
    Starting,
    Running,
    RetryWait,
    Stopping,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SerialSessionRef {
    Base {
        id: BaseSessionId,
    },
    Fc {
        connection_id: String,
        generation: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RetryDisposition {
    Backoff,
    WaitForDevice,
    WaitForPort,
    UserAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SerialFailure {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub retry: RetryDisposition,
}

impl SerialFailure {
    pub fn discovery(error: &SerialDiscoveryError) -> Self {
        use SerialDiscoveryError as E;
        let (code, retry) = match error {
            E::PortBusy { .. } => ("port_busy", RetryDisposition::WaitForPort),
            E::DeviceGone { .. } | E::DeviceChanged { .. } => {
                ("device_changed", RetryDisposition::WaitForDevice)
            }
            E::InventoryUnavailable => ("inventory_unavailable", RetryDisposition::WaitForDevice),
            E::StableIdentityRequired => ("stable_identity_required", RetryDisposition::UserAction),
            E::AmbiguousDevices { .. } => ("ambiguous_devices", RetryDisposition::UserAction),
            E::AmbiguousTargets { .. } => ("ambiguous_targets", RetryDisposition::UserAction),
            E::PermissionDenied { .. } => ("permission_denied", RetryDisposition::UserAction),
            E::UnexpectedTarget { .. } => ("unexpected_target", RetryDisposition::UserAction),
            E::UnsupportedAutopilot { .. } | E::UnsupportedMavlinkVersion => {
                ("unsupported_autopilot", RetryDisposition::UserAction)
            }
            E::LocalSystemIdConflict { .. } => ("local_id_conflict", RetryDisposition::UserAction),
            E::Enumeration(_)
            | E::EnumerationTimeout
            | E::NoAutopilot
            | E::ProbeTimeout { .. }
            | E::OpenFailed { .. } => ("probe_failed", RetryDisposition::Backoff),
            E::Cancelled => ("cancelled", RetryDisposition::UserAction),
            _ => ("serial_error", RetryDisposition::UserAction),
        };
        Self {
            code: code.into(),
            message: error.to_string(),
            stage: "discovery".into(),
            retry,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoConnectStatus {
    pub intent: AutoConnectIntentRef,
    pub revision: String,
    pub phase: AutoConnectPhase,
    pub attempt: Option<String>,
    pub port: Option<SerialPortRef>,
    pub session: Option<SerialSessionRef>,
    pub retry_in_ms: Option<u32>,
    pub error: Option<SerialFailure>,
}

impl AutoConnectStatus {
    pub fn new(role: AutoConnectRole) -> Result<Self, SerialDiscoveryError> {
        Ok(Self {
            intent: AutoConnectIntentRef::next(role)?,
            revision: "1".into(),
            phase: AutoConnectPhase::WaitingForDevice,
            attempt: None,
            port: None,
            session: None,
            retry_in_ms: None,
            error: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PrimaryStartupOwner {
    LegacyEndpoint,
    SerialDiscovery,
    ManualSerialSelection,
    BlockedSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SerialAutoConnectSnapshot {
    pub revision: String,
    pub settings_revision: String,
    pub settings: SerialAutoConnectSettings,
    pub primary_startup_owner: PrimaryStartupOwner,
    pub fc: AutoConnectStatus,
    pub rtk: AutoConnectStatus,
    pub settings_error: Option<String>,
}

pub fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs(RETRY_SECONDS[failures.min((RETRY_SECONDS.len() - 1) as u32) as usize])
}

pub fn select_candidate(
    inventory: &SerialInventory,
    settings: &SerialAutoConnectSettings,
    role: AutoConnectRole,
    pinned: Option<&StableUsbId>,
) -> Result<Option<SerialPortRef>, SerialDiscoveryError> {
    if inventory.health != InventoryHealth::Ready {
        return Err(SerialDiscoveryError::InventoryUnavailable);
    }
    let policy = settings.policy(role);
    if !policy.enabled || policy.paused_by_user {
        return Ok(None);
    }
    let saved = policy
        .saved
        .as_ref()
        .map(|saved| {
            saved
                .stable_id
                .as_ref()
                .ok_or(SerialDiscoveryError::StableIdentityRequired)
        })
        .transpose()?;
    let desired = saved.or(pinned);
    let candidates: Vec<_> = inventory
        .ports
        .iter()
        .filter(|port| {
            if let Some(desired) = desired {
                return port.stable_id.as_ref() == Some(desired);
            }
            if port.lease.is_some() {
                return false;
            }
            match role {
                AutoConnectRole::PrimaryFc => port.suggested_role == DeviceRole::FlightController,
                AutoConnectRole::RtkBase => {
                    matches!(settings.rtk.receiver.mode, BaseMode::SurveyIn(_))
                        && port.port.device.device_id == Some(F9P_USB)
                }
            }
        })
        .collect();
    if candidates.len() > 1 {
        return Err(SerialDiscoveryError::AmbiguousDevices {
            ports: candidates.iter().map(|p| p.port.instance_id).collect(),
        });
    }
    let Some(candidate) = candidates.first() else {
        return Ok(None);
    };
    let stable = candidate
        .stable_id
        .as_ref()
        .ok_or(SerialDiscoveryError::StableIdentityRequired)?;
    let matching: Vec<_> = inventory
        .ports
        .iter()
        .filter(|p| p.stable_id.as_ref() == Some(stable))
        .collect();
    if matching.len() != 1 {
        return Err(SerialDiscoveryError::AmbiguousDevices {
            ports: matching.iter().map(|p| p.port.instance_id).collect(),
        });
    }
    if let Some(lease) = &candidate.lease {
        return Err(SerialDiscoveryError::PortBusy {
            owner: Some(lease.owner),
        });
    }
    if role == AutoConnectRole::RtkBase && candidate.suggested_role == DeviceRole::FlightController
    {
        return Err(SerialDiscoveryError::RoleMismatch {
            expected: DeviceRole::RtkBaseStation,
            observed: DeviceRole::FlightController,
        });
    }
    Ok(Some(candidate.port.clone()))
}

#[cfg(test)]
mod tests;
