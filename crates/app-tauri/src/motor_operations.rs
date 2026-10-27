//! Actual-window owned Motor admission and retained release receipts.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use maggcs_core::motor_test::operation_types::*;
use maggcs_core::motor_test::{MotorTestCommand, MotorTestService, SessionState, StopReceipt};
use maggcs_core::params::operation_types::ParameterCommandError;
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::{OperationId, RequestId, Revision};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};
use tokio::sync::{mpsc, oneshot};

use crate::runtime_facade::{RuntimeFacade, TargetRuntime};
use crate::state::AppState;

const SESSION_BUDGET_MS: u64 = 5 * 60 * 1000;
const START_BUDGET: Duration = Duration::from_secs(12);
const POLL: Duration = Duration::from_millis(50);
const MAX_RETAINED: usize = 32;
const MAX_REQUEST_IDS: usize = 16384;

#[derive(Default)]
pub struct MotorOperations {
    entries: Mutex<BTreeMap<OperationId, Arc<MotorScope>>>,
}
impl MotorOperations {
    pub(crate) fn is_active(&self) -> bool {
        !self.entries.lock().is_empty()
    }
}

pub(crate) struct MotorScope {
    pub session: RuntimeClientSession,
    pub operation: OperationRef,
    pub service: Mutex<Option<MotorTestService>>,
    permission: MotorPermissionRef,
    request: MotorStartRequest,
    pub deadline: tokio::time::Instant,
    last_revision: Mutex<u64>,
    requests: Mutex<BTreeSet<RequestId>>,
    stopping: AtomicBool,
    receipt: Mutex<Option<MotorReleaseReceipt>>,
    stops: mpsc::Sender<StopCall>,
}
struct StopCall {
    emergency: bool,
    reply: oneshot::Sender<Result<MotorReleaseReceipt, MotorOperationError>>,
}

pub(crate) fn binding<R: Runtime>(
    app: &AppHandle<R>,
    session: &RuntimeClientSession,
) -> Result<(), MotorOperationError> {
    let facade = RuntimeFacade::new(app);
    let views = facade
        .targets()
        .map_err(|error| OperationError::Runtime { error })?;
    if !views.iter().any(|view| {
        view.roles
            .contains(&maggcs_core::runtime::TargetRole::MotorTest)
            && view.target.as_ref() == Some(&session.target)
    }) {
        return Err(OperationError::TargetChanged.into());
    }
    facade
        .bind(&session.target)
        .map_err(|error| OperationError::Runtime { error })?;
    let state = app.state::<AppState>();
    let target = state.motor_test.target.lock();
    let target = target.as_ref().ok_or(OperationError::TargetChanged)?;
    if !target.generation_is_current()
        || target.target.handle.generation() != session.target.generation.get()
        || target.target.target_system_id != session.target.mavlink_target.system_id
    {
        return Err(OperationError::TargetChanged.into());
    }
    Ok(())
}

fn check_request<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: &MotorStartRequest,
) -> Result<(), MotorOperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window, &request.context.session)?;
    binding(app, &request.context.session)?;
    let state = app.state::<AppState>();
    crate::motor_control_service::ensure_owned(
        &state,
        &request.context.session,
        &request.permission,
    )?;
    let config = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .and_then(|target| target.target.config.lock().clone())
        .ok_or(MotorProtocolError::PermissionChanged)?;
    if config.revision != request.config_revision.get() {
        return Err(MotorProtocolError::PermissionChanged.into());
    }
    if let MotorStartSource::Manual { values } = &request.source {
        if values.len() != usize::from(config.map.active_count()) {
            return Err(MotorProtocolError::InvalidRequest {
                message: "manual values do not match the captured motor mapping".into(),
            }
            .into());
        }
    }
    if let MotorStartSource::Forward { source } = &request.source {
        if state.motor_test.sitl.lock().as_ref().is_none_or(|link| {
            &link.source_ref != source || !link.generation_is_current(state.connection().as_ref())
        }) {
            return Err(OperationError::TargetChanged.into());
        }
    }
    Ok(())
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("motor_operation", status);
}
fn plain(message: String) -> MotorOperationError {
    MotorProtocolError::InvalidRequest { message }.into()
}
pub(crate) fn invalid_request(message: impl Into<String>) -> MotorOperationError {
    plain(message.into())
}
pub(crate) fn parameter_error(error: MotorOperationError) -> ParameterCommandError {
    match error {
        MotorOperationError::Operation { error } => error.into(),
        other => OperationError::InvalidRequest {
            message: other.to_string(),
        }
        .into(),
    }
}
pub(crate) fn invalid_source() -> MotorOperationError {
    plain("motor start source does not match the native entry point".into())
}

