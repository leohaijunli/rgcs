//! Owner-bound PX4 MAVLink shell (`SERIAL_CONTROL_DEV_SHELL`).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use maggcs_core::mavlink::connection::ConnectionEvent;
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::mavlink::MavMessage;
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::{RequestId, Revision, RuntimeError};
use maggcs_core::shell::{
    validate_command, ShellCommandRequest, ShellOperationError, ShellOutputEnvelope,
    ShellProtocolError, ShellRequestContext, ShellSessionReply, ShellWriteReceipt,
};
use parking_lot::Mutex as ParkingMutex;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::runtime_facade::{RuntimeFacade, ShellBinding, TargetRuntime};
use crate::state::AppState;

const SERIAL_CONTROL_ID: u32 = 126;
const SHELL_SESSION_BUDGET: Duration = Duration::from_secs(24 * 60 * 60);
const TARGET_POLL: Duration = Duration::from_millis(100);
const CLOSE_BUDGET: Duration = Duration::from_secs(1);
const COMMAND_WRITE_BUDGET: Duration = Duration::from_secs(2);
const OUTPUT_CAPACITY: usize = 64;
const COMMAND_CAPACITY: usize = 8;
const MAX_SESSION_REQUEST_IDS: usize = 4096;
type CloseReply = oneshot::Sender<Result<OperationStatus, ShellOperationError>>;

struct ActiveConsole {
    session: RuntimeClientSession,
    operation: OperationRef,
    commands: mpsc::Sender<SessionCommand>,
    closes: mpsc::Sender<CloseReply>,
}

struct SessionReceivers {
    commands: mpsc::Receiver<SessionCommand>,
    closes: mpsc::Receiver<CloseReply>,
}

enum SessionCommand {
    Send {
        request_id: RequestId,
        command: String,
        bytes: u16,
        deadline: tokio::time::Instant,
        reply: oneshot::Sender<Result<ShellWriteReceipt, ShellOperationError>>,
    },
}

pub struct ConsoleState {
    active: ParkingMutex<Option<ActiveConsole>>,
    /// Internal marker exchanges and interactive writes share the one FC shell.
    shell_lock: Arc<Mutex<()>>,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            active: ParkingMutex::new(None),
            shell_lock: Arc::new(Mutex::new(())),
        }
    }
}

impl ConsoleState {
    pub(crate) async fn lock_shell(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.shell_lock.lock().await
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.lock().is_some()
    }

    fn active(
        &self,
        session: &RuntimeClientSession,
        operation: &OperationRef,
    ) -> Result<mpsc::Sender<SessionCommand>, ShellOperationError> {
        let active = self.active.lock();
        let active = active.as_ref().ok_or(ShellProtocolError::Stopped)?;
        if &active.session != session || &active.operation != operation {
            return Err(OperationError::NotOwner.into());
        }
        Ok(active.commands.clone())
    }

    fn clear(&self, operation: &OperationRef) {
        let mut active = self.active.lock();
        if active
            .as_ref()
            .is_some_and(|active| &active.operation == operation)
        {
            active.take();
        }
    }

    fn close_channel(
        &self,
        session: &RuntimeClientSession,
        operation: &OperationRef,
    ) -> Result<mpsc::Sender<CloseReply>, ShellOperationError> {
        let active = self.active.lock();
        let active = active.as_ref().ok_or(ShellProtocolError::Stopped)?;
        if &active.session != session || &active.operation != operation {
            return Err(OperationError::NotOwner.into());
        }
        Ok(active.closes.clone())
    }
}

fn runtime_error(error: RuntimeError) -> ShellOperationError {
    OperationError::Runtime { error }.into()
}

fn epoch_ms(received_at: Instant) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    now.saturating_sub(received_at.elapsed().as_millis().min(u64::MAX as u128) as u64)
}

