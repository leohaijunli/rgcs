use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{LogEntryInfo, LogError};
use crate::runtime::operations::{OperationError, OperationRef, RuntimeClientSession};
use crate::runtime::RequestId;

pub const DELETE_ALL_CONFIRMATION: &str = "DELETE ALL LOGS";
/// The current single-buffer downloader is bounded until the streaming adapter lands.
pub const MAX_LOG_DOWNLOAD_BYTES: u32 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogDownloadRequest {
    pub context: LogRequestContext,
    pub entry: LogEntryInfo,
    pub save_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogDeleteRequest {
    pub context: LogRequestContext,
    pub entry: LogEntryInfo,
    pub confirmation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogEraseRequest {
    pub context: LogRequestContext,
    pub confirmation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogResult {
    Listed { entries: Vec<LogEntryInfo> },
    Downloaded { entry: LogEntryInfo, path: String },
    Deleted { entries: Vec<LogEntryInfo> },
    Erased,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogReply {
    pub operation: OperationRef,
    pub result: LogResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogProgressEnvelope {
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub entry: LogEntryInfo,
    pub fetched: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum LogProtocolError {
    #[error("log service stopped")]
    Stopped,
    #[error("another log exchange is active")]
    Busy,
    #[error("the owned log operation was cancelled")]
    Cancelled,
    #[error("the log operation belongs to an earlier target generation")]
    TargetChanged,
    #[error("the log operation deadline expired")]
    TimedOut,
    #[error("this log transaction was already submitted")]
    AlreadySubmitted,
    #[error("log protocol cleanup did not reach a quiet receipt")]
    CleanupTimeout,
    #[error("invalid log request: {message}")]
    InvalidRequest { message: String },
    #[error("no reply to {what} after the retry budget")]
    NoAck { what: String },
    #[error("log {id}: malformed chunk at offset {offset} (count {count})")]
    MalformedChunk { id: u16, offset: u32, count: u8 },
}

impl From<LogError> for LogProtocolError {
    fn from(error: LogError) -> Self {
        match error {
            LogError::NoAck { what } => Self::NoAck { what },
            LogError::MalformedChunk { id, ofs, count } => Self::MalformedChunk {
                id,
                offset: ofs,
                count,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum LogOperationError {
    #[error("{error}")]
    Operation { error: OperationError },
    #[error("{error}")]
    Protocol { error: LogProtocolError },
    #[error("local log file I/O failed: {message}")]
    LocalIo { message: String },
    #[error("log outcome requires reconciliation: {reason}")]
    ReconciliationRequired {
        operation: Box<OperationRef>,
        reason: String,
    },
}

impl From<OperationError> for LogOperationError {
    fn from(error: OperationError) -> Self {
        Self::Operation { error }
    }
}
impl From<LogProtocolError> for LogOperationError {
    fn from(error: LogProtocolError) -> Self {
        Self::Protocol { error }
    }
}

pub fn delete_confirmation(entry: &LogEntryInfo) -> String {
    format!("DELETE LOG {}", entry.id)
}
