//! Owner-bound Motor SD capture; persistent file identity outlives shell calls.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use maggcs_core::logs::capture::{CapturePhase, SdCaptureError, SdCaptureStatus};
use maggcs_core::motor_test::operation_types::*;
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::{ClientId, OperationId};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::{mpsc, oneshot};

use crate::state::AppState;

const CAPTURE_BUDGET_MS: u64 = 4 * 60 * 60 * 1000;
const STOP_BUDGET: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(100);
const MAX_RETAINED: usize = 16;

#[derive(Default)]
pub struct MotorCaptureOperations {
    entries: Mutex<BTreeMap<OperationId, Arc<CaptureScope>>>,
    current: Mutex<Option<OperationRef>>,
}
impl MotorCaptureOperations {
    pub(crate) fn current(&self) -> Option<OperationRef> {
        self.current.lock().clone()
    }
    pub(crate) fn is_active(&self) -> bool {
        self.current.lock().is_some()
    }
}
struct CaptureScope {
    session: RuntimeClientSession,
    operation: OperationRef,
    test_id: String,
    path: Mutex<Option<String>>,
    stops: mpsc::Sender<oneshot::Sender<Result<MotorCaptureReply, MotorOperationError>>>,
    receipt: Mutex<Option<MotorCaptureReply>>,
}
struct Work<R: Runtime> {
    app: AppHandle<R>,
    window: String,
    context: MotorRequestContext,
    lease: Option<OperationLease>,
    scope: Arc<CaptureScope>,
    stops: mpsc::Receiver<oneshot::Sender<Result<MotorCaptureReply, MotorOperationError>>>,
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("motor_capture_operation", status);
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
fn check_context<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    context: &MotorRequestContext,
) -> Result<(), MotorOperationError> {
    crate::motor_operations::authorize_context(app, window, context)
}

fn begin<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorCaptureStartRequest,
) -> Result<Work<R>, MotorOperationError> {
    crate::run_service::validate_test_id(&request.test_id)
        .map_err(crate::motor_operations::invalid_request)?;
    check_context(app, window, &request.context)?;
    let state = app.state::<AppState>();
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: request.context.request_id.clone(),
        owner: request.context.session.owner.clone(),
        target: request.context.session.target.clone(),
        resources: vec![OperationResource::Capture],
        budget_ms: CAPTURE_BUDGET_MS,
    })?;
    check_context(app, window, &request.context)?;
    let (stops, receiver) = mpsc::channel(1);
    let scope = Arc::new(CaptureScope {
        session: request.context.session.clone(),
        operation: lease.operation().clone(),
        test_id: request.test_id,
        path: Mutex::new(None),
        stops,
        receipt: Mutex::new(None),
    });
    let mut entries = state.motor_test.capture_operations.entries.lock();
    if entries.len() >= MAX_RETAINED {
        if let Some(expired) = entries
            .iter()
            .find(|(_, scope)| scope.receipt.lock().is_some())
            .map(|(id, _)| id.clone())
        {
            entries.remove(&expired);
        }
    }
    if entries.len() >= MAX_RETAINED {
        return Err(OperationError::CapacityExceeded.into());
    }
    entries.insert(scope.operation.id.clone(), scope.clone());
    *state.motor_test.capture_operations.current.lock() = Some(scope.operation.clone());
    Ok(Work {
        app: app.clone(),
        window: window.into(),
        context: request.context,
        lease: Some(lease),
        scope,
        stops: receiver,
    })
}

