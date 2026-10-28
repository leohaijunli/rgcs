//! AF-02b.4a: owned native command work, starting with Parameters reboot.

use std::time::Duration;

use maggcs_core::commands::operation_types::{
    CommandOperationError, CommandReply, CommandRequestContext,
};
use maggcs_core::commands::{CommandError, CommandTransaction};
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::RuntimeError;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};

use crate::runtime_facade::{CommandBinding, RuntimeFacade, TargetRuntime};
use crate::state::AppState;

const REBOOT_BUDGET: Duration = Duration::from_secs(10);
const TARGET_POLL: Duration = Duration::from_millis(100);
const REBOOT_COMMAND: mavlink::common::MavCmd =
    mavlink::common::MavCmd::MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN;
const REBOOT_PARAMS: [f32; 7] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

pub(crate) struct CommandWork<R: Runtime> {
    app: AppHandle<R>,
    window: String,
    session: RuntimeClientSession,
    binding: CommandBinding,
    lease: OperationLease,
    transaction: CommandTransaction,
}

fn runtime_error(error: RuntimeError) -> CommandOperationError {
    OperationError::Runtime { error }.into()
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn reboot_admission<R: Runtime>(
    app: &AppHandle<R>,
    session: &RuntimeClientSession,
) -> Result<(), CommandOperationError> {
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
    let landed_fresh = snapshot
        .field_ages
        .extended_sys_state_at_ms
        .is_some_and(|at| {
            epoch_ms().saturating_sub(at) <= maggcs_core::runtime::TARGET_HEARTBEAT_FRESH_MS
        });
    if snapshot
        .heartbeat
        .as_ref()
        .is_none_or(|heartbeat| heartbeat.base_mode.safety_armed)
        || !landed_fresh
        || snapshot
            .extended_sys_state
            .as_ref()
            .is_none_or(|state| state.landed_state != maggcs_core::telemetry::LandedState::OnGround)
    {
        return Err(OperationError::InvalidRequest {
            message: "Reboot requires fresh telemetry confirming a disarmed, landed target".into(),
        }
        .into());
    }
    let state = app.state::<AppState>();
    crate::main_link_service::ensure_primary_tools_available(&state)
        .map_err(|message| OperationError::InvalidRequest { message })?;
    if state.logs.is_busy() {
        return Err(OperationError::InvalidRequest {
            message: "Finish or cancel the target's log transfer before rebooting".into(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn begin_reboot<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: CommandRequestContext,
) -> Result<CommandWork<R>, CommandOperationError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window, &request.session)?;
    let binding = RuntimeFacade::new(app)
        .command_binding(&request.session.target)
        .map_err(runtime_error)?;
    reboot_admission(app, &request.session)?;
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: request.request_id,
        owner: request.session.owner.clone(),
        target: request.session.target.clone(),
        // Reboot changes the whole FC context, including transfers in progress.
        resources: vec![
            OperationResource::Command,
            OperationResource::ParameterTransfer,
            OperationResource::ParameterWrite,
            OperationResource::MissionTransfer,
            OperationResource::Shell,
            OperationResource::LogTransfer,
            OperationResource::Capture,
        ],
        budget_ms: REBOOT_BUDGET.as_millis() as u64,
    })?;
    state.runtime_clients.authorize(window, &request.session)?;
    RuntimeFacade::new(app)
        .bind(&request.session.target)
        .map_err(runtime_error)?;
    let remaining = state
        .runtime_clients
        .operations
        .status(&request.session.owner, lease.operation())?
        .remaining_budget_ms;
    let transaction = CommandTransaction::new(
        lease.operation().id.clone(),
        request.session.target.generation,
        Duration::from_millis(remaining),
    )?
    .with_owner_cancellation(lease.cancellation());
    Ok(CommandWork {
        app: app.clone(),
        window: window.into(),
        session: request.session,
        binding,
        lease,
        transaction,
    })
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("command_operation", status);
}

fn retained_status<R: Runtime>(
    app: &AppHandle<R>,
    operation: &OperationRef,
) -> Result<OperationStatus, CommandOperationError> {
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
) -> Result<(), CommandOperationError> {
    let operation = lease.operation().clone();
    match lease.finish(outcome, cleanup) {
        Ok(status) => {
            publish(app, &status);
            Ok(())
        }
        Err(error) => {
            // Cancellation can win after the final reply check. Lease drop
            // retains the original outcome/claim rather than reporting success.
            if let Ok(status) = retained_status(app, &operation) {
                publish(app, &status);
                if status.cleanup == CleanupOutcome::Uncertain {
                    return Err(CommandOperationError::ReconciliationRequired {
                        operation: Box::new(operation),
                        reason: error.to_string(),
                    });
                }
            }
            Err(error.into())
        }
    }
}

impl<R: Runtime> CommandWork<R> {
    pub(crate) fn operation(&self) -> &OperationRef {
        self.lease.operation()
    }

    fn check(&self) -> Result<(), CommandOperationError> {
        self.lease.check()?;
        self.app
            .state::<AppState>()
            .runtime_clients
            .authorize(&self.window, &self.session)?;
        if self.binding.connection.generation() != self.session.target.generation.get() {
            return Err(CommandError::TargetChanged.into());
        }
        reboot_admission(&self.app, &self.session)
    }

    /// Spawned independently of the renderer reply so owner loss still retires
    /// local retries and records any possibly dispatched command.
    pub(crate) async fn run(self) -> Result<CommandReply, CommandOperationError> {
        if let Err(error) = self
            .check()
            .and_then(|_| self.lease.mark_running().map_err(Into::into))
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
        let service = self.binding.service.clone();
        let mut execution =
            Box::pin(service.send_owned(self.transaction.clone(), REBOOT_COMMAND, REBOOT_PARAMS));
        let cancellation = self.lease.cancellation();
        let mut tick = tokio::time::interval(TARGET_POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => break Err(self.lease.check().err().unwrap_or(OperationError::Cancelled).into()),
                result = &mut execution => break result.map_err(CommandOperationError::from),
                _ = tick.tick() => if let Err(error) = self.check() { break Err(error); },
            }
        };
        drop(execution);
        match result {
            Ok(result) => {
                let operation = self.operation().clone();
                if result.is_success() {
                    RuntimeFacade::new(&self.app)
                        .retire_target(&self.session.target)
                        .map_err(runtime_error)?;
                }
                let outcome = if result.is_success() {
                    OperationOutcome::Succeeded
                } else {
                    OperationOutcome::Failed {
                        message: format!(
                            "FC command verdict: {}",
                            crate::command_service::result_name(result)
                        ),
                    }
                };
                finish(&self.app, self.lease, outcome, CleanupOutcome::NotRequired)?;
                if result.is_success() {
                    let _ = self
                        .app
                        .state::<AppState>()
                        .runtime_clients
                        .operations
                        .invalidate_target(&self.session.target);
                }
                Ok(CommandReply { operation, result })
            }
            Err(error) => self.failed(error).await,
        }
    }

    async fn failed(
        self,
        error: CommandOperationError,
    ) -> Result<CommandReply, CommandOperationError> {
        let operation = self.operation().clone();
        let stopped = self.binding.service.cancel_owned(&self.transaction).await;
        if self.transaction.may_have_dispatched() || stopped.is_err() {
            let reason =
                format!("{error}; local retry retirement: {stopped:?}; FC outcome is unconfirmed");
            finish(
                &self.app,
                self.lease,
                OperationOutcome::Uncertain {
                    reason: reason.clone(),
                },
                CleanupOutcome::Uncertain,
            )?;
            self.app
                .state::<AppState>()
                .alerts()
                .raise(maggcs_core::alerts::AlertInput::new(
                    maggcs_core::alerts::AlertSource::System,
                    maggcs_core::alerts::Severity::Warn,
                    format!(
                        "Command operation {} requires reconciliation: {reason}",
                        operation.id
                    ),
                ));
            return Err(CommandOperationError::ReconciliationRequired {
                operation: Box::new(operation),
                reason,
            });
        }
        let cancelled = matches!(
            error,
            CommandOperationError::Operation {
                error: OperationError::Cancelled
                    | OperationError::NotOwner
                    | OperationError::TargetChanged
            } | CommandOperationError::Protocol {
                error: CommandError::Cancelled | CommandError::TargetChanged
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
) -> Result<OperationStatus, CommandOperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window, session)?;
    if operation.owner != session.owner || operation.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    let status = retained_status(app, operation)?;
    if !status.resources.contains(&OperationResource::Command)
        && !status
            .resources
            .contains(&OperationResource::EmergencyCommand)
    {
        return Err(OperationError::InvalidRequest {
            message: "The operation is not a command".into(),
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
) -> Result<OperationStatus, CommandOperationError> {
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
pub fn command_cancel(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, CommandOperationError> {
    cancel(&app, window.label(), &session, &operation)
}

#[tauri::command]
pub fn command_operation_status(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
    operation: OperationRef,
) -> Result<OperationStatus, CommandOperationError> {
    owned_status(&app, window.label(), &session, &operation)
}

#[cfg(all(test, unix))]
#[path = "command_operation_tests.rs"]
mod tests;