pub(crate) fn authorize_context<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    context: &MotorRequestContext,
) -> Result<(), MotorOperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window, &context.session)?;
    binding(app, &context.session)
}

pub(crate) fn authorize_configuration<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    context: &MotorRequestContext,
    permission: &MotorPermissionRef,
    config_revision: Option<Revision>,
) -> Result<(), MotorOperationError> {
    authorize_context(app, window, context)?;
    let state = app.state::<AppState>();
    crate::motor_control_service::ensure_owned(&state, &context.session, permission)?;
    if let Some(expected) = config_revision {
        let actual = state.motor_test.target.lock().as_ref().and_then(|target| {
            target
                .target
                .config
                .lock()
                .as_ref()
                .map(|config| config.revision)
        });
        if actual != Some(expected.get()) {
            return Err(MotorProtocolError::PermissionChanged.into());
        }
    }
    Ok(())
}

pub(crate) struct Work<R: Runtime> {
    app: AppHandle<R>,
    window: String,
    request: MotorStartRequest,
    lease: Option<OperationLease>,
    pub scope: Arc<MotorScope>,
    stops: mpsc::Receiver<StopCall>,
}

pub(crate) fn begin<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorStartRequest,
) -> Result<Work<R>, MotorOperationError> {
    validate_start(&request)?;
    check_request(app, window, &request)?;
    let state = app.state::<AppState>();
    crate::actuator_service::ensure_idle_session(&state).map_err(plain)?;
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: request.context.request_id.clone(),
        owner: request.context.session.owner.clone(),
        target: request.context.session.target.clone(),
        resources: vec![OperationResource::ActuatorOutput],
        budget_ms: SESSION_BUDGET_MS,
    })?;
    check_request(app, window, &request)?;
    let (stops, receiver) = mpsc::channel(2);
    let remaining = state
        .runtime_clients
        .operations
        .status(&request.context.session.owner, lease.operation())?
        .remaining_budget_ms;
    let scope = Arc::new(MotorScope {
        session: request.context.session.clone(),
        operation: lease.operation().clone(),
        request: request.clone(),
        deadline: tokio::time::Instant::now() + Duration::from_millis(remaining),
        permission: request.permission.clone(),
        service: Mutex::new(None),
        last_revision: Mutex::new(0),
        requests: Mutex::new(BTreeSet::new()),
        stopping: AtomicBool::new(false),
        receipt: Mutex::new(None),
        stops,
    });
    let mut entries = state.motor_test.operations.entries.lock();
    if entries.len() >= MAX_RETAINED {
        let expired = entries
            .iter()
            .find(|(_, scope)| scope.receipt.lock().is_some())
            .map(|(id, _)| id.clone());
        if let Some(expired) = expired {
            entries.remove(&expired);
        }
    }
    if entries.len() >= MAX_RETAINED {
        return Err(OperationError::CapacityExceeded.into());
    }
    entries.insert(scope.operation.id.clone(), scope.clone());
    Ok(Work {
        app: app.clone(),
        window: window.into(),
        request,
        lease: Some(lease),
        scope,
        stops: receiver,
    })
}

fn retained<R: Runtime>(
    app: &AppHandle<R>,
    operation: &OperationRef,
) -> Result<OperationStatus, MotorOperationError> {
    Ok(app
        .state::<AppState>()
        .runtime_clients
        .operations
        .status(&operation.owner, operation)?)
}

