//! Owned Motor-target log operations over the target's one existing LogService.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use maggcs_core::logs::operation_types::*;
use maggcs_core::logs::{LogEntryInfo, LogServiceEvent, LogTransaction};
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::OperationId;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};

use crate::runtime_facade::{LogBinding, RuntimeFacade};
use crate::state::AppState;

const LIST_BUDGET: Duration = Duration::from_secs(10);
const DOWNLOAD_BUDGET: Duration = Duration::from_secs(10 * 60);
const DELETE_BUDGET: Duration = Duration::from_secs(20);
const FILE_CHUNK: usize = 64 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const ERASE_READBACK: Duration = Duration::from_secs(10);
const MAX_RETAINED: usize = 32;

#[derive(Default)]
pub struct MotorLogOperations {
    entries: Mutex<BTreeMap<OperationId, Arc<Scope>>>,
    reconciliation: Mutex<Option<OperationRef>>,
}
impl MotorLogOperations {
    pub(crate) fn is_active(&self) -> bool {
        !self.entries.lock().is_empty()
    }
}
struct Scope {
    session: RuntimeClientSession,
    operation: OperationRef,
    request_id: maggcs_core::runtime::RequestId,
    transaction: Mutex<Option<LogTransaction>>,
}
enum Action {
    List,
    Download { entry: LogEntryInfo, path: String },
    Delete { entry: LogEntryInfo },
    Erase,
}
impl Action {
    fn budget(&self) -> Duration {
        match self {
            Self::List => LIST_BUDGET,
            Self::Download { .. } => DOWNLOAD_BUDGET,
            _ => DELETE_BUDGET,
        }
    }
    fn resources(&self) -> Vec<OperationResource> {
        vec![OperationResource::Shell, OperationResource::LogTransfer]
    }
}
struct Work<R: Runtime> {
    app: AppHandle<R>,
    window: String,
    session: RuntimeClientSession,
    scope: Arc<Scope>,
    lease: Option<OperationLease>,
    binding: LogBinding,
    action: Option<Action>,
    destructive: Arc<AtomicBool>,
    file_may_commit: Arc<AtomicBool>,
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("motor_logs_operation", status);
}
fn retained<R: Runtime>(
    app: &AppHandle<R>,
    operation: &OperationRef,
) -> Result<OperationStatus, LogOperationError> {
    Ok(app
        .state::<AppState>()
        .runtime_clients
        .operations
        .status(&operation.owner, operation)?)
}

fn log_binding<R: Runtime>(
    app: &AppHandle<R>,
    session: &RuntimeClientSession,
) -> Result<LogBinding, LogOperationError> {
    crate::motor_operations::binding(app, session).map_err(|error| {
        OperationError::InvalidRequest {
            message: error.to_string(),
        }
    })?;
    let state = app.state::<AppState>();
    let (origin, connection) = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|target| (target.origin(), target.target.handle.clone()))
        .ok_or(OperationError::TargetChanged)?;
    if origin == crate::motor_test_state::TargetLinkOrigin::MainSerial {
        return RuntimeFacade::new(app)
            .log_binding(&session.target)
            .map_err(|error| OperationError::Runtime { error }.into());
    }
    let service = state
        .motor_test
        .logs
        .lock()
        .runtime
        .as_ref()
        .and_then(crate::motor_test_state::ActuatorLogsRuntime::service)
        .ok_or_else(|| OperationError::InvalidRequest {
            message: "dedicated Motor log service is unavailable".into(),
        })?;
    Ok(LogBinding {
        connection,
        service,
    })
}

