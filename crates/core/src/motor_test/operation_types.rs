//! Owned Motor wire/data contract. Native registry checks remain authoritative.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::tone::MotorToneSettings;
use super::{
    PresetKind, ServoPort, SessionState, WaveformPreset, DEFAULT_TICK_HZ, MAX_MOTOR_FUNCTIONS,
};
use crate::params::{ParamType, ParamValue};
use crate::runtime::operations::{
    OperationError, OperationOwner, OperationRef, RuntimeClientSession,
};
use crate::runtime::{LinkId, MavlinkTarget, RequestId, Revision, TargetRef, TransportGeneration};

const MAX_PERIODIC_FREQUENCY_HZ: f32 = DEFAULT_TICK_HZ as f32 / 2.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorPermissionRef {
    pub owner: OperationOwner,
    pub target: TargetRef,
    pub revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorPermissionRequest {
    pub context: MotorRequestContext,
    pub enabled: bool,
    pub permission: Option<MotorPermissionRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorPermissionStatus {
    pub permission: Option<MotorPermissionRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorToneSettingsRequest {
    pub context: MotorRequestContext,
    pub permission: MotorPermissionRef,
    pub config_revision: Revision,
    pub settings: MotorToneSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorParameterReadRequest {
    pub context: MotorRequestContext,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorParameterSetRequest {
    pub context: MotorRequestContext,
    pub permission: MotorPermissionRef,
    pub name: String,
    pub value: ParamValue,
    pub ptype: ParamType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorForwardSourceRef {
    pub link_id: LinkId,
    pub generation: TransportGeneration,
    pub mavlink_target: MavlinkTarget,
    pub port: ServoPort,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MotorStartSource {
    Manual { values: Vec<f32> },
    Preset { preset: WaveformPreset },
    Forward { source: MotorForwardSourceRef },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorStartRequest {
    pub context: MotorRequestContext,
    pub permission: MotorPermissionRef,
    pub config_revision: Revision,
    pub source: MotorStartSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorValuesRequest {
    pub session: RuntimeClientSession,
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub revision: Revision,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorStopRequest {
    pub session: RuntimeClientSession,
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub emergency: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorStartReply {
    pub operation: OperationRef,
    pub config_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorValuesReceipt {
    pub operation: OperationRef,
    pub revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MotorReleaseReceipt {
    pub operation: OperationRef,
    pub released_motors: u8,
    pub state: SessionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum MotorProtocolError {
    #[error("motor control is OFF")]
    Disabled,
    #[error("the native motor permission/configuration changed")]
    PermissionChanged,
    #[error("the motor service stopped")]
    ServiceStopped,
    #[error("the motor operation deadline expired")]
    TimedOut,
    #[error("invalid motor request: {message}")]
    InvalidRequest { message: String },
    #[error("motor {motor} release write failed: {message}")]
    ReleaseWrite { motor: u8, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum MotorOperationError {
    #[error("{error}")]
    Operation { error: OperationError },
    #[error("{error}")]
    Protocol { error: MotorProtocolError },
    #[error("motor operation requires reconciliation: {reason}")]
    ReconciliationRequired {
        operation: Box<OperationRef>,
        reason: String,
    },
}
impl From<OperationError> for MotorOperationError {
    fn from(error: OperationError) -> Self {
        Self::Operation { error }
    }
}
impl From<MotorProtocolError> for MotorOperationError {
    fn from(error: MotorProtocolError) -> Self {
        Self::Protocol { error }
    }
}

pub fn validate_values(values: &[f32]) -> Result<(), MotorProtocolError> {
    if values.is_empty()
        || values.len() > MAX_MOTOR_FUNCTIONS
        || values
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(MotorProtocolError::InvalidRequest {
            message: format!("expected 1–{MAX_MOTOR_FUNCTIONS} finite demands in [0, 1]"),
        });
    }
    Ok(())
}

pub fn validate_start(request: &MotorStartRequest) -> Result<(), MotorOperationError> {
    let session = &request.context.session;
    session
        .target
        .validate()
        .map_err(|error| OperationError::Runtime { error })?;
    if request.permission.owner != session.owner || request.permission.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    match &request.source {
        MotorStartSource::Manual { values } => validate_values(values)?,
        MotorStartSource::Preset { preset } => {
            let periodic = matches!(preset.kind, PresetKind::Square | PresetKind::Sine);
            if !preset.amplitude.is_finite()
                || !(0.0..=1.0).contains(&preset.amplitude)
                || !preset.duration_s.is_finite()
                || preset.duration_s <= 0.0
                || !preset.frequency_hz.is_finite()
                || preset.frequency_hz < 0.0
                || periodic
                    && !(0.0 < preset.frequency_hz
                        && preset.frequency_hz <= MAX_PERIODIC_FREQUENCY_HZ)
            {
                return Err(MotorProtocolError::InvalidRequest {
                    message: "invalid preset amplitude/duration/frequency for the motor ticker"
                        .into(),
                }
                .into());
            }
        }
        MotorStartSource::Forward { source } => {
            if source.mavlink_target.system_id == 0 || source.mavlink_target.component_id == 0 {
                return Err(MotorProtocolError::InvalidRequest {
                    message: "forward source must use a nonzero unicast node".into(),
                }
                .into());
            }
        }
    }
    Ok(())
}

fn validate_operation(
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<(), MotorOperationError> {
    session
        .target
        .validate()
        .map_err(|error| OperationError::Runtime { error })?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    Ok(())
}
impl MotorValuesRequest {
    pub fn validate(&self) -> Result<(), MotorOperationError> {
        validate_operation(&self.session, &self.operation)?;
        validate_values(&self.values)?;
        Ok(())
    }
}
impl MotorStopRequest {
    pub fn validate(&self) -> Result<(), MotorOperationError> {
        validate_operation(&self.session, &self.operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{ClientId, ModuleInstanceId, OperationId, VehicleId, VehicleSessionId};

    fn request() -> MotorStartRequest {
        let owner = OperationOwner {
            client_id: ClientId::new("window-a").expect("client"),
            instance_id: ModuleInstanceId::new("motor-a").expect("instance"),
        };
        let target = TargetRef {
            vehicle_id: VehicleId::new("bench").expect("vehicle"),
            session_id: VehicleSessionId::new("flight-a").expect("session"),
            link_id: LinkId::new(u64::MAX).expect("link"),
            generation: TransportGeneration::new(u64::MAX).expect("generation"),
            mavlink_target: crate::telemetry::VehicleId::new(42, 1),
        };
        MotorStartRequest {
            context: MotorRequestContext {
                session: RuntimeClientSession {
                    owner: owner.clone(),
                    target: target.clone(),
                    module_id: "motor-test".into(),
                },
                request_id: RequestId::new("request-a").expect("request"),
            },
            permission: MotorPermissionRef {
                owner,
                target,
                revision: Revision::new(u64::MAX).expect("revision"),
            },
            config_revision: Revision::new(u64::MAX).expect("revision"),
            source: MotorStartSource::Manual {
                values: vec![0.0, 0.5, 1.0],
            },
        }
    }

    #[test]
    fn foreign_and_old_permission_or_operation_scopes_are_rejected() {
        let mut input = request();
        assert!(validate_start(&input).is_ok());
        input.permission.owner.client_id = ClientId::new("window-b").expect("client");
        assert_eq!(validate_start(&input), Err(OperationError::NotOwner.into()));
        let mut input = request();
        input.permission.target.generation = TransportGeneration::new(1).expect("gen");
        assert_eq!(validate_start(&input), Err(OperationError::NotOwner.into()));
        let mut operation = OperationRef {
            id: OperationId::new("operation-a").expect("id"),
            owner: input.context.session.owner.clone(),
            target: input.context.session.target.clone(),
        };
        operation.owner.instance_id = ModuleInstanceId::new("retired-instance").expect("instance");
        assert_eq!(
            MotorStopRequest {
                session: input.context.session,
                operation,
                request_id: RequestId::new("stop").expect("id"),
                emergency: true
            }
            .validate(),
            Err(OperationError::NotOwner.into())
        );
    }

    #[test]
    fn malformed_demands_and_unrepresentable_presets_reject_instead_of_becoming_different_outputs()
    {
        for values in [
            vec![],
            vec![0.0; MAX_MOTOR_FUNCTIONS + 1],
            vec![f32::NAN],
            vec![f32::INFINITY],
            vec![f32::NEG_INFINITY],
            vec![-0.01],
            vec![1.01],
        ] {
            assert!(validate_values(&values).is_err());
        }
        let preset = WaveformPreset {
            kind: PresetKind::Sine,
            amplitude: 0.4,
            duration_s: 10.0,
            frequency_hz: 1.0,
        };
        for invalid in [
            WaveformPreset {
                amplitude: f32::NAN,
                ..preset
            },
            WaveformPreset {
                duration_s: 0.0,
                ..preset
            },
            WaveformPreset {
                frequency_hz: MAX_PERIODIC_FREQUENCY_HZ + 0.1,
                ..preset
            },
            WaveformPreset {
                frequency_hz: f32::INFINITY,
                ..preset
            },
        ] {
            let mut input = request();
            input.source = MotorStartSource::Preset { preset: invalid };
            assert!(validate_start(&input).is_err());
        }
        let mut input = request();
        input.source = MotorStartSource::Preset { preset };
        assert!(validate_start(&input).is_ok());
    }

    #[test]
    fn serde_preserves_full_width_generation_and_revision_as_strings() {
        let input = request();
        let wire = serde_json::to_value(&input).expect("encode");
        assert_eq!(wire["permission"]["revision"], u64::MAX.to_string());
        assert_eq!(
            wire["context"]["session"]["target"]["generation"],
            u64::MAX.to_string()
        );
        assert_eq!(wire["config_revision"], u64::MAX.to_string());
        assert_eq!(
            serde_json::from_value::<MotorStartRequest>(wire).expect("decode"),
            input
        );
    }
}
