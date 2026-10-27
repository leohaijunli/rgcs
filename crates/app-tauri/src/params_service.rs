//! Parameters window service (parameters plan PM3): window lifecycle plus
//! Tauri command adapters. The `ParamService` protocol task lives in
//! `core::params::service` (ADR-001); it spawns on the primary link when the
//! link connects and survives window close (the cache is read by
//! other services too, PM9), with a fresh task per reconnect.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use tauri::Runtime;
use tauri::State;
use tauri::WebviewUrl;
use tauri::WebviewWindow;
use tauri::WebviewWindowBuilder;

use maggcs_core::lifecycle::StopOutcome;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::params::operation_types::{
    ParameterCommandError, ParameterEventEnvelope, ParameterEventPayload, ParameterReadRequest,
    ParameterRequestContext, ParameterSetEcho, ParameterSetRequest,
};
use maggcs_core::params::service::{ParamService, ScopedParamEvent};
use maggcs_core::runtime::operations::{OperationRef, OperationStatus, RuntimeClientSession};
use maggcs_core::runtime::TargetRole;

use crate::params_operations::{self, ParameterAction, ParameterResult};
use crate::runtime_facade::{RuntimeFacade, TargetRuntime};
use crate::state::AppState;

/// `params_fc_version` retry budget: 2 attempts × 1.2 s settle ≈ 2.5 s
/// worst case on the saturated serial link, near-instant when the FC has
/// already answered.
const FC_VERSION_ATTEMPTS: u32 = 2;
const FC_VERSION_SETTLE: Duration = Duration::from_millis(1200);

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
        let (sys, comp) = handle
            .target_identity()
            .map(|id| (id.system_id, id.component_id))
            .unwrap_or((
                handle.config().target_system_id,
                handle.config().target_component_id,
            ));
        let (svc, _evt_rx, protocol_task) =
            ParamService::spawn_with_task(handle.clone(), sys, comp);
        *self.last_cache.lock().expect("params cache lock") = Some(svc.cache());
        let forwarder = tauri::async_runtime::spawn(forward_events(
            app.clone(),
            handle,
            svc.subscribe_scoped(),
        ));
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
    handle: ConnectionHandle,
    mut rx: tokio::sync::broadcast::Receiver<ScopedParamEvent>,
) {
    use maggcs_core::params::service::ParamEvent as E;
    while let Some(ev) = next_param_event(&mut rx).await {
        if ev.generation != handle.generation()
            || app
                .state::<AppState>()
                .connection()
                .is_none_or(|current| current.handle_id() != handle.handle_id())
        {
            continue;
        }
        let Some(target) = RuntimeFacade::new(&app).targets().ok().and_then(|targets| {
            targets
                .into_iter()
                .find(|target| target.roles.contains(&TargetRole::Primary))
                .and_then(|target| target.target)
        }) else {
            continue;
        };
        if target.generation.get() != ev.generation {
            continue;
        }
        let event = match &ev.event {
            E::PullStarted { total } => ParameterEventPayload::PullStarted { total: *total },
            E::Batch(entries) => ParameterEventPayload::Batch {
                entries: entries.clone(),
            },
            E::Progress { received, total } => ParameterEventPayload::Progress {
                received: *received,
                total: *total,
            },
            E::PullDone { received, total } => ParameterEventPayload::PullDone {
                received: *received,
                total: *total,
            },
            E::ExternalChange(entry) => ParameterEventPayload::ExternalChange {
                name: entry.name.clone(),
                value: entry.value,
                ptype: entry.ptype,
            },
            E::FcVersion(v) => ParameterEventPayload::FcVersion { version: v.clone() },
        };
        let payload = ParameterEventEnvelope {
            target,
            operation_id: ev.transaction_id,
            event,
        };
        let _ = app.emit("params_event", &payload);
    }
}

/// Losing older events during a large pull must not permanently disconnect
/// the window's event forwarder. Continue with the retained events.
async fn next_param_event<T: Clone>(rx: &mut tokio::sync::broadcast::Receiver<T>) -> Option<T> {
    loop {
        match rx.recv().await {
            Ok(event) => return Some(event),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
        }
    }
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
    WebviewWindowBuilder::new(&app, "params", WebviewUrl::App("params.html".into()))
        .title("Parameters")
        .min_inner_size(900.0, 600.0)
        .build()
        .map_err(|e| e.to_string())?;
    // Native client registration captures this window's actual owner IDs.
    // Its Destroyed handler cancels only those operations, never a global service.
    Ok(())
}

/// Admit an owned pull. Its reference is returned before background protocol
/// completion; `params_operation` carries terminal/cleanup evidence.
#[tauri::command]
pub async fn params_pull(
    app: AppHandle,
    window: WebviewWindow,
    request: ParameterRequestContext,
) -> Result<OperationRef, ParameterCommandError> {
    let work = params_operations::begin(&app, window.label(), request, ParameterAction::Pull)?;
    let operation = work.operation().clone();
    tauri::async_runtime::spawn(async move {
        let _ = work.run().await;
    });
    Ok(operation)
}

