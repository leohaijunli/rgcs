//! Errors raised by the MAVLink layer.

use std::time::Duration;

use thiserror::Error;

/// MAVLink layer errors.
#[derive(Debug, Error)]
pub enum MavlinkError {
    /// The endpoint string or address could not be parsed.
    #[error("invalid MAVLink endpoint: {0}")]
    InvalidEndpoint(String),

    /// I/O failure on the underlying transport.
    #[error("link I/O failure: {0}")]
    Io(#[from] std::io::Error),

    /// Serial port failure.
    #[error("serial port failure: {0}")]
    Serial(#[from] serialport::Error),

    /// MAVLink framing or payload decode failure.
    #[error("protocol failure: {0}")]
    Protocol(String),

    /// The flight controller heartbeat was not seen for the configured
    /// timeout.
    #[error("heartbeat timeout after {0:?}")]
    HeartbeatTimeout(Duration),

    /// The connection worker task terminated unexpectedly.
    #[error("connection worker terminated: {0}")]
    WorkerGone(String),

    /// A required channel is closed.
    #[error("channel closed")]
    ChannelClosed,

    /// The operation was cancelled.
    #[error("cancelled")]
    Cancelled,
}
