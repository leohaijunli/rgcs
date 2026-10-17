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
    /// Auto-adopt the first autopilot heartbeat seen on the link when it
    /// differs from the expected target ids above. When disabled, only the
    /// configured `target_system_id`/`target_component_id` pair is treated
    /// as the FC (ADR-003 ids are an expectation, not a hard pin).
    #[serde(default = "default_auto_discover")]
    pub auto_discover_target: bool,
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

fn default_auto_discover() -> bool {
    true
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
            auto_discover_target: true,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
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

/// Whether a read error only affects the one frame just read (a decode
/// failure) as opposed to the transport itself. Decode failures leave the byte
/// stream intact, so the worker keeps the link and reads on.
fn is_recoverable_read_error(e: &::mavlink::error::MessageReadError) -> bool {
    matches!(e, ::mavlink::error::MessageReadError::Parse(_))
}

/// Capacity of the raw-frame tap channel (the recorder subscribes to it; A4).
pub const RAW_EVENT_CAPACITY: usize = EVENT_CAPACITY;

/// A received MAVLink frame in its exact on-wire form (plan §3 A4).
///
/// The flight recorder's tlog stores these bytes verbatim behind a timestamp
/// prefix, so a replay is byte-identical to what the flight controller sent.
/// Unlike [`ConnectionEvent::Message`], the tap is only fed while a subscriber
/// is attached ([`ConnectionHandle::subscribe_raw`]), so the extra byte copy
/// costs nothing for the normal telemetry path.
#[derive(Debug, Clone)]
pub struct RawFrame {
    /// Frame header (system/component ids, sequence number).
    pub header: MavHeader,
    /// Exact MAVLink frame bytes (STX … checksum, signature included).
    pub bytes: Vec<u8>,
    /// Monotonic time the frame was received.
    pub received_at: Instant,
    /// Wall-clock time the frame was received, in microseconds since the Unix
    /// epoch — the tlog timestamp (captured only while a subscriber is attached).
    pub utc_us: u64,
}

/// Microseconds since the Unix epoch (`0` before the epoch).
fn now_utc_us() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_micros().min(u64::MAX as u128) as u64,
        Err(_) => 0,
    }
}

/// The exact wire bytes of a raw frame, whichever MAVLink version carried it.
fn raw_wire_bytes(frame: &::mavlink::MAVLinkMessageRaw) -> &[u8] {
    match frame {
        ::mavlink::MAVLinkMessageRaw::V1(m) => m.raw_bytes(),
        ::mavlink::MAVLinkMessageRaw::V2(m) => m.raw_bytes(),
    }
}

/// Events emitted by the connection worker.
#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    /// Link established (first connect or reconnect).
    Connected { endpoint: Endpoint },
    /// An autopilot node was adopted as the FC from its first heartbeat,
    /// replacing the configured target expectation (auto-discovery).
    TargetDiscovered { system_id: u8, component_id: u8 },
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
#[derive(Debug)]
struct OutboundFrame {
    header: MavHeader,
    message: MavMessage,
    sent: Option<oneshot::Sender<Result<(), String>>>,
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
    raw: broadcast::Sender<RawFrame>,
}

impl ConnectionHandle {
    /// Identity of the underlying connection — stable for one link session;
    /// used to detect a link swap without comparing endpoints.
    pub fn handle_id(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    /// Subscribe to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<ConnectionEvent> {
        self.events.subscribe()
    }

    /// Subscribe to the raw-frame tap (plan §3 A4).
    ///
    /// The tap carries each received frame's exact wire bytes, so the recorder
    /// can write a lossless tlog. It is only populated while at least one
    /// receiver is attached; the copy is skipped entirely otherwise.
    pub fn subscribe_raw(&self) -> broadcast::Receiver<RawFrame> {
        self.raw.subscribe()
    }

    /// Subscribe to the event stream filtered by a [`MessageRoute`] (issue #20).
    ///
    /// Lifecycle events are always delivered; only inbound messages are
    /// filtered, so a subscriber is not woken for traffic it does not care
    /// about.
    /// Subscribe to the LOG protocol messages (log download, O1).
    pub fn subscribe_log_messages(&self) -> crate::mavlink::router::RoutedEvents {
        use ::mavlink::MessageData;
        self.subscribe_route(crate::mavlink::router::MessageRoute::messages(&[
            ::mavlink::common::LOG_ENTRY_DATA::ID,
            ::mavlink::common::LOG_DATA_DATA::ID,
        ]))
    }

