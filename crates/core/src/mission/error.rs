//! Mission protocol errors.

use thiserror::Error;

/// Errors from the mission protocol state machine.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MissionError {
    /// A reply was not received in time.
    #[error("mission protocol timeout waiting for {0}")]
    Timeout(&'static str),

    /// A message arrived that does not fit the current protocol state.
    #[error("unexpected message: {0}")]
    Unexpected(&'static str),

    /// The flight controller denied the mission operation.
    #[error("mission ack denied (type {0})")]
    AckDenied(u8),

    /// Sequence number mismatch.
    #[error("sequence mismatch: expected {expected}, got {got}")]
    SeqMismatch { expected: u16, got: u16 },

    /// Item count mismatch.
    #[error("count mismatch: expected {expected}, got {got}")]
    CountMismatch { expected: u16, got: u16 },

    /// The FC reported a MAV_FRAME this build does not model.
    #[error("unsupported mission frame: {0}")]
    UnsupportedFrame(u8),

    /// Upload called with an empty mission.
    #[error("no mission items")]
    NoItems,

    /// Retransmission limit reached.
    #[error("too many retries")]
    RetriesExhausted,
}
