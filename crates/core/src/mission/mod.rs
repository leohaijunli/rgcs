//! MAVLink mission protocol (Phase 1).
//!
//! Implements the MISSION_COUNT / MISSION_ITEM_INT / MISSION_REQUEST_INT /
//! MISSION_ACK upload-download state machines with timeouts and
//! retransmission, plus MISSION_CLEAR_ALL and MISSION_SET_CURRENT.

pub mod error;
pub mod protocol;
pub mod types;

pub use error::MissionError;
pub use protocol::{MissionEvent, MissionOperation, MissionProtocol};
pub use types::{Mission, MissionFrame, MissionItem};