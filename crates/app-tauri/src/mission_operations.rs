//! AF-02b.4: native-owned Mission work over the captured existing worker.

use std::collections::BTreeMap;
use std::time::Duration;

use maggcs_core::mission::operation_types::{
    MissionOperationError, MissionProtocolError, MissionReply, MissionRequestContext,
    MissionResult, MissionSetCurrentRequest, MissionUploadRequest,
};
use maggcs_core::mission::{MissionItem, MissionService, MissionTransaction};
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::{OperationId, RuntimeError};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};

use crate::runtime_facade::{MissionBinding, RuntimeFacade, TargetRuntime};
use crate::state::AppState;

const TRANSFER_BUDGET: Duration = Duration::from_secs(30);
const CONTROL_BUDGET: Duration = Duration::from_secs(10);
const TARGET_POLL: Duration = Duration::from_millis(100);
const MAX_ACTIVE_MISSION_OPERATIONS: usize = 32;

/// Bounded native event correlation only; no transport/cache/protocol ownership.
#[derive(Default)]
pub(crate) struct MissionState {
    active: Mutex<BTreeMap<OperationId, OperationRef>>,
}

impl MissionState {
    pub(crate) fn operation(&self, id: &OperationId) -> Option<OperationRef> {
        self.active.lock().get(id).cloned()
    }
    fn register(&self, operation: &OperationRef) -> Result<(), OperationError> {
        let mut active = self.active.lock();
        if active.len() >= MAX_ACTIVE_MISSION_OPERATIONS {
            return Err(OperationError::CapacityExceeded);
        }
        active.insert(operation.id.clone(), operation.clone());
        Ok(())
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
            .missions
            .active
            .lock()
            .remove(&self.id);
    }
}

pub(crate) enum MissionAction {
    Upload(Vec<MissionItem>),
    Download,
    Clear,
    SetCurrent(u16),
}

impl MissionAction {
    fn budget(&self) -> Duration {
        match self {
            Self::Upload(_) | Self::Download => TRANSFER_BUDGET,
            _ => CONTROL_BUDGET,
        }
    }
    async fn execute(
        self,
        service: MissionService,
        transaction: MissionTransaction,
    ) -> Result<MissionResult, MissionProtocolError> {
        match self {
            Self::Upload(items) => service
                .upload_owned(transaction, items)
                .await
                .map(|count| MissionResult::Uploaded { count }),
            Self::Download => service
                .download_owned(transaction)
                .await
                .map(|items| MissionResult::Downloaded { items }),
            Self::Clear => service
                .clear_owned(transaction)
                .await
                .map(|()| MissionResult::Cleared),
            Self::SetCurrent(seq) => service
                .set_current_owned(transaction, seq)
                .await
                .map(|seq| MissionResult::Current { seq }),
        }
    }
}

pub(crate) struct MissionWork<R: Runtime> {
    app: AppHandle<R>,
    window: String,
    session: RuntimeClientSession,
    lease: OperationLease,
    binding: MissionBinding,
    transaction: MissionTransaction,
    action: Option<MissionAction>,
    _registration: Registration<R>,
}

fn runtime_error(error: RuntimeError) -> MissionOperationError {
    OperationError::Runtime { error }.into()
}

fn admission(state: &AppState) -> Result<(), MissionOperationError> {
    crate::main_link_service::ensure_primary_tools_available(state)
        .map_err(|message| OperationError::InvalidRequest { message }.into())
}

pub(crate) fn begin<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    context: MissionRequestContext,
    action: MissionAction,
) -> Result<MissionWork<R>, MissionOperationError> {
    if let MissionAction::Upload(items) = &action {
        maggcs_core::mission::types::validate_upload(items).map_err(MissionProtocolError::from)?;
    }
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, &context.session)?;
    let runtime = RuntimeFacade::new(app);
    let binding = runtime
        .mission_binding(&context.session.target)
        .map_err(runtime_error)?;
    admission(&state)?;
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: context.request_id,
        owner: context.session.owner.clone(),
        target: context.session.target.clone(),
        resources: vec![OperationResource::MissionTransfer],
        budget_ms: action.budget().as_millis() as u64,
    })?;
    state.runtime_clients.authorize(window, &context.session)?;
    runtime
        .bind(&context.session.target)
        .map_err(runtime_error)?;
    let remaining = state
        .runtime_clients
        .operations
        .status(&context.session.owner, lease.operation())?
        .remaining_budget_ms;
    let transaction = MissionTransaction::new(
        lease.operation().id.clone(),
        context.session.target.generation,
        Duration::from_millis(remaining),
    )?
    .with_owner_cancellation(lease.cancellation());
    state.missions.register(lease.operation())?;
    let registration = Registration {
        app: app.clone(),
        id: lease.operation().id.clone(),
    };
    if let Ok(status) = retained_status(app, lease.operation()) {
        publish(app, &status);
    }
    Ok(MissionWork {
        app: app.clone(),
        window: window.into(),
        session: context.session,
        lease,
        binding,
        transaction,
        action: Some(action),
        _registration: registration,
    })
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("mission_operation", status);
}

pub(crate) fn retained_status<R: Runtime>(
    app: &AppHandle<R>,
    operation: &OperationRef,
) -> Result<OperationStatus, MissionOperationError> {
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
) -> Result<(), MissionOperationError> {
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
                    return Err(MissionOperationError::ReconciliationRequired {
                        operation: Box::new(operation),
                        reason: error.to_string(),
                    });
                }
            }
            Err(error.into())
        }
    }
}

