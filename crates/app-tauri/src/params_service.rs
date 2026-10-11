//! Parameters window service (parameters plan PM3): window lifecycle plus
//! Tauri command adapters. The `ParamService` protocol task lives in
//! `core::params::service` (ADR-001); it spawns on the primary link when the
//! window first connects and survives window close (the cache is read by
//! other services too, PM9), with a fresh task per reconnect.

use serde::Serialize;
use std::sync::Mutex as StdMutex;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use tauri::State;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tauri::WindowEvent;

use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::params::model::{ParamType, ParamValue};
use maggcs_core::params::service::{ParamEvent, ParamService, SetResult};

use crate::state::AppState;

/// The running protocol task + its event forwarder (survives window close;
/// re-spawned on reconnect like the inspector tap). Not `Clone`: it lives
/// once in `AppState`.
#[derive(Default)]
pub struct ParamsState {
    inner: StdMutex<Option<RunningParams>>,
}

struct RunningParams {
    svc: ParamService,
    forwarder: tauri::async_runtime::JoinHandle<()>,
}

impl ParamsState {
    /// Ensure a protocol task exists on this link (fresh per link) and
    /// forward its events to the `params` window as `params_event`.
    pub fn attach(&self, app: &AppHandle, handle: ConnectionHandle) {
        let mut guard = self.inner.lock().expect("params state lock");
        // A live task from an earlier link is replaced (its route dies with
        // the old link anyway).
        if let Some(old) = guard.take() {
            old.forwarder.abort();
        }
        let (svc, evt_rx) = ParamService::spawn(handle, 1, 1);
        let forwarder = tauri::async_runtime::spawn(forward_events(app.clone(), evt_rx));
        *guard = Some(RunningParams { svc, forwarder });
    }

    fn service(&self) -> Option<ParamService> {
        self.inner
            .lock()
            .expect("params state lock")
            .as_ref()
            .map(|r| r.svc.clone())
    }
}

/// Forward protocol events to the window (low-rate control events; batches
/// ride the same channel, ~100 entries each).
async fn forward_events(app: AppHandle, mut rx: tokio::sync::broadcast::Receiver<ParamEvent>) {
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
}

/// Open the parameters window, or focus it if it already exists.
#[tauri::command]
pub fn params_open(app: AppHandle) -> Result<(), String> {
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
            if let Some(svc) = state.params.service() {
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
    let Some(svc) = state.params.service() else {
        return Err("no link — connect first".into());
    };
    svc.pull().await.map_err(|e| format!("{e:?}"))
}

/// Cancel the running transaction (idempotent).
#[tauri::command]
pub async fn params_cancel(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(svc) = state.params.service() {
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
    let Some(svc) = state.params.service() else {
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

/// Read one parameter's cached value (PM9 integration point).
#[tauri::command]
pub fn params_cache_get(
    state: State<'_, AppState>,
    name: String,
) -> Option<maggcs_core::params::model::ParamEntry> {
    state
        .params
        .service()
        .and_then(|svc| svc.cache().get(&name))
}

/// Number of cached parameters.
#[tauri::command]
pub fn params_cache_len(state: State<'_, AppState>) -> usize {
    state
        .params
        .service()
        .map(|svc| svc.cache().len())
        .unwrap_or(0)
}
