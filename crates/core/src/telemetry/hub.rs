//! Telemetry hub: aggregates MAVLink messages and publishes throttled
//! snapshots + link status on `watch` channels.
//!
//! Lives in `core` (ADR-001) so the desktop app and the headless server share
//! the same aggregation and throttling logic. The desktop pump is a thin
//! adapter forwarding hub channels onto the Tauri event bus.

use std::time::Duration;

use tokio::sync::{broadcast, watch};

use crate::mavlink::connection::{ConnectionEvent, LinkStatus};
use crate::mavlink::router::{MessageRoute, RoutedEvents};
use crate::mavlink::LinkState;
use crate::telemetry::{
    TelemetryAggregator, TelemetryError, TelemetrySnapshot, TelemetryUpdate, VehicleId,
};

/// Default snapshot push rate.
pub const DEFAULT_PUSH_HZ: u64 = 20;

/// Buffer depth for the lossless error channel.
const ERROR_BUFFER: usize = 64;

/// Aggregation/throttling hub owned by the app layer.
#[derive(Debug, Clone)]
pub struct TelemetryHub {
    snapshot_rx: watch::Receiver<TelemetrySnapshot>,
    link_rx: watch::Receiver<LinkStatus>,
    error_tx: broadcast::Sender<TelemetryError>,
    dropped_rx: watch::Receiver<u64>,
    shutdown_tx: watch::Sender<bool>,
}

