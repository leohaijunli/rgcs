//! AF-02b.4b: typed flight intents over one owned command worker/target.

use std::time::Duration;

use maggcs_core::commands::operation_types::{
    CommandOperationError, FlightCommandError, FlightCommandIntent, FlightCommandKind,
    FlightCommandReply, FlightCommandRequest, FlightCommandStep, FlightCommandStepResult,
};
use maggcs_core::commands::{CommandError, CommandTransaction};
use maggcs_core::height::{Height, HeightDatum};
use maggcs_core::runtime::operations::{
    CleanupOutcome, OperationError, OperationLease, OperationOutcome, OperationRef,
    OperationRequest, OperationResource, OperationStatus, RuntimeClientSession,
};
use maggcs_core::runtime::RuntimeError;
use maggcs_core::safety::{
    arm_gate_with_ack, evaluate, evaluate_force_arm, ArmGate, ChecklistItemId, ForceArmOutcome,
    PlanState, PreflightRefusal, RefusalKind,
};
use mavlink::common::MavCmd;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};

use crate::runtime_facade::{CommandBinding, RuntimeFacade, TargetRuntime};
use crate::state::AppState;

const COMMAND_BUDGET: Duration = Duration::from_secs(10);
const TAKEOFF_BUDGET: Duration = Duration::from_secs(20);
const TARGET_POLL: Duration = Duration::from_millis(100);
const MIN_ALTITUDE_AGL_M: f64 = 1.0;
const MAX_ALTITUDE_AGL_M: f64 = 120.0;
/// PX4's multirotor velocity envelope: `MPC_XY_CRUISE` defaults to 5 m/s and
/// `MPC_XY_VEL_MAX` to 12 m/s, so a cruising-speed command outside this band
/// can only be a mistyped value.
const MIN_SPEED_M_S: f64 = 1.0;
const MAX_SPEED_M_S: f64 = 15.0;

#[derive(Clone, Copy)]
struct CommandSpec {
    step: FlightCommandStep,
    command: MavCmd,
    params: [f32; 7],
}

enum StepFailure {
    BeforeDispatch(FlightCommandError),
    Active(FlightCommandError, Box<CommandTransaction>),
}

pub(crate) struct FlightCommandWork<R: Runtime> {
    app: AppHandle<R>,
    window: String,
    session: RuntimeClientSession,
    binding: CommandBinding,
    lease: OperationLease,
    intent: FlightCommandIntent,
    kind: FlightCommandKind,
    emergency: bool,
}

fn runtime_error(error: RuntimeError) -> FlightCommandError {
    CommandOperationError::from(OperationError::Runtime { error }).into()
}

fn invalid(message: impl Into<String>) -> FlightCommandError {
    OperationError::InvalidRequest {
        message: message.into(),
    }
    .into()
}

fn intent_kind(intent: &FlightCommandIntent) -> FlightCommandKind {
    match intent {
        FlightCommandIntent::Takeoff { .. } => FlightCommandKind::Takeoff,
        FlightCommandIntent::Land { .. } => FlightCommandKind::Land,
        FlightCommandIntent::ReturnToLaunch => FlightCommandKind::ReturnToLaunch,
        FlightCommandIntent::SetMode { .. } => FlightCommandKind::SetMode,
        FlightCommandIntent::Arm { .. } => FlightCommandKind::Arm,
        FlightCommandIntent::Disarm => FlightCommandKind::Disarm,
        FlightCommandIntent::Goto { .. } => FlightCommandKind::Goto,
        FlightCommandIntent::ChangeAltitude { .. } => FlightCommandKind::ChangeAltitude,
        FlightCommandIntent::ChangeSpeed { .. } => FlightCommandKind::ChangeSpeed,
        FlightCommandIntent::SetHome { .. } => FlightCommandKind::SetHome,
    }
}

fn intent_is_emergency(intent: &FlightCommandIntent) -> bool {
    matches!(
        intent,
        FlightCommandIntent::Land { .. }
            | FlightCommandIntent::ReturnToLaunch
            | FlightCommandIntent::Arm { .. }
            | FlightCommandIntent::Disarm
    )
}

fn validate_coordinate(latitude_deg: f64, longitude_deg: f64) -> Result<(), FlightCommandError> {
    if !latitude_deg.is_finite()
        || !longitude_deg.is_finite()
        || !(-90.0..=90.0).contains(&latitude_deg)
        || !(-180.0..=180.0).contains(&longitude_deg)
    {
        return Err(invalid(
            "latitude/longitude are not finite WGS84 coordinates",
        ));
    }
    Ok(())
}

