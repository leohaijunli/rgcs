//! AF-02b.3: existing parameter protocol driven by an owned native operation.
//! No worker/cache/transport duplication and no global "cancel current service".

use std::time::Duration;

use maggcs_core::params::model::{ParamEntry, ParamType, ParamValue};
use maggcs_core::params::operation_types::{
    ParameterCommandError, ParameterRequestContext, ParameterSetEcho, ParameterSetStatus,
};
use maggcs_core::params::service::{
    validate_name, validate_value, ParamError, ParamService, ParamTransaction, SetResult,
    TRANSACTION_TIMEOUT,
};
use maggcs_core::params::session::PullReport;
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::RuntimeError;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::runtime_facade::{ParameterBinding, RuntimeFacade, TargetRuntime};
use crate::state::AppState;

const PULL_BUDGET: Duration = Duration::from_secs(60);
const TARGET_POLL: Duration = Duration::from_millis(100);
const READBACK_BUDGET: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub(crate) enum ParameterAction {
    Pull,
    Read {
        name: String,
    },
    Set {
        name: String,
        value: ParamValue,
        ptype: ParamType,
    },
}

pub(crate) enum ParameterResult {
    Pull(PullReport),
    Read(ParamEntry),
    Set(SetResult),
}

impl ParameterAction {
    fn validate(&self) -> Result<(), ParamError> {
        match self {
            Self::Pull => Ok(()),
            Self::Read { name } => validate_name(name),
            Self::Set { name, value, ptype } => {
                validate_name(name)?;
                validate_value(*value, *ptype)
            }
        }
    }

    fn budget(&self) -> Duration {
        match self {
            Self::Pull => PULL_BUDGET,
            _ => TRANSACTION_TIMEOUT,
        }
    }
    fn resource(&self) -> OperationResource {
        match self {
            Self::Set { .. } => OperationResource::ParameterWrite,
            _ => OperationResource::ParameterTransfer,
        }
    }

    async fn execute(
        self,
        service: ParamService,
        transaction: ParamTransaction,
    ) -> Result<ParameterResult, ParamError> {
        match self {
            Self::Pull => service
                .pull_owned(transaction)
                .await
                .map(ParameterResult::Pull),
            Self::Read { name } => service
                .read_owned(transaction, &name)
                .await
                .map(ParameterResult::Read),
            Self::Set { name, value, ptype } => service
                .set_owned(transaction, &name, value, ptype)
                .await
                .map(ParameterResult::Set),
        }
    }
}

pub(crate) struct ParameterWork<R: Runtime> {
    app: AppHandle<R>,
    window: String,
    session: RuntimeClientSession,
    lease: OperationLease,
    binding: ParameterBinding,
    transaction: ParamTransaction,
    action: ParameterAction,
}

fn runtime_error(error: RuntimeError) -> ParameterCommandError {
    OperationError::Runtime { error }.into()
}

pub(crate) fn begin<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    context: ParameterRequestContext,
    action: ParameterAction,
) -> Result<ParameterWork<R>, ParameterCommandError> {
    action.validate()?;
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, &context.session)?;
    let runtime = RuntimeFacade::new(app);
    let binding = runtime
        .parameter_binding(&context.session.target)
        .map_err(runtime_error)?;
    if matches!(action, ParameterAction::Set { .. }) {
        let running = state.motor_test.running.lock();
        if running.as_ref().is_some_and(|running| {
            running.target.connection_id == binding.connection.handle_id()
                && state
                    .motor_test
                    .session()
                    .is_some_and(|session| session.is_sending())
        }) {
            return Err(ParamError::Busy.into());
        }
    }
    let budget = action.budget();
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: context.request_id,
        owner: context.session.owner.clone(),
        target: context.session.target.clone(),
        resources: vec![action.resource()],
        budget_ms: budget.as_millis().min(u64::MAX as u128) as u64,
    })?;
    // Owner destruction can race between initial authorization and prepare.
    // The record exists before the second check; expiry or this failure revokes it.
    if let Err(error) = state.runtime_clients.authorize(window, &context.session) {
        let _ = state
            .runtime_clients
            .operations
            .cancel(&context.session.owner, lease.operation());
        return Err(error.into());
    }
    runtime
        .bind(&context.session.target)
        .map_err(runtime_error)?;
    let transaction = ParamTransaction::new(
        lease.operation().id.clone(),
        context.session.target.generation,
        budget,
    )?;
    Ok(ParameterWork {
        app: app.clone(),
        window: window.into(),
        session: context.session,
        lease,
        binding,
        transaction,
        action,
    })
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("params_operation", status);
}