impl TelemetryHub {
    /// Spawn the pump task, consuming the connection's initial event stream
    /// and pushing snapshots at `hz` Hz.
    ///
    /// The receiver is taken from [`crate::mavlink::connection::spawn_connection`]
    /// so the pump is subscribed to the event bus from the very first event;
    /// subscribing afterwards (in the pump task itself) could miss a `Connected`
    /// already emitted and leave the UI reporting a dead link (issue #2).
    /// `target` is the FC whose telemetry is aggregated; frames from other
    /// nodes on the link are dropped (issue #6).
    pub fn spawn(events: broadcast::Receiver<ConnectionEvent>, hz: u64, target: VehicleId) -> Self {
        let (snapshot_tx, snapshot_rx) = watch::channel(TelemetrySnapshot::default());
        let (link_tx, link_rx) = watch::channel(LinkStatus::disconnected());
        // Errors go through a broadcast channel so two errors arriving between
        // reads are not collapsed into one (issues.md #17).
        let (error_tx, _error_rx) = broadcast::channel(ERROR_BUFFER);
        let (dropped_tx, dropped_rx) = watch::channel(0u64);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Route inbound messages from the target FC only (issue #20); the
        // aggregator keeps its own source check as a safety net (issue #6).
        let route = MessageRoute::from_node(target.system_id, target.component_id);
        tokio::spawn(pump(
            RoutedEvents::new(events, route),
            hz,
            target,
            PumpSinks {
                snapshot: snapshot_tx,
                link: link_tx,
                error: error_tx.clone(),
                dropped: dropped_tx,
            },
            shutdown_rx,
        ));

        Self {
            snapshot_rx,
            link_rx,
            error_tx,
            dropped_rx,
            shutdown_tx,
        }
    }

    /// Latest snapshot.
    pub fn snapshot(&self) -> TelemetrySnapshot {
        self.snapshot_rx.borrow().clone()
    }

    /// Subscribe to throttled snapshots.
    pub fn subscribe_snapshot(&self) -> watch::Receiver<TelemetrySnapshot> {
        self.snapshot_rx.clone()
    }

    /// Subscribe to link status changes.
    pub fn subscribe_link(&self) -> watch::Receiver<LinkStatus> {
        self.link_rx.clone()
    }

    /// Subscribe to link errors (lossless: every error is delivered).
    pub fn subscribe_error(&self) -> broadcast::Receiver<TelemetryError> {
        self.error_tx.subscribe()
    }

    /// Total inbound frames dropped because the pump lagged the event bus.
    pub fn subscribe_dropped_frames(&self) -> watch::Receiver<u64> {
        self.dropped_rx.clone()
    }

    /// Ask the pump to stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Pump task: fold messages, publish snapshots at `hz`, link on change.
struct PumpSinks {
    snapshot: watch::Sender<TelemetrySnapshot>,
    link: watch::Sender<LinkStatus>,
    error: broadcast::Sender<TelemetryError>,
    dropped: watch::Sender<u64>,
}

async fn pump(
    mut events: RoutedEvents,
    hz: u64,
    target: VehicleId,
    sinks: PumpSinks,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let PumpSinks {
        snapshot: snapshot_tx,
        link: link_tx,
        error: error_tx,
        dropped: dropped_tx,
    } = sinks;
    let mut agg = TelemetryAggregator::for_vehicle(target);
    let mut dropped_frames: u64 = 0;
    let mut dirty = false;
    let period = Duration::from_millis(1000 / hz.max(1));
    // Latched from `ConnectionEvent::Connected` so heartbeat transitions keep
    // reporting the endpoint the UI is connected to.
    let mut endpoint = String::new();
    // Event-driven: inbound frames are consumed as soon as they arrive and
    // snapshots are throttled to `hz` by a ticker, instead of polling the bus
    // with `try_recv` + `sleep` (issues.md #19). The first tick fires
    // immediately, so a snapshot is never delayed by more than one period.
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            ev = events.recv() => {
                match ev {
                Ok(ConnectionEvent::Message(env)) => {
                    let source = VehicleId::from_envelope(&env);
                    if agg.accepts(source) {
                        if let Some(update) = TelemetryUpdate::try_from_envelope(&env) {
                            agg.apply(source, update, epoch_ms());
                            dirty = true;
                        }
                    }
                }
                Ok(ConnectionEvent::HeartbeatRestored) => {
                    let _ = link_tx.send(LinkStatus::alive(endpoint.clone()));
                }
                Ok(ConnectionEvent::HeartbeatLost { .. }) => {
                    let _ = link_tx.send(LinkStatus::lost(endpoint.clone()));
                }
                Ok(ConnectionEvent::Connected {
                    endpoint: connected,
                }) => {
                    endpoint = connected.to_address_string();
                    let _ = link_tx.send(LinkStatus {
                        link_state: LinkState::Connected,
                        endpoint: endpoint.clone(),
                        fc_alive: false,
                    });
                }
                Ok(ConnectionEvent::LinkError(f)) => {
                    let _ = error_tx.send(TelemetryError {
                        kind: f.kind,
                        message: f.message,
                        at_ms: epoch_ms(),
                    });
                }
                Ok(ConnectionEvent::Failed(f)) => {
                    let _ = link_tx.send(LinkStatus::disconnected());
                    let _ = error_tx.send(TelemetryError {
                        kind: f.kind,
                        message: f.message,
                        at_ms: epoch_ms(),
                    });
                    return;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    // Do not drop silently: count and expose it (issues.md #17).
                    dropped_frames = dropped_frames.saturating_add(n);
                    let _ = dropped_tx.send(dropped_frames);
                }
            }
        }
            _ = ticker.tick() => {
                if dirty {
                    let _ = snapshot_tx.send(agg.snapshot().clone());
                    dirty = false;
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { return; }
            }
        }
    }
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::connection::{ConnectionEvent, LinkErrorKind, LinkFailure};
    use crate::mavlink::message::{MavMessage, MessageEnvelope};
    use crate::mavlink::{Endpoint, LinkState};

    fn link_error_event(message: &str) -> ConnectionEvent {
        ConnectionEvent::LinkError(LinkFailure {
            kind: LinkErrorKind::Protocol,
            message: message.to_string(),
        })
    }

    fn heartbeat_event(system_id: u8, component_id: u8, custom_mode: u32) -> ConnectionEvent {
        ConnectionEvent::Message(Box::new(MessageEnvelope {
            header: ::mavlink::MavHeader {
                system_id,
                component_id,
                sequence: 0,
            },
            message: MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
                custom_mode,
                mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
                autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
                base_mode: ::mavlink::common::MavModeFlag::empty(),
                system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
                mavlink_version: 3,
            }),
            received_at: std::time::Instant::now(),
        }))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn connected_emitted_before_spawn_is_not_lost() {
        // Issue #2 regression: the pump consumes the initial event stream, so a
        // `Connected` emitted before `TelemetryHub::spawn` runs must still reach
        // the link status channel.
        let (events_tx, events_rx) = broadcast::channel(16);
        events_tx
            .send(ConnectionEvent::Connected {
                endpoint: Endpoint::UdpListener {
                    addr: "0.0.0.0:14550".parse().expect("static addr"),
                },
            })
            .expect("send");

        let hub = TelemetryHub::spawn(events_rx, 100, VehicleId::new(1, 1));
        let mut link_rx = hub.subscribe_link();

        let status = tokio::time::timeout(Duration::from_secs(2), link_rx.changed())
            .await
            .expect("link status update timed out");
        assert!(status.is_ok());
        assert_eq!(link_rx.borrow().link_state, LinkState::Connected);
        assert!(!link_rx.borrow().fc_alive);
        hub.shutdown();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hub_drops_foreign_node_telemetry() {
        // Issue #6: a heartbeat from a QGC-like node on the same link must not
        // reach the aggregated snapshot.
        let (events_tx, events_rx) = broadcast::channel(16);
        let hub = TelemetryHub::spawn(events_rx, 100, VehicleId::new(1, 1));
        let mut snap_rx = hub.subscribe_snapshot();

        events_tx
            .send(heartbeat_event(255, 190, 0x0504_0000))
            .expect("send foreign");
        events_tx
            .send(heartbeat_event(1, 1, 0x0004_0000))
            .expect("send target");

        let heartbeat = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let current = snap_rx.borrow_and_update().heartbeat;
                if let Some(hb) = current {
                    return Some(hb);
                }
                if snap_rx.changed().await.is_err() {
                    return None;
                }
            }
        })
        .await
        .expect("snapshot timed out")
        .expect("target heartbeat never reached the snapshot");

        assert_eq!(heartbeat.custom_mode, 0x0004_0000);
        hub.shutdown();
    }

    #[tokio::test]
    async fn errors_are_delivered_losslessly() {
        // Issue #17: two errors arriving between reads must both reach the
        // subscriber (a `watch` slot would collapse them into one).
        let (events_tx, events_rx) = broadcast::channel(16);
        let hub = TelemetryHub::spawn(events_rx, 100, VehicleId::new(1, 1));
        let mut err_rx = hub.subscribe_error();

        events_tx.send(link_error_event("first")).expect("send 1");
        events_tx.send(link_error_event("second")).expect("send 2");

        let first = tokio::time::timeout(Duration::from_secs(2), err_rx.recv())
            .await
            .expect("first error timed out")
            .expect("first error delivered");
        let second = tokio::time::timeout(Duration::from_secs(2), err_rx.recv())
            .await
            .expect("second error timed out")
            .expect("second error delivered");

        assert_eq!(first.message, "first");
        assert_eq!(second.message, "second");
        assert_eq!(first.kind, LinkErrorKind::Protocol);
        assert!(first.at_ms <= second.at_ms);
        hub.shutdown();
    }

    #[tokio::test]
    async fn lagging_pump_reports_dropped_frames() {
        // Issue #17: overrunning the event bus must not drop frames silently;
        // the cumulative counter is exposed for link diagnostics.
        let (events_tx, events_rx) = broadcast::channel(8);
        let hub = TelemetryHub::spawn(events_rx, 10, VehicleId::new(1, 1));
        let mut dropped_rx = hub.subscribe_dropped_frames();

        // No `.await` between sends, so the pump cannot drain the 8-slot bus
        // before it overflows.
        for _ in 0..200 {
            events_tx
                .send(heartbeat_event(1, 1, 0x0004_0000))
                .expect("send");
        }

        let dropped = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let current = *dropped_rx.borrow_and_update();
                if current > 0 {
                    return current;
                }
                if dropped_rx.changed().await.is_err() {
                    return 0;
                }
            }
        })
        .await
        .expect("dropped-frame counter timed out");

        assert!(dropped > 0, "lagging pump must report dropped frames");
        hub.shutdown();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn snapshot_latency_is_bounded() {
        // Issue #19: the pump is event-driven (no `try_recv` + `sleep` poll),
        // so a frame must surface within a small multiple of the publish
        // period.
        let (events_tx, events_rx) = broadcast::channel(16);
        let hub = TelemetryHub::spawn(events_rx, DEFAULT_PUSH_HZ, VehicleId::new(1, 1));
        let mut snap_rx = hub.subscribe_snapshot();

        let start = tokio::time::Instant::now();
        events_tx
            .send(heartbeat_event(1, 1, 0x0004_0000))
            .expect("send");

        let heartbeat = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(hb) = snap_rx.borrow_and_update().heartbeat {
                    return hb;
                }
                if snap_rx.changed().await.is_err() {
                    panic!("snapshot channel closed");
                }
            }
        })
        .await
        .expect("snapshot timed out");

        assert_eq!(heartbeat.custom_mode, 0x0004_0000);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "snapshot took {:?}",
            start.elapsed()
        );
        hub.shutdown();
    }
}