impl<R: Runtime> Work<R> {
    fn check(&self) -> Result<(), MotorOperationError> {
        self.lease
            .as_ref()
            .ok_or(OperationError::RegistryUnavailable)?
            .check()?;
        check_context(&self.app, &self.window, &self.context)
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
            Err(error)
                if matches!(
                    cleanup,
                    CleanupOutcome::NotRequired | CleanupOutcome::Confirmed
                ) =>
            {
                let status = self
                    .app
                    .state::<AppState>()
                    .runtime_clients
                    .operations
                    .confirm_cleanup(&self.scope.operation, cleanup)?;
                publish(&self.app, &status);
                if !matches!(error, OperationError::AlreadyFinished) {
                    return Err(error.into());
                }
            }
            Err(error) => return Err(error.into()),
        }
        if cleanup != CleanupOutcome::Uncertain {
            let state = self.app.state::<AppState>();
            let mut current = state.motor_test.capture_operations.current.lock();
            if current.as_ref() == Some(&self.scope.operation) {
                current.take();
            }
        }
        Ok(())
    }
    async fn stop_capture(&self) -> Result<SdCaptureStatus, MotorOperationError> {
        // Trusted actor cleanup keeps its captured target authority after the
        // renderer/session is gone; it never reselects a current target.
        crate::motor_operations::binding(&self.app, &self.scope.session)?;
        let state = self.app.state::<AppState>();
        let current = crate::capture_service::capture_status_blocking(&state);
        let expected_path = self.scope.path.lock().clone();
        if current.test_id.as_deref() != Some(&self.scope.test_id)
            || current.remote_path != expected_path
        {
            return Err(SdCaptureError::TargetChanged.into());
        }
        let _ops = state.ops().lock().await;
        crate::motor_operations::binding(&self.app, &self.scope.session)?;
        let current = crate::capture_service::capture_status_blocking(&state);
        if current.test_id.as_deref() != Some(&self.scope.test_id)
            || current.remote_path != expected_path
        {
            return Err(SdCaptureError::TargetChanged.into());
        }
        crate::capture_service::stop_capture_locked(&state)
            .await
            .map_err(crate::motor_operations::capture_error)
    }
    async fn retire(
        &mut self,
        cause: MotorOperationError,
    ) -> Result<MotorCaptureReply, MotorOperationError> {
        match self.stop_capture().await {
            Ok(status) if status.phase == CapturePhase::Complete => {
                let reply = MotorCaptureReply {
                    operation: self.scope.operation.clone(),
                    status,
                };
                *self.scope.receipt.lock() = Some(reply.clone());
                self.finish(OperationOutcome::Cancelled, CleanupOutcome::Confirmed)?;
                Ok(reply)
            }
            result => {
                let reason = match result {
                    Ok(status) => {
                        format!("{cause}; unexpected capture stop state: {:?}", status.phase)
                    }
                    Err(error) => format!("{cause}; capture cleanup: {error}"),
                };
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
    async fn run(mut self, reply: oneshot::Sender<Result<MotorCaptureReply, MotorOperationError>>) {
        let mut reply = Some(reply);
        if let Err(error) = self.check().and_then(|()| {
            self.lease
                .as_ref()
                .ok_or(OperationError::RegistryUnavailable)?
                .mark_running()
                .map_err(Into::into)
        }) {
            let state = self.app.state::<AppState>();
            state
                .motor_test
                .capture_operations
                .entries
                .lock()
                .remove(&self.scope.operation.id);
            let mut current = state.motor_test.capture_operations.current.lock();
            if current.as_ref() == Some(&self.scope.operation) {
                current.take();
            }
            if let Some(reply) = reply.take() {
                let _ = reply.send(Err(error));
            }
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
        let dispatched = Arc::new(AtomicBool::new(false));
        let dispatched_start = dispatched.clone();
        let start = async {
            let state = self.app.state::<AppState>();
            let _ops = state.ops().lock().await;
            self.check()?;
            crate::run_service::ensure_no_active_run(&state)
                .map_err(crate::motor_operations::invalid_request)?;
            dispatched_start.store(true, Ordering::Release);
            crate::capture_service::start_capture_locked(&state, self.scope.test_id.clone())
                .await
                .map_err(crate::motor_operations::capture_error)
        };
        let status = tokio::select! { biased;
            _ = cancellation.cancelled() => Err(OperationError::Cancelled.into()),
            result = start => result,
        };
        let status = match status {
            Ok(status) if status.phase == CapturePhase::Recording => status,
            Ok(status) => {
                let error = crate::motor_operations::invalid_request(format!(
                    "capture did not enter Recording: {:?}",
                    status.phase
                ));
                let retired = self.retire(error.clone()).await;
                if let Some(reply) = reply.take() {
                    let _ = reply.send(Err(retired.err().unwrap_or(error)));
                }
                return;
            }
            Err(error) => {
                let retired = if dispatched.load(Ordering::Acquire) {
                    self.retire(error.clone()).await.err()
                } else {
                    let outcome = if matches!(
                        &error,
                        MotorOperationError::Operation {
                            error: OperationError::Cancelled
                                | OperationError::NotOwner
                                | OperationError::TargetChanged
                        }
                    ) {
                        OperationOutcome::Cancelled
                    } else {
                        OperationOutcome::Failed {
                            message: error.to_string(),
                        }
                    };
                    let _ = self.finish(outcome, CleanupOutcome::NotRequired);
                    None
                };
                if let Some(reply) = reply.take() {
                    let _ = reply.send(Err(retired.unwrap_or(error)));
                }
                return;
            }
        };
        *self.scope.path.lock() = status.remote_path.clone();
        if let Err(error) = self.check() {
            let retired = self.retire(error.clone()).await;
            if let Some(reply) = reply.take() {
                let _ = reply.send(Err(retired.err().unwrap_or(error)));
            }
            return;
        }
        let started = MotorCaptureReply {
            operation: self.scope.operation.clone(),
            status,
        };
        if let Some(reply) = reply.take() {
            let _ = reply.send(Ok(started));
        }
        let mut tick = tokio::time::interval(POLL);
        loop {
            tokio::select! { biased;
                _ = cancellation.cancelled() => { let _ = self.retire(OperationError::Cancelled.into()).await; return; },
                stop = self.stops.recv() => if let Some(stop) = stop {
                    let result = self.retire(OperationError::Cancelled.into()).await;
                    let _ = stop.send(result); return;
                },
                _ = tick.tick() => if let Err(error) = self.check() { let _ = self.retire(error).await; return; },
            }
        }
    }
}

pub(crate) async fn start<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorCaptureStartRequest,
) -> Result<MotorCaptureReply, MotorOperationError> {
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
    request: &MotorCaptureStopRequest,
) -> Result<Arc<CaptureScope>, MotorOperationError> {
    request.validate()?;
    app.state::<AppState>()
        .runtime_clients
        .authorize(window, &request.session)?;
    let scope = app
        .state::<AppState>()
        .motor_test
        .capture_operations
        .entries
        .lock()
        .get(&request.operation.id)
        .cloned()
        .ok_or(OperationError::NotFound)?;
    if scope.session != request.session || scope.operation != request.operation {
        return Err(OperationError::NotOwner.into());
    }
    Ok(scope)
}

pub(crate) async fn stop<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorCaptureStopRequest,
) -> Result<MotorCaptureReply, MotorOperationError> {
    let scope = owned(app, window, &request)?;
    if let Some(receipt) = scope.receipt.lock().clone() {
        return Ok(receipt);
    }
    let (reply, response) = oneshot::channel();
    tokio::time::timeout(STOP_BUDGET, async {
        scope
            .stops
            .send(reply)
            .await
            .map_err(|_| MotorProtocolError::ServiceStopped)?;
        response
            .await
            .map_err(|_| MotorProtocolError::ServiceStopped)?
    })
    .await
    .map_err(|_| MotorProtocolError::TimedOut)?
}

pub(crate) async fn verify<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    context: MotorRequestContext,
) -> Result<SdCaptureStatus, MotorOperationError> {
    check_context(app, window, &context)?;
    let state = app.state::<AppState>();
    let status = crate::capture_service::verify_capture_locked(&state)
        .await
        .map_err(crate::motor_operations::capture_error)?;
    check_context(app, window, &context)?;
    Ok(status)
}