fn close_frame(session: &RuntimeClientSession) -> MavMessage {
    MavMessage::SERIAL_CONTROL(::mavlink::common::SERIAL_CONTROL_DATA {
        device: ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_SHELL,
        flags: ::mavlink::common::SerialControlFlag::empty(),
        target_system: session.target.mavlink_target.system_id,
        target_component: session.target.mavlink_target.component_id,
        timeout: 0,
        baudrate: 0,
        count: 0,
        data: [0; 70],
    })
}

async fn release_shell(
    console: &ConsoleState,
    binding: &ShellBinding,
    session: &RuntimeClientSession,
) -> Result<(), ShellProtocolError> {
    release_binding(console, &binding.connection, session).await
}

pub(crate) async fn release_binding(
    console: &ConsoleState,
    connection: &maggcs_core::mavlink::ConnectionHandle,
    session: &RuntimeClientSession,
) -> Result<(), ShellProtocolError> {
    let deadline = tokio::time::Instant::now() + CLOSE_BUDGET;
    let _shell = tokio::time::timeout_at(deadline, console.shell_lock.lock())
        .await
        .map_err(|_| ShellProtocolError::TimedOut)?;
    connection
        .send_in_generation(
            close_frame(session),
            session.target.generation.get(),
            deadline,
        )
        .await
        .map_err(|_| {
            if connection.generation() != session.target.generation.get() {
                ShellProtocolError::TargetChanged
            } else {
                ShellProtocolError::Stopped
            }
        })
}

pub(crate) async fn exec_binding(
    console: &ConsoleState,
    connection: &maggcs_core::mavlink::ConnectionHandle,
    session: &RuntimeClientSession,
    command: &str,
) -> Result<String, ShellProtocolError> {
    if console.is_active() {
        return Err(ShellProtocolError::Busy);
    }
    let _shell = console.shell_lock.lock().await;
    if connection.generation() != session.target.generation.get() {
        return Err(ShellProtocolError::TargetChanged);
    }
    let mut route = connection.subscribe_route(MessageRoute::messages(&[SERIAL_CONTROL_ID]).from(
        session.target.mavlink_target.system_id,
        session.target.mavlink_target.component_id,
    ));
    crate::px4_shell::exchange_in_generation(
        connection,
        session.target.generation.get(),
        session.target.mavlink_target.system_id,
        session.target.mavlink_target.component_id,
        &mut route,
        command,
    )
    .await
    .map_err(map_shell_error)
}

pub(crate) async fn probe_binding(
    console: &ConsoleState,
    connection: &maggcs_core::mavlink::ConnectionHandle,
    session: &RuntimeClientSession,
) -> Result<crate::px4_shell::ShellKind, ShellProtocolError> {
    if console.is_active() {
        return Err(ShellProtocolError::Busy);
    }
    let _shell = console.shell_lock.lock().await;
    if connection.generation() != session.target.generation.get() {
        return Err(ShellProtocolError::TargetChanged);
    }
    let mut route = connection.subscribe_route(MessageRoute::messages(&[SERIAL_CONTROL_ID]).from(
        session.target.mavlink_target.system_id,
        session.target.mavlink_target.component_id,
    ));
    crate::px4_shell::probe_shell_in_generation(
        connection,
        session.target.generation.get(),
        session.target.mavlink_target.system_id,
        session.target.mavlink_target.component_id,
        &mut route,
    )
    .await
    .map_err(map_shell_error)
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("console_operation", status);
}

fn retained_status<R: Runtime>(
    app: &AppHandle<R>,
    operation: &OperationRef,
) -> Result<OperationStatus, ShellOperationError> {
    Ok(app
        .state::<AppState>()
        .runtime_clients
        .operations
        .status(&operation.owner, operation)?)
}