fn begin<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    context: LogRequestContext,
    action: Action,
) -> Result<Work<R>, LogOperationError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, &context.session)?;
    let binding = log_binding(app, &context.session)?;
    if matches!(action, Action::Delete { .. } | Action::Erase)
        && state
            .motor_test
            .log_operations
            .reconciliation
            .lock()
            .is_some()
    {
        return Err(LogProtocolError::InvalidRequest {
            message: "refresh the verified Motor log list before another deletion".into(),
        }
        .into());
    }
    if let Action::Download { path, entry } = &action {
        if path.is_empty()
            || path.len() > 4096
            || path.contains('\0')
            || entry.size > MAX_LOG_DOWNLOAD_BYTES
        {
            return Err(LogProtocolError::InvalidRequest {
                message: "invalid destination or oversized buffered log".into(),
            }
            .into());
        }
    }
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: context.request_id.clone(),
        owner: context.session.owner.clone(),
        target: context.session.target.clone(),
        resources: action.resources(),
        budget_ms: action.budget().as_millis() as u64,
    })?;
    state.runtime_clients.authorize(window, &context.session)?;
    log_binding(app, &context.session)?;
    let scope = Arc::new(Scope {
        session: context.session.clone(),
        operation: lease.operation().clone(),
        request_id: context.request_id,
        transaction: Mutex::new(None),
    });
    let mut entries = state.motor_test.log_operations.entries.lock();
    if entries.len() >= MAX_RETAINED {
        return Err(OperationError::CapacityExceeded.into());
    }
    entries.insert(scope.operation.id.clone(), scope.clone());
    Ok(Work {
        app: app.clone(),
        window: window.into(),
        session: context.session,
        scope,
        lease: Some(lease),
        binding,
        action: Some(action),
        destructive: Arc::new(AtomicBool::new(false)),
        file_may_commit: Arc::new(AtomicBool::new(false)),
    })
}