fn status<R: Runtime>(
    app: &AppHandle<R>,
    operation: &OperationRef,
) -> Result<OperationStatus, ParameterCommandError> {
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
) -> Result<OperationStatus, ParameterCommandError> {
    let operation = lease.operation().clone();
    match lease.finish(outcome, cleanup) {
        Ok(result) => {
            publish(app, &result);
            Ok(result)
        }
        Err(error) => {
            // Cancellation may win just after the final source check. Lease
            // drop retains uncertainty; publish that terminal evidence too.
            if let Ok(status) = status(app, &operation) {
                publish(app, &status);
                if status.cleanup == CleanupOutcome::Uncertain {
                    return Err(ParameterCommandError::ReconciliationRequired {
                        operation: Box::new(operation),
                        reason: error.to_string(),
                    });
                }
            }
            Err(error.into())
        }
    }
}

impl<R: Runtime> ParameterWork<R> {
    pub(crate) fn operation(&self) -> &OperationRef {
        self.lease.operation()
    }

    fn check(&self) -> Result<(), ParameterCommandError> {
        self.lease.check()?;
        self.app
            .state::<AppState>()
            .runtime_clients
            .authorize(&self.window, &self.session)?;
        RuntimeFacade::new(&self.app)
            .bind(&self.session.target)
            .map_err(runtime_error)?;
        Ok(())
    }