impl<R: Runtime> MissionWork<R> {
    pub(crate) fn operation(&self) -> &OperationRef {
        self.lease.operation()
    }

    fn check(&self) -> Result<(), MissionOperationError> {
        self.lease.check()?;
        let state = self.app.state::<AppState>();
        state
            .runtime_clients
            .authorize(&self.window, &self.session)?;
        if self.binding.connection.generation() != self.session.target.generation.get() {
            return Err(MissionProtocolError::TargetChanged.into());
        }
        RuntimeFacade::new(&self.app)
            .bind(&self.session.target)
            .map_err(runtime_error)?;
        admission(&state)
    }

    pub(crate) async fn run(mut self) -> Result<MissionReply, MissionOperationError> {
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
        let mut execution =
            Box::pin(action.execute(self.binding.service.clone(), self.transaction.clone()));
        let cancellation = self.lease.cancellation();
        let mut tick = tokio::time::interval(TARGET_POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => break Err(self.lease.check().err().unwrap_or(OperationError::Cancelled).into()),
                result = &mut execution => break result.map_err(MissionOperationError::from),
                _ = tick.tick() => if let Err(error) = self.check() { break Err(error); },
            }
        };
        drop(execution);
        let result = result.and_then(|result| {
            self.check()?;
            Ok(result)
        });
        match result {
            Ok(result) => {
                let operation = self.operation().clone();
                finish(
                    &self.app,
                    self.lease,
                    OperationOutcome::Succeeded,
                    CleanupOutcome::NotRequired,
                )?;
                Ok(MissionReply { operation, result })
            }
            Err(error) => self.failed(error).await,
        }
    }

    async fn failed(
        self,
        error: MissionOperationError,
    ) -> Result<MissionReply, MissionOperationError> {
        let operation = self.operation().clone();
        let stopped = self.binding.service.cancel_owned(&self.transaction).await;
        let rejected = matches!(
            &error,
            MissionOperationError::Protocol {
                error: MissionProtocolError::AckDenied { .. }
            }
        );
        if (!rejected && self.transaction.may_have_dispatched()) || stopped.is_err() {
            let reason = format!(
                "{error}; local retry retirement: {stopped:?}; FC mission state requires read-back"
            );
            let cleanup = if stopped.is_ok() {
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
                        "Mission operation {} requires reconciliation: {reason}",
                        operation.id
                    ),
                ));
            return Err(MissionOperationError::ReconciliationRequired {
                operation: Box::new(operation),
                reason,
            });
        }
        let cancelled = matches!(
            &error,
            MissionOperationError::Operation {
                error: OperationError::Cancelled
                    | OperationError::NotOwner
                    | OperationError::TargetChanged
            } | MissionOperationError::Protocol {
                error: MissionProtocolError::Cancelled | MissionProtocolError::TargetChanged
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

pub(crate) fn owned_status<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<OperationStatus, MissionOperationError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    let status = retained_status(app, operation)?;
    if status.resources != [OperationResource::MissionTransfer] {
        return Err(OperationError::InvalidRequest {
            message: "the operation is not an owned Mission exchange".into(),
        }
        .into());
    }
    Ok(status)
}

pub(crate) fn cancel<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<OperationStatus, MissionOperationError> {
    owned_status(app, window, session, operation)?;
    let status = app
        .state::<AppState>()
        .runtime_clients
        .operations
        .cancel(&session.owner, operation)?;
    publish(app, &status);
    Ok(status)
}

async fn execute<R: Runtime>(work: MissionWork<R>) -> Result<MissionReply, MissionOperationError> {
    tauri::async_runtime::spawn(work.run())
        .await
        .map_err(|_| OperationError::RegistryUnavailable)?
}

#[tauri::command]
pub async fn mission_upload(
    app: AppHandle,
    window: WebviewWindow,
    request: MissionUploadRequest,
) -> Result<MissionReply, MissionOperationError> {
    execute(begin(
        &app,
        window.label(),
        request.context,
        MissionAction::Upload(request.items),
    )?)
    .await
}
#[tauri::command]
pub async fn mission_download(
    app: AppHandle,
    window: WebviewWindow,
    request: MissionRequestContext,
) -> Result<MissionReply, MissionOperationError> {
    execute(begin(
        &app,
        window.label(),
        request,
        MissionAction::Download,
    )?)
    .await
}
#[tauri::command]
pub async fn mission_clear(
    app: AppHandle,
    window: WebviewWindow,
    request: MissionRequestContext,
) -> Result<MissionReply, MissionOperationError> {
    execute(begin(&app, window.label(), request, MissionAction::Clear)?).await
}
#[tauri::command]
pub async fn mission_set_current(
    app: AppHandle,
    window: WebviewWindow,
    request: MissionSetCurrentRequest,
) -> Result<MissionReply, MissionOperationError> {
    execute(begin(
        &app,
        window.label(),
        request.context,
        MissionAction::SetCurrent(request.seq),
    )?)
    .await
}
#[tauri::command]
pub fn mission_cancel(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, MissionOperationError> {
    cancel(&app, window.label(), &session, &operation)
}
#[tauri::command]
pub fn mission_operation_status(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, MissionOperationError> {
    owned_status(&app, window.label(), &session, &operation)
}

#[cfg(all(test, unix))]
#[path = "mission_operation_tests.rs"]
mod tests;