fn validate_height(height: Height) -> Result<f64, FlightCommandError> {
    if height.datum() != HeightDatum::AmslEgm96 || !height.meters().is_finite() {
        return Err(invalid("flight command height must be finite AMSL_EGM96"));
    }
    Ok(height.meters())
}

fn validate_altitude_agl(agl_m: f64) -> Result<(), FlightCommandError> {
    if !agl_m.is_finite() || !(MIN_ALTITUDE_AGL_M..=MAX_ALTITUDE_AGL_M).contains(&agl_m) {
        return Err(invalid(format!(
            "target altitude must be between {MIN_ALTITUDE_AGL_M:.0} and {MAX_ALTITUDE_AGL_M:.0} m AGL"
        )));
    }
    Ok(())
}

fn validate_speed(speed_m_s: f64) -> Result<(), FlightCommandError> {
    if !speed_m_s.is_finite() || !(MIN_SPEED_M_S..=MAX_SPEED_M_S).contains(&speed_m_s) {
        return Err(invalid(format!(
            "target speed must be between {MIN_SPEED_M_S:.0} and {MAX_SPEED_M_S:.0} m/s"
        )));
    }
    Ok(())
}

fn ensure_altitude_change_allowed(
    snapshot: &maggcs_core::telemetry::TelemetrySnapshot,
) -> Result<(), FlightCommandError> {
    let heartbeat = snapshot
        .heartbeat
        .as_ref()
        .ok_or_else(|| invalid("no heartbeat yet - cannot change altitude"))?;
    if !heartbeat.base_mode.safety_armed {
        return Err(invalid("the vehicle must be armed to change altitude"));
    }
    let main_mode = ((heartbeat.custom_mode >> 16) & 0xff) as u8;
    let sub_mode = ((heartbeat.custom_mode >> 24) & 0xff) as u8;
    if main_mode == maggcs_core::telemetry::px4_mode::main_mode::AUTO
        && sub_mode != maggcs_core::telemetry::px4_mode::auto_sub::LOITER
    {
        return Err(invalid(
            "altitude changes are blocked during autonomous flight; pause into Hold first",
        ));
    }
    Ok(())
}

/// PX4's `DO_CHANGE_SPEED` handler only stores a cruising speed for the next
/// setpoint, so it needs the same armed guard as the altitude command: an
/// unarmed vehicle resets the value on takeoff/loiter activation anyway.
fn ensure_speed_change_allowed(
    snapshot: &maggcs_core::telemetry::TelemetrySnapshot,
) -> Result<(), FlightCommandError> {
    let heartbeat = snapshot
        .heartbeat
        .as_ref()
        .ok_or_else(|| invalid("no heartbeat yet - cannot change speed"))?;
    if !heartbeat.base_mode.safety_armed {
        return Err(invalid("the vehicle must be armed to change speed"));
    }
    Ok(())
}

fn ground_amsl_at(
    state: &AppState,
    latitude_deg: f64,
    longitude_deg: f64,
) -> Result<Height, FlightCommandError> {
    use maggcs_core::terrain::ElevationSource;

    let dem = state
        .dem
        .lock()
        .as_ref()
        .map(|(_, dem)| dem.clone())
        .ok_or_else(|| invalid("no DTM is loaded - cannot convert AGL to AMSL"))?;
    let (dtm, _) = dem
        .sample_pair(maggcs_core::plan::GeoPoint::new(
            latitude_deg,
            longitude_deg,
        ))
        .map_err(|error| invalid(format!("DTM is unavailable under the vehicle: {error}")))?;
    maggcs_core::height::to_px4_amsl(dtm, latitude_deg, longitude_deg)
        .map_err(|error| invalid(format!("cannot convert DTM height to AMSL: {error}")))
}

