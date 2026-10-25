//! MAVLink transport, connection management, and heartbeat monitoring.
//!
//! - [`endpoint`] — transport endpoints (UDP/TCP/serial) rendered to the
//!   MAVLink address string format.
//! - [`connection`] — a reconnecting connection worker emitting typed events.
//! - [`heartbeat`] — heartbeat tracking for a single node.
//! - [`message`] — inbound frame envelope with routing metadata.
//! - [`router`] — inbound message routing predicates for subscribers.
//! - [`streams`] — generated standard-message metadata and exact selection.
//!
//! The MAVLink dialect is `common` (PX4/ArduPilot) from the `mavlink` crate.
//! Coexistence with QGC follows ADR-003: independent system/component IDs,
//! MAVLink 2.

pub mod connection;
pub mod endpoint;
pub mod error;
pub mod heartbeat;
pub mod message;
pub mod router;
pub mod serial;
pub mod stream_control;
pub mod streams;

pub use connection::{
    ConnectionConfig, ConnectionEvent, ConnectionHandle, LinkErrorKind, LinkFailure, LinkState,
    RawFrame,
};
pub use endpoint::{Endpoint, TransportKind};
pub use error::MavlinkError;
pub use heartbeat::{HeartbeatMonitor, HeartbeatStatus};
pub use message::{MavHeader, MavMessage, MessageEnvelope};
pub use router::{MessageRoute, RoutedEvents};

/// MAVLink protocol version used for transmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MavVersion {
    V1,
    #[default]
    V2,
}
