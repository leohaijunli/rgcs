//! Telemetry hub: aggregates MAVLink messages and publishes throttled
//! snapshots + link status on `watch` channels.
//!
//! Lives in `core` (ADR-001) so the desktop app and the headless server share
//! the same aggregation and throttling logic. The desktop pump is a thin
//! adapter forwarding hub channels onto the Tauri event bus.

use std::time::Duration;

use tokio::sync::watch;

use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle, LinkStatus};
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
    /// Spawn the pump task for `conn`, pushing snapshots at `hz` Hz.
    pub fn spawn(conn: ConnectionHandle, hz: u64) -> Self {
        let (snapshot_tx, snapshot_rx) = watch::channel(TelemetrySnapshot::default());
        let (link_tx, link_rx) = watch::channel(LinkStatus::disconnected());
        let (error_tx, error_rx) = watch::channel(None);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(pump(conn, hz, snapshot_tx, link_tx, error_tx, shutdown_rx));

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
    conn: ConnectionHandle,
    hz: u64,
    snapshot_tx: watch::Sender<TelemetrySnapshot>,
    link_tx: watch::Sender<LinkStatus>,
    error_tx: watch::Sender<Option<String>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut events = conn.subscribe();
    let mut agg = TelemetryAggregator::new();
    let interval = Duration::from_millis(1000 / hz.max(1));

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
                    let _ = link_tx.send(LinkStatus::alive());
                }
                Ok(ConnectionEvent::HeartbeatLost { .. }) => {
                    let _ = link_tx.send(LinkStatus::lost());
                }
                Ok(ConnectionEvent::Connected { endpoint }) => {
                    let _ = link_tx.send(LinkStatus {
                        link_state: LinkState::Connected,
                        endpoint: endpoint.to_address_string(),
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