fn validate_intent(intent: &FlightCommandIntent) -> Result<(), FlightCommandError> {
    match intent {
        FlightCommandIntent::Land {
            ground_amsl: Some(height),
        } => {
            validate_height(*height)?;
        }
        FlightCommandIntent::ChangeAltitude { agl_m } => validate_altitude_agl(*agl_m)?,
        FlightCommandIntent::ChangeSpeed { speed_m_s } => validate_speed(*speed_m_s)?,
        FlightCommandIntent::Goto {
            latitude_deg,
            longitude_deg,
        }
        | FlightCommandIntent::SetHome {
            latitude_deg,
            longitude_deg,
            ..
        } => validate_coordinate(*latitude_deg, *longitude_deg)?,
        _ => {}
    }
    if let FlightCommandIntent::SetHome {
        altitude: Some(height),
        ..
    } = intent
    {
        validate_height(*height)?;
    }
    Ok(())
}

pub(crate) fn begin<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: FlightCommandRequest,
) -> Result<FlightCommandWork<R>, FlightCommandError> {
    validate_intent(&request.intent)?;
    let state = app.state::<AppState>();
    state
        .runtime_clients
        .authorize(window, &request.context.session)?;
    let binding = RuntimeFacade::new(app)
        .command_binding(&request.context.session.target)
        .map_err(runtime_error)?;
    let emergency = intent_is_emergency(&request.intent);
    if !emergency && crate::main_link_service::is_attached(&state) {
        return Err(invalid(
            "detach the borrowed Motor Test target before a normal flight command",
        ));
    }
    let kind = intent_kind(&request.intent);
    let budget = if kind == FlightCommandKind::Takeoff {
        TAKEOFF_BUDGET
    } else {
        COMMAND_BUDGET
    };
    let lease = state.runtime_clients.operations.prepare(OperationRequest {
        request_id: request.context.request_id,
        owner: request.context.session.owner.clone(),
        target: request.context.session.target.clone(),
        resources: vec![if emergency {
            OperationResource::EmergencyCommand
        } else {
            OperationResource::Command
        }],
        budget_ms: budget.as_millis() as u64,
    })?;
    state
        .runtime_clients
        .authorize(window, &request.context.session)?;
    RuntimeFacade::new(app)
        .bind(&request.context.session.target)
        .map_err(runtime_error)?;
    Ok(FlightCommandWork {
        app: app.clone(),
        window: window.into(),
        session: request.context.session,
        binding,
        lease,
        intent: request.intent,
        kind,
        emergency,
    })
}

fn publish<R: Runtime>(app: &AppHandle<R>, status: &OperationStatus) {
    let _ = app.emit("flight_command_operation", status);
}

fn status<R: Runtime>(
    app: &AppHandle<R>,
    operation: &OperationRef,
) -> Result<OperationStatus, FlightCommandError> {
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
) -> Result<(), FlightCommandError> {
    let operation = lease.operation().clone();
    match lease.finish(outcome, cleanup) {
        Ok(result) => {
            publish(app, &result);
            Ok(())
        }
        Err(error) => {
            if let Ok(retained) = status(app, &operation) {
                publish(app, &retained);
                if retained.cleanup == CleanupOutcome::Uncertain {
                    return Err(CommandOperationError::ReconciliationRequired {
                        operation: Box::new(operation),
                        reason: error.to_string(),
                    }
                    .into());
                }
            }
            Err(CommandOperationError::from(error).into())
        }
    }
}

