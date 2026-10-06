//! Reconnecting MAVLink connection manager (Phase 0, task 2).
//!
//! A single worker task owns the underlying MAVLink transport, feeds the
//! heartbeat monitor, and emits typed events to subscribers. Sends are
//! queued through a channel so callers never block on the wire.
//!
//! The worker:
//! 1. connects to the [`Endpoint`] via `mavlink::connect_async`,
//! 2. emits [`ConnectionEvent::Connected`],
//! 3. loops on `recv` / outbound sends / heartbeat timeout / reconnect
//!    requests, and
//! 4. on a link failure or explicit [`ConnectionHandle::reconnect`],
//!    tears down and retries after `reconnect_delay`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Mutex};

use super::endpoint::Endpoint;
use super::error::MavlinkError;
use super::heartbeat::HeartbeatMonitor;
use super::message::{MavHeader, MavMessage, MessageEnvelope};
use crate::mavlink::MavVersion;

/// Event bus capacity (number of frames buffered for slow subscribers).
pub const EVENT_CAPACITY: usize = 4096;
/// Outbound queue capacity.
pub const OUTBOUND_CAPACITY: usize = 256;
/// Default heartbeat timeout before `HeartbeatLost` is emitted.
pub const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);
/// Poll period of the heartbeat watchdog. The watchdog runs on a fixed
/// interval instead of a recreated sleep, so inbound traffic from non-target
/// nodes can no longer reset the timeout (issue #1).
pub const HEARTBEAT_WATCHDOG_TICK: Duration = Duration::from_millis(500);
/// Period of the GCS heartbeat advertised to the link. PX4 uses the peer
/// heartbeat for link-loss detection and — on `udpin` — to learn where to
/// reply, so the GCS must announce itself even before the FC talks to it
/// (issue #3).
pub const GCS_HEARTBEAT_PERIOD: Duration = Duration::from_secs(1);
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

impl From<MavVersion> for ::mavlink::MavlinkVersion {
    fn from(v: MavVersion) -> Self {
        match v {
            MavVersion::V1 => ::mavlink::MavlinkVersion::V1,
            MavVersion::V2 => ::mavlink::MavlinkVersion::V2,
        }
    }
}

/// Link state reported by the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

/// Desktop/headless bridge: a snapshot of the MAVLink link status emitted
/// to the frontend alongside telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct LinkStatus {
    pub link_state: LinkState,
    pub endpoint: String,
    /// True when a target-FC heartbeat was seen recently.
    pub fc_alive: bool,
}

impl LinkStatus {
    /// Status representing a torn-down link.
    pub fn disconnected() -> Self {
        Self {
            link_state: LinkState::Disconnected,
            endpoint: String::new(),
            fc_alive: false,
        }
    }

    /// FC heartbeat alive on a connected transport.
    ///
    /// `endpoint` is the address the link is connected to; heartbeat
    /// transitions must not blank it out.
    pub fn alive(endpoint: String) -> Self {
        Self {
            link_state: LinkState::Connected,
            endpoint,
            fc_alive: true,
        }
    }

    /// FC heartbeat lost while the transport itself is still connected.
    pub fn lost(endpoint: String) -> Self {
        Self {
            link_state: LinkState::Connected,
            endpoint,
            fc_alive: false,
        }
    }
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
struct OutboundFrame {
    header: MavHeader,
    message: MavMessage,
}

/// Internal shared state of a connection.
struct ConnectionInner {
    config: ConnectionConfig,
    outbound: mpsc::Sender<OutboundFrame>,
    reconnect: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
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

    /// Send a message with an explicit header (the worker assigns the
    /// sequence number).
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
        self.inner.reconnect.send_modify(|n| *n = n.wrapping_add(1));
        Ok(())
    }

    /// Ask the worker to stop permanently (app shutdown / disconnect).
    pub async fn shutdown(&self) {
        self.inner.shutdown.send_modify(|s| *s = true);
    }
}