pub(crate) async fn stop_all<R: Runtime>(
    app: &AppHandle<R>,
    budget: Duration,
) -> Result<(), String> {
    stop_all_matching(app, budget, |_| true).await
}

/// Finalize only the capture owned by the closing Motor window. A replacement
/// window's recording must keep its own file/stop authority.
pub(crate) async fn stop_all_for_client<R: Runtime>(
    app: &AppHandle<R>,
    client_id: &ClientId,
    budget: Duration,
) -> Result<(), String> {
    stop_all_matching(app, budget, |scope| {
        &scope.session.owner.client_id == client_id
    })
    .await
}

async fn stop_all_matching<R: Runtime>(
    app: &AppHandle<R>,
    budget: Duration,
    matches: impl Fn(&CaptureScope) -> bool,
) -> Result<(), String> {
    let scopes = app
        .state::<AppState>()
        .motor_test
        .capture_operations
        .entries
        .lock()
        .values()
        .filter(|scope| matches(scope))
        .cloned()
        .collect::<Vec<_>>();
    for scope in &scopes {
        let status = retained(app, &scope.operation).map_err(|error| error.to_string())?;
        if status.phase != maggcs_core::runtime::operations::OperationPhase::Finished {
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
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .map_err(|_| "timed out stopping the owned SD capture".to_string())?;
    if scopes.iter().any(|scope| {
        !retained(app, &scope.operation)
            .is_ok_and(|status| status.cleanup != CleanupOutcome::Uncertain)
    }) {
        return Err("SD capture cleanup is unconfirmed; target link preserved".into());
    }
    Ok(())
}