fn map_shell_error(error: maggcs_core::logs::SdCaptureError) -> ShellProtocolError {
    use maggcs_core::logs::SdCaptureError;
    match error {
        SdCaptureError::TargetChanged => ShellProtocolError::TargetChanged,
        SdCaptureError::TargetUnavailable => ShellProtocolError::Stopped,
        SdCaptureError::ShellTimeout { .. } => ShellProtocolError::TimedOut,
        SdCaptureError::UnexpectedResponse { output } => {
            ShellProtocolError::UnexpectedResponse { message: output }
        }
        other => ShellProtocolError::UnexpectedResponse {
            message: other.to_string(),
        },
    }
}

async fn run_session<R: Runtime>(
    app: AppHandle<R>,
    console: Arc<ConsoleState>,
    window: String,
    session: RuntimeClientSession,
    binding: ShellBinding,
    lease: OperationLease,
    mut receivers: SessionReceivers,
) {
    let operation = lease.operation().clone();
    let cancellation = lease.cancellation();
    let mut route =
        binding
            .connection
            .subscribe_route(MessageRoute::messages(&[SERIAL_CONTROL_ID]).from(
                session.target.mavlink_target.system_id,
                session.target.mavlink_target.component_id,
            ));
    let opened = binding.connection.connected_since_snapshot();
    let (output_tx, mut output_rx) = mpsc::channel::<ShellOutputEnvelope>(OUTPUT_CAPACITY);
    let output_app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(output) = output_rx.recv().await {
            if RuntimeFacade::new(&output_app)
                .bind(&output.target)
                .is_err()
                || !retained_status(&output_app, &output.operation).is_ok_and(|status| {
                    status.phase == maggcs_core::runtime::operations::OperationPhase::Running
                })
            {
                continue;
            }
            let _ = output_app.emit("console_output", output);
        }
    });
    let mut sequence = 0u64;
    let mut request_ids = BTreeSet::new();
    let mut tick = tokio::time::interval(TARGET_POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut close_reply = None;
    let (outcome, cleanup) = 'session: loop {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                break match release_shell(&console, &binding, &session).await {
                    Ok(()) => (OperationOutcome::Cancelled, CleanupOutcome::Confirmed),
                    Err(error) => (OperationOutcome::Uncertain { reason: format!("owner cancellation; shell release failed: {error}") }, CleanupOutcome::Uncertain),
                };
            }
            close = receivers.closes.recv() => {
                close_reply = close;
                break match release_shell(&console, &binding, &session).await {
                    Ok(()) => (OperationOutcome::Succeeded, CleanupOutcome::Confirmed),
                    Err(error) => (OperationOutcome::Uncertain { reason: format!("shell close is unconfirmed: {error}") }, CleanupOutcome::Uncertain),
                };
            }
            command = receivers.commands.recv() => {
                let Some(command) = command else {
                    break match release_shell(&console, &binding, &session).await {
                        Ok(()) => (OperationOutcome::Cancelled, CleanupOutcome::Confirmed),
                        Err(error) => (OperationOutcome::Uncertain { reason: format!("shell driver closed; release failed: {error}") }, CleanupOutcome::Uncertain),
                    };
                };
                match command {
                    SessionCommand::Send { request_id, command, bytes, deadline, reply } => {
                        if reply.is_closed() || tokio::time::Instant::now() >= deadline { continue; }
                        if request_ids.contains(&request_id) {
                            let _ = reply.send(Err(ShellProtocolError::InvalidCommand { message: "duplicate shell request ID".into() }.into()));
                            continue;
                        }
                        if request_ids.len() >= MAX_SESSION_REQUEST_IDS {
                            let _ = reply.send(Err(ShellProtocolError::InvalidCommand { message: "shell session request history is full; close and reopen the console".into() }.into()));
                            continue;
                        }
                        request_ids.insert(request_id);
                        let mut sending = Box::pin(async {
                            lease.check()?;
                            app.state::<AppState>().runtime_clients.authorize(&window, &session)?;
                            RuntimeFacade::new(&app).bind(&session.target).map_err(runtime_error)?;
                            let _shell = tokio::time::timeout_at(deadline, console.shell_lock.lock()).await
                                .map_err(|_| ShellProtocolError::TimedOut)?;
                            lease.check()?;
                            tokio::select! { biased;
                                _ = cancellation.cancelled() => return Err(OperationError::Cancelled.into()),
                                result = tokio::time::timeout_at(deadline, crate::px4_shell::send_lines(
                                &binding.connection,
                                session.target.generation.get(),
                                session.target.mavlink_target.system_id,
                                session.target.mavlink_target.component_id,
                                vec![command],
                                )) => result.map_err(|_| ShellProtocolError::TimedOut)?.map_err(map_shell_error)?,
                            }
                            Ok(ShellWriteReceipt { operation: operation.clone(), bytes })
                        });
                        let result = tokio::select! { biased;
                            close = receivers.closes.recv() => {
                                drop(sending);
                                close_reply = close;
                                break 'session match release_shell(&console, &binding, &session).await {
                                    Ok(()) => (OperationOutcome::Succeeded, CleanupOutcome::Confirmed),
                                    Err(error) => (OperationOutcome::Uncertain { reason: format!("priority shell close is unconfirmed: {error}") }, CleanupOutcome::Uncertain),
                                };
                            }
                            _ = cancellation.cancelled() => Err(OperationError::Cancelled.into()),
                            result = &mut sending => result,
                        };
                        drop(sending); // Release a cancelled write/lock waiter before cleanup.
                        let fatal = result.is_err(); // A partial line must never prefix a later command.
                        let _ = reply.send(result);
                        if fatal {
                            break match release_shell(&console, &binding, &session).await {
                                Ok(()) => (OperationOutcome::Cancelled, CleanupOutcome::Confirmed),
                                Err(error) => (OperationOutcome::Uncertain { reason: format!("shell write failed and release is unconfirmed: {error}") }, CleanupOutcome::Uncertain),
                            };
                        }
                    }
                }
            }
            event = route.recv() => {
                match event {
                    Ok(ConnectionEvent::Message(envelope)) => {
                        if binding.connection.generation() != session.target.generation.get()
                            || RuntimeFacade::new(&app).bind(&session.target).is_err() { continue; }
                        if opened.is_some_and(|opened| envelope.received_at < opened) { continue; }
                        let MavMessage::SERIAL_CONTROL(message) = envelope.message else { continue; };
                        let Some(chunk) = crate::px4_shell::reply(
                            &message, binding.connection.config().system_id, binding.connection.config().component_id,
                        ) else { continue; };
                        let Some(next) = sequence.checked_add(1) else {
                            break (OperationOutcome::Failed { message: ShellProtocolError::OutputLimit.to_string() }, CleanupOutcome::Uncertain);
                        };
                        sequence = next;
                        let Ok(revision) = Revision::new(sequence) else {
                            break (OperationOutcome::Failed { message: ShellProtocolError::OutputLimit.to_string() }, CleanupOutcome::Uncertain);
                        };
                        let _ = output_tx.try_send(ShellOutputEnvelope {
                            operation: operation.clone(), target: session.target.clone(), sequence: revision,
                            received_at_ms: epoch_ms(envelope.received_at), chunk,
                        });
                    }
                    Ok(ConnectionEvent::Connected { .. } | ConnectionEvent::HeartbeatLost { .. }
                        | ConnectionEvent::LinkError(_) | ConnectionEvent::Failed(_))
                    | Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break (OperationOutcome::Uncertain { reason: "shell target changed or disconnected before release".into() }, CleanupOutcome::Uncertain);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        sequence = sequence.saturating_add(1); // The next envelope exposes the gap.
                    }
                    _ => {}
                }
            }
            _ = tick.tick() => {
                if lease.check().is_err()
                    || binding.connection.generation() != session.target.generation.get()
                    || RuntimeFacade::new(&app).bind(&session.target).is_err() {
                    break match release_shell(&console, &binding, &session).await {
                        Ok(()) => (OperationOutcome::Cancelled, CleanupOutcome::Confirmed),
                        Err(error) => (OperationOutcome::Uncertain { reason: format!("target/owner expired; shell release failed: {error}") }, CleanupOutcome::Uncertain),
                    };
                }
            }
        }
    };
    drop(output_tx);
    console.clear(&operation);
    let status = match lease.finish(outcome, cleanup) {
        Ok(status) => status,
        Err(_) => match retained_status(&app, &operation) {
            Ok(status) => status,
            Err(error) => {
                if let Some(reply) = close_reply {
                    let _ = reply.send(Err(error));
                }
                return;
            }
        },
    };
    publish(&app, &status);
    if status.cleanup == CleanupOutcome::Uncertain {
        app.state::<AppState>()
            .alerts()
            .raise(maggcs_core::alerts::AlertInput::new(
                maggcs_core::alerts::AlertSource::System,
                maggcs_core::alerts::Severity::Warn,
                format!(
                    "Shell operation {} requires release reconciliation",
                    operation.id
                ),
            ));
    }
    if let Some(reply) = close_reply {
        let _ = reply.send(Ok(status));
    }
}

