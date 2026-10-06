//! Telemetry hub: aggregates MAVLink messages and publishes throttled
//! snapshots + link status on `watch` channels.
//!
//! Lives in `core` (ADR-001) so the desktop app and the headless server share
//! the same aggregation and throttling logic. The desktop pump is a thin
//! adapter forwarding hub channels onto the Tauri event bus.

use std::time::Duration;

use tokio::sync::{broadcast, watch};

use crate::mavlink::connection::{ConnectionEvent, LinkStatus};
use crate::mavlink::LinkState;
use crate::telemetry::{TelemetryAggregator, TelemetrySnapshot, TelemetryUpdate};

/// Default snapshot push rate.
pub const DEFAULT_PUSH_HZ: u64 = 20;

/// Aggregation/throttling hub owned by the app layer.
#[derive(Debug, Clone)]
pub struct TelemetryHub {
    snapshot_rx: watch::Receiver<TelemetrySnapshot>,
    link_rx: watch::Receiver<LinkStatus>,
    error_rx: watch::Receiver<Option<String>>,
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
    pub fn spawn(
        events: broadcast::Receiver<ConnectionEvent>,
        hz: u64,
    ) -> Self {
        let (snapshot_tx, snapshot_rx) = watch::channel(TelemetrySnapshot::default());
        let (link_tx, link_rx) = watch::channel(LinkStatus::disconnected());
        let (error_tx, error_rx) = watch::channel(None);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(pump(events, hz, snapshot_tx, link_tx, error_tx, shutdown_rx));

        Self {
            snapshot_rx,
            link_rx,
            error_rx,
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

    /// Subscribe to transient error messages.
    pub fn subscribe_error(&self) -> watch::Receiver<Option<String>> {
        self.error_rx.clone()
    }

    /// Ask the pump to stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Pump task: fold messages, publish snapshots at `hz`, link on change.
async fn pump(
    mut events: broadcast::Receiver<ConnectionEvent>,
    hz: u64,
    snapshot_tx: watch::Sender<TelemetrySnapshot>,
    link_tx: watch::Sender<LinkStatus>,
    error_tx: watch::Sender<Option<String>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut agg = TelemetryAggregator::new();
    let interval = Duration::from_millis(1000 / hz.max(1));
    // Latched from `ConnectionEvent::Connected` so heartbeat transitions keep
    // reporting the endpoint the UI is connected to.
    let mut endpoint = String::new();

    loop {
        let mut dirty = false;
        // Drain currently buffered events.
        loop {
            match events.try_recv() {
                Ok(ConnectionEvent::Message(env)) => {
                    if let Some(update) = TelemetryUpdate::try_from_envelope(&env) {
                        let now_ms = epoch_ms();
                        agg.apply(update, now_ms);
                        dirty = true;
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
                    let _ = error_tx.send(Some(f.message));
                }
                Ok(ConnectionEvent::Failed(f)) => {
                    let _ = link_tx.send(LinkStatus::disconnected());
                    let _ = error_tx.send(Some(f.message));
                    return;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }

        if dirty {
            let _ = snapshot_tx.send(agg.snapshot().clone());
        }

        if *shutdown_rx.borrow() {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
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
    use crate::mavlink::connection::ConnectionEvent;
    use crate::mavlink::{Endpoint, LinkState};

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

        let hub = TelemetryHub::spawn(events_rx, 100);
        let mut link_rx = hub.subscribe_link();

        let status = tokio::time::timeout(Duration::from_secs(2), link_rx.changed())
            .await
            .expect("link status update timed out");
        assert!(status.is_ok());
        assert_eq!(link_rx.borrow().link_state, LinkState::Connected);
        assert!(!link_rx.borrow().fc_alive);
        hub.shutdown();
    }
}
