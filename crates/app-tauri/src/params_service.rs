//! Parameters window service (parameters plan PM3): window lifecycle plus
//! Tauri command adapters. The `ParamService` protocol task lives in
//! `core::params::service` (ADR-001); it spawns on the primary link when the
//! window first connects and survives window close (the cache is read by
//! other services too, PM9), with a fresh task per reconnect.

use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use tauri::Runtime;
use tauri::State;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tauri::WindowEvent;

use maggcs_core::lifecycle::StopOutcome;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::params::model::{ParamType, ParamValue};
use maggcs_core::params::service::{ParamEvent, ParamService, SetResult};

use crate::state::AppState;

/// Process-wide parameter cache/events adapter. The active service runtime
/// itself is owned by `ActiveLink` and stops with that link.
#[derive(Default)]
pub struct ParamsState {
    last_cache: std::sync::Mutex<Option<maggcs_core::params::service::ParamCache>>,
}

/// Runtime owned by one active link: protocol task and UI event forwarder.
pub struct ParamsRuntime {
    svc: ParamService,
    protocol_task: Option<tokio::task::JoinHandle<()>>,
    forwarder: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl ParamsRuntime {
    /// Stop any active parameter transaction, then close and join the
    /// protocol task before the owning link is shut down.
    pub async fn stop(mut self, deadline: Duration) -> StopOutcome {
        let started = Instant::now();
        if let Some(task) = self.forwarder.take() {
            task.abort();
        }
        if tokio::time::timeout(deadline, self.svc.shutdown())
            .await
            .is_err()
        {
            if let Some(task) = &self.protocol_task {
                task.abort();
            }
            return StopOutcome::TimedOut {
                elapsed_ms: elapsed_ms(started),
            };
        }
        let Some(task) = self.protocol_task.take() else {
            return StopOutcome::NotRunning;
        };
        stop_protocol_task(task, deadline.saturating_sub(started.elapsed()), started).await
    }
}

impl Drop for ParamsRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.protocol_task {
            task.abort();
        }
        if let Some(task) = &self.forwarder {
            task.abort();
        }
    }
}

async fn stop_protocol_task(
    mut task: tokio::task::JoinHandle<()>,
    deadline: Duration,
    started: Instant,
) -> StopOutcome {
    match tokio::time::timeout(deadline, &mut task).await {
        Ok(Ok(())) => StopOutcome::Stopped {
            elapsed_ms: elapsed_ms(started),
        },
        Ok(Err(error)) => StopOutcome::Failed {
            message: format!("parameter protocol task failed: {error}"),
        },
        Err(_) => {
            task.abort();
            let _ = task.await;
            StopOutcome::TimedOut {
                elapsed_ms: elapsed_ms(started),
            }
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

impl ParamsState {
    /// Spawn a runtime bound to the specified link and retain its read cache.
    pub fn spawn<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        handle: ConnectionHandle,
    ) -> ParamsRuntime {
        let sys = handle.config().target_system_id;
        let comp = handle.config().target_component_id;
        let (svc, evt_rx, protocol_task) = ParamService::spawn_with_task(handle, sys, comp);
        *self.last_cache.lock().expect("params cache lock") = Some(svc.cache());
        let forwarder = tauri::async_runtime::spawn(forward_events(app.clone(), evt_rx));
        ParamsRuntime {
            svc,
            protocol_task: Some(protocol_task),
            forwarder: Some(forwarder),
        }
    }

    pub(crate) fn service(&self, link: &crate::state::ActiveLink) -> Option<ParamService> {
        link.params.as_ref().map(|runtime| runtime.svc.clone())
    }

    pub(crate) fn cache(&self) -> Option<maggcs_core::params::service::ParamCache> {
        self.last_cache.lock().ok()?.clone()
    }
}

/// Forward protocol events to the window (low-rate control events; batches
/// ride the same channel, ~100 entries each).
async fn forward_events<R: Runtime>(
    app: AppHandle<R>,
    mut rx: tokio::sync::broadcast::Receiver<ParamEvent>,
) {
    use maggcs_core::params::service::ParamEvent as E;
    while let Ok(ev) = rx.recv().await {
        let payload = match &ev {
            E::PullStarted { total } => ParamsEvent::PullStarted { total: *total },
            E::Batch(entries) => ParamsEvent::Batch {
                entries: entries.clone(),
            },
            E::Progress { received, total } => ParamsEvent::Progress {
                received: *received,
                total: *total,
            },
            E::PullDone { received, total } => ParamsEvent::PullDone {
                received: *received,
                total: *total,
            },
            E::ExternalChange(entry) => ParamsEvent::ExternalChange {
                name: entry.name.clone(),
                value: entry.value,
                ptype: entry.ptype,
            },
            E::FcVersion(v) => ParamsEvent::FcVersion { version: v.clone() },
        };
        let _ = app.emit("params_event", &payload);
    }
}

/// Serializable event payloads (`params_event`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamsEvent {
    PullStarted {
        total: u16,
    },
    Batch {
        entries: Vec<maggcs_core::params::model::ParamEntry>,
    },
    Progress {
        received: u16,
        total: u16,
    },
    PullDone {
        received: u16,
        total: u16,
    },
    ExternalChange {
        name: String,
        value: ParamValue,
        ptype: ParamType,
    },
    FcVersion {
        version: String,
    },
}

/// Open the parameters window, or focus it if it already exists.
/// Must run asynchronously: synchronous WebView2 creation deadlocks on Windows.
#[tauri::command]
pub async fn params_open(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("params") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, "params", WebviewUrl::App("params.html".into()))
        .title("Parameters")
        .min_inner_size(900.0, 600.0)
        .build()
        .map_err(|e| e.to_string())?;
    // Closing the window cancels any running pull (plan §4.3).
    let app_for_cleanup = app.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            let state = app_for_cleanup.state::<AppState>();
            if let Some(svc) = state.primary_params() {
                tauri::async_runtime::spawn(async move {
                    svc.cancel().await;
                });
            }
        }
    });
    Ok(())
}

