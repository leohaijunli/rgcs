//! Parameters client/operation payloads, generated from the native contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::model::{ParamEntry, ParamType, ParamValue};
use super::service::ParamError;
use crate::runtime::operations::{OperationError, OperationRef, RuntimeClientSession};
use crate::runtime::{OperationId, RequestId, TargetRef};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ParameterRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ParameterReadRequest {
    pub context: ParameterRequestContext,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ParameterSetRequest {
    pub context: ParameterRequestContext,
    pub name: String,
    pub value: ParamValue,
    pub ptype: ParamType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ParameterSetStatus {
    Confirmed,
    Clamped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ParameterSetEcho {
    pub operation: OperationRef,
    pub status: ParameterSetStatus,
    pub actual: ParamValue,
    pub requested: Option<ParamValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParameterEventPayload {
    PullStarted {
        total: u16,
    },
    Batch {
        entries: Vec<ParamEntry>,
    },
    Progress {
        received: u16,
        total: u16,
    },
    PullDone {
        received: u16,
        total: u16,
    },
    ExternalChange {
        name: String,
        value: ParamValue,
        ptype: ParamType,
    },
    FcVersion {
        version: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ParameterEventEnvelope {
    pub target: TargetRef,
    pub operation_id: Option<OperationId>,
    #[serde(flatten)]
    pub event: ParameterEventPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ParameterCommandError {
    #[error("{error}")]
    Operation { error: OperationError },
    #[error("{error}")]
    Protocol { error: ParamError },
    #[error("parameter write needs read-back reconciliation: {reason}")]
    ReconciliationRequired {
        operation: Box<OperationRef>,
        reason: String,
    },
}

impl From<OperationError> for ParameterCommandError {
    fn from(error: OperationError) -> Self {
        Self::Operation { error }
    }
}

impl From<ParamError> for ParameterCommandError {
    fn from(error: ParamError) -> Self {
        Self::Protocol { error }
    }
}
