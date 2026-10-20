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
    now_utc_us, pump_raw, ForceArmAudit, RecordEvent, Recorder, RecorderConfig, RecorderStats,
    RECORDINGS_DIR,
};
use maggcs_core::safety::{ChecklistItemId, ForceArmReason};
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
    /// Last session directory (kept after stop: the Review panel and the
    /// DR7 report/export commands read its logs).
    last_dir: PMutex<Option<String>>,
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
    /// The current/last session's forced-arm authorization (L5), read from the
    /// session manifest; `None` when the session never needed one, has no
    /// manifest yet, or the manifest is unreadable.
    pub forced_arm: Option<ForcedArmSummary>,
}

/// The part of a forced-arm audit the project/review UI shows (PJ3).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ForcedArmSummary {
    /// The operator-selected reason (serde snake_case, as acknowledged).
    pub reason: String,
    pub note: Option<String>,
    pub time_utc_us: u64,
}

/// Read the forced-arm audit out of a session manifest (best-effort: a
/// missing or unreadable manifest simply means "no mark").
fn read_forced_arm(dir: Option<&str>) -> Option<ForcedArmSummary> {
    use maggcs_core::recorder::SessionManifest;
    let dir = dir?;
    let text = std::fs::read_to_string(std::path::Path::new(dir).join("session.json")).ok()?;
    let manifest: SessionManifest = serde_json::from_str(&text).ok()?;
    let audit = manifest.forced_arm?;
    Some(ForcedArmSummary {
        reason: serde_json::to_value(audit.reason)
            .ok()?
            .as_str()?
            .to_string(),
        note: audit.note,
        time_utc_us: audit.time_utc_us,
    })
}

impl AppState {
    /// The flight-recorder state.
    pub fn recorder(&self) -> &RecorderState {
        &self.recorder
    }
}

impl RecorderState {
    /// The running recorder, when a session is recording (DR7's drape
    /// samples ride it; alerts already do via the center bridge).
    #[must_use]
    pub fn running_recorder(&self) -> Option<std::sync::Arc<Recorder>> {
        self.inner.lock().as_ref().map(|r| r.recorder.clone())
    }

    /// Record a forced-arm authorization in the JSONL log and manifest.
    ///
    /// Returns an error if no session is recording or durable audit fails.
    pub async fn record_force_arm(
        &self,
        items: Vec<ChecklistItemId>,
        reason: ForceArmReason,
        note: Option<String>,
    ) -> Result<ForceArmAudit, String> {
        let running = self
            .inner
            .lock()
            .as_ref()
            .map(|r| r.recorder.clone())
            .ok_or_else(|| "flight recorder is not running; forced arm was not sent".to_string())?;
        let audit_items = items.clone();
        let audit_note = note.clone();
        let time_utc_us = tauri::async_runtime::spawn_blocking(move || {
            running.record_force_arm(audit_items, reason, audit_note)
        })
        .await
        .map_err(|e| format!("forced-arm audit task failed: {e}"))?
        .map_err(|e| format!("forced-arm audit could not be saved: {e}"))?;
        Ok(ForceArmAudit {
            items,
            reason,
            note,
            time_utc_us,
        })
    }

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
            gcs_system_id: link.system_id,
            gcs_component_id: link.component_id,
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
        // Keep the session dir for the post-flight Review panel (DR7).
        *self.last_dir.lock() = Some(recorder.dir().display().to_string());
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
            Some(r) => {
                let dir = r.recorder.dir().display().to_string();
                let forced_arm = read_forced_arm(Some(&dir));
                RecorderStatus {
                    running: true,
                    dir: Some(dir),
                    stats: Some(r.recorder.stats()),
                    forced_arm,
                }
            }
            None => RecorderStatus {
                running: false,
                dir: self.last_dir.lock().clone(),
                stats: *self.last.lock(),
                forced_arm: None,
            },
        }
    }

    /// The last session's directory (running or stopped).
    #[must_use]
    pub fn last_session_dir(&self) -> Option<String> {
        let running = self.inner.lock();
        match running.as_ref() {
            Some(r) => Some(r.recorder.dir().display().to_string()),
            None => self.last_dir.lock().clone(),
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

    #[tokio::test]
    async fn force_arm_audit_without_a_session_is_refused() {
        let state = RecorderState::default();
        let error = state
            .record_force_arm(
                vec![ChecklistItemId::Battery],
                ForceArmReason::SafetyOverride,
                None,
            )
            .await
            .expect_err("must not claim a forced arm was archived");
        assert!(error.contains("not running"), "{error}");
    }

    #[test]
    fn record_event_without_a_session_is_a_no_op() {
        let state = RecorderState::default();
        assert!(!state.record_event(RecordEvent::Note {
            text: "ignored".to_string(),
        }));
    }
}
