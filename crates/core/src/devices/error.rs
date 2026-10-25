//! Errors raised by the device layer.

use thiserror::Error;

/// Device layer errors.
#[derive(Debug, Clone, Error)]
pub enum DeviceError {
    /// Serial port enumeration failed.
    #[error("serial port enumeration failed: {0}")]
    Enumeration(#[from] serialport::Error),

    /// The watcher task terminated unexpectedly.
    #[error("device watcher terminated: {0}")]
    WatcherGone(String),
}