/// Spawn the connection worker for the given config.
///
/// Returns a handle, a receiver primed with the worker's event stream, and a
/// one-shot that resolves with the outcome of the first connection attempt:
/// `Ok(())` once the transport is bound/connected, or `Err(failure)` when the
/// first attempt fails. The caller can await it so errors (e.g. a UDP port
/// already in use) surface synchronously instead of being reported only as a
/// later event (issue #4).
pub async fn spawn_connection(
    config: ConnectionConfig,
) -> Result<
    (
        ConnectionHandle,
        broadcast::Receiver<ConnectionEvent>,
        oneshot::Receiver<Result<(), LinkFailure>>,
    ),
    MavlinkError,
> {
    let (events_tx, events_rx) = broadcast::channel(EVENT_CAPACITY);
    let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let (reconnect_tx, reconnect_rx) = watch::channel(0u64);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (first_tx, first_rx) = oneshot::channel();
    let inner = Arc::new(ConnectionInner {
        config,
        outbound: outbound_tx,
        reconnect: reconnect_tx,
        shutdown: shutdown_tx,
        state: Mutex::new(LinkState::Connecting),
    });
    let handle = ConnectionHandle {
        inner: inner.clone(),
        events: events_tx.clone(),
    };

    tokio::spawn(run_worker(
        inner,
        events_tx,
        outbound_rx,
        reconnect_rx,
        shutdown_rx,
        first_tx,
    ));

    Ok((handle, events_rx, first_rx))
}

type Conn = Box<dyn ::mavlink::AsyncMavConnection<MavMessage> + Sync + Send>;

/// A `MAV_TYPE_GCS` heartbeat, announced at [`GCS_HEARTBEAT_PERIOD`] so the
/// FC and other GCS software see us as a live ground station (issue #3).
fn gcs_heartbeat_message() -> MavMessage {
    MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
        custom_mode: 0,
        mavtype: ::mavlink::common::MavType::MAV_TYPE_GCS,
        autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_GENERIC,
        base_mode: ::mavlink::common::MavModeFlag::empty(),
        system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
        mavlink_version: 3,
    })
}