impl<R: Runtime> Work<R> {
    fn check(&self) -> Result<(), MotorOperationError> {
        self.lease
            .as_ref()
            .ok_or(OperationError::RegistryUnavailable)?
            .check()?;
        check_request(&self.app, &self.window, &self.request)
    }
    fn finish(
        &mut self,
        outcome: OperationOutcome,
        cleanup: CleanupOutcome,
    ) -> Result<(), MotorOperationError> {
        let lease = self
            .lease
            .take()
            .ok_or(OperationError::RegistryUnavailable)?;
        match lease.finish(outcome, cleanup) {
            Ok(status) => publish(&self.app, &status),
            Err(error) => {
                if cleanup != CleanupOutcome::Uncertain {
                    let status = self
                        .app
                        .state::<AppState>()
                        .runtime_clients
                        .operations
                        .confirm_cleanup(&self.scope.operation, cleanup)?;
                    publish(&self.app, &status);
                }
                if cleanup == CleanupOutcome::Uncertain {
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }
    async fn cleanup(&self, emergency: bool) -> Result<StopReceipt, MotorOperationError> {
        let service = self.scope.service.lock().clone();
        let Some(service) = service else {
            return Ok(StopReceipt {
                released_motors: 0,
                state: if emergency {
                    SessionState::Emergency
                } else {
                    SessionState::Idle
                },
            });
        };
        let result = if emergency {
            service.emergency_stop_and_wait().await
        } else {
            service.stop_and_wait().await
        };
        match result {
            Ok(receipt) => Ok(receipt),
            Err(error) => match service.release_result() {
                Some(Ok(receipt)) => Ok(receipt),
                _ => Err(MotorProtocolError::ReleaseWrite {
                    motor: 0,
                    message: error.to_string(),
                }
                .into()),
            },
        }
    }
    async fn retired(
        &mut self,
        error: MotorOperationError,
        emergency: bool,
    ) -> Result<MotorReleaseReceipt, MotorOperationError> {
        self.scope.stopping.store(true, Ordering::Release);
        match self.cleanup(emergency).await {
            Ok(receipt) => {
                let cancelled = matches!(
                    &error,
                    MotorOperationError::Operation {
                        error: OperationError::Cancelled
                            | OperationError::NotOwner
                            | OperationError::TargetChanged
                    }
                );
                let outcome = if cancelled {
                    OperationOutcome::Cancelled
                } else {
                    OperationOutcome::Failed {
                        message: error.to_string(),
                    }
                };
                let receipt = MotorReleaseReceipt {
                    operation: self.scope.operation.clone(),
                    released_motors: receipt.released_motors as u8,
                    state: receipt.state,
                };
                *self.scope.receipt.lock() = Some(receipt.clone());
                self.finish(outcome, CleanupOutcome::Confirmed)?;
                Ok(receipt)
            }
            Err(cleanup) => {
                let reason = format!("{error}; original motor release: {cleanup}");
                self.finish(
                    OperationOutcome::Uncertain {
                        reason: reason.clone(),
                    },
                    CleanupOutcome::Uncertain,
                )?;
                Err(MotorOperationError::ReconciliationRequired {
                    operation: Box::new(self.scope.operation.clone()),
                    reason,
                })
            }
        }
    }
    pub(crate) async fn run(
        mut self,
        reply: oneshot::Sender<Result<MotorStartReply, MotorOperationError>>,
    ) {
        let mut reply = Some(reply);
        if let Err(error) = self.check().and_then(|()| {
            self.lease
                .as_mut()
                .ok_or(OperationError::RegistryUnavailable)?
                .mark_running()
                .map_err(Into::into)
        }) {
            if let Some(reply) = reply.take() {
                let _ = reply.send(Err(error));
            }
            self.app
                .state::<AppState>()
                .motor_test
                .operations
                .entries
                .lock()
                .remove(&self.scope.operation.id);
            return;
        }
        if let Ok(status) = retained(&self.app, &self.scope.operation) {
            publish(&self.app, &status);
        }
        let cancellation = self
            .lease
            .as_ref()
            .map(OperationLease::cancellation)
            .unwrap_or_default();
        let source = match &self.request.source {
            MotorStartSource::Manual { values } => {
                crate::actuator_service::StartSource::Manual(values.clone())
            }
            MotorStartSource::Preset { preset } => {
                crate::actuator_service::StartSource::Preset(*preset)
            }
            MotorStartSource::Forward { .. } => crate::actuator_service::StartSource::Forward,
        };
        let app = self.app.clone();
        let state = app.state::<AppState>();
        let scope = self.scope.clone();
        let window = self.window.clone();
        let check_start = || {
            if cancellation.is_cancelled() || scope.stopping.load(Ordering::Acquire) {
                return Err(OperationError::Cancelled.to_string());
            }
            check_request(&app, &window, &scope.request).map_err(|error| error.to_string())
        };
        let mut starting = Box::pin(crate::actuator_service::start_scoped(
            &app,
            &state,
            source,
            Some((&scope, cancellation.clone())),
            check_start,
        ));
        let mut poll = tokio::time::interval(POLL);
        enum Started {
            Ready,
            Failed(MotorOperationError),
            Stop(StopCall),
        }
        let deadline = tokio::time::Instant::now() + START_BUDGET;
        let step = loop {
            tokio::select! { biased;
                _ = cancellation.cancelled() => break Started::Failed(OperationError::Cancelled.into()),
                call = self.stops.recv() => if let Some(call) = call { break Started::Stop(call); },
                result = &mut starting => break match result { Ok(()) => Started::Ready, Err(error) => Started::Failed(plain(error)) },
                _ = tokio::time::sleep_until(deadline) => break Started::Failed(MotorProtocolError::TimedOut.into()),
                _ = poll.tick() => if let Err(error) = self.check() { break Started::Failed(error); },
            }
        };
        drop(starting);
        let step = match step {
            Started::Ready => loop {
                let service = self.scope.service.lock().clone();
                if service
                    .as_ref()
                    .is_some_and(MotorTestService::first_control_written)
                {
                    break Started::Ready;
                }
                if let Some(receipt) = service.as_ref().and_then(MotorTestService::release_result) {
                    break Started::Failed(plain(format!(
                        "motor service stopped before confirming a first output bank: {receipt:?}"
                    )));
                }
                tokio::select! { biased;
                    _ = cancellation.cancelled() => break Started::Failed(OperationError::Cancelled.into()),
                    call = self.stops.recv() => if let Some(call) = call { break Started::Stop(call); },
                    _ = tokio::time::sleep_until(deadline) => break Started::Failed(MotorProtocolError::TimedOut.into()),
                    _ = poll.tick() => if let Err(error) = self.check() { break Started::Failed(error); },
                }
            },
            other => other,
        };
        match step {
            Started::Failed(error) => {
                let retired = self.retired(error.clone(), false).await;
                if let Some(reply) = reply.take() {
                    let _ = reply.send(Err(retired.err().unwrap_or(error)));
                }
                return;
            }
            Started::Stop(call) => {
                let result = self
                    .retired(OperationError::Cancelled.into(), call.emergency)
                    .await;
                let _ = call.reply.send(result);
                if let Some(reply) = reply.take() {
                    let _ = reply.send(Err(OperationError::Cancelled.into()));
                }
                return;
            }
            Started::Ready => {
                if let Err(error) = self.check() {
                    let _ = self.retired(error.clone(), false).await;
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(Err(error));
                    }
                    return;
                }
                if let Err(error) = crate::run_service::confirm_motor_started(
                    &self.app,
                    &self.app.state::<AppState>(),
                ) {
                    let error = plain(error);
                    let _ = self.retired(error.clone(), false).await;
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(Err(error));
                    }
                    return;
                }
                if let Some(reply) = reply.take() {
                    let _ = reply.send(Ok(MotorStartReply {
                        operation: self.scope.operation.clone(),
                        config_revision: self.request.config_revision,
                    }));
                }
            }
        }
        loop {
            tokio::select! { biased;
                _ = cancellation.cancelled() => { let _ = self.retired(OperationError::Cancelled.into(), false).await; return; },
                call = self.stops.recv() => if let Some(call) = call {
                    let result = self.retired(OperationError::Cancelled.into(), call.emergency).await;
                    let _ = call.reply.send(result); return;
                },
                _ = poll.tick() => {
                    if let Err(error) = self.check() { let _ = self.retired(error, false).await; return; }
                    let receipt = self.scope.service.lock().as_ref().and_then(MotorTestService::release_result);
                    if let Some(Ok(receipt)) = receipt {
                        let reason = self.scope.service.lock().as_ref().and_then(MotorTestService::stop_reason);
                        *self.scope.receipt.lock() = Some(MotorReleaseReceipt { operation: self.scope.operation.clone(), released_motors: receipt.released_motors as u8, state: receipt.state });
                        self.scope.stopping.store(true, Ordering::Release);
                        let outcome = if reason == Some(maggcs_core::motor_test::StopReason::Completed) { OperationOutcome::Succeeded }
                            else { OperationOutcome::Failed { message: format!("motor session stopped: {reason:?}") } };
                        let _ = self.finish(outcome, CleanupOutcome::Confirmed); return;
                    }
                    if let Some(Err(error)) = receipt { let _ = self.retired(plain(error.to_string()), false).await; return; }
                }
            }
        }
    }
}

pub(crate) async fn start<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorStartRequest,
) -> Result<MotorStartReply, MotorOperationError> {
    let work = begin(app, window, request)?;
    let (reply, response) = oneshot::channel();
    tauri::async_runtime::spawn(work.run(reply));
    response
        .await
        .map_err(|_| MotorProtocolError::ServiceStopped)?
}

