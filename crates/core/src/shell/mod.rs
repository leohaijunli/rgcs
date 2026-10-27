//! Owner-bound PX4 shell wire types and command admission.

pub mod operation_types;

pub use operation_types::{
    validate_command, ShellCommandRequest, ShellOperationError, ShellOutputEnvelope,
    ShellProtocolError, ShellRequestContext, ShellSessionReply, ShellWriteReceipt,
    MAX_SHELL_COMMAND_BYTES, SHELL_CONFIRMATION,
};