#[tauri::command]
pub async fn console_open(
    app: AppHandle,
    window: WebviewWindow,
    console: tauri::State<'_, Arc<ConsoleState>>,
    request: ShellRequestContext,
) -> Result<ShellSessionReply, ShellOperationError> {
    open_owned(&app, window.label(), Arc::clone(&console), request).await
}

pub(crate) async fn open_owned<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    console: Arc<ConsoleState>,
    request: ShellRequestContext,
) -> Result<ShellSessionReply, ShellOperationError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, &request.session)?;
    crate::main_link_service::ensure_primary_tools_available(&state)
        .map_err(|message| OperationError::InvalidRequest { message })?;
    if console.is_active() {
        return Err(ShellProtocolError::Busy.into());
    }
    let runtime = RuntimeFacade::new(app);
    let binding = runtime
        .shell_binding(&request.session.target)
        .map_err(runtime_error)?;
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: request.request_id,
        owner: request.session.owner.clone(),
        target: request.session.target.clone(),
        resources: vec![OperationResource::Shell],
        budget_ms: SHELL_SESSION_BUDGET.as_millis() as u64,
    })?;
    state.runtime_clients.authorize(window, &request.session)?;
    runtime
        .bind(&request.session.target)
        .map_err(runtime_error)?;
    lease.mark_running()?;
    let operation = lease.operation().clone();
    let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (closes, close_receiver) = mpsc::channel(1);
    *console.active.lock() = Some(ActiveConsole {
        session: request.session.clone(),
        operation: operation.clone(),
        commands,
        closes,
    });
    if let Ok(status) = retained_status(app, &operation) {
        publish(app, &status);
    }
    tauri::async_runtime::spawn(run_session(
        app.clone(),
        console,
        window.into(),
        request.session,
        binding,
        lease,
        SessionReceivers {
            commands: receiver,
            closes: close_receiver,
        },
    ));
    Ok(ShellSessionReply { operation })
}