fn owned<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<Arc<MotorScope>, MotorOperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window, session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    let scope = app
        .state::<AppState>()
        .motor_test
        .operations
        .entries
        .lock()
        .get(&operation.id)
        .cloned()
        .ok_or(OperationError::NotFound)?;
    if scope.operation != *operation || scope.session != *session {
        return Err(OperationError::NotOwner.into());
    }
    Ok(scope)
}

pub(crate) fn values<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorValuesRequest,
) -> Result<MotorValuesReceipt, MotorOperationError> {
    request.validate()?;
    let scope = owned(app, window, &request.session, &request.operation)?;
    binding(app, &request.session)?;
    check_request(app, window, &scope.request)?;
    if !matches!(scope.request.source, MotorStartSource::Manual { .. }) {
        return Err(plain("value updates require a manual operation".into()));
    }
    if let MotorStartSource::Manual { values } = &scope.request.source {
        if values.len() != request.values.len() {
            return Err(plain(
                "value count changed from the captured motor mapping".into(),
            ));
        }
    }
    let status = retained(app, &request.operation)?;
    if status.phase != maggcs_core::runtime::operations::OperationPhase::Running
        || status.stop_reason.is_some()
    {
        return Err(OperationError::Cancelled.into());
    }
    let mut revision = scope.last_revision.lock();
    if scope.stopping.load(Ordering::Acquire) {
        return Err(OperationError::Cancelled.into());
    }
    if request.revision.get() <= *revision {
        return Err(MotorProtocolError::InvalidRequest {
            message: "obsolete motor value revision".into(),
        }
        .into());
    }
    let mut requests = scope.requests.lock();
    if requests.len() >= MAX_REQUEST_IDS {
        return Err(OperationError::CapacityExceeded.into());
    }
    if !requests.insert(request.request_id) {
        return Err(plain("duplicate motor value request".into()));
    }
    let service = scope
        .service
        .lock()
        .clone()
        .ok_or(MotorProtocolError::ServiceStopped)?;
    if service.session_state() != Some(SessionState::ManualRunning) {
        return Err(MotorProtocolError::ServiceStopped.into());
    }
    service
        .try_send(MotorTestCommand::SetValues(request.values))
        .map_err(|error| plain(error.to_string()))?;
    *revision = request.revision.get();
    Ok(MotorValuesReceipt {
        operation: request.operation,
        revision: request.revision,
    })
}

