//! Flight-recorder adapter (plan §3 A4): owns the core [`Recorder`] for the
//! primary link, pumps the connection's raw-frame tap into its tlog, and
//! mirrors alert-center transitions into its JSONL event log.
//!
//! One recorder per link: `connect` retires the previous session (flush + join
//! in the background) and opens a new one; `disconnect`/shutdown stop it through
//! the teardown supervisor's `Recorder` stage, so its files are always flushed
//! after the services that feed it. A link swap never blocks on the flush.

use std::sync::Arc;
use std::time::Duration;

use maggcs_core::lifecycle::StopOutcome;
use maggcs_core::mavlink::connection::ConnectionHandle;
use maggcs_core::recorder::{
    now_utc_us, pump_raw, RecordEvent, Recorder, RecorderConfig, RecorderStats, RECORDINGS_DIR,
};
use parking_lot::Mutex as PMutex;
use tauri::{AppHandle, Manager, State};
use tokio::sync::{broadcast, watch};

use crate::alert_service::AlertState;
use crate::state::AppState;

/// How long a background retire waits for a flushed, joined writer.
const RETIRE_TIMEOUT: Duration = Duration::from_secs(2);

/// The current recording, if any.
struct Running {
    recorder: Arc<Recorder>,
    /// Flipped to `true` to end the tap/alert pump tasks.
    stop: watch::Sender<bool>,
}

/// Process-wide recorder state (survives webview reloads, not link swaps).
#[derive(Default)]
pub struct RecorderState {
    inner: PMutex<Option<Running>>,
    /// Last counters, for a status badge after a session has closed.
    last: PMutex<Option<RecorderStats>>,
}

/// What the UI shows about the recorder.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecorderStatus {
    /// A session is recording right now.
    pub running: bool,
    /// Session directory of the current (or last) recording.
    pub dir: Option<String>,
    /// Live counters (or the last snapshot once stopped).
    pub stats: Option<RecorderStats>,
}

impl AppState {
    /// The flight-recorder state.
    pub fn recorder(&self) -> &RecorderState {
        &self.recorder
    }
}

impl RecorderState {
    /// Enqueue an application event into the live session's JSONL log.
    ///
    /// Returns `false` when nothing is recording, so callers can ignore it.
    pub fn record_event(&self, event: RecordEvent) -> bool {
        match self.inner.lock().as_ref() {
            Some(running) => {
                running.recorder.record_event(event);
                true
            }
            None => false,
        }
    }

    /// Start a new session for `handle`, retiring any previous one first.
    pub fn start(
        &self,
        app: &AppHandle,
        handle: &ConnectionHandle,
        alerts: Arc<AlertState>,
    ) -> Result<(), String> {
        self.retire();
        let base = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("cannot resolve app data dir: {e}"))?
            .join(RECORDINGS_DIR);
        let started = now_utc_us();
        let dir = base.join(format!("session-{started}"));
        let link = handle.config();
        let config = RecorderConfig {
            endpoint: link.endpoint.to_address_string(),
            target_system_id: link.target_system_id,
            target_component_id: link.target_component_id,
            ..RecorderConfig::new(&dir)
        };
        let recorder = Recorder::spawn(config).map_err(|e| e.to_string())?;
        recorder.record_event(RecordEvent::SessionOpened {
            endpoint: link.endpoint.to_address_string(),
            system_id: link.target_system_id,
            component_id: link.target_component_id,
        });

        let (stop_tx, stop_rx) = watch::channel(false);
        tauri::async_runtime::spawn(pump_raw(
            recorder.clone(),
            handle.subscribe_raw(),
            stop_rx.clone(),
        ));
        tauri::async_runtime::spawn(alert_feed(recorder.clone(), alerts, stop_rx));
        *self.inner.lock() = Some(Running {
            recorder,
            stop: stop_tx,
        });
        Ok(())
    }

    /// Stop the current session, flushing and joining the writer.
    pub async fn stop(&self, deadline: Duration) -> StopOutcome {
        // Take the running state and drop the synchronous lock before awaiting.
        let running = self.inner.lock().take();
        let Some(running) = running else {
            return StopOutcome::NotRunning;
        };
        let _ = running.stop.send(true);
        let recorder = running.recorder;
        recorder.record_event(RecordEvent::SessionClosed {
            frames: recorder.stats().frames,
            events: recorder.stats().events,
            dropped: recorder.stats().dropped,
        });
        let outcome = recorder.flush(deadline).await;
        *self.last.lock() = Some(recorder.stats());
        outcome
    }

    /// End the pump tasks and flush the previous session in the background, so
    /// a link swap never waits on the disk.
    fn retire(&self) {
        let running = self.inner.lock().take();
        if let Some(running) = running {
            let _ = running.stop.send(true);
            let recorder = running.recorder;
            tauri::async_runtime::spawn(async move {
                let _ = recorder.flush(RETIRE_TIMEOUT).await;
            });
        }
    }

    /// A status snapshot for the UI.
    pub fn status(&self) -> RecorderStatus {
        let running = self.inner.lock();
        match running.as_ref() {
            Some(r) => RecorderStatus {
                running: true,
                dir: Some(r.recorder.dir().display().to_string()),
                stats: Some(r.recorder.stats()),
            },
            None => RecorderStatus {
                running: false,
                dir: None,
                stats: *self.last.lock(),
            },
        }
    }
}

/// Mirror alert-center transitions into the session's event log.
async fn alert_feed(
    recorder: Arc<Recorder>,
    alerts: Arc<AlertState>,
    mut stop: watch::Receiver<bool>,
) {
    let mut rx = alerts.subscribe();
    loop {
        if *stop.borrow() {
            return;
        }
        tokio::select! {
            _ = stop.changed() => {
                if *stop.borrow() {
                    return;
                }
            }
            res = rx.recv() => match res {
                Ok(event) => {
                    let alert = event.alert;
                    recorder.record_event(RecordEvent::Alert {
                        severity: alert.severity,
                        source: alert.source,
                        text: alert.text,
                        key: Some(alert.id),
                    });
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

/// Recording status (running, directory, counters).
#[tauri::command]
pub fn recorder_status(state: State<'_, AppState>) -> RecorderStatus {
    state.recorder().status()
}

/// Append an application event to the live session's JSONL log (modes, mission
/// syncs, walk-survey line markers). A no-op when nothing is recording.
#[tauri::command]
pub fn recorder_log_event(state: State<'_, AppState>, event: RecordEvent) -> bool {
    state.recorder().record_event(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_state_reports_not_running() {
        let state = RecorderState::default();
        let status = state.status();
        assert!(!status.running);
        assert!(status.dir.is_none());
        assert!(status.stats.is_none());
    }

    #[test]
    fn record_event_without_a_session_is_a_no_op() {
        let state = RecorderState::default();
        assert!(!state.record_event(RecordEvent::Note {
            text: "ignored".to_string(),
        }));
    }
}