#[tauri::command]
pub async fn console_send(
    app: AppHandle,
    window: WebviewWindow,
    console: tauri::State<'_, Arc<ConsoleState>>,
    request: ShellCommandRequest,
) -> Result<ShellWriteReceipt, ShellOperationError> {
    send_owned(&app, window.label(), &console, request).await
}

pub(crate) async fn send_owned<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    console: &ConsoleState,
    request: ShellCommandRequest,
) -> Result<ShellWriteReceipt, ShellOperationError> {
    let bytes = validate_command(&request.command, request.confirmation.as_deref())?;
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, &request.session)?;
    RuntimeFacade::new(app)
        .bind(&request.session.target)
        .map_err(runtime_error)?;
    let commands = console.active(&request.session, &request.operation)?;
    let (reply, response) = oneshot::channel();
    let deadline = tokio::time::Instant::now() + COMMAND_WRITE_BUDGET;
    tokio::time::timeout_at(deadline, async {
        commands
            .send(SessionCommand::Send {
                request_id: request.request_id,
                command: request.command,
                bytes,
                deadline,
                reply,
            })
            .await
            .map_err(|_| ShellProtocolError::Stopped)?;
        response.await.map_err(|_| ShellProtocolError::Stopped)?
    })
    .await
    .map_err(|_| ShellProtocolError::TimedOut)?
}

