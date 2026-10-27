use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::runtime::operations::{OperationError, OperationRef, RuntimeClientSession};
use crate::runtime::{RequestId, Revision, TargetRef};

pub const MAX_SHELL_COMMAND_BYTES: usize = 1024;
pub const SHELL_CONFIRMATION: &str = "RUN";

const READ_ONLY_COMMANDS: &[&str] = &[
    "help",
    "ver",
    "top",
    "dmesg",
    "free",
    "ps",
    "param show",
    "gps status",
];
const READ_ONLY_PREFIXES: &[&str] = &["listener ", "param show "];

pub fn validate_command(
    command: &str,
    confirmation: Option<&str>,
) -> Result<u16, ShellProtocolError> {
    if command.is_empty() || command.trim() != command {
        return Err(ShellProtocolError::InvalidCommand {
            message: "command must be nonempty and have no surrounding whitespace".into(),
        });
    }
    if command.len() > MAX_SHELL_COMMAND_BYTES || command.len() > usize::from(u16::MAX) {
        return Err(ShellProtocolError::InvalidCommand {
            message: format!("command exceeds {MAX_SHELL_COMMAND_BYTES} UTF-8 bytes"),
        });
    }
    if command.chars().any(|character| character.is_control()) {
        return Err(ShellProtocolError::InvalidCommand {
            message: "command must be one printable line".into(),
        });
    }
    let safe_arguments = command.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, ' ' | '_' | '-' | '.' | ':' | '*')
    });
    let read_only = READ_ONLY_COMMANDS.contains(&command)
        || safe_arguments
            && READ_ONLY_PREFIXES
                .iter()
                .any(|prefix| command.starts_with(prefix));
    if !read_only && confirmation != Some(SHELL_CONFIRMATION) {
        return Err(ShellProtocolError::ConfirmationRequired);
    }
    Ok(command.len() as u16)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShellRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShellCommandRequest {
    pub session: RuntimeClientSession,
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub command: String,
    pub confirmation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShellSessionReply {
    pub operation: OperationRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShellWriteReceipt {
    pub operation: OperationRef,
    pub bytes: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShellOutputEnvelope {
    pub operation: OperationRef,
    pub target: TargetRef,
    pub sequence: Revision,
    #[ts(type = "number")]
    pub received_at_ms: u64,
    pub chunk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ShellProtocolError {
    #[error("shell service stopped")]
    Stopped,
    #[error("the shell is busy")]
    Busy,
    #[error("the owned shell was cancelled")]
    Cancelled,
    #[error("the shell belongs to an earlier target generation")]
    TargetChanged,
    #[error("the shell deadline expired")]
    TimedOut,
    #[error("invalid shell command: {message}")]
    InvalidCommand { message: String },
    #[error("free-form shell commands require exact RUN confirmation")]
    ConfirmationRequired,
    #[error("shell output sequence has a gap")]
    OutputGap,
    #[error("shell output exceeded its configured limit")]
    OutputLimit,
    #[error("unexpected shell response: {message}")]
    UnexpectedResponse { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ShellOperationError {
    #[error("{error}")]
    Operation { error: OperationError },
    #[error("{error}")]
    Protocol { error: ShellProtocolError },
    #[error("shell cleanup requires reconciliation: {reason}")]
    ReconciliationRequired {
        operation: Box<OperationRef>,
        reason: String,
    },
}

impl From<OperationError> for ShellOperationError {
    fn from(error: OperationError) -> Self {
        Self::Operation { error }
    }
}
impl From<ShellProtocolError> for ShellOperationError {
    fn from(error: ShellProtocolError) -> Self {
        Self::Protocol { error }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_presets_need_no_confirmation_but_free_form_does() {
        assert_eq!(validate_command("ver", None), Ok(3));
        assert!(validate_command("listener sensor_combined", None).is_ok());
        assert_eq!(
            validate_command("reboot", None),
            Err(ShellProtocolError::ConfirmationRequired)
        );
        assert_eq!(validate_command("reboot", Some("RUN")), Ok(6));
        for command in [
            "listener sensor_combined; reboot",
            "param show FOO && reboot",
            "param show $(reboot)",
            "listener `reboot`",
            "param show FOO|reboot",
        ] {
            assert_eq!(
                validate_command(command, None),
                Err(ShellProtocolError::ConfirmationRequired)
            );
        }
    }

    #[test]
    fn command_admission_rejects_truncation_and_multiline_ambiguity() {
        for command in ["", " ver", "ver ", "ver\nreboot", "a\0b"] {
            assert!(matches!(
                validate_command(command, Some("RUN")),
                Err(ShellProtocolError::InvalidCommand { .. })
            ));
        }
        assert!(matches!(
            validate_command(&"x".repeat(MAX_SHELL_COMMAND_BYTES + 1), Some("RUN")),
            Err(ShellProtocolError::InvalidCommand { .. })
        ));
        assert_eq!(validate_command(&"x".repeat(200), Some("RUN")), Ok(200));
    }
}