/// Start a full pull. Events arrive on `params_event`; `Err` when a
/// transaction is already running or there is no link.
#[tauri::command]
pub async fn params_pull(state: State<'_, AppState>) -> Result<(), String> {
    let Some(svc) = state.primary_params() else {
        return Err("no link — connect first".into());
    };
    svc.pull().await.map_err(|e| format!("{e:?}"))
}

/// Cancel the running transaction (idempotent).
#[tauri::command]
pub async fn params_cancel(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(svc) = state.primary_params() {
        svc.cancel().await;
    }
    Ok(())
}

/// Set one parameter; resolves with the FC's echo.
#[tauri::command]
pub async fn params_set(
    state: State<'_, AppState>,
    name: String,
    value: maggcs_core::params::model::ParamValue,
    ptype: maggcs_core::params::model::ParamType,
) -> Result<SetEcho, String> {
    let Some(svc) = state.primary_params() else {
        return Err("no link — connect first".into());
    };
    let result = svc
        .set(&name, value, ptype)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(match result {
        SetResult::Confirmed(actual) => SetEcho {
            status: "confirmed".into(),
            actual,
            requested: None,
        },
        SetResult::Clamped { requested, actual } => SetEcho {
            status: "clamped".into(),
            actual,
            requested: Some(requested),
        },
        SetResult::Timeout => SetEcho {
            status: "timeout".into(),
            actual: value,
            requested: None,
        },
    })
}

/// Serializable set outcome for the window.
#[derive(Debug, Clone, Serialize)]
pub struct SetEcho {
    /// `confirmed` | `clamped` | `timeout`.
    pub status: String,
    /// The value the FC actually holds (the echo), or the request on timeout.
    pub actual: ParamValue,
    /// Present only when clamped: what we asked for.
    pub requested: Option<ParamValue>,
}

/// Reboot the flight controller (MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN,
/// param1 = 1). The link drops and comes back per the ADR-011 state model;
/// the window should re-pull after reconnecting.
/// How long `params_reboot` waits for the FC's `COMMAND_ACK`. The session's
/// own retry budget is ~4 s; the headroom tolerates slow links.
const REBOOT_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Reboot the flight controller (`MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN`,
/// param1=1). Awaits the FC's verdict: SITL denies this command (PX4 only
/// compiles the reboot path for `CONFIG_BOARDCTL_RESET` boards), so the UI
/// must see `denied` instead of assuming the reboot is on its way.
#[tauri::command]
pub async fn params_reboot(state: State<'_, AppState>) -> Result<String, String> {
    let service = state.command().ok_or("not connected")?;
    let result = maggcs_core::CommandService::send_await(
        &service,
        ::mavlink::common::MavCmd::MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN,
        [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        REBOOT_ACK_TIMEOUT,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(crate::command_service::result_name(result).to_string())
}

/// The FC's firmware version ("" until AUTOPILOT_VERSION arrives). Requests
/// it as a side effect when empty (the reply lands on `params_event`).
#[tauri::command]
pub async fn params_fc_version(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let Some(svc) = state.primary_params() else {
        return Ok(None);
    };
    if svc.fc_version().is_none() {
        let _ = svc.request_fc_version().await;
    }
    Ok(svc.fc_version())
}

/// Lazy-load one parameter's current value from the FC (by name). The
/// window fetches values on selection instead of pulling the whole set
/// (operator request 2026-10-11).
#[tauri::command]
pub async fn params_read(
    state: State<'_, AppState>,
    name: String,
) -> Result<Option<maggcs_core::params::model::ParamEntry>, String> {
    let Some(svc) = state.primary_params() else {
        return Err("no link — connect first".into());
    };
    // Serve from the cache when fresh (a value already pulled/read).
    if let Some(entry) = svc.cache().get(&name) {
        return Ok(Some(entry));
    }
    match svc.read(&name).await {
        Ok(entry) => Ok(Some(entry)),
        Err(maggcs_core::params::service::ParamError::Timeout) => Ok(None),
        Err(e) => Err(format!("{e:?}")),
    }
}

/// Read one parameter's cached value (PM9 integration point).
#[tauri::command]
pub fn params_cache_get(
    state: State<'_, AppState>,
    name: String,
) -> Option<maggcs_core::params::model::ParamEntry> {
    state.params_cache().and_then(|cache| cache.get(&name))
}

/// Number of cached parameters.
#[tauri::command]
pub fn params_cache_len(state: State<'_, AppState>) -> usize {
    state.params_cache().map(|cache| cache.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn parameter_runtime_stop_joins_protocol_task() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let exited = Arc::new(AtomicBool::new(false));
        let task_exited = Arc::clone(&exited);
        let task = tokio::spawn(async move {
            let _ = release_rx.await;
            task_exited.store(true, Ordering::SeqCst);
        });
        drop(release_tx);

        let result = stop_protocol_task(task, Duration::from_secs(1), Instant::now()).await;

        assert!(matches!(result, StopOutcome::Stopped { .. }));
        assert!(exited.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn parameter_runtime_stop_aborts_worker_after_deadline() {
        let task = tokio::spawn(async { std::future::pending::<()>().await });

        let result = stop_protocol_task(task, Duration::ZERO, Instant::now()).await;

        assert!(matches!(result, StopOutcome::TimedOut { .. }));
    }
}
