//! Inbound MAVLink frame envelope and message type aliases.

use std::time::Instant;

/// MAVLink message payloads (common dialect, PX4/ArduPilot).
pub use ::mavlink::common::MavMessage;
/// MAVLink frame header (routing, sequence, sys/comp ids).
pub use ::mavlink::MavHeader;

/// A received MAVLink frame with routing and timing metadata.
#[derive(Debug, Clone)]
pub struct MessageEnvelope {
    /// Frame header (system/component ids, sequence number).
    pub header: MavHeader,
    /// Decoded payload.
    pub message: MavMessage,
    /// Wall-clock time the frame was received.
    pub received_at: Instant,
}

impl MessageEnvelope {
    /// Sender system id.
    pub fn system_id(&self) -> u8 {
        self.header.system_id
    }

    /// Sender component id.
    pub fn component_id(&self) -> u8 {
        self.header.component_id
    }

    /// Sequence number in the sender's stream.
    pub fn sequence(&self) -> u8 {
        self.header.sequence
    }

    /// Convenience: true if this is a HEARTBEAT frame from the given node.
    pub fn is_heartbeat_from(&self, system_id: u8, component_id: u8) -> bool {
        self.system_id() == system_id
            && self.component_id() == component_id
            && matches!(self.message, MavMessage::HEARTBEAT(_))
    }
}