pub(crate) async fn stop<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorStopRequest,
) -> Result<MotorReleaseReceipt, MotorOperationError> {
    request.validate()?;
    let scope = owned(app, window, &request.session, &request.operation)?;
    if let Some(mut receipt) = scope.receipt.lock().clone() {
        receipt.released_motors = 0;
        return Ok(receipt);
    }
    {
        let _input = scope.last_revision.lock();
        scope.stopping.store(true, Ordering::Release);
    }
    let (reply, response) = oneshot::channel();
    let call = StopCall {
        emergency: request.emergency,
        reply,
    };
    if scope.stops.is_closed() {
        let service = scope
            .service
            .lock()
            .clone()
            .ok_or(MotorProtocolError::ServiceStopped)?;
        let receipt = if request.emergency {
            service.emergency_stop_and_wait().await
        } else {
            service.stop_and_wait().await
        }
        .map_err(|error| plain(error.to_string()))?;
        let status = app
            .state::<AppState>()
            .runtime_clients
            .operations
            .confirm_cleanup(&request.operation, CleanupOutcome::Confirmed)?;
        publish(app, &status);
        let receipt = MotorReleaseReceipt {
            operation: request.operation,
            released_motors: receipt.released_motors as u8,
            state: receipt.state,
        };
        *scope.receipt.lock() = Some(receipt.clone());
        return Ok(receipt);
    }
    tokio::time::timeout(
        maggcs_core::motor_test::service::MOTOR_TEST_STOP_TIMEOUT,
        async {
            scope
                .stops
                .send(call)
                .await
                .map_err(|_| MotorProtocolError::ServiceStopped)?;
            response
                .await
                .map_err(|_| MotorProtocolError::ServiceStopped)?
        },
    )
    .await
    .map_err(|_| MotorProtocolError::TimedOut)?
}

