//! Log protocol errors.

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// No reply after the retry budget.
    #[error("no reply to {what} after the retry budget")]
    NoAck { what: String },
    /// A `LOG_DATA` chunk arrived past the collected tail (FC restarted the
    /// download mid-stream).
    #[error("log {id}: data gap at offset {ofs}")]
    Gap { id: u16, ofs: u32 },
}