    pub fn subscribe_route(
        &self,
        route: crate::mavlink::router::MessageRoute,
    ) -> crate::mavlink::router::RoutedEvents {
        crate::mavlink::router::RoutedEvents::new(self.events.subscribe(), route)
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
        let frame = OutboundFrame {
            header,
            message,
            sent: None,
        };
        self.inner
            .outbound
            .send(frame)
            .await
            .map_err(|_| MavlinkError::ChannelClosed)
    }

    /// Send a message and wait until the underlying transport's write call
    /// succeeds. Unlike [`Self::send`], this does not merely acknowledge queueing.
    pub async fn send_confirmed(&self, message: MavMessage) -> Result<(), MavlinkError> {
        let config = &self.inner.config;
        let (sent, confirmation) = oneshot::channel();
        self.inner
            .outbound
            .send(OutboundFrame {
                header: MavHeader {
                    system_id: config.system_id,
                    component_id: config.component_id,
                    sequence: 0,
                },
                message,
                sent: Some(sent),
            })
            .await
            .map_err(|_| MavlinkError::ChannelClosed)?;
        match confirmation.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(MavlinkError::Protocol(error)),
            Err(error) => Err(MavlinkError::WorkerGone(error.to_string())),
        }
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
    let (raw_tx, _raw_rx) = broadcast::channel(RAW_EVENT_CAPACITY);
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
        raw: raw_tx.clone(),
    };

    tokio::spawn(run_worker(
        inner,
        events_tx,
        raw_tx,
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

/// A HEARTBEAT from an autopilot node, eligible for auto-discovery adoption.
///
/// The autopilot check is the discriminator: our own GCS heartbeat is
/// `MAV_AUTOPILOT_GENERIC`, and the FC's system id may coincide with ours
/// (both default to 250), so no source-id exclusion is possible.
fn candidate_autopilot_node(env: &MessageEnvelope) -> Option<(u8, u8)> {
    match &env.message {
        MavMessage::HEARTBEAT(hb)
            if hb.autopilot != ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_GENERIC =>
        {
            Some((env.header.system_id, env.header.component_id))
        }
        _ => None,
    }
}

/// Connection worker task.
async fn run_worker(
    inner: Arc<ConnectionInner>,
    events: broadcast::Sender<ConnectionEvent>,
    raw: broadcast::Sender<RawFrame>,
    mut outbound_rx: mpsc::Receiver<OutboundFrame>,
    mut reconnect_rx: watch::Receiver<u64>,
    mut shutdown_rx: watch::Receiver<bool>,
    first_result: oneshot::Sender<Result<(), LinkFailure>>,
) {
    let config = &inner.config;
    let address = config.endpoint.to_address_string();
    let mut seq: u8 = 0;
    let mut first_result = Some(first_result);

    loop {
        if *shutdown_rx.borrow() {
            return;
        }
        *inner.state.lock().await = LinkState::Connecting;
        // Per-link discovery state: each fresh connection may expose a
        // different FC, so the monitor and the adopted node reset on reconnect.
        let mut heartbeat = HeartbeatMonitor::new(
            config.target_system_id,
            config.target_component_id,
            config.heartbeat_timeout,
        );
        let mut adopted_target: Option<(u8, u8)> = None;
        let mut fc_alive = false;
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
        let mut skipped_frames: u64 = 0;
        let mut watchdog = tokio::time::interval(HEARTBEAT_WATCHDOG_TICK);
        watchdog.tick().await;
        // GCS heartbeat so PX4 sees us as a live link partner (ADR-003,
        // issue #3): on `udpin` it also learns our reply address from the
        // first packet we send.
        let mut gcs_heartbeat = tokio::time::interval(GCS_HEARTBEAT_PERIOD);

        'connected: loop {
            let reconnect = reconnect_rx.changed();
            tokio::select! {
                res = conn.recv_raw() => match res {
                    Ok(frame) => {
                        let now = Instant::now();
                        let header = MavHeader {
                            sequence: frame.sequence(),
                            system_id: frame.system_id(),
                            component_id: frame.component_id(),
                        };
                        // Raw tap (A4): only copy/emit while a recorder is
                        // attached, so the telemetry path is unaffected.
                        if raw.receiver_count() > 0 {
                            let _ = raw.send(RawFrame {
                                header,
                                bytes: raw_wire_bytes(&frame).to_vec(),
                                received_at: now,
                                utc_us: now_utc_us(),
                            });
                        }
                        // Decode ourselves (instead of `conn.recv`) so the exact
                        // wire bytes are still in hand for the raw tap (A4).
                        // The library has already validated the framing/CRC;
                        // only a message the dialect cannot represent (e.g. PX4
                        // v1.17's out-of-range enum) fails here.
                        let message = match <MavMessage as ::mavlink::Message>::parse(
                            frame.version(),
                            frame.message_id(),
                            frame.payload(),
                        ) {
                            Ok(message) => message,
                            Err(e) => {
                                skipped_frames = skipped_frames.saturating_add(1);
                                if skipped_frames == 1 {
                                    eprintln!("mavlink: skipping undecodable frame: {e}");
                                }
                                continue 'connected;
                            }
                        };
                        let env = MessageEnvelope { header, message, received_at: now };

                        // Auto-discovery: adopt the first autopilot heartbeat
                        // on the link when it differs from the configured
                        // target. The FC is then tracked and addressed by its
                        // real ids instead of the config expectation.
                        let mut just_adopted = false;
                        if config.auto_discover_target && adopted_target.is_none() {
                            if let Some(node) = candidate_autopilot_node(&env) {
                                if node != (config.target_system_id, config.target_component_id) {
                                    let was_alive = fc_alive;
                                    adopted_target = Some(node);
                                    heartbeat.adopt(node.0, node.1, now);
                                    fc_alive = true;
                                    just_adopted = true;
                                    let _ = events.send(ConnectionEvent::TargetDiscovered {
                                        system_id: node.0,
                                        component_id: node.1,
                                    });
                                    if !was_alive {
                                        let _ =
                                            events.send(ConnectionEvent::HeartbeatRestored);
                                    }
                                }
                            }
                        }

                        let target =
                            adopted_target.unwrap_or((config.target_system_id, config.target_component_id));
                        let is_target_heartbeat = env.header.system_id == target.0
                            && env.header.component_id == target.1
                            && matches!(env.message, MavMessage::HEARTBEAT(_));
                        let _ = events.send(ConnectionEvent::Message(Box::new(env)));
                        if is_target_heartbeat && !just_adopted {
                            let was_alive = fc_alive;
                            heartbeat.observe(target.0, target.1, now);
                            fc_alive = true;
                            // First target heartbeat (configured or adopted)
                            // locks the node for this link: a QGC-like peer
                            // on the same bus must not steal the target.
                            if adopted_target.is_none() {
                                adopted_target = Some(target);
                            }
                            if !was_alive {
                                let _ = events.send(ConnectionEvent::HeartbeatRestored);
                            }
                        }
                    }
                    // One undecodable frame (e.g. PX4 v1.17 sends
                    // CURRENT_EVENT_SEQUENCE with flags = 0, which mavlink 0.17's
                    // non-bitmask `MavEventCurrentSequenceFlags` rejects) must not
                    // tear the link down. The frame was already consumed from the
                    // stream, so skip it and read the next one.
                    Err(e) if is_recoverable_read_error(&e) => {
                        skipped_frames = skipped_frames.saturating_add(1);
                        if skipped_frames == 1 {
                            eprintln!("mavlink: skipping undecodable frame: {e}");
                        }
                        continue 'connected;
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
                            let message = e.to_string();
                            if let Some(sent) = frame.sent {
                                let _ = sent.send(Err(message.clone()));
                            }
                            let _ = events.send(ConnectionEvent::LinkError(LinkFailure::from(
                                &MavlinkError::Protocol(message),
                            )));
                            break 'connected;
                        }
                        if let Some(sent) = frame.sent {
                            let _ = sent.send(Ok(()));
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
    fn autopilot_heartbeat_is_a_discovery_candidate() {
        let env = MessageEnvelope {
            header: MavHeader {
                system_id: 250,
                component_id: 1,
                sequence: 0,
            },
            message: MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
                custom_mode: 0,
                mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
                autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
                base_mode: ::mavlink::common::MavModeFlag::empty(),
                system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
                mavlink_version: 3,
            }),
            received_at: Instant::now(),
        };
        assert_eq!(
            candidate_autopilot_node(&env),
            Some((250, 1)),
            "an autopilot heartbeat is a candidate even when its system id equals ours"
        );
    }

    #[test]
    fn gcs_and_generic_heartbeats_are_not_candidates() {
        let hb = MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: ::mavlink::common::MavType::MAV_TYPE_GCS,
            autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_GENERIC,
            base_mode: ::mavlink::common::MavModeFlag::empty(),
            system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        });
        // Our own GCS heartbeat must never be adopted (autopilot GENERIC).
        let own = MessageEnvelope {
            header: MavHeader {
                system_id: 250,
                component_id: 250,
                sequence: 0,
            },
            message: hb.clone(),
            received_at: Instant::now(),
        };
        assert_eq!(
            candidate_autopilot_node(&own),
            None,
            "own GCS heartbeat is ignored"
        );
        // Generic-autopilot nodes are not a flight controller.
        let generic = MessageEnvelope {
            header: MavHeader {
                system_id: 2,
                component_id: 190,
                sequence: 0,
            },
            message: hb,
            received_at: Instant::now(),
        };
        assert_eq!(candidate_autopilot_node(&generic), None);
        // Non-heartbeat traffic is not a candidate.
        let att = MessageEnvelope {
            header: MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            message: MavMessage::ATTITUDE(::mavlink::common::ATTITUDE_DATA {
                time_boot_ms: 0,
                roll: 0.0,
                pitch: 0.0,
                yaw: 0.0,
                rollspeed: 0.0,
                pitchspeed: 0.0,
                yawspeed: 0.0,
            }),
            received_at: Instant::now(),
        };
        assert_eq!(candidate_autopilot_node(&att), None);
    }

    use super::is_recoverable_read_error;
    use ::mavlink::error::{MessageReadError, ParserError};

    #[test]
    fn decode_failures_are_recoverable_but_io_is_not() {
        // PX4 v1.17 CURRENT_EVENT_SEQUENCE with flags = 0.
        let invalid_enum = MessageReadError::Parse(ParserError::InvalidEnum {
            enum_type: "MavEventCurrentSequenceFlags",
            value: 0,
        });
        assert!(is_recoverable_read_error(&invalid_enum));
        let unknown = MessageReadError::Parse(ParserError::UnknownMessage { id: 99_999 });
        assert!(is_recoverable_read_error(&unknown));
        let io = MessageReadError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        assert!(!is_recoverable_read_error(&io));
    }

    #[tokio::test]
    async fn the_raw_tap_carries_the_exact_wire_bytes() {
        use tokio::net::UdpSocket;

        // Reserve an ephemeral port, then hand it to the listener.
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").expect("probe port");
        let port = probe.local_addr().expect("addr").port();
        drop(probe);

        let config = ConnectionConfig {
            endpoint: Endpoint::UdpListener {
                addr: format!("127.0.0.1:{port}").parse().expect("addr"),
            },
            ..Default::default()
        };
        let (handle, _events, first) = spawn_connection(config).await.expect("spawn");
        first.await.expect("first result").expect("connected");

        let mut raw = handle.subscribe_raw();
        let msg = MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
            autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: ::mavlink::common::MavModeFlag::empty(),
            system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        });
        let header = MavHeader {
            system_id: 1,
            component_id: 1,
            sequence: 7,
        };
        let mut wire = Vec::new();
        ::mavlink::write_v2_msg(&mut wire, header, &msg).expect("serialize");

        let client = UdpSocket::bind("127.0.0.1:0").await.expect("client");
        client
            .send_to(&wire, format!("127.0.0.1:{port}"))
            .await
            .expect("send");

        let frame = tokio::time::timeout(Duration::from_secs(2), raw.recv())
            .await
            .expect("raw frame in time")
            .expect("raw frame");
        assert_eq!(frame.bytes, wire, "the tap must be byte-exact");
        assert_eq!(frame.header.sequence, 7);
        assert!(frame.utc_us > 0);

        handle.shutdown().await;
    }

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
