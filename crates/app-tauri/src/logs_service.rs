//! Owner-bound primary FC log list/download/delete adapter.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use maggcs_core::lifecycle::StopOutcome;
use maggcs_core::logs::operation_types::{
    delete_confirmation, LogDeleteRequest, LogDownloadRequest, LogEraseRequest, LogOperationError,
    LogProgressEnvelope, LogProtocolError, LogReply, LogRequestContext, LogResult,
    DELETE_ALL_CONFIRMATION,
};
use maggcs_core::logs::{LogEntryInfo, LogService, LogServiceEvent, LogTransaction};
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::{OperationId, RuntimeError};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime, State, WebviewWindow};

use crate::runtime_facade::{LogBinding, RuntimeFacade, TargetRuntime};
use crate::state::AppState;

const LIST_BUDGET: Duration = Duration::from_secs(10);
const DOWNLOAD_BUDGET: Duration = Duration::from_secs(10 * 60);
const DELETE_BUDGET: Duration = Duration::from_secs(20);
const TARGET_POLL: Duration = Duration::from_millis(100);
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_SAVE_PATH_BYTES: usize = 4096;
const MAX_ACTIVE_LOG_OPERATIONS: usize = 32;
const FILE_CHUNK_BYTES: usize = 64 * 1024;
const FILE_CLEANUP_BUDGET: Duration = Duration::from_millis(750);
const ERASE_READBACK_BUDGET: Duration = Duration::from_secs(10);
const ERASE_POLL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct LogsState {
    active: Mutex<BTreeMap<OperationId, OperationRef>>,
    reconciliation: Mutex<Option<OperationRef>>,
}

impl LogsState {
    pub(crate) async fn stop_operations<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        budget: Duration,
    ) -> StopOutcome {
        let operations = self.active.lock().values().cloned().collect::<Vec<_>>();
        if operations.is_empty() {
            return StopOutcome::NotRunning;
        }
        let started = Instant::now();
        for operation in &operations {
            match app
                .state::<AppState>()
                .runtime_clients
                .operations
                .cancel(&operation.owner, operation)
            {
                Ok(status) => publish(app, &status),
                Err(error) => {
                    return StopOutcome::Failed {
                        message: error.to_string(),
                    }
                }
            }
        }
        if tokio::time::timeout(budget, async {
            while !self.active.lock().is_empty() {
                tokio::time::sleep(TARGET_POLL).await;
            }
        })
        .await
        .is_err()
        {
            return StopOutcome::TimedOut {
                elapsed_ms: elapsed_ms(started),
            };
        }
        if operations.iter().any(|operation| {
            !retained_status(app, operation)
                .is_ok_and(|status| status.cleanup != CleanupOutcome::Uncertain)
        }) {
            StopOutcome::Failed {
                message: "log operation cleanup is unconfirmed".into(),
            }
        } else {
            StopOutcome::Stopped {
                elapsed_ms: elapsed_ms(started),
            }
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        !self.active.lock().is_empty()
    }
    pub(crate) fn operation(&self, id: &OperationId) -> Option<OperationRef> {
        self.active.lock().get(id).cloned()
    }
    fn register(&self, operation: &OperationRef) -> Result<(), OperationError> {
        let mut active = self.active.lock();
        if active.len() >= MAX_ACTIVE_LOG_OPERATIONS {
            return Err(OperationError::CapacityExceeded);
        }
        active.insert(operation.id.clone(), operation.clone());
        Ok(())
    }

    pub fn spawn<R: Runtime>(
        self: &Arc<Self>,
        app: AppHandle<R>,
        connection: maggcs_core::mavlink::ConnectionHandle,
    ) -> LogsRuntime {
        let identity = connection.target_identity();
        let (target_system, target_component) = identity
            .map(|identity| (identity.system_id, identity.component_id))
            .unwrap_or((
                connection.config().target_system_id,
                connection.config().target_component_id,
            ));
        let (service, mut events) = LogService::spawn(connection, target_system, target_component);
        let forwarder = tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                forward_event(&app, event);
            }
        });
        LogsRuntime {
            service: Some(service),
            forwarder: Some(forwarder),
        }
    }
}

