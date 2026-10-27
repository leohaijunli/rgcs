//! MAVLink log download (issues O1 / operator request): pull the FC's SD
//! log list and download log files. Pattern matches `params`/`mission`:
//! pure state machine in core, the app shell routes frames and persists
//! bytes.

pub mod capture;
pub mod error;
pub mod operation_types;
pub mod protocol;
pub mod service;
mod transaction;

pub use capture::{
    parse_logger_status, CaptureConfig, CapturePhase, LoggerBackend, LoggerStatus, SdCapture,
    SdCaptureCommand, SdCaptureError, SdCaptureEvent, SdCaptureStatus, SdCaptureStep,
};
pub use error::LogError;
pub use protocol::{LogEntryInfo, LogEvent, LogOutbound, LogProtocol, LOG_DATA_CHUNK};
pub use service::{LogService, LogServiceEvent};
pub use transaction::{LogTransaction, OWNED_CANCEL_TIMEOUT};