/// Connection worker task.
async fn run_worker(
    inner: Arc<ConnectionInner>,
    events: broadcast::Sender<ConnectionEvent>,
    mut outbound_rx: mpsc::Receiver<OutboundFrame>,
    mut reconnect_rx: watch::Receiver<u64>,
    mut shutdown_rx: watch::Receiver<bool>,
    first_result: oneshot::Sender<Result<(), LinkFailure>>,
) {
    let config = &inner.config;
    let address = config.endpoint.to_address_string();
    let mut heartbeat = HeartbeatMonitor::new(
        config.target_system_id,
        config.target_component_id,
        config.heartbeat_timeout,
    );
    let mut fc_alive = false;
    let mut seq: u8 = 0;
    let mut first_result = Some(first_result);

    loop {
        if *shutdown_rx.borrow() {
            return;
        }
        *inner.state.lock().await = LinkState::Connecting;
        let mut conn: Conn = match ::mavlink::connect_async::<MavMessage>(&address).await {
            Ok(c) => c,
            Err(e) => {
                let failure = LinkFailure::from(&MavlinkError::Io(e));
                let _ = events.send(ConnectionEvent::LinkError(failure.clone()));
                if let Some(tx) = first_result.take() {
                    let _ = tx.send(Err(failure));
                }
                tokio::select! {
                    _ = tokio::time::sleep(config.reconnect_delay) => {}
                    _ = reconnect_rx.changed() => {}
                }
                continue;
            }
        };
        if let Some(tx) = first_result.take() {
            let _ = tx.send(Ok(()));
        }
        conn.set_protocol_version(config.mavlink_version.into());
        *inner.state.lock().await = LinkState::Connected;
        let _ = events.send(ConnectionEvent::Connected {
            endpoint: config.endpoint.clone(),
        });

        // Fixed-interval heartbeat watchdog. A `sleep(timeout)` recreated on
        // each loop iteration would be reset by any inbound frame, so the
        // FC heartbeat could never time out while other traffic flowed;
        // polling the monitor on a short fixed tick keeps the deadline real.
        let mut watchdog = tokio::time::interval(HEARTBEAT_WATCHDOG_TICK);
        watchdog.tick().await;
        // GCS heartbeat so PX4 sees us as a live link partner (ADR-003,
        // issue #3): on `udpin` it also learns our reply address from the
        // first packet we send.
        let mut gcs_heartbeat = tokio::time::interval(GCS_HEARTBEAT_PERIOD);

        'connected: loop {
            let reconnect = reconnect_rx.changed();
            tokio::select! {
                res = conn.recv() => match res {
                    Ok((header, message)) => {
                        let now = Instant::now();
                        let is_target_heartbeat = header.system_id == config.target_system_id
                            && header.component_id == config.target_component_id
                            && matches!(message, MavMessage::HEARTBEAT(_));
                        let env = MessageEnvelope { header, message, received_at: now };
                        let _ = events.send(ConnectionEvent::Message(Box::new(env)));
                        if is_target_heartbeat {
                            let was_alive = fc_alive;
                            heartbeat.observe(header.system_id, header.component_id, now);
                            fc_alive = true;
                            if !was_alive {
                                let _ = events.send(ConnectionEvent::HeartbeatRestored);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = events.send(ConnectionEvent::LinkError(LinkFailure::from(
                            &MavlinkError::Protocol(e.to_string()),
                        )));
                        break 'connected;
                    }
                },
                cmd = outbound_rx.recv() => match cmd {
                    Some(mut frame) => {
                        frame.header.sequence = seq;
                        seq = seq.wrapping_add(1);
                        if let Err(e) = conn.send(&frame.header, &frame.message).await {
                            let _ = events.send(ConnectionEvent::LinkError(LinkFailure::from(
                                &MavlinkError::Protocol(e.to_string()),
                            )));
                            break 'connected;
                        }
                    }
                    None => return,
                },
                _ = watchdog.tick() => {
                    let now = Instant::now();
                    if fc_alive && !heartbeat.is_alive(now) {
                        fc_alive = false;
                        let age = heartbeat
                            .last_seen()
                            .map(|t| now.duration_since(t))
                            .unwrap_or_default();
                        let _ = events.send(ConnectionEvent::HeartbeatLost { last_seen_age: age });
                    }
                }
                _ = gcs_heartbeat.tick() => {
                    // Errors are ignored: on `udpin` the peer address may not
                    // be known yet Voice
                    if conn
                        .send(
                            &MavHeader {
                                system_id: config.system_id,
                                component_id: config.component_id,
                                sequence: seq,
                            },
                            &gcs_heartbeat_message(),
                        )
                        .await
                        .is_ok()
                    {
                        seq = seq.wrapping_add(1);
                    }
                }
                _ = reconnect => {
                    break 'connected;
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        return;
                    }
                    break 'connected;
                }
            }
        }

        if *shutdown_rx.borrow() {
            return;
        }

        *inner.state.lock().await = LinkState::Reconnecting;
        tokio::select! {
            _ = tokio::time::sleep(config.reconnect_delay) => {}
            _ = reconnect_rx.changed() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_status_keeps_endpoint() {
        let endpoint = "udpin:0.0.0.0:14550".to_string();

        let alive = LinkStatus::alive(endpoint.clone());
        assert_eq!(alive.link_state, LinkState::Connected);
        assert!(alive.fc_alive);
        assert_eq!(alive.endpoint, endpoint);

        let lost = LinkStatus::lost(endpoint.clone());
        assert_eq!(lost.link_state, LinkState::Connected);
        assert!(!lost.fc_alive);
        assert_eq!(lost.endpoint, endpoint);
    }

    #[test]
    fn disconnected_status_clears_link() {
        let down = LinkStatus::disconnected();
        assert_eq!(down.link_state, LinkState::Disconnected);
        assert!(!down.fc_alive);
        assert!(down.endpoint.is_empty());
    }
}
