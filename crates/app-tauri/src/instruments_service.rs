//! Walk-instrument service (W1): attaches `core::instruments::run_instruments`
//! to the primary link and forwards typed samples and stream-health events to a
//! window over a Tauri channel. A thin adapter like `inspector_service`:
//! decoding, monitoring and the synthetic generator live in `core` (ADR-020).

use std::sync::Arc;

use maggcs_core::instruments::{
    default_sources, route, run_instruments, InstrumentEvent, MonitorConfig, StreamHealth,
    StreamMonitor, INSTRUMENT_CHANNEL_CAP,
};
use maggcs_core::mavlink::connection::ConnectionHandle;
use parking_lot::Mutex as PMutex;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::{mpsc, oneshot, Mutex as TMutex};

use crate::state::AppState;

/// The running instrument runner and the window channel it feeds. Survives link
/// swaps (like `InspectorState`): a reconnect re-attaches without losing the
/// window.
pub struct InstrumentsState {
    /// The running runner's stop signal, one per attached link. Re-attaching
    /// (reconnect) or closing the window fires it so the runner exits promptly.
    runner: PMutex<Option<oneshot::Sender<()>>>,
    /// The window's event channel, registered by `instruments_connect`.
    window: Arc<PMutex<Option<Channel<InstrumentEvent>>>>,
    /// Shared stream monitor (health readable without a window).
    monitor: Arc<TMutex<StreamMonitor>>,
}

/// A snapshot of the instrument link for a status badge.
#[derive(Debug, Clone, Serialize)]
pub struct InstrumentsStatus {
    /// A primary link exists (the runner can stream).
    pub connected: bool,
    /// A runner task is attached to a link.
    pub running: bool,
    /// A window channel is registered.
    pub window_open: bool,
    /// Coarse stream health.
    pub health: StreamHealth,
}

impl Default for InstrumentsState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// The walk-instrument runner state.
    pub fn instruments(&self) -> &InstrumentsState {
        &self.instruments
    }
}

impl InstrumentsState {
    /// New state with the default operational thresholds.
    pub fn new() -> Self {
        Self {
            runner: PMutex::new(None),
            window: Arc::new(PMutex::new(None)),
            monitor: Arc::new(TMutex::new(StreamMonitor::new(MonitorConfig::default()))),
        }
    }

    /// Whether a window is registered (so a link (re)connect may attach).
    pub fn has_window(&self) -> bool {
        self.window.lock().is_some()
    }

    /// Stop the current runner (if any). Idempotent.
    pub fn detach(&self) {
        if let Some(stop) = self.runner.lock().take() {
            let _ = stop.send(());
        }
    }

    /// Attach the runner to a link and forward its events to the window.
    pub fn attach(&self, handle: ConnectionHandle) {
        self.detach();
        let events = handle.subscribe_route(route());
        let monitor = self.monitor.clone();
        let (tx, rx) = mpsc::channel(INSTRUMENT_CHANNEL_CAP);
        let (stop_tx, stop_rx) = oneshot::channel();
        *self.runner.lock() = Some(stop_tx);
        tauri::async_runtime::spawn(run_instruments(
            events,
            default_sources(),
            monitor,
            tx,
            stop_rx,
        ));
        tauri::async_runtime::spawn(forward_loop(rx, self.window.clone()));
    }

    /// Register the window channel and attach to a link if one is present.
    pub fn connect(&self, handle: Option<ConnectionHandle>, channel: Channel<InstrumentEvent>) {
        *self.window.lock() = Some(channel);
        if let Some(handle) = handle {
            self.attach(handle);
        }
    }

    /// The window closed: stop the runner and drop the channel.
    pub fn disconnect(&self) {
        self.detach();
        *self.window.lock() = None;
    }

    /// A status snapshot.
    pub async fn status(&self, connected: bool) -> InstrumentsStatus {
        // Drop the synchronous locks before the await: `parking_lot` guards are
        // not `Send`, so holding one across `.await` breaks the command future.
        let running = self.runner.lock().is_some();
        let window_open = self.window.lock().is_some();
        let health = self.monitor.lock().await.health();
        InstrumentsStatus {
            connected,
            running,
            window_open,
            health,
        }
    }
}

/// Forward runner events to the window channel until the runner ends.
async fn forward_loop(
    mut rx: mpsc::Receiver<InstrumentEvent>,
    window: Arc<PMutex<Option<Channel<InstrumentEvent>>>>,
) {
    while let Some(event) = rx.recv().await {
        let channel = window.lock().clone();
        if let Some(channel) = channel {
            let _ = channel.send(event);
        }
    }
}

/// Register the window's instrument channel and attach to the current link. A
/// missing link is fine: the runner attaches when a link comes up.
#[tauri::command]
pub fn instruments_connect(
    state: State<'_, AppState>,
    channel: Channel<InstrumentEvent>,
) -> Result<(), String> {
    state.instruments().connect(state.connection(), channel);
    Ok(())
}

/// Stop streaming to this window (the window itself stays open).
#[tauri::command]
pub fn instruments_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    state.instruments().disconnect();
    Ok(())
}

/// Snapshot of the runner and stream health, for a status badge.
#[tauri::command]
pub async fn instruments_status(state: State<'_, AppState>) -> Result<InstrumentsStatus, String> {
    let connected = state.connection().is_some();
    Ok(state.instruments().status(connected).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_state_is_idle_and_unknown() {
        let state = InstrumentsState::new();
        assert!(!state.has_window());
        let status = state.status(false).await;
        assert!(!status.connected);
        assert!(!status.running);
        assert!(!status.window_open);
        assert_eq!(status.health, StreamHealth::Unknown);
    }

    #[test]
    fn detach_without_attach_is_a_no_op() {
        let state = InstrumentsState::new();
        state.detach();
        state.detach();
        assert!(!state.has_window());
    }
}
