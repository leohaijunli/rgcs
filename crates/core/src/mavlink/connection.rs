//! Reconnecting MAVLink connection manager (Phase 0, task 2).
//!
//! A single worker task owns the underlying MAVLink transport, feeds the
//! heartbeat monitor, and emits typed events to subscribers. Sends are
//! queued through a channel so callers never block on the wire.
//!
//! **Status:** type surface is fixed; the worker loop is implemented after
//! signature confirmation (see AGENTS.md rule 2 and plan §12).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, Mutex};

use super::endpoint::Endpoint;
use super::error::MavlinkError;
use super::message::{MavHeader, MavMessage, MessageEnvelope};
use crate::mavlink::MavVersion;

/// Event bus capacity (number of frames buffered for slow subscribers).
pub const EVENT_CAPACITY: usize = 4096;
/// Outbound queue capacity.
pub const OUTBOUND_CAPACITY: usize = 256;
/// Default heartbeat timeout before `HeartbeatLost` is emitted.
pub const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);
/// Default delay before reconnect attempts after a link failure.
pub const DEFAULT_RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// Configuration for a MAVLink connection to a single flight controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Transport endpoint.
    pub endpoint: Endpoint,
    /// Our system id — must differ from the FC and from QGC (ADR-003).
    pub system_id: u8,
    /// Our component id.
    pub component_id: u8,
    /// Expected flight controller system id (PX4 default: 1).
    pub target_system_id: u8,
    /// Expected flight controller component id (PX4 autopilot: 1).
    pub target_component_id: u8,
    /// Heartbeat timeout before the link is declared lost.
    #[serde(default = "default_heartbeat_timeout")]
    pub heartbeat_timeout: Duration,
    /// Delay between reconnect attempts.
    #[serde(default = "default_reconnect_delay")]
    pub reconnect_delay: Duration,
    /// MAVLink protocol version used for transmission.
    #[serde(default)]
    pub mavlink_version: MavVersion,
}

fn default_heartbeat_timeout() -> Duration {
    DEFAULT_HEARTBEAT_TIMEOUT
}

fn default_reconnect_delay() -> Duration {
    DEFAULT_RECONNECT_DELAY
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            endpoint: Endpoint::UdpListener {
                addr: "0.0.0.0:14550".parse().expect("static addr"),
            },
            system_id: 250,
            component_id: 250,
            target_system_id: 1,
            target_component_id: 1,
            heartbeat_timeout: DEFAULT_HEARTBEAT_TIMEOUT,
            reconnect_delay: DEFAULT_RECONNECT_DELAY,
            mavlink_version: MavVersion::default(),
        }
    }
}

/// Link state reported by the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Connected,
    Reconnecting,
}

/// Clone-friendly classification of a link failure (the underlying
/// `MavlinkError` is not `Clone`, so events carry this summary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkErrorKind {
    Io,
    Serial,
    Protocol,
    HeartbeatTimeout,
    InvalidEndpoint,
    Other,
}

/// Clone-friendly link failure carried on the event bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFailure {
    pub kind: LinkErrorKind,
    pub message: String,
}

impl From<&MavlinkError> for LinkFailure {
    fn from(e: &MavlinkError) -> Self {
        let kind = match e {
            MavlinkError::Io(_) => LinkErrorKind::Io,
            MavlinkError::Serial(_) => LinkErrorKind::Serial,
            MavlinkError::Protocol(_) => LinkErrorKind::Protocol,
            MavlinkError::HeartbeatTimeout(_) => LinkErrorKind::HeartbeatTimeout,
            MavlinkError::InvalidEndpoint(_) => LinkErrorKind::InvalidEndpoint,
            MavlinkError::WorkerGone(_) | MavlinkError::ChannelClosed | MavlinkError::Cancelled => {
                LinkErrorKind::Other
            }
        };
        LinkFailure {
            kind,
            message: e.to_string(),
        }
    }
}

/// Events emitted by the connection worker.
#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    /// Link established (first connect or reconnect).
    Connected { endpoint: Endpoint },
    /// A frame was received from any node on the link.
    Message(Box<MessageEnvelope>),
    /// Heartbeat from the target FC was lost.
    HeartbeatLost { last_seen_age: Duration },
    /// Heartbeat from the target FC was restored.
    HeartbeatRestored,
    /// Recoverable link error; the worker will reconnect.
    LinkError(LinkFailure),
    /// Permanent failure; the worker has stopped.
    Failed(LinkFailure),
}

/// Outbound frame queued to the worker.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read by the worker loop (implemented after signature confirmation)
struct OutboundFrame {
    header: MavHeader,
    message: MavMessage,
}

/// Internal shared state of a connection.
struct ConnectionInner {
    config: ConnectionConfig,
    outbound: mpsc::Sender<OutboundFrame>,
    state: Mutex<LinkState>,
}

/// Handle for driving a MAVLink connection.
#[derive(Clone)]
pub struct ConnectionHandle {
    inner: Arc<ConnectionInner>,
    events: broadcast::Sender<ConnectionEvent>,
}

impl ConnectionHandle {
    /// Subscribe to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<ConnectionEvent> {
        self.events.subscribe()
    }

    /// Connection configuration.
    pub fn config(&self) -> &ConnectionConfig {
        &self.inner.config
    }

    /// Current link state.
    pub async fn state(&self) -> LinkState {
        *self.inner.state.lock().await
    }

    /// Send a message to the configured target (FC).
    pub async fn send(&self, message: MavMessage) -> Result<(), MavlinkError> {
        let config = &self.inner.config;
        self.send_to(
            MavHeader {
                system_id: config.system_id,
                component_id: config.component_id,
                sequence: 0,
            },
            message,
        )
        .await
    }

    /// Send a message with an explicit header.
    pub async fn send_to(
        &self,
        header: MavHeader,
        message: MavMessage,
    ) -> Result<(), MavlinkError> {
        let frame = OutboundFrame { header, message };
        self.inner
            .outbound
            .send(frame)
            .await
            .map_err(|_| MavlinkError::ChannelClosed)
    }

    /// Ask the worker to drop the link and reconnect.
    pub async fn reconnect(&self) -> Result<(), MavlinkError> {
        // Worker listens on a command channel; implemented with the worker.
        Ok(())
    }
}

/// Spawn the connection worker for the given config.
///
/// Returns a handle plus a receiver primed with the worker's first event.
pub async fn spawn_connection(
    config: ConnectionConfig,
) -> Result<(ConnectionHandle, broadcast::Receiver<ConnectionEvent>), MavlinkError> {
    let (events_tx, events_rx) = broadcast::channel(EVENT_CAPACITY);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let inner = Arc::new(ConnectionInner {
        config,
        outbound: outbound_tx,
        state: Mutex::new(LinkState::Connecting),
    });
    let handle = ConnectionHandle {
        inner: inner.clone(),
        events: events_tx.clone(),
    };

    tokio::spawn(async move {
        let _ = (&mut outbound_rx, inner, events_tx);
        // Worker loop (recv / heartbeat / reconnect / send) is implemented
        // after signature confirmation.
    });

    Ok((handle, events_rx))
}