    /// This future is spawned independently of the webview reply, so native
    /// owner expiry can stop/reconcile it even if the renderer vanishes.
    pub(crate) async fn run(self) -> Result<ParameterResult, ParameterCommandError> {
        if let Err(error) = self
            .check()
            .and_then(|_| self.lease.mark_running().map_err(Into::into))
        {
            let operation = self.operation().clone();
            let app = self.app.clone();
            drop(self);
            if let Ok(status) = status(&app, &operation) {
                publish(&app, &status);
            }
            return Err(error);
        }
        if let Ok(status) = status(&self.app, self.operation()) {
            publish(&self.app, &status);
        }
        let action = self.action.clone();
        let mut execution =
            Box::pin(action.execute(self.binding.service.clone(), self.transaction.clone()));
        let cancellation = self.lease.cancellation();
        let mut tick = tokio::time::interval(TARGET_POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result: Result<ParameterResult, ParameterCommandError> = loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => break Err(self.lease.check().err().unwrap_or(OperationError::Cancelled).into()),
                result = &mut execution => break result.map_err(Into::into),
                _ = tick.tick() => if let Err(error) = self.check() { break Err(error); },
            }
        };
        drop(execution);
        let result = result.and_then(|result| {
            self.check()?;
            if matches!(result, ParameterResult::Set(SetResult::Timeout)) {
                return Err(ParamError::Timeout.into());
            }
            Ok(result)
        });
        match result {
            Ok(result) => {
                let outcome = match &result {
                    ParameterResult::Set(SetResult::Clamped { .. }) => OperationOutcome::Failed {
                        message: "FC echo differs from the requested parameter value".into(),
                    },
                    ParameterResult::Pull(report) if !report.is_complete() => {
                        OperationOutcome::Failed {
                            message: format!(
                                "Parameter pull is incomplete: {} of {}",
                                report.received, report.total
                            ),
                        }
                    }
                    _ => OperationOutcome::Succeeded,
                };
                finish(&self.app, self.lease, outcome, CleanupOutcome::NotRequired)?;
                Ok(result)
            }
            Err(error) => self.failed(error).await,
        }
    }

    async fn failed(
        self,
        error: ParameterCommandError,
    ) -> Result<ParameterResult, ParameterCommandError> {
        let operation = self.operation().clone();
        let stopped = self.binding.service.cancel_owned(&self.transaction).await;
        let definitely_unsent = matches!(
            error,
            ParameterCommandError::Protocol {
                error: ParamError::Busy | ParamError::InvalidName | ParamError::OutOfRange
            }
        );
        let write_name = match &self.action {
            ParameterAction::Set { name, .. } if !definitely_unsent => Some(name.clone()),
            _ => None,
        };
        if let Some(name) = write_name {
            // Read through the original worker only after its exact cancellation
            // receipt. Never read/write whichever primary happens to be current.
            let reconciled = if stopped.is_ok()
                && RuntimeFacade::new(&self.app)
                    .bind(&self.session.target)
                    .is_ok()
            {
                match ParamTransaction::new(
                    operation.id.clone(),
                    self.session.target.generation,
                    READBACK_BUDGET,
                ) {
                    Ok(transaction) => self
                        .binding
                        .service
                        .read_owned(transaction, &name)
                        .await
                        .ok(),
                    Err(_) => None,
                }
            } else {
                None
            };
            let cleanup = if reconciled.is_some() {
                CleanupOutcome::Confirmed
            } else {
                CleanupOutcome::Uncertain
            };
            let reason = if let Some(entry) = reconciled {
                format!("{error}; {name} read-back observed {:?}", entry.value)
            } else {
                format!("{error}; {name} read-back is unavailable")
            };
            finish(
                &self.app,
                self.lease,
                OperationOutcome::Uncertain {
                    reason: reason.clone(),
                },
                cleanup,
            )?;
            if cleanup == CleanupOutcome::Uncertain {
                self.app
                    .state::<AppState>()
                    .alerts()
                    .raise(maggcs_core::alerts::AlertInput::new(
                        maggcs_core::alerts::AlertSource::System,
                        maggcs_core::alerts::Severity::Warn,
                        format!(
                            "Parameter operation {} requires reconciliation: {reason}",
                            operation.id
                        ),
                    ));
                return Err(ParameterCommandError::ReconciliationRequired {
                    operation: Box::new(operation),
                    reason,
                });
            }
        } else {
            let cancelled = matches!(
                error,
                ParameterCommandError::Operation {
                    error: OperationError::Cancelled
                        | OperationError::TargetChanged
                        | OperationError::NotOwner
                } | ParameterCommandError::Protocol {
                    error: ParamError::Cancelled | ParamError::TargetChanged
                }
            );
            let cleanup = if stopped.is_ok() || matches!(stopped, Err(ParamError::NotConnected)) {
                CleanupOutcome::Confirmed
            } else {
                CleanupOutcome::Failed
            };
            let outcome = if cancelled {
                OperationOutcome::Cancelled
            } else {
                OperationOutcome::Failed {
                    message: error.to_string(),
                }
            };
            finish(&self.app, self.lease, outcome, cleanup)?;
        }
        Err(error)
    }
}

pub(crate) fn cancel<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<OperationStatus, ParameterCommandError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    let result = state
        .runtime_clients
        .operations
        .cancel(&session.owner, operation)?;
    publish(app, &result);
    Ok(result)
}

pub(crate) fn owned_status<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    session: &RuntimeClientSession,
    operation: &OperationRef,
) -> Result<OperationStatus, ParameterCommandError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    status(app, operation)
}

pub(crate) fn set_echo(
    operation: OperationRef,
    result: SetResult,
) -> Result<ParameterSetEcho, ParameterCommandError> {
    match result {
        SetResult::Confirmed(actual) => Ok(ParameterSetEcho {
            operation,
            status: ParameterSetStatus::Confirmed,
            actual,
            requested: None,
        }),
        SetResult::Clamped { requested, actual } => Ok(ParameterSetEcho {
            operation,
            status: ParameterSetStatus::Clamped,
            actual,
            requested: Some(requested),
        }),
        SetResult::Timeout => Err(ParamError::Timeout.into()),
    }
}

#[cfg(all(test, unix))]
#[path = "params_operation_tests.rs"]
mod tests;