pub(crate) async fn reset_emergency<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorStopRequest,
) -> Result<(), MotorOperationError> {
    request.validate()?;
    let scope = owned(app, window, &request.session, &request.operation)?;
    binding(app, &request.session)?;
    if scope.receipt.lock().is_none() {
        return Err(plain(
            "confirm the original motor release before resetting emergency".into(),
        ));
    }
    let state = app.state::<AppState>();
    let _ops = state.ops().lock().await;
    let mut running = state.motor_test.running.lock();
    if running
        .as_ref()
        .is_some_and(|running| running.operation.as_ref() != Some(&request.operation))
    {
        return Err(OperationError::NotOwner.into());
    }
    if let Some(old) = running.take() {
        old.forwarder.abort();
    }
    state.motor_test.note_state(SessionState::Idle);
    Ok(())
}

pub(crate) fn cancel_permission<R: Runtime>(
    app: &AppHandle<R>,
    permission: &MotorPermissionRef,
) -> Result<(), MotorOperationError> {
    // The permission owner is one Motor module instance. This also retires its
    // owned parameter/tone/config work, not operations from another module.
    app.state::<AppState>()
        .runtime_clients
        .operations
        .cancel_owner(&permission.owner)?;
    let scopes = app
        .state::<AppState>()
        .motor_test
        .operations
        .entries
        .lock()
        .values()
        .filter(|scope| &scope.permission == permission)
        .cloned()
        .collect::<Vec<_>>();
    for scope in scopes {
        let status = retained(app, &scope.operation)?;
        if status.phase != maggcs_core::runtime::operations::OperationPhase::Finished {
            app.state::<AppState>()
                .runtime_clients
                .operations
                .cancel(&scope.session.owner, &scope.operation)?;
        }
    }
    Ok(())
}

pub(crate) async fn stop_all<R: Runtime>(
    app: &AppHandle<R>,
    budget: Duration,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let scopes = state
        .motor_test
        .operations
        .entries
        .lock()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for scope in &scopes {
        let status = retained(app, &scope.operation).map_err(|error| error.to_string())?;
        if status.phase != maggcs_core::runtime::operations::OperationPhase::Finished {
            state
                .runtime_clients
                .operations
                .cancel(&scope.session.owner, &scope.operation)
                .map_err(|error| error.to_string())?;
        }
    }
    tokio::time::timeout(budget, async {
        loop {
            let complete = scopes.iter().all(|scope| {
                retained(app, &scope.operation).is_ok_and(|status| {
                    status.phase == maggcs_core::runtime::operations::OperationPhase::Finished
                        && status.cleanup != CleanupOutcome::Pending
                })
            });
            if complete {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .map_err(|_| "timed out waiting for owned Motor operations to release".to_string())?;
    if scopes.iter().any(|scope| {
        !retained(app, &scope.operation)
            .is_ok_and(|status| status.cleanup != CleanupOutcome::Uncertain)
    }) {
        return Err(
            "Motor output release is unconfirmed; target link preserved for STOP retry".into(),
        );
    }
    Ok(())
}

#[tauri::command]
pub fn motor_operation_status(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, MotorOperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window.label(), &session)?;
    if session.owner != operation.owner || session.target != operation.target {
        return Err(OperationError::NotOwner.into());
    }
    let status = retained(&app, &operation)?;
    if status.resources != [OperationResource::ActuatorOutput] {
        return Err(plain("operation is not a motor session".into()));
    }
    Ok(status)
}

#[tauri::command]
pub fn motor_operation_cancel(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, MotorOperationError> {
    owned(&app, window.label(), &session, &operation)?;
    let status = app
        .state::<AppState>()
        .runtime_clients
        .operations
        .cancel(&session.owner, &operation)?;
    publish(&app, &status);
    Ok(status)
}

#[cfg(all(test, unix))]
#[path = "motor_operation_tests.rs"]
mod tests;