fn gate_refusal(
    state: &AppState,
    snapshot: &maggcs_core::telemetry::TelemetrySnapshot,
    plan_state: Option<PlanState>,
    acknowledged_warnings: &[ChecklistItemId],
) -> Result<(), PreflightRefusal> {
    let checklist = evaluate(
        snapshot,
        &crate::commands::app_plan_state(state, plan_state),
        &crate::commands::safety_config(state),
        epoch_ms(),
    );
    match arm_gate_with_ack(&checklist, acknowledged_warnings) {
        ArmGate::Allowed => Ok(()),
        ArmGate::RequiresAck { items } => Err(PreflightRefusal::new(
            RefusalKind::RequiresAck,
            &checklist,
            &items,
            "confirm the listed warnings before sending",
        )),
        ArmGate::Blocked { items } => Err(PreflightRefusal::new(
            RefusalKind::Blocked,
            &checklist,
            &items,
            "preflight blocks the command",
        )),
        ArmGate::NoLink => Err(crate::commands::refusal(
            RefusalKind::NoLink,
            "no link - cannot send a preflight-gated command",
        )),
    }
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

impl<R: Runtime> FlightCommandWork<R> {
    pub(crate) fn operation(&self) -> &OperationRef {
        self.lease.operation()
    }

    fn check(&self) -> Result<(), FlightCommandError> {
        self.lease.check()?;
        self.app
            .state::<AppState>()
            .runtime_clients
            .authorize(&self.window, &self.session)?;
        if self.binding.connection.generation() != self.session.target.generation.get() {
            return Err(CommandError::TargetChanged.into());
        }
        RuntimeFacade::new(&self.app)
            .bind(&self.session.target)
            .map_err(runtime_error)?;
        if !self.emergency && crate::main_link_service::is_attached(&self.app.state::<AppState>()) {
            return Err(invalid(
                "the target was attached to Motor Test before dispatch",
            ));
        }
        Ok(())
    }

    fn telemetry(&self) -> Result<maggcs_core::telemetry::TelemetrySnapshot, FlightCommandError> {
        RuntimeFacade::new(&self.app)
            .bind(&self.session.target)
            .map_err(runtime_error)?
            .telemetry_snapshot()
            .map_err(runtime_error)?
            .ok_or_else(|| invalid("target telemetry is unavailable"))
    }

    /// Recheck after an ACK wait or durable audit, immediately before an arm
    /// step. An earlier clean checklist cannot authorize a later arm.
    fn recheck_arm(&self) -> Result<(), FlightCommandError> {
        let state = self.app.state::<AppState>();
        let snapshot = self.telemetry()?;
        match &self.intent {
            FlightCommandIntent::Takeoff {
                plan_state,
                acknowledged_warnings,
            }
            | FlightCommandIntent::Arm {
                force: None,
                plan_state,
                acknowledged_warnings,
            } => gate_refusal(&state, &snapshot, *plan_state, acknowledged_warnings)
                .map_err(Into::into),
            FlightCommandIntent::Arm {
                force: Some(_),
                plan_state,
                ..
            } => {
                let checklist = evaluate(
                    &snapshot,
                    &crate::commands::app_plan_state(&state, *plan_state),
                    &crate::commands::safety_config(&state),
                    epoch_ms(),
                );
                match evaluate_force_arm(&snapshot, &checklist) {
                    ForceArmOutcome::Allowed => Ok(()),
                    ForceArmOutcome::NoLink => Err(crate::commands::refusal(
                        RefusalKind::NoLink,
                        "no link - a forced arm is not possible",
                    )
                    .into()),
                    ForceArmOutcome::AlreadyArmed => Err(crate::commands::refusal(
                        RefusalKind::AlreadyArmed,
                        "the vehicle is already armed",
                    )
                    .into()),
                    ForceArmOutcome::NotNeeded => Err(crate::commands::refusal(
                        RefusalKind::NotNeeded,
                        "the checklist is already clear - no force needed",
                    )
                    .into()),
                }
            }
            _ => Err(invalid(
                "the arm step does not belong to this flight intent",
            )),
        }
    }

    fn recheck_altitude_change(&self) -> Result<(), FlightCommandError> {
        match &self.intent {
            FlightCommandIntent::ChangeAltitude { .. } => {
                ensure_altitude_change_allowed(&self.telemetry()?)
            }
            _ => Err(invalid(
                "the altitude-change step does not belong to this flight intent",
            )),
        }
    }

    fn recheck_speed_change(&self) -> Result<(), FlightCommandError> {
        match &self.intent {
            FlightCommandIntent::ChangeSpeed { .. } => {
                ensure_speed_change_allowed(&self.telemetry()?)
            }
            _ => Err(invalid(
                "the speed-change step does not belong to this flight intent",
            )),
        }
    }

    async fn primary_spec(&self) -> Result<(CommandSpec, bool), FlightCommandError> {
        let state = self.app.state::<AppState>();
        match &self.intent {
            FlightCommandIntent::Takeoff {
                plan_state,
                acknowledged_warnings,
            } => {
                let snapshot = self.telemetry()?;
                let armed = snapshot
                    .heartbeat
                    .as_ref()
                    .is_some_and(|heartbeat| heartbeat.base_mode.safety_armed);
                if !armed {
                    gate_refusal(&state, &snapshot, *plan_state, acknowledged_warnings)?;
                }
                let fix = snapshot.global_position.ok_or_else(|| {
                    invalid("no position fix yet - cannot compute the takeoff target")
                })?;
                Ok((
                    CommandSpec {
                        step: FlightCommandStep::Takeoff,
                        command: MavCmd::MAV_CMD_NAV_TAKEOFF,
                        params: crate::commands::takeoff_params(
                            fix.latitude_deg,
                            fix.longitude_deg,
                            fix.altitude.meters(),
                        ),
                    },
                    !armed,
                ))
            }
            FlightCommandIntent::Land { ground_amsl } => {
                let fix = self.telemetry()?.global_position.ok_or_else(|| {
                    invalid("no position fix yet - cannot compute the landing target")
                })?;
                let ground = match ground_amsl {
                    Some(height) => validate_height(*height)?,
                    None => fix.altitude.meters(),
                };
                Ok((
                    CommandSpec {
                        step: FlightCommandStep::Land,
                        command: MavCmd::MAV_CMD_NAV_LAND,
                        params: crate::commands::land_params(
                            fix.latitude_deg,
                            fix.longitude_deg,
                            ground,
                        ),
                    },
                    false,
                ))
            }
            FlightCommandIntent::ReturnToLaunch => Ok((
                CommandSpec {
                    step: FlightCommandStep::ReturnToLaunch,
                    command: MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
                    params: [0.0; 7],
                },
                false,
            )),
            FlightCommandIntent::SetMode {
                main_mode,
                sub_mode,
            } => Ok((
                CommandSpec {
                    step: FlightCommandStep::SetMode,
                    command: MavCmd::MAV_CMD_DO_SET_MODE,
                    params: crate::commands::set_mode_params(*main_mode, *sub_mode),
                },
                false,
            )),
            FlightCommandIntent::Arm {
                force,
                plan_state,
                acknowledged_warnings,
            } => {
                let snapshot = self.telemetry()?;
                let checklist = evaluate(
                    &snapshot,
                    &crate::commands::app_plan_state(&state, *plan_state),
                    &crate::commands::safety_config(&state),
                    epoch_ms(),
                );
                if let Some(force) = force {
                    match evaluate_force_arm(&snapshot, &checklist) {
                        ForceArmOutcome::Allowed => {
                            let items = checklist
                                .items
                                .iter()
                                .filter(|item| {
                                    item.status != maggcs_core::safety::ChecklistStatus::Pass
                                })
                                .map(|item| item.id)
                                .collect();
                            state
                                .recorder()
                                .record_force_arm(
                                    &self.binding.connection,
                                    items,
                                    force.reason,
                                    force.note.clone(),
                                )
                                .await
                                .map_err(|message| {
                                    PreflightRefusal::new(
                                        RefusalKind::Blocked,
                                        &checklist,
                                        &[],
                                        &format!("forced arm not sent: {message}"),
                                    )
                                })?;
                        }
                        ForceArmOutcome::NoLink => {
                            return Err(crate::commands::refusal(
                                RefusalKind::NoLink,
                                "no link - a forced arm is not possible",
                            )
                            .into())
                        }
                        ForceArmOutcome::AlreadyArmed => {
                            return Err(crate::commands::refusal(
                                RefusalKind::AlreadyArmed,
                                "the vehicle is already armed",
                            )
                            .into())
                        }
                        ForceArmOutcome::NotNeeded => {
                            return Err(crate::commands::refusal(
                                RefusalKind::NotNeeded,
                                "the checklist is already clear - no force needed",
                            )
                            .into())
                        }
                    }
                } else {
                    match arm_gate_with_ack(&checklist, acknowledged_warnings) {
                        ArmGate::Allowed => {}
                        ArmGate::RequiresAck { items } => {
                            return Err(PreflightRefusal::new(
                                RefusalKind::RequiresAck,
                                &checklist,
                                &items,
                                "confirm the listed warnings before arming",
                            )
                            .into())
                        }
                        ArmGate::Blocked { items } => {
                            return Err(PreflightRefusal::new(
                                RefusalKind::Blocked,
                                &checklist,
                                &items,
                                "preflight blocks arming",
                            )
                            .into())
                        }
                        ArmGate::NoLink => {
                            return Err(crate::commands::refusal(
                                RefusalKind::NoLink,
                                "no link - cannot arm",
                            )
                            .into())
                        }
                    }
                }
                Ok((
                    CommandSpec {
                        step: FlightCommandStep::Arm,
                        command: MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
                        params: crate::commands::arm_disarm_params(true),
                    },
                    false,
                ))
            }
            FlightCommandIntent::Disarm => Ok((
                CommandSpec {
                    step: FlightCommandStep::Disarm,
                    command: MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
                    params: crate::commands::arm_disarm_params(false),
                },
                false,
            )),
            FlightCommandIntent::Goto {
                latitude_deg,
                longitude_deg,
            } => {
                let fix = self
                    .telemetry()?
                    .global_position
                    .ok_or_else(|| invalid("no position fix yet - cannot command a move"))?;
                let safety = state.config().safety;
                let max_distance = safety
                    .goto_max_distance_m
                    .unwrap_or(crate::commands::DEFAULT_GOTO_MAX_DISTANCE_M);
                let dem = state.dem.lock().as_ref().map(|(_, dem)| dem.clone());
                let terrain_m = if let (Some(_), Some(dem)) = (safety.goto_min_clearance_m, dem) {
                    use maggcs_core::terrain::ElevationSource;
                    dem.sample_pair(maggcs_core::plan::GeoPoint::new(
                        *latitude_deg,
                        *longitude_deg,
                    ))
                    .ok()
                    .map(|(dtm, _)| dtm.meters())
                } else {
                    None
                };
                crate::commands::goto_guard(
                    (fix.latitude_deg, fix.longitude_deg),
                    (*latitude_deg, *longitude_deg),
                    max_distance,
                    terrain_m,
                    fix.altitude.meters(),
                    safety.goto_min_clearance_m,
                )
                .map_err(invalid)?;
                Ok((
                    CommandSpec {
                        step: FlightCommandStep::Goto,
                        command: MavCmd::MAV_CMD_DO_REPOSITION,
                        params: crate::commands::reposition_params(
                            *latitude_deg,
                            *longitude_deg,
                            fix.altitude.meters(),
                        ),
                    },
                    false,
                ))
            }
            FlightCommandIntent::ChangeAltitude { agl_m } => {
                let snapshot = self.telemetry()?;
                ensure_altitude_change_allowed(&snapshot)?;
                let fix = snapshot.global_position.ok_or_else(|| {
                    invalid("no position fix yet - cannot compute the altitude target")
                })?;
                let ground = ground_amsl_at(&state, fix.latitude_deg, fix.longitude_deg)?;
                let target = Height::new(HeightDatum::AmslEgm96, ground.meters() + *agl_m);
                Ok((
                    CommandSpec {
                        step: FlightCommandStep::ChangeAltitude,
                        command: MavCmd::MAV_CMD_DO_CHANGE_ALTITUDE,
                        params: crate::commands::change_altitude_params(target.meters()),
                    },
                    false,
                ))
            }
            FlightCommandIntent::ChangeSpeed { speed_m_s } => {
                ensure_speed_change_allowed(&self.telemetry()?)?;
                Ok((
                    CommandSpec {
                        step: FlightCommandStep::ChangeSpeed,
                        command: MavCmd::MAV_CMD_DO_CHANGE_SPEED,
                        params: crate::commands::change_speed_params(*speed_m_s),
                    },
                    false,
                ))
            }
            FlightCommandIntent::SetHome {
                latitude_deg,
                longitude_deg,
                altitude,
            } => {
                let fix = self
                    .telemetry()?
                    .global_position
                    .ok_or_else(|| invalid("no position fix yet - cannot set home"))?;
                let altitude = match altitude {
                    Some(height) => validate_height(*height)?,
                    None => fix.altitude.meters(),
                };
                Ok((
                    CommandSpec {
                        step: FlightCommandStep::SetHome,
                        command: MavCmd::MAV_CMD_DO_SET_HOME,
                        params: crate::commands::set_home_params(
                            *latitude_deg,
                            *longitude_deg,
                            altitude,
                        ),
                    },
                    false,
                ))
            }
        }
    }

    fn next_transaction(&self) -> Result<CommandTransaction, FlightCommandError> {
        let remaining = self
            .app
            .state::<AppState>()
            .runtime_clients
            .operations
            .status(&self.session.owner, self.operation())?
            .remaining_budget_ms;
        Ok(CommandTransaction::new(
            self.operation().id.clone(),
            self.session.target.generation,
            Duration::from_millis(remaining),
        )?
        .with_owner_cancellation(self.lease.cancellation()))
    }

    async fn execute_step(
        &self,
        spec: CommandSpec,
    ) -> Result<(FlightCommandStepResult, CommandTransaction), StepFailure> {
        self.check().map_err(StepFailure::BeforeDispatch)?;
        if spec.step == FlightCommandStep::Arm {
            self.recheck_arm().map_err(StepFailure::BeforeDispatch)?;
        }
        if spec.step == FlightCommandStep::ChangeAltitude {
            self.recheck_altitude_change()
                .map_err(StepFailure::BeforeDispatch)?;
        }
        if spec.step == FlightCommandStep::ChangeSpeed {
            self.recheck_speed_change()
                .map_err(StepFailure::BeforeDispatch)?;
        }
        let transaction = self
            .next_transaction()
            .map_err(StepFailure::BeforeDispatch)?;
        let mut execution = Box::pin(self.binding.service.send_owned(
            transaction.clone(),
            spec.command,
            spec.params,
        ));
        let cancellation = self.lease.cancellation();
        let mut tick = tokio::time::interval(TARGET_POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => break Err(self.lease.check().err().unwrap_or(OperationError::Cancelled).into()),
                result = &mut execution => break result.map_err(FlightCommandError::from),
                _ = tick.tick() => if let Err(error) = self.check() { break Err(error); },
            }
        };
        drop(execution);
        match result {
            Ok(result) => Ok((
                FlightCommandStepResult {
                    step: spec.step,
                    result,
                },
                transaction,
            )),
            Err(error) => Err(StepFailure::Active(error, Box::new(transaction))),
        }
    }

    pub(crate) async fn run(self) -> Result<FlightCommandReply, FlightCommandError> {
        if let Err(error) = self.check() {
            let app = self.app.clone();
            let operation = self.operation().clone();
            drop(self);
            if let Ok(retained) = status(&app, &operation) {
                publish(&app, &retained);
            }
            return Err(error);
        }
        // Durable audit preparation may wait on disk. Owner loss, target loss
        // and the original operation deadline still bound this pre-write phase.
        let prepared = {
            let mut preparation = Box::pin(self.primary_spec());
            let cancellation = self.lease.cancellation();
            let mut tick = tokio::time::interval(TARGET_POLL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => break Err(self.lease.check().err().unwrap_or(OperationError::Cancelled).into()),
                    _ = tick.tick() => if let Err(error) = self.check() { break Err(error); },
                    result = &mut preparation => break result,
                }
            }
        };
        let (primary, arm_after_takeoff) = match prepared {
            Ok(spec) => spec,
            Err(error) => {
                finish(
                    &self.app,
                    self.lease,
                    OperationOutcome::Failed {
                        message: error.to_string(),
                    },
                    CleanupOutcome::NotRequired,
                )?;
                return Err(error);
            }
        };
        self.check()?;
        self.lease.mark_running()?;
        if self.emergency {
            let interrupted = self
                .app
                .state::<AppState>()
                .runtime_clients
                .operations
                .supersede_commands(self.operation())?;
            for operation in interrupted {
                publish(&self.app, &operation);
            }
        }
        if let Ok(running) = status(&self.app, self.operation()) {
            publish(&self.app, &running);
        }
        let (primary_result, _) = match self.execute_step(primary).await {
            Ok(result) => result,
            Err(StepFailure::BeforeDispatch(error)) => return self.failed_before_dispatch(error),
            Err(StepFailure::Active(error, transaction)) => {
                return self.failed(error, *transaction, false).await
            }
        };
        self.complete_steps(primary_result, arm_after_takeoff).await
    }

    async fn complete_steps(
        self,
        primary_result: FlightCommandStepResult,
        arm_after_takeoff: bool,
    ) -> Result<FlightCommandReply, FlightCommandError> {
        let mut steps = Vec::with_capacity(if arm_after_takeoff { 2 } else { 1 });
        let primary_accepted = primary_result.result.is_success();
        steps.push(primary_result);
        if !primary_accepted {
            let operation = self.operation().clone();
            let result = FlightCommandReply {
                operation,
                command: self.kind,
                steps,
            };
            finish(
                &self.app,
                self.lease,
                OperationOutcome::Failed {
                    message: "flight controller rejected the command".into(),
                },
                CleanupOutcome::NotRequired,
            )?;
            return Ok(result);
        }
        if arm_after_takeoff {
            if let Err(error) = self.check() {
                return self.partial_failure(error);
            }
            let armed = match self.telemetry() {
                Ok(snapshot) => snapshot,
                Err(error) => return self.partial_failure(error),
            }
            .heartbeat
            .as_ref()
            .is_some_and(|heartbeat| heartbeat.base_mode.safety_armed);
            if !armed {
                let arm = CommandSpec {
                    step: FlightCommandStep::Arm,
                    command: MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
                    params: crate::commands::arm_disarm_params(true),
                };
                let (arm_result, _) = match self.execute_step(arm).await {
                    Ok(result) => result,
                    Err(StepFailure::BeforeDispatch(error)) => return self.partial_failure(error),
                    Err(StepFailure::Active(error, transaction)) => {
                        return self.failed(error, *transaction, true).await
                    }
                };
                let accepted = arm_result.result.is_success();
                steps.push(arm_result);
                if !accepted {
                    let operation = self.operation().clone();
                    let result = FlightCommandReply {
                        operation,
                        command: self.kind,
                        steps,
                    };
                    finish(
                        &self.app,
                        self.lease,
                        OperationOutcome::Failed {
                            message: "flight controller rejected takeoff arming".into(),
                        },
                        CleanupOutcome::NotRequired,
                    )?;
                    return Ok(result);
                }
            }
        }
        let operation = self.operation().clone();
        finish(
            &self.app,
            self.lease,
            OperationOutcome::Succeeded,
            CleanupOutcome::NotRequired,
        )?;
        Ok(FlightCommandReply {
            operation,
            command: self.kind,
            steps,
        })
    }

    async fn failed(
        self,
        error: FlightCommandError,
        transaction: CommandTransaction,
        previous_step_accepted: bool,
    ) -> Result<FlightCommandReply, FlightCommandError> {
        let operation = self.operation().clone();
        let stopped = self.binding.service.cancel_owned(&transaction).await;
        if previous_step_accepted || transaction.may_have_dispatched() || stopped.is_err() {
            let reason =
                format!("{error}; local retry retirement: {stopped:?}; FC outcome is unconfirmed");
            // This claim owns the local protocol exchange, not a continuing
            // flight/motor output. Retirement releases it while the uncertain
            // outcome and same-MAV_CMD ACK quarantine remain authoritative.
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
                        "Flight command operation {} requires reconciliation: {reason}",
                        operation.id
                    ),
                ));
            return Err(CommandOperationError::ReconciliationRequired {
                operation: Box::new(operation),
                reason,
            }
            .into());
        }
        let cancelled = matches!(
            error,
            FlightCommandError::Command {
                error: CommandOperationError::Operation {
                    error: OperationError::Cancelled
                        | OperationError::Superseded
                        | OperationError::NotOwner
                        | OperationError::TargetChanged
                }
            } | FlightCommandError::Command {
                error: CommandOperationError::Protocol {
                    error: CommandError::Cancelled | CommandError::TargetChanged
                }
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

    fn failed_before_dispatch(
        self,
        error: FlightCommandError,
    ) -> Result<FlightCommandReply, FlightCommandError> {
        let cancelled = matches!(
            error,
            FlightCommandError::Command {
                error: CommandOperationError::Operation {
                    error: OperationError::Cancelled
                        | OperationError::Superseded
                        | OperationError::NotOwner
                        | OperationError::TargetChanged
                }
            } | FlightCommandError::Command {
                error: CommandOperationError::Protocol {
                    error: CommandError::Cancelled | CommandError::TargetChanged
                }
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

    fn partial_failure(
        self,
        error: FlightCommandError,
    ) -> Result<FlightCommandReply, FlightCommandError> {
        let operation = self.operation().clone();
        let reason = format!("{error}; a previous command step was accepted");
        finish(
            &self.app,
            self.lease,
            OperationOutcome::Uncertain {
                reason: reason.clone(),
            },
            CleanupOutcome::Confirmed,
        )?;
        Err(CommandOperationError::ReconciliationRequired {
            operation: Box::new(operation),
            reason,
        }
        .into())
    }
}

#[tauri::command]
pub async fn flight_command_execute(
    app: AppHandle,
    window: WebviewWindow,
    request: FlightCommandRequest,
) -> Result<FlightCommandReply, FlightCommandError> {
    let work = begin(&app, window.label(), request)?;
    tauri::async_runtime::spawn(work.run())
        .await
        .map_err(|_| OperationError::RegistryUnavailable)?
}

#[cfg(all(test, unix))]
#[path = "flight_command_operation_tests.rs"]
mod tests;
