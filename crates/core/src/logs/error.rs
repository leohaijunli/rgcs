//! Log protocol errors.

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// No reply after the retry budget.
    #[error("no reply to {what} after the retry budget")]
    NoAck { what: String },
    /// A `LOG_DATA` chunk is inconsistent with the frame size or the
    /// requested range; reject it instead of panicking or allocating.
    #[error("log {id}: malformed chunk at offset {ofs} (count {count})")]
    MalformedChunk { id: u16, ofs: u32, count: u8 },
}