/// Request cancellation of the caller's exact operation, never a global slot.
#[tauri::command]
pub async fn params_cancel(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, ParameterCommandError> {
    params_operations::cancel(&app, window.label(), &session, &operation)
}

#[tauri::command]
pub fn params_operation_status(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, ParameterCommandError> {
    params_operations::owned_status(&app, window.label(), &session, &operation)
}

/// Set one parameter; resolves with the FC's echo.
#[tauri::command]
pub async fn params_set(
    app: AppHandle,
    window: WebviewWindow,
    request: ParameterSetRequest,
) -> Result<ParameterSetEcho, ParameterCommandError> {
    let work = params_operations::begin(
        &app,
        window.label(),
        request.context,
        ParameterAction::Set {
            name: request.name,
            value: request.value,
            ptype: request.ptype,
        },
    )?;
    let operation = work.operation().clone();
    let result = tauri::async_runtime::spawn(work.run())
        .await
        .map_err(|_| maggcs_core::runtime::operations::OperationError::RegistryUnavailable)??;
    match result {
        ParameterResult::Set(result) => params_operations::set_echo(operation, result),
        _ => Err(maggcs_core::runtime::operations::OperationError::RegistryUnavailable.into()),
    }
}

/// Reboot the flight controller (`MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN`,
/// param1=1). Awaits the FC's verdict: SITL denies this command (PX4 only
/// compiles the reboot path for `CONFIG_BOARDCTL_RESET` boards), so the UI
/// must see `denied` instead of assuming the reboot is on its way.
#[tauri::command]
pub async fn params_reboot(
    app: AppHandle,
    window: WebviewWindow,
    request: maggcs_core::commands::operation_types::CommandRequestContext,
) -> Result<
    maggcs_core::commands::operation_types::CommandReply,
    maggcs_core::commands::operation_types::CommandOperationError,
> {
    let work = crate::command_operations::begin_reboot(&app, window.label(), request)?;
    tauri::async_runtime::spawn(work.run())
        .await
        .map_err(|_| maggcs_core::runtime::operations::OperationError::RegistryUnavailable)?
}

/// The FC's firmware version (null until AUTOPILOT_VERSION arrives). Waits
/// for a requested reply when empty; the reply also lands on `params_event`.
#[tauri::command]
pub async fn params_fc_version(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
) -> Result<Option<String>, ParameterCommandError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window.label(), &session)?;
    let runtime = RuntimeFacade::new(&app);
    let binding = runtime
        .parameter_binding(&session.target)
        .map_err(|error| maggcs_core::runtime::operations::OperationError::Runtime { error })?;
    // Queueing a request is not receiving its reply. Return the arrived
    // version, with a bounded wait rather than racing the cache read.
    let version = binding
        .service
        .wait_fc_version(FC_VERSION_ATTEMPTS, FC_VERSION_SETTLE)
        .await;
    runtime
        .bind(&session.target)
        .map_err(|error| maggcs_core::runtime::operations::OperationError::Runtime { error })?;
    Ok(version)
}

/// Lazy-load one parameter's current value from the FC (by name). The
/// window fetches values on selection instead of pulling the whole set
/// (operator request 2026-10-11).
#[tauri::command]
pub async fn params_read(
    app: AppHandle,
    window: WebviewWindow,
    request: ParameterReadRequest,
) -> Result<Option<maggcs_core::params::model::ParamEntry>, ParameterCommandError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window.label(), &request.context.session)?;
    let target = RuntimeFacade::new(&app)
        .bind(&request.context.session.target)
        .map_err(|error| maggcs_core::runtime::operations::OperationError::Runtime { error })?;
    if let Some(observation) = target
        .parameter_observation(&request.name)
        .map_err(|error| maggcs_core::runtime::operations::OperationError::Runtime { error })?
    {
        return Ok(Some(observation.entry));
    }
    let work = params_operations::begin(
        &app,
        window.label(),
        request.context,
        ParameterAction::Read { name: request.name },
    )?;
    match tauri::async_runtime::spawn(work.run())
        .await
        .map_err(|_| maggcs_core::runtime::operations::OperationError::RegistryUnavailable)?
    {
        Ok(ParameterResult::Read(entry)) => Ok(Some(entry)),
        Err(ParameterCommandError::Protocol {
            error: maggcs_core::params::service::ParamError::Timeout,
        }) => Ok(None),
        Err(error) => Err(error),
        _ => Err(maggcs_core::runtime::operations::OperationError::RegistryUnavailable.into()),
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
    use maggcs_core::params::service::ParamEvent;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn parameter_events_resume_after_a_lagged_receiver() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        tx.send(ParamEvent::FcVersion("old".into()))
            .expect("first event");
        tx.send(ParamEvent::FcVersion("1.17.0".into()))
            .expect("retained event");
        assert!(matches!(
            next_param_event(&mut rx).await,
            Some(ParamEvent::FcVersion(version)) if version == "1.17.0"
        ));
        drop(tx);
        assert!(next_param_event(&mut rx).await.is_none());
    }

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