impl<R: Runtime> Work<R> {
    fn operation(&self) -> &OperationRef {
        &self.scope.operation
    }
    fn check(&self) -> Result<(), LogOperationError> {
        self.lease
            .as_ref()
            .ok_or(OperationError::RegistryUnavailable)?
            .check()?;
        self.app
            .state::<AppState>()
            .runtime_clients
            .authorize(&self.window, &self.session)?;
        log_binding(&self.app, &self.session)?;
        Ok(())
    }
    fn transaction(&self) -> Result<LogTransaction, LogOperationError> {
        self.check()?;
        let remaining = retained(&self.app, self.operation())?.remaining_budget_ms;
        let transaction = LogTransaction::new(
            self.operation().id.clone(),
            self.session.target.generation,
            Duration::from_millis(remaining.max(1)),
        )?
        .with_owner_cancellation(
            self.lease
                .as_ref()
                .ok_or(OperationError::RegistryUnavailable)?
                .cancellation(),
        );
        *self.scope.transaction.lock() = Some(transaction.clone());
        Ok(transaction)
    }
    async fn list(&self) -> Result<Vec<LogEntryInfo>, LogOperationError> {
        let transaction = self.transaction()?;
        match self.binding.service.list_owned(transaction.clone()).await {
            Ok(entries) => Ok(entries),
            Err(error @ LogProtocolError::NoAck { .. }) => {
                self.binding.service.cancel_owned(&transaction).await?;
                self.check()?;
                if crate::capture_service::logs_directory_is_empty(&self.app.state::<AppState>())
                    .await
                    .map_err(|message| LogProtocolError::InvalidRequest { message })?
                {
                    Ok(Vec::new())
                } else {
                    Err(error.into())
                }
            }
            Err(error) => Err(error.into()),
        }
    }
    async fn execute(&self, action: Action) -> Result<LogResult, LogOperationError> {
        match action {
            Action::List => self
                .list()
                .await
                .map(|entries| LogResult::Listed { entries }),
            Action::Download { entry, path } => {
                self.destructive_admission().await?;
                let entries = self.list().await?;
                crate::actuator_service::same_log_entry(&entry, &entries)
                    .map_err(|message| LogProtocolError::InvalidRequest { message })?;
                let transaction = self.transaction()?;
                self.destructive_admission().await?;
                let bytes = self
                    .binding
                    .service
                    .download_owned(transaction, entry.clone())
                    .await?;
                self.check()?;
                atomic_write(
                    &path,
                    bytes,
                    &self.scope.operation.id.to_string(),
                    self.lease
                        .as_ref()
                        .ok_or(OperationError::RegistryUnavailable)?
                        .cancellation(),
                    self.file_may_commit.clone(),
                    self.binding.connection.clone(),
                    self.session.target.generation.get(),
                )
                .await?;
                self.check()?;
                Ok(LogResult::Downloaded { entry, path })
            }
            Action::Delete { entry } => {
                self.destructive_admission().await?;
                let entries = self.list().await?;
                let entry = crate::actuator_service::same_log_entry(&entry, &entries)
                    .map_err(|message| LogProtocolError::InvalidRequest { message })?;
                let listing = crate::capture_service::target_shell(
                    &self.app.state::<AppState>(),
                    "cat /fs/microsd/logdata.txt",
                )
                .await
                .map_err(|message| LogProtocolError::InvalidRequest { message })?;
                let path = crate::px4_shell::log_path_from_listing(&listing, &entry)
                    .map_err(|message| LogProtocolError::InvalidRequest { message })?;
                self.destructive_admission().await?;
                self.check()?;
                self.destructive.store(true, Ordering::Release);
                let output = crate::capture_service::target_shell(
                    &self.app.state::<AppState>(),
                    &format!("rm {path}"),
                )
                .await
                .map_err(|message| LogProtocolError::InvalidRequest { message })?;
                if let Some(error) = crate::px4_shell::command_error(&output) {
                    return Err(LogProtocolError::InvalidRequest {
                        message: format!("SD log deletion failed: {error}"),
                    }
                    .into());
                }
                let output = crate::capture_service::target_shell(
                    &self.app.state::<AppState>(),
                    &format!("ls -l {path}"),
                )
                .await
                .map_err(|message| LogProtocolError::InvalidRequest { message })?;
                if !output.lines().any(|line| {
                    line.starts_with("nsh: ls: stat failed:")
                        && line.contains("No such file or directory")
                }) {
                    return Err(LogProtocolError::InvalidRequest {
                        message: "FC did not confirm the Motor log was removed".into(),
                    }
                    .into());
                }
                crate::actuator_service::invalidate_actuator_log_list(
                    &self.app.state::<AppState>(),
                );
                self.list()
                    .await
                    .map(|entries| LogResult::Deleted { entries })
            }
            Action::Erase => {
                self.destructive_admission().await?;
                self.check()?;
                self.destructive.store(true, Ordering::Release);
                let mut protocol = maggcs_core::logs::LogProtocol::new(
                    self.session.target.mavlink_target.system_id,
                    self.session.target.mavlink_target.component_id,
                );
                for frame in protocol.begin_erase() {
                    self.binding
                        .connection
                        .send_in_generation(
                            frame,
                            self.session.target.generation.get(),
                            tokio::time::Instant::now() + WRITE_TIMEOUT,
                        )
                        .await
                        .map_err(|_| LogProtocolError::TargetChanged)?;
                }
                let deadline = Instant::now() + ERASE_READBACK;
                loop {
                    self.check()?;
                    if crate::capture_service::logs_directory_is_empty(
                        &self.app.state::<AppState>(),
                    )
                    .await
                    .map_err(|message| LogProtocolError::InvalidRequest { message })?
                    {
                        return Ok(LogResult::Erased);
                    }
                    if Instant::now() >= deadline {
                        return Err(LogProtocolError::TimedOut.into());
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    }
    async fn destructive_admission(&self) -> Result<(), LogOperationError> {
        self.check()?;
        let state = self.app.state::<AppState>();
        crate::actuator_service::ensure_idle_session(&state)
            .map_err(|message| LogProtocolError::InvalidRequest { message })?;
        let target = state
            .motor_test
            .target
            .lock()
            .as_ref()
            .map(|target| target.target.target_ref())
            .ok_or(OperationError::TargetChanged)?;
        crate::actuator_service::ensure_target_disarmed(&state, target)
            .map_err(|message| LogProtocolError::InvalidRequest { message })?;
        crate::capture_service::ensure_logger_not_recording(&state)
            .await
            .map_err(|message| LogProtocolError::InvalidRequest { message })?;
        crate::actuator_service::ensure_target_disarmed(&state, target)
            .map_err(|message| LogProtocolError::InvalidRequest { message })?;
        self.check()
    }
    fn finish(
        &mut self,
        outcome: OperationOutcome,
        cleanup: CleanupOutcome,
    ) -> Result<(), LogOperationError> {
        let lease = self
            .lease
            .take()
            .ok_or(OperationError::RegistryUnavailable)?;
        match lease.finish(outcome, cleanup) {
            Ok(status) => publish(&self.app, &status),
            Err(error) if cleanup != CleanupOutcome::Uncertain => {
                let status = self
                    .app
                    .state::<AppState>()
                    .runtime_clients
                    .operations
                    .confirm_cleanup(self.operation(), cleanup)?;
                publish(&self.app, &status);
                if !matches!(error, OperationError::AlreadyFinished) {
                    return Err(error.into());
                }
            }
            Err(error) => return Err(error.into()),
        }
        if cleanup != CleanupOutcome::Uncertain {
            self.app
                .state::<AppState>()
                .motor_test
                .log_operations
                .entries
                .lock()
                .remove(&self.operation().id);
        }
        Ok(())
    }
    async fn failed(mut self, error: LogOperationError) -> Result<LogReply, LogOperationError> {
        let transaction = self.scope.transaction.lock().clone();
        let cleanup = match transaction {
            Some(transaction) => self
                .binding
                .service
                .cancel_owned(&transaction)
                .await
                .map(|_| ()),
            None => Ok(()),
        };
        let uncertain = self.destructive.load(Ordering::Acquire)
            || self.file_may_commit.load(Ordering::Acquire)
            || cleanup.is_err()
            || matches!(
                &error,
                LogOperationError::Protocol {
                    error: LogProtocolError::CleanupTimeout
                        | LogProtocolError::TargetChanged
                        | LogProtocolError::Stopped
                }
            );
        if uncertain {
            if self.destructive.load(Ordering::Acquire) {
                *self
                    .app
                    .state::<AppState>()
                    .motor_test
                    .log_operations
                    .reconciliation
                    .lock() = Some(self.operation().clone());
            }
            let reason = format!(
                "{error}; Motor log cleanup: {cleanup:?}; target/file state requires read-back"
            );
            self.finish(
                OperationOutcome::Uncertain {
                    reason: reason.clone(),
                },
                if cleanup.is_ok() {
                    CleanupOutcome::Confirmed
                } else {
                    CleanupOutcome::Uncertain
                },
            )?;
            Err(LogOperationError::ReconciliationRequired {
                operation: Box::new(self.operation().clone()),
                reason,
            })
        } else {
            let cancelled = matches!(
                &error,
                LogOperationError::Operation {
                    error: OperationError::Cancelled
                        | OperationError::NotOwner
                        | OperationError::TargetChanged
                } | LogOperationError::Protocol {
                    error: LogProtocolError::Cancelled | LogProtocolError::TargetChanged
                }
            );
            self.finish(
                if cancelled {
                    OperationOutcome::Cancelled
                } else {
                    OperationOutcome::Failed {
                        message: error.to_string(),
                    }
                },
                CleanupOutcome::Confirmed,
            )?;
            Err(error)
        }
    }
    async fn run(mut self) -> Result<LogReply, LogOperationError> {
        if let Err(error) = self.check().and_then(|()| {
            self.lease
                .as_ref()
                .ok_or(OperationError::RegistryUnavailable)?
                .mark_running()
                .map_err(Into::into)
        }) {
            self.app
                .state::<AppState>()
                .motor_test
                .log_operations
                .entries
                .lock()
                .remove(&self.operation().id);
            return Err(error);
        }
        if let Ok(status) = retained(&self.app, self.operation()) {
            publish(&self.app, &status);
        }
        let action = self
            .action
            .take()
            .ok_or(OperationError::RegistryUnavailable)?;
        let result = self
            .execute(action)
            .await
            .and_then(|result| self.check().map(|()| result));
        match result {
            Ok(result) => {
                let operation = self.operation().clone();
                match &result {
                    LogResult::Listed { entries } | LogResult::Deleted { entries } => {
                        let reconciled = self
                            .app
                            .state::<AppState>()
                            .motor_test
                            .log_operations
                            .reconciliation
                            .lock()
                            .take();
                        if let Some(reconciled) = reconciled {
                            self.app
                                .state::<AppState>()
                                .motor_test
                                .log_operations
                                .entries
                                .lock()
                                .remove(&reconciled.id);
                        }
                        crate::actuator_service::publish_actuator_log_list(
                            &self.app,
                            &self.app.state::<AppState>(),
                            entries.clone(),
                        )
                    }
                    LogResult::Erased => {
                        let reconciled = self
                            .app
                            .state::<AppState>()
                            .motor_test
                            .log_operations
                            .reconciliation
                            .lock()
                            .take();
                        if let Some(reconciled) = reconciled {
                            self.app
                                .state::<AppState>()
                                .motor_test
                                .log_operations
                                .entries
                                .lock()
                                .remove(&reconciled.id);
                        }
                        crate::actuator_service::publish_actuator_log_list(
                            &self.app,
                            &self.app.state::<AppState>(),
                            Vec::new(),
                        )
                    }
                    _ => {}
                }
                self.finish(OperationOutcome::Succeeded, CleanupOutcome::NotRequired)?;
                Ok(LogReply { operation, result })
            }
            Err(error) => self.failed(error).await,
        }
    }
}

async fn atomic_write(
    path: &str,
    bytes: Vec<u8>,
    operation: &str,
    cancellation: maggcs_core::runtime::operations::OperationCancellation,
    may_commit: Arc<AtomicBool>,
    connection: maggcs_core::mavlink::ConnectionHandle,
    generation: u64,
) -> Result<(), LogOperationError> {
    let path = path.to_string();
    let operation = operation.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let destination = PathBuf::from(path);
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = destination
            .file_name()
            .ok_or_else(|| "destination has no file name".to_string())?;
        let temporary = parent.join(format!(".{}.{}.partial", name.to_string_lossy(), operation));
        let result = (|| -> Result<(), String> {
            let check = || -> Result<(), String> {
                if cancellation.is_cancelled() {
                    return Err("local log write was cancelled".into());
                }
                if connection.generation() != generation || connection.is_shutdown_requested() {
                    return Err("Motor log target changed during local write".into());
                }
                Ok(())
            };
            check()?;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            for chunk in bytes.chunks(FILE_CHUNK) {
                check()?;
                file.write_all(chunk).map_err(|error| error.to_string())?;
            }
            file.sync_all().map_err(|error| error.to_string())?;
            check()?;
            may_commit.store(true, Ordering::Release);
            std::fs::rename(&temporary, &destination).map_err(|error| error.to_string())?;
            #[cfg(unix)]
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    })
    .await
    .map_err(|error| LogOperationError::LocalIo {
        message: error.to_string(),
    })?
    .map_err(|message| LogOperationError::LocalIo { message })
}

async fn execute<R: Runtime>(work: Work<R>) -> Result<LogReply, LogOperationError> {
    tauri::async_runtime::spawn(work.run())
        .await
        .map_err(|_| OperationError::RegistryUnavailable)?
}

pub(crate) fn forward_event<R: Runtime>(app: &AppHandle<R>, event: LogServiceEvent) {
    let LogServiceEvent::Progress {
        operation_id,
        generation,
        entry,
        fetched,
        total,
    } = event;
    let state = app.state::<AppState>();
    let scope = state
        .motor_test
        .log_operations
        .entries
        .lock()
        .get(&operation_id)
        .cloned();
    let Some(scope) = scope else {
        return;
    };
    if scope.operation.target.generation != generation {
        return;
    }
    let Ok(status) = retained(app, &scope.operation) else {
        return;
    };
    if status.phase != maggcs_core::runtime::operations::OperationPhase::Running {
        return;
    }
    let _ = app.emit(
        "motor_logs_progress",
        LogProgressEnvelope {
            operation: scope.operation.clone(),
            request_id: scope.request_id.clone(),
            entry,
            fetched,
            total,
        },
    );
}

fn context_for(request: &LogDownloadRequest) -> LogRequestContext {
    request.context.clone()
}

#[tauri::command]
pub async fn actuator_logs_request_list(
    app: AppHandle,
    window: WebviewWindow,
    request: LogRequestContext,
) -> Result<LogReply, LogOperationError> {
    execute(begin(&app, window.label(), request, Action::List)?).await
}
#[tauri::command]
pub async fn actuator_logs_download(
    app: AppHandle,
    window: WebviewWindow,
    request: LogDownloadRequest,
) -> Result<LogReply, LogOperationError> {
    let context = context_for(&request);
    execute(begin(
        &app,
        window.label(),
        context,
        Action::Download {
            entry: request.entry,
            path: request.save_path,
        },
    )?)
    .await
}
#[tauri::command]
pub async fn actuator_logs_delete(
    app: AppHandle,
    window: WebviewWindow,
    request: LogDeleteRequest,
) -> Result<LogReply, LogOperationError> {
    if request.confirmation != delete_confirmation(&request.entry) {
        return Err(LogProtocolError::InvalidRequest {
            message: "delete confirmation does not match the log id".into(),
        }
        .into());
    }
    execute(begin(
        &app,
        window.label(),
        request.context,
        Action::Delete {
            entry: request.entry,
        },
    )?)
    .await
}
#[tauri::command]
pub async fn actuator_logs_erase_all(
    app: AppHandle,
    window: WebviewWindow,
    request: LogEraseRequest,
) -> Result<LogReply, LogOperationError> {
    if request.confirmation != DELETE_ALL_CONFIRMATION {
        return Err(LogProtocolError::InvalidRequest {
            message: "erase confirmation must be DELETE ALL LOGS".into(),
        }
        .into());
    }
    execute(begin(&app, window.label(), request.context, Action::Erase)?).await
}

fn owned<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<(), LogOperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window, session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    if !app
        .state::<AppState>()
        .motor_test
        .log_operations
        .entries
        .lock()
        .contains_key(&operation.id)
    {
        return Err(OperationError::NotFound.into());
    }
    Ok(())
}
#[tauri::command]
pub fn actuator_logs_cancel(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, LogOperationError> {
    owned(&app, window.label(), &session, &operation)?;
    let status = app
        .state::<AppState>()
        .runtime_clients
        .operations
        .cancel(&session.owner, &operation)?;
    publish(&app, &status);
    Ok(status)
}
#[tauri::command]
pub fn actuator_logs_operation_status(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, LogOperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window.label(), &session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    retained(&app, &operation)
}

pub(crate) async fn stop_all<R: Runtime>(
    app: &AppHandle<R>,
    budget: Duration,
) -> Result<(), String> {
    let scopes = app
        .state::<AppState>()
        .motor_test
        .log_operations
        .entries
        .lock()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for scope in &scopes {
        if retained(app, &scope.operation).is_ok_and(|status| {
            status.phase != maggcs_core::runtime::operations::OperationPhase::Finished
        }) {
            app.state::<AppState>()
                .runtime_clients
                .operations
                .cancel(&scope.session.owner, &scope.operation)
                .map_err(|error| error.to_string())?;
        }
    }
    tokio::time::timeout(budget, async {
        loop {
            if scopes.iter().all(|scope| {
                retained(app, &scope.operation).is_ok_and(|status| {
                    status.phase == maggcs_core::runtime::operations::OperationPhase::Finished
                })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "timed out stopping Motor log operations".to_string())?;
    if scopes.iter().any(|scope| {
        !retained(app, &scope.operation)
            .is_ok_and(|status| status.cleanup != CleanupOutcome::Uncertain)
    }) {
        return Err("Motor log cleanup is unconfirmed; target link preserved".into());
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "motor_log_operation_tests.rs"]
mod tests;