#[tauri::command]
pub async fn console_close(
    app: AppHandle,
    window: WebviewWindow,
    console: tauri::State<'_, Arc<ConsoleState>>,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, ShellOperationError> {
    close_owned(&app, window.label(), &console, &session, &operation).await
}

pub(crate) async fn close_owned<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    console: &ConsoleState,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<OperationStatus, ShellOperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window, session)?;
    let closes = console.close_channel(session, operation)?;
    let (reply, response) = oneshot::channel();
    tokio::time::timeout(CLOSE_BUDGET, async {
        closes
            .send(reply)
            .await
            .map_err(|_| ShellProtocolError::Stopped)?;
        response.await.map_err(|_| ShellProtocolError::Stopped)?
    })
    .await
    .map_err(|_| ShellProtocolError::TimedOut)?
}

/// Trusted lifecycle hook: stop only the recorded session, before primary I/O.
pub(crate) async fn stop<R: Runtime>(
    app: &AppHandle<R>,
    budget: Duration,
) -> maggcs_core::lifecycle::StopOutcome {
    use maggcs_core::lifecycle::StopOutcome;
    let Some(console) = app.try_state::<Arc<ConsoleState>>() else {
        return StopOutcome::NotRunning;
    };
    let Some(operation) = console
        .active
        .lock()
        .as_ref()
        .map(|active| active.operation.clone())
    else {
        return StopOutcome::NotRunning;
    };
    let started = Instant::now();
    let result = app
        .state::<AppState>()
        .runtime_clients
        .operations
        .cancel(&operation.owner, &operation);
    if let Err(error) = result {
        return StopOutcome::Failed {
            message: error.to_string(),
        };
    }
    match tokio::time::timeout(budget, async {
        while console.is_active() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        retained_status(app, &operation)
    })
    .await
    {
        Ok(Ok(status)) if status.cleanup != CleanupOutcome::Uncertain => StopOutcome::Stopped {
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
        Ok(Ok(_)) => StopOutcome::Failed {
            message: "shell release is unconfirmed".into(),
        },
        Ok(Err(error)) => StopOutcome::Failed {
            message: error.to_string(),
        },
        Err(_) => StopOutcome::TimedOut {
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
    }
}

fn owned_status<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<OperationStatus, ShellOperationError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    let status = retained_status(app, operation)?;
    if status.resources != [OperationResource::Shell] {
        return Err(OperationError::InvalidRequest {
            message: "operation is not an interactive shell".into(),
        }
        .into());
    }
    Ok(status)
}

#[tauri::command]
pub fn console_cancel(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, ShellOperationError> {
    cancel_owned(&app, window.label(), &session, &operation)
}

pub(crate) fn cancel_owned<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<OperationStatus, ShellOperationError> {
    owned_status(app, window, session, operation)?;
    let status = app
        .state::<AppState>()
        .runtime_clients
        .operations
        .cancel(&session.owner, operation)?;
    publish(app, &status);
    Ok(status)
}

#[tauri::command]
pub fn console_operation_status(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, ShellOperationError> {
    owned_status(&app, window.label(), &session, &operation)
}

#[cfg(all(test, unix))]
#[path = "console_operation_tests.rs"]
mod tests;