pub struct LogsRuntime {
    service: Option<LogService>,
    forwarder: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl LogsRuntime {
    pub(crate) fn service(&self) -> Option<LogService> {
        self.service.clone()
    }
    pub async fn stop(mut self, deadline: Duration) -> StopOutcome {
        let started = Instant::now();
        if let Some(service) = self.service.take() {
            service.shutdown();
        }
        let Some(mut task) = self.forwarder.take() else {
            return StopOutcome::NotRunning;
        };
        match tokio::time::timeout(deadline, &mut task).await {
            Ok(Ok(())) => StopOutcome::Stopped {
                elapsed_ms: elapsed_ms(started),
            },
            Ok(Err(error)) => StopOutcome::Failed {
                message: format!("log event forwarder failed: {error}"),
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
}

impl Drop for LogsRuntime {
    fn drop(&mut self) {
        if let Some(service) = self.service.take() {
            service.shutdown();
        }
        if let Some(task) = &self.forwarder {
            task.abort();
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

// Shared with the dedicated Motor log adapter until its owned migration.
pub(crate) enum RouteStep {
    Message(Box<maggcs_core::mavlink::MessageEnvelope>),
    Skip,
    Closed,
}
pub(crate) fn classify_route(
    event: Result<maggcs_core::mavlink::ConnectionEvent, tokio::sync::broadcast::error::RecvError>,
) -> RouteStep {
    match event {
        Ok(maggcs_core::mavlink::ConnectionEvent::Message(message)) => RouteStep::Message(message),
        Err(tokio::sync::broadcast::error::RecvError::Closed) => RouteStep::Closed,
        _ => RouteStep::Skip,
    }
}

struct Registration<R: Runtime> {
    app: AppHandle<R>,
    id: OperationId,
}
impl<R: Runtime> Drop for Registration<R> {
    fn drop(&mut self) {
        self.app
            .state::<AppState>()
            .logs
            .active
            .lock()
            .remove(&self.id);
    }
}

pub(crate) fn forward_event<R: Runtime>(app: &AppHandle<R>, event: LogServiceEvent) {
    match event {
        LogServiceEvent::Progress {
            operation_id,
            generation,
            entry,
            fetched,
            total,
        } => {
            let state = app.state::<AppState>();
            let Some(operation) = state.logs.operation(&operation_id) else {
                return;
            };
            if operation.target.generation != generation
                || RuntimeFacade::new(app).bind(&operation.target).is_err()
            {
                return;
            }
            let Ok(status) = retained_status(app, &operation) else {
                return;
            };
            if status.phase != maggcs_core::runtime::operations::OperationPhase::Running {
                return;
            }
            let _ = app.emit(
                "logs_progress",
                LogProgressEnvelope {
                    operation,
                    request_id: status.request_id,
                    entry,
                    fetched,
                    total,
                },
            );
        }
    }
}

pub(crate) enum LogAction {
    List,
    Download {
        entry: LogEntryInfo,
        save_path: String,
    },
    Delete {
        entry: LogEntryInfo,
    },
    Erase,
}

impl LogAction {
    fn budget(&self) -> Duration {
        match self {
            Self::List => LIST_BUDGET,
            Self::Download { .. } => DOWNLOAD_BUDGET,
            _ => DELETE_BUDGET,
        }
    }
    fn resources(&self) -> Vec<OperationResource> {
        match self {
            Self::List | Self::Delete { .. } | Self::Erase => {
                vec![OperationResource::Shell, OperationResource::LogTransfer]
            }
            _ => vec![OperationResource::LogTransfer],
        }
    }
    fn uses_shell(&self) -> bool {
        matches!(self, Self::Delete { .. } | Self::Erase)
    }
}

pub(crate) struct LogWork<R: Runtime> {
    app: AppHandle<R>,
    window: String,
    session: RuntimeClientSession,
    binding: LogBinding,
    lease: OperationLease,
    transaction: Mutex<Option<LogTransaction>>,
    action: Option<LogAction>,
    console: Option<Arc<crate::console_service::ConsoleState>>,
    destructive_dispatched: Arc<AtomicBool>,
    shell_used: Arc<AtomicBool>,
    file: Arc<FileControl>,
    _registration: Registration<R>,
}

struct FileControl {
    cancelled: maggcs_core::devices::SerialCancellation,
    finished: AtomicBool,
    may_commit: AtomicBool,
}
impl Default for FileControl {
    fn default() -> Self {
        Self {
            cancelled: maggcs_core::devices::SerialCancellation::new(),
            finished: AtomicBool::new(true),
            may_commit: AtomicBool::new(false),
        }
    }
}
struct CancelFile(Arc<FileControl>);
impl Drop for CancelFile {
    fn drop(&mut self) {
        self.0.cancelled.cancel();
    }
}
struct FinishedFile(Arc<FileControl>);
impl Drop for FinishedFile {
    fn drop(&mut self) {
        self.0.finished.store(true, Ordering::Release);
    }
}

fn runtime_error(error: RuntimeError) -> LogOperationError {
    OperationError::Runtime { error }.into()
}

fn validate_path(path: &str) -> Result<(), LogProtocolError> {
    if path.is_empty() || path.len() > MAX_SAVE_PATH_BYTES || path.contains('\0') {
        Err(LogProtocolError::InvalidRequest {
            message: "save path is empty, oversized or contains NUL".into(),
        })
    } else {
        Ok(())
    }
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn destructive_admission<R: Runtime>(
    app: &AppHandle<R>,
    session: &RuntimeClientSession,
) -> Result<(), LogOperationError> {
    let snapshot = RuntimeFacade::new(app)
        .bind(&session.target)
        .map_err(runtime_error)?
        .telemetry_snapshot()
        .map_err(runtime_error)?
        .ok_or_else(|| {
            runtime_error(RuntimeError::ServiceUnavailable {
                service: "telemetry".into(),
            })
        })?;
    let heartbeat_fresh = snapshot.field_ages.heartbeat_at_ms.is_some_and(|at| {
        epoch_ms().saturating_sub(at) <= maggcs_core::runtime::TARGET_HEARTBEAT_FRESH_MS
    });
    let landed_fresh = snapshot
        .field_ages
        .extended_sys_state_at_ms
        .is_some_and(|at| {
            epoch_ms().saturating_sub(at) <= maggcs_core::runtime::TARGET_HEARTBEAT_FRESH_MS
        });
    if !heartbeat_fresh
        || !landed_fresh
        || snapshot
            .heartbeat
            .as_ref()
            .is_none_or(|heartbeat| heartbeat.base_mode.safety_armed)
        || snapshot
            .extended_sys_state
            .as_ref()
            .is_none_or(|state| state.landed_state != maggcs_core::telemetry::LandedState::OnGround)
    {
        return Err(OperationError::InvalidRequest {
            message:
                "SD-log deletion requires fresh telemetry confirming a disarmed, landed target"
                    .into(),
        }
        .into());
    }
    crate::main_link_service::ensure_primary_tools_available(&app.state::<AppState>())
        .map_err(|message| OperationError::InvalidRequest { message }.into())
}

pub(crate) fn begin<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    context: LogRequestContext,
    action: LogAction,
    console: Option<Arc<crate::console_service::ConsoleState>>,
) -> Result<LogWork<R>, LogOperationError> {
    if let LogAction::Download { save_path, entry } = &action {
        validate_path(save_path)?;
        if entry.size > maggcs_core::logs::operation_types::MAX_LOG_DOWNLOAD_BYTES {
            return Err(LogProtocolError::InvalidRequest {
                message: "log exceeds the 256 MiB buffered-download limit".into(),
            }
            .into());
        }
    }
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, &context.session)?;
    crate::main_link_service::ensure_primary_tools_available(&state)
        .map_err(|message| OperationError::InvalidRequest { message })?;
    let runtime = RuntimeFacade::new(app);
    let binding = runtime
        .log_binding(&context.session.target)
        .map_err(runtime_error)?;
    if action.uses_shell() {
        if state.logs.reconciliation.lock().is_some() {
            return Err(LogProtocolError::InvalidRequest {
                message: "refresh the verified FC log list before another destructive action"
                    .into(),
            }
            .into());
        }
        destructive_admission(app, &context.session)?;
        if console.as_ref().is_none_or(|console| console.is_active()) {
            return Err(OperationError::InvalidRequest {
                message: "close the interactive console before deleting logs".into(),
            }
            .into());
        }
    }
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: context.request_id,
        owner: context.session.owner.clone(),
        target: context.session.target.clone(),
        resources: action.resources(),
        budget_ms: action.budget().as_millis() as u64,
    })?;
    state.runtime_clients.authorize(window, &context.session)?;
    runtime
        .bind(&context.session.target)
        .map_err(runtime_error)?;
    state.logs.register(lease.operation())?;
    let registration = Registration {
        app: app.clone(),
        id: lease.operation().id.clone(),
    };
    Ok(LogWork {
        app: app.clone(),
        window: window.into(),
        session: context.session,
        binding,
        lease,
        transaction: Mutex::new(None),
        action: Some(action),
        console,
        destructive_dispatched: Arc::new(AtomicBool::new(false)),
        shell_used: Arc::new(AtomicBool::new(false)),
        file: Arc::new(FileControl::default()),
        _registration: registration,
    })
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("logs_operation", status);
}
fn retained_status<R: Runtime>(
    app: &AppHandle<R>,
    operation: &OperationRef,
) -> Result<OperationStatus, LogOperationError> {
    Ok(app
        .state::<AppState>()
        .runtime_clients
        .operations
        .status(&operation.owner, operation)?)
}
fn finish<R: Runtime>(
    app: &AppHandle<R>,
    lease: OperationLease,
    outcome: OperationOutcome,
    cleanup: CleanupOutcome,
) -> Result<(), LogOperationError> {
    let operation = lease.operation().clone();
    match lease.finish(outcome, cleanup) {
        Ok(status) => {
            publish(app, &status);
            Ok(())
        }
        Err(error) => {
            if let Ok(status) = retained_status(app, &operation) {
                publish(app, &status);
                if status.cleanup == CleanupOutcome::Uncertain {
                    return Err(LogOperationError::ReconciliationRequired {
                        operation: Box::new(operation),
                        reason: error.to_string(),
                    });
                }
            }
            Err(error.into())
        }
    }
}

fn map_shell(error: maggcs_core::shell::ShellProtocolError) -> LogProtocolError {
    use maggcs_core::shell::ShellProtocolError;
    match error {
        ShellProtocolError::TargetChanged => LogProtocolError::TargetChanged,
        ShellProtocolError::Cancelled => LogProtocolError::Cancelled,
        ShellProtocolError::TimedOut => LogProtocolError::TimedOut,
        ShellProtocolError::Stopped => LogProtocolError::Stopped,
        other => LogProtocolError::InvalidRequest {
            message: other.to_string(),
        },
    }
}

async fn verify_delete_shell<R: Runtime>(
    work: &LogWork<R>,
    console: &crate::console_service::ConsoleState,
) -> Result<(), LogOperationError> {
    destructive_admission(&work.app, &work.session)?;
    work.shell_used.store(true, Ordering::Release);
    if crate::console_service::probe_binding(console, &work.binding.connection, &work.session)
        .await
        .map_err(map_shell)?
        != crate::px4_shell::ShellKind::NuttShell
    {
        return Err(LogProtocolError::InvalidRequest {
            message:
                "SD-log deletion requires a NuttX FC; this target exposes the SITL/POSIX shell"
                    .into(),
        }
        .into());
    }
    verify_logger_stopped(work, console).await
}

async fn verify_logger_stopped<R: Runtime>(
    work: &LogWork<R>,
    console: &crate::console_service::ConsoleState,
) -> Result<(), LogOperationError> {
    work.check()?;
    let output = crate::console_service::exec_binding(
        console,
        &work.binding.connection,
        &work.session,
        "logger status",
    )
    .await
    .map_err(map_shell)?;
    let logger = maggcs_core::logs::parse_logger_status(&output).map_err(|error| {
        LogProtocolError::InvalidRequest {
            message: error.to_string(),
        }
    })?;
    if logger.file_logging {
        return Err(LogProtocolError::InvalidRequest {
            message: "the FC logger is recording; stop it before deleting logs".into(),
        }
        .into());
    }
    destructive_admission(&work.app, &work.session)
}

async fn list_for_readback<R: Runtime>(
    work: &LogWork<R>,
) -> Result<Vec<LogEntryInfo>, LogOperationError> {
    let transaction = work.new_transaction()?;
    match work.binding.service.list_owned(transaction.clone()).await {
        Ok(entries) => Ok(entries),
        Err(error @ LogProtocolError::NoAck { .. }) if work.console.is_some() => {
            work.binding.service.cancel_owned(&transaction).await?;
            work.check()?;
            let console = work
                .console
                .as_ref()
                .ok_or(OperationError::RegistryUnavailable)?;
            work.shell_used.store(true, Ordering::Release);
            let output = crate::console_service::exec_binding(
                console,
                &work.binding.connection,
                &work.session,
                &format!("ls -R {}", crate::px4_shell::LOG_DIRECTORY),
            )
            .await
            .map_err(map_shell)?;
            if crate::px4_shell::parse_empty_log_directory(&output)
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

async fn atomic_write(
    path: String,
    bytes: Vec<u8>,
    operation: OperationId,
    control: Arc<FileControl>,
    check: impl Fn() -> Result<(), LogOperationError> + Send + 'static,
) -> Result<(), LogOperationError> {
    control.finished.store(false, Ordering::Release);
    let _cancel = CancelFile(control.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let _finished = FinishedFile(control.clone());
        let destination = PathBuf::from(path);
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = destination
            .file_name()
            .ok_or_else(|| "destination has no file name".to_string())?;
        let temporary = parent.join(format!(".{}.{}.partial", name.to_string_lossy(), operation));
        let mut created = false;
        let result = (|| -> Result<(), String> {
            let authorized = || -> Result<(), String> {
                if control.cancelled.is_cancelled() {
                    return Err("local file write was cancelled".into());
                }
                check().map_err(|error| error.to_string())
            };
            authorized()?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            created = true;
            for chunk in bytes.chunks(FILE_CHUNK_BYTES) {
                authorized()?;
                file.write_all(chunk).map_err(|error| error.to_string())?;
            }
            file.sync_all().map_err(|error| error.to_string())?;
            authorized()?;
            control.may_commit.store(true, Ordering::Release);
            // Do not delete an existing destination as a non-atomic fallback.
            std::fs::rename(&temporary, &destination).map_err(|error| error.to_string())?;
            #[cfg(unix)]
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())?;
            Ok(())
        })();
        if created && result.is_err() {
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

impl<R: Runtime> LogWork<R> {
    pub(crate) fn operation(&self) -> &OperationRef {
        self.lease.operation()
    }
    fn new_transaction(&self) -> Result<LogTransaction, LogOperationError> {
        self.check()?;
        let remaining = retained_status(&self.app, self.operation())?.remaining_budget_ms;
        let transaction = LogTransaction::new(
            self.operation().id.clone(),
            self.session.target.generation,
            Duration::from_millis(remaining),
        )?
        .with_owner_cancellation(self.lease.cancellation());
        *self.transaction.lock() = Some(transaction.clone());
        Ok(transaction)
    }
    fn check(&self) -> Result<(), LogOperationError> {
        self.lease.check()?;
        let state = self.app.state::<AppState>();
        state
            .runtime_clients
            .authorize(&self.window, &self.session)?;
        RuntimeFacade::new(&self.app)
            .bind(&self.session.target)
            .map_err(runtime_error)?;
        crate::main_link_service::ensure_primary_tools_available(&state)
            .map_err(|message| OperationError::InvalidRequest { message }.into())
    }

    async fn execute(&self, action: LogAction) -> Result<LogResult, LogOperationError> {
        match action {
            LogAction::List => list_for_readback(self)
                .await
                .map(|entries| LogResult::Listed { entries }),
            LogAction::Download { entry, save_path } => {
                let entries = list_for_readback(self).await?;
                if !entries.contains(&entry) {
                    return Err(LogProtocolError::InvalidRequest {
                        message: "log identity changed; refresh the list before downloading".into(),
                    }
                    .into());
                }
                let transaction = self.new_transaction()?;
                let bytes = self
                    .binding
                    .service
                    .download_owned(transaction, entry.clone())
                    .await?;
                self.check()?;
                let app = self.app.clone();
                let window = self.window.clone();
                let session = self.session.clone();
                let cancellation = self.lease.cancellation();
                let operation = self.operation().clone();
                atomic_write(
                    save_path.clone(),
                    bytes,
                    operation.id.clone(),
                    self.file.clone(),
                    move || {
                        if cancellation.is_cancelled() {
                            return Err(OperationError::Cancelled.into());
                        }
                        app.state::<AppState>()
                            .runtime_clients
                            .authorize(&window, &session)?;
                        RuntimeFacade::new(&app)
                            .bind(&session.target)
                            .map_err(runtime_error)?;
                        if retained_status(&app, &operation)?.remaining_budget_ms == 0 {
                            return Err(OperationError::DeadlineExceeded.into());
                        }
                        Ok(())
                    },
                )
                .await?;
                Ok(LogResult::Downloaded {
                    entry,
                    path: save_path,
                })
            }
            LogAction::Delete { entry } => {
                let console = self
                    .console
                    .as_ref()
                    .ok_or(OperationError::RegistryUnavailable)?;
                verify_delete_shell(self, console).await?;
                let listing = crate::console_service::exec_binding(
                    console,
                    &self.binding.connection,
                    &self.session,
                    "cat /fs/microsd/logdata.txt",
                )
                .await
                .map_err(map_shell)?;
                let path = crate::px4_shell::log_path_from_listing(&listing, &entry)
                    .map_err(|message| LogProtocolError::InvalidRequest { message })?;
                verify_logger_stopped(self, console).await?;
                destructive_admission(&self.app, &self.session)?;
                self.check()?;
                self.destructive_dispatched.store(true, Ordering::Release);
                let output = crate::console_service::exec_binding(
                    console,
                    &self.binding.connection,
                    &self.session,
                    &format!("rm {path}"),
                )
                .await
                .map_err(map_shell)?;
                if let Some(error) = crate::px4_shell::command_error(&output) {
                    return Err(LogProtocolError::InvalidRequest {
                        message: format!("SD log deletion failed: {error}"),
                    }
                    .into());
                }
                let output = crate::console_service::exec_binding(
                    console,
                    &self.binding.connection,
                    &self.session,
                    &format!("ls -l {path}"),
                )
                .await
                .map_err(map_shell)?;
                if !output.lines().any(|line| {
                    line.starts_with("nsh: ls: stat failed:")
                        && line.contains("No such file or directory")
                }) {
                    return Err(LogProtocolError::InvalidRequest {
                        message: "FC did not confirm the log was removed".into(),
                    }
                    .into());
                }
                list_for_readback(self)
                    .await
                    .map(|entries| LogResult::Deleted { entries })
            }
            LogAction::Erase => {
                let console = self
                    .console
                    .as_ref()
                    .ok_or(OperationError::RegistryUnavailable)?;
                verify_delete_shell(self, console).await?;
                destructive_admission(&self.app, &self.session)?;
                self.check()?;
                self.destructive_dispatched.store(true, Ordering::Release);
                let mut protocol = maggcs_core::logs::LogProtocol::new(
                    self.session.target.mavlink_target.system_id,
                    self.session.target.mavlink_target.component_id,
                );
                for frame in protocol.begin_erase() {
                    let deadline = tokio::time::Instant::now() + WRITE_TIMEOUT;
                    self.binding
                        .connection
                        .send_in_generation(frame, self.session.target.generation.get(), deadline)
                        .await
                        .map_err(|_| LogProtocolError::TargetChanged)?;
                }
                let deadline = Instant::now() + ERASE_READBACK_BUDGET;
                loop {
                    self.check()?;
                    let output = crate::console_service::exec_binding(
                        console,
                        &self.binding.connection,
                        &self.session,
                        &format!("ls -R {}", crate::px4_shell::LOG_DIRECTORY),
                    )
                    .await
                    .map_err(map_shell)?;
                    if crate::px4_shell::parse_empty_log_directory(&output)
                        .map_err(|message| LogProtocolError::InvalidRequest { message })?
                    {
                        return Ok(LogResult::Erased);
                    }
                    if Instant::now() >= deadline {
                        return Err(LogProtocolError::TimedOut.into());
                    }
                    tokio::time::sleep(ERASE_POLL).await;
                }
            }
        }
    }

    pub(crate) async fn run(mut self) -> Result<LogReply, LogOperationError> {
        if let Err(error) = self
            .check()
            .and_then(|()| self.lease.mark_running().map_err(Into::into))
        {
            let app = self.app.clone();
            let operation = self.operation().clone();
            drop(self);
            if let Ok(status) = retained_status(&app, &operation) {
                publish(&app, &status);
            }
            return Err(error);
        }
        if let Ok(status) = retained_status(&self.app, self.operation()) {
            publish(&self.app, &status);
        }
        let action = self
            .action
            .take()
            .ok_or(OperationError::RegistryUnavailable)?;
        let uses_shell = action.uses_shell();
        let cancellation = self.lease.cancellation();
        let mut execution = Box::pin(self.execute(action));
        let mut tick = tokio::time::interval(TARGET_POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! { biased;
                _ = cancellation.cancelled() => break Err(self.lease.check().err().unwrap_or(OperationError::Cancelled).into()),
                result = &mut execution => break result,
                _ = tick.tick() => if let Err(error) = self.check() { break Err(error); },
            }
        };
        drop(execution);
        let uses_shell = uses_shell || self.shell_used.load(Ordering::Acquire);
        let shell_cleanup = if uses_shell {
            let console = self
                .console
                .as_ref()
                .ok_or(OperationError::RegistryUnavailable)?;
            Some(
                crate::console_service::release_binding(
                    console,
                    &self.binding.connection,
                    &self.session,
                )
                .await,
            )
        } else {
            None
        };
        let result = result.and_then(|result| self.check().map(|()| result));
        match result {
            Ok(result) if shell_cleanup.as_ref().is_none_or(Result::is_ok) => {
                let operation = self.operation().clone();
                let cleanup = if uses_shell {
                    CleanupOutcome::Confirmed
                } else {
                    CleanupOutcome::NotRequired
                };
                finish(&self.app, self.lease, OperationOutcome::Succeeded, cleanup)?;
                if matches!(
                    result,
                    LogResult::Listed { .. } | LogResult::Deleted { .. } | LogResult::Erased
                ) {
                    self.app
                        .state::<AppState>()
                        .logs
                        .reconciliation
                        .lock()
                        .take();
                }
                Ok(LogReply { operation, result })
            }
            Ok(_) => {
                self.failed(LogProtocolError::CleanupTimeout.into(), shell_cleanup)
                    .await
            }
            Err(error) => self.failed(error, shell_cleanup).await,
        }
    }

    async fn failed(
        self,
        error: LogOperationError,
        shell_cleanup: Option<Result<(), maggcs_core::shell::ShellProtocolError>>,
    ) -> Result<LogReply, LogOperationError> {
        let operation = self.operation().clone();
        let transaction = self.transaction.lock().clone();
        let worker_cleanup = match transaction {
            Some(transaction) => self
                .binding
                .service
                .cancel_owned(&transaction)
                .await
                .map(|_| ()),
            None => Ok(()),
        };
        self.file.cancelled.cancel();
        let file_cleanup = tokio::time::timeout(FILE_CLEANUP_BUDGET, async {
            while !self.file.finished.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let destructive = self.destructive_dispatched.load(Ordering::Acquire)
            || self.file.may_commit.load(Ordering::Acquire);
        let protocol_uncertain = matches!(
            &error,
            LogOperationError::Protocol {
                error: LogProtocolError::CleanupTimeout
                    | LogProtocolError::TargetChanged
                    | LogProtocolError::Stopped
            }
        );
        let cleanup_confirmed = worker_cleanup.is_ok()
            && file_cleanup.is_ok()
            && shell_cleanup.as_ref().is_none_or(Result::is_ok)
            && !matches!(
                &error,
                LogOperationError::Protocol {
                    error: LogProtocolError::CleanupTimeout
                }
            );
        if destructive || protocol_uncertain || !cleanup_confirmed {
            if self.destructive_dispatched.load(Ordering::Acquire) {
                *self.app.state::<AppState>().logs.reconciliation.lock() = Some(operation.clone());
            }
            let reason = format!("{error}; log cleanup: {worker_cleanup:?}; shell release: {shell_cleanup:?}; target log state requires read-back");
            let cleanup = if cleanup_confirmed {
                CleanupOutcome::Confirmed
            } else {
                CleanupOutcome::Uncertain
            };
            finish(
                &self.app,
                self.lease,
                OperationOutcome::Uncertain {
                    reason: reason.clone(),
                },
                cleanup,
            )?;
            self.app
                .state::<AppState>()
                .alerts()
                .raise(maggcs_core::alerts::AlertInput::new(
                    maggcs_core::alerts::AlertSource::System,
                    maggcs_core::alerts::Severity::Warn,
                    format!(
                        "Log operation {} requires reconciliation: {reason}",
                        operation.id
                    ),
                ));
            return Err(LogOperationError::ReconciliationRequired {
                operation: Box::new(operation),
                reason,
            });
        }
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
        let outcome = if cancelled {
            OperationOutcome::Cancelled
        } else {
            OperationOutcome::Failed {
                message: error.to_string(),
            }
        };
        finish(&self.app, self.lease, outcome, CleanupOutcome::Confirmed)?;
        Err(error)
    }
}

fn owned_status<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<OperationStatus, LogOperationError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    let status = retained_status(app, operation)?;
    if !status.resources.contains(&OperationResource::LogTransfer) {
        return Err(OperationError::InvalidRequest {
            message: "operation is not an owned log operation".into(),
        }
        .into());
    }
    Ok(status)
}

async fn execute<R: Runtime>(work: LogWork<R>) -> Result<LogReply, LogOperationError> {
    tauri::async_runtime::spawn(work.run())
        .await
        .map_err(|_| OperationError::RegistryUnavailable)?
}

#[tauri::command]
pub async fn logs_request_list(
    app: AppHandle,
    window: WebviewWindow,
    console: State<'_, Arc<crate::console_service::ConsoleState>>,
    request: LogRequestContext,
) -> Result<LogReply, LogOperationError> {
    execute(begin(
        &app,
        window.label(),
        request,
        LogAction::List,
        Some(Arc::clone(&console)),
    )?)
    .await
}

#[tauri::command]
pub async fn logs_download(
    app: AppHandle,
    window: WebviewWindow,
    request: LogDownloadRequest,
) -> Result<LogReply, LogOperationError> {
    execute(begin(
        &app,
        window.label(),
        request.context,
        LogAction::Download {
            entry: request.entry,
            save_path: request.save_path,
        },
        None,
    )?)
    .await
}

#[tauri::command]
pub async fn logs_delete(
    app: AppHandle,
    window: WebviewWindow,
    console: State<'_, Arc<crate::console_service::ConsoleState>>,
    request: LogDeleteRequest,
) -> Result<LogReply, LogOperationError> {
    if request.confirmation != delete_confirmation(&request.entry) {
        return Err(LogProtocolError::InvalidRequest {
            message: "single-log deletion confirmation does not match its id".into(),
        }
        .into());
    }
    execute(begin(
        &app,
        window.label(),
        request.context,
        LogAction::Delete {
            entry: request.entry,
        },
        Some(Arc::clone(&console)),
    )?)
    .await
}

#[tauri::command]
pub async fn logs_erase_all(
    app: AppHandle,
    window: WebviewWindow,
    console: State<'_, Arc<crate::console_service::ConsoleState>>,
    request: LogEraseRequest,
) -> Result<LogReply, LogOperationError> {
    if request.confirmation != DELETE_ALL_CONFIRMATION {
        return Err(LogProtocolError::InvalidRequest {
            message: "erase-all confirmation must be DELETE ALL LOGS".into(),
        }
        .into());
    }
    execute(begin(
        &app,
        window.label(),
        request.context,
        LogAction::Erase,
        Some(Arc::clone(&console)),
    )?)
    .await
}

#[tauri::command]
pub fn logs_cancel(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, LogOperationError> {
    owned_status(&app, window.label(), &session, &operation)?;
    let status = app
        .state::<AppState>()
        .runtime_clients
        .operations
        .cancel(&session.owner, &operation)?;
    publish(&app, &status);
    Ok(status)
}

#[tauri::command]
pub fn logs_operation_status(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, LogOperationError> {
    owned_status(&app, window.label(), &session, &operation)
}

#[cfg(all(test, unix))]
#[path = "logs_operation_tests.rs"]
pub(crate) mod operation_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_paths_and_delete_confirmations_fail_closed() {
        assert!(validate_path("").is_err());
        assert!(validate_path("log.ulg").is_ok());
        let entry = LogEntryInfo {
            id: 7,
            size: 1,
            time_utc: 2,
        };
        assert_eq!(delete_confirmation(&entry), "DELETE LOG 7");
    }
}
