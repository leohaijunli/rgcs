//! Reviewed owner/target-bound Mission IPC types (AF-02b.4).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{MissionError, MissionItem};
use crate::runtime::operations::{OperationError, OperationRef, RuntimeClientSession};
use crate::runtime::{RequestId, TargetRef};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MissionRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MissionUploadRequest {
    pub context: MissionRequestContext,
    pub items: Vec<MissionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MissionSetCurrentRequest {
    pub context: MissionRequestContext,
    pub seq: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MissionResult {
    Uploaded { count: u16 },
    Downloaded { items: Vec<MissionItem> },
    Cleared,
    Current { seq: u16 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MissionReply {
    pub operation: OperationRef,
    pub result: MissionResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum MissionProtocolError {
    #[error("mission service stopped")]
    Stopped,
    #[error("another mission exchange is active")]
    Busy,
    #[error("the owned mission was cancelled")]
    Cancelled,
    #[error("the mission belongs to an earlier transport generation")]
    TargetChanged,
    #[error("the owned mission deadline expired")]
    TimedOut,
    #[error("this mission transaction was already submitted")]
    AlreadySubmitted,
    #[error("an unresolved exchange makes mission reply correlation ambiguous")]
    AmbiguousReply,
    #[error("invalid mission request: {message}")]
    InvalidRequest { message: String },
    #[error("mission protocol timeout waiting for {waiting_for}")]
    Timeout { waiting_for: String },
    #[error("unexpected mission result: {message}")]
    Unexpected { message: String },
    #[error("flight controller rejected the mission (type {ack_type})")]
    AckDenied { ack_type: u8 },
    #[error("sequence mismatch: expected {expected}, got {got}")]
    SeqMismatch { expected: u16, got: u16 },
    #[error("count mismatch: expected {expected}, got {got}")]
    CountMismatch { expected: u16, got: u16 },
    #[error("unsupported mission frame: {frame}")]
    UnsupportedFrame { frame: u8 },
    #[error("no mission items")]
    NoItems,
    #[error("mission retry limit exhausted")]
    RetriesExhausted,
}

impl From<MissionError> for MissionProtocolError {
    fn from(error: MissionError) -> Self {
        match error {
            MissionError::Timeout(waiting_for) => Self::Timeout {
                waiting_for: waiting_for.into(),
            },
            MissionError::Unexpected(message) => Self::Unexpected {
                message: message.into(),
            },
            MissionError::AckDenied(ack_type) => Self::AckDenied { ack_type },
            MissionError::SeqMismatch { expected, got } => Self::SeqMismatch { expected, got },
            MissionError::CountMismatch { expected, got } => Self::CountMismatch { expected, got },
            MissionError::UnsupportedFrame(frame) => Self::UnsupportedFrame { frame },
            MissionError::NoItems => Self::NoItems,
            MissionError::RetriesExhausted => Self::RetriesExhausted,
            MissionError::InvalidRequest { message } => Self::InvalidRequest { message },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum MissionOperationError {
    #[error("{error}")]
    Operation { error: OperationError },
    #[error("{error}")]
    Protocol { error: MissionProtocolError },
    #[error("mission outcome requires reconciliation: {reason}")]
    ReconciliationRequired {
        operation: Box<OperationRef>,
        reason: String,
    },
}

impl From<OperationError> for MissionOperationError {
    fn from(error: OperationError) -> Self {
        Self::Operation { error }
    }
}

impl From<MissionProtocolError> for MissionOperationError {
    fn from(error: MissionProtocolError) -> Self {
        Self::Protocol { error }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MissionProgressEnvelope {
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub sent: u16,
    pub total: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MissionCurrentSnapshot {
    pub target: TargetRef,
    pub seq: u16,
    #[ts(type = "number")]
    pub received_at_ms: u64,
}
