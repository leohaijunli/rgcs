//! Native AF-02b.4b flight-command ownership fixtures over production PTYs.

use super::*;
use crate::main_link_service::tests::{wait_until, Harness};
use maggcs_core::commands::CommandResult;
use maggcs_core::height::{Height, HeightDatum};
use maggcs_core::mavlink::MavMessage;
use maggcs_core::runtime::operations::RuntimeClientSession;
use maggcs_core::runtime::RequestId;
use maggcs_core::safety::{DrapeCheckFacts, ForceArmReason, ForceArmRequest, PlanState};
use std::sync::atomic::Ordering;

fn attach(h: &Harness, window: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(h.app.handle())
        .targets()
        .expect("targets")
        .remove(0)
        .target
        .expect("target");
    h.state()
        .runtime_clients
        .attach(window, "flight-controls".into(), target)
        .expect("client")
        .0
}

fn request(session: &RuntimeClientSession, intent: FlightCommandIntent) -> FlightCommandRequest {
    FlightCommandRequest {
        context: maggcs_core::commands::operation_types::CommandRequestContext {
            session: session.clone(),
            request_id: RequestId::fresh().expect("request"),
        },
        intent,
    }
}

fn commands(h: &Harness) -> Vec<mavlink::common::COMMAND_LONG_DATA> {
    h.trace
        .lock()
        .iter()
        .filter_map(|frame| match frame {
            MavMessage::COMMAND_LONG(command) => Some(command.clone()),
            _ => None,
        })
        .collect()
}

async fn wait_ready(h: &Harness) {
    wait_until(|| {
        h.state().hub().is_some_and(|hub| {
            let snapshot = hub.snapshot();
            snapshot.heartbeat.is_some()
                && snapshot.global_position.is_some()
                && snapshot.gps.is_some()
                && snapshot.sys_status.is_some()
                && snapshot.home.is_some()
                && snapshot.extended_sys_state.is_some()
        })
    })
    .await;
}

async fn execute(
    h: &Harness,
    session: &RuntimeClientSession,
    intent: FlightCommandIntent,
) -> FlightCommandReply {
    begin(h.app.handle(), "main", request(session, intent))
        .expect("begin")
        .run()
        .await
        .expect("terminal reply")
}

#[tokio::test(flavor = "multi_thread")]
async fn typed_single_step_intents_use_one_exact_target_and_return_terminal_verdicts() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let session = attach(&h, "main");
    let intents = [
        FlightCommandIntent::ReturnToLaunch,
        FlightCommandIntent::SetMode {
            main_mode: 4,
            sub_mode: 3,
        },
        FlightCommandIntent::Arm {
            force: None,
            plan_state: Some(PlanState::default()),
            acknowledged_warnings: Vec::new(),
        },
        FlightCommandIntent::Disarm,
        FlightCommandIntent::Land {
            ground_amsl: Some(Height::new(HeightDatum::AmslEgm96, 42.0)),
        },
        FlightCommandIntent::Goto {
            latitude_deg: 49.25,
            longitude_deg: -123.10,
        },
        FlightCommandIntent::SetHome {
            latitude_deg: 49.25,
            longitude_deg: -123.10,
            altitude: Some(Height::new(HeightDatum::AmslEgm96, 41.0)),
        },
    ];
    for intent in intents {
        let reply = execute(&h, &session, intent).await;
        assert_eq!(reply.operation.owner, session.owner);
        assert_eq!(reply.operation.target, session.target);
        assert_eq!(reply.steps.len(), 1);
        assert_eq!(reply.steps[0].result, CommandResult::Accepted);
    }
    let sent = commands(&h);
    assert_eq!(sent.len(), 7);
    assert_eq!(sent[0].command, MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH);
    assert_eq!(sent[1].command, MavCmd::MAV_CMD_DO_SET_MODE);
    assert_eq!((sent[1].param2, sent[1].param3), (4.0, 3.0));
    assert_eq!(sent[2].command, MavCmd::MAV_CMD_COMPONENT_ARM_DISARM);
    assert_eq!((sent[2].param1, sent[2].param2), (1.0, 0.0));
    assert_eq!(sent[3].command, MavCmd::MAV_CMD_COMPONENT_ARM_DISARM);
    assert_eq!((sent[3].param1, sent[3].param2), (0.0, 0.0));
    assert_eq!(sent[4].command, MavCmd::MAV_CMD_NAV_LAND);
    assert_eq!(sent[4].param7, 42.0);
    assert_eq!(sent[5].command, MavCmd::MAV_CMD_DO_REPOSITION);
    assert_eq!(sent[6].command, MavCmd::MAV_CMD_DO_SET_HOME);
    assert_eq!(sent[6].param7, 41.0);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_caller_invalid_height_and_reopened_generation_send_zero_effects() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    assert!(matches!(
        begin(
            h.app.handle(),
            "a",
            request(&b, FlightCommandIntent::ReturnToLaunch)
        ),
        Err(FlightCommandError::Command {
            error: CommandOperationError::Operation {
                error: OperationError::NotOwner
            }
        })
    ));
    assert!(begin(
        h.app.handle(),
        "a",
        request(
            &a,
            FlightCommandIntent::SetHome {
                latitude_deg: 49.25,
                longitude_deg: -123.10,
                altitude: Some(Height::new(HeightDatum::EllipsoidWgs84, 41.0)),
            }
        )
    )
    .is_err());
    let work = begin(
        h.app.handle(),
        "a",
        request(
            &a,
            FlightCommandIntent::Goto {
                latitude_deg: 49.25,
                longitude_deg: -123.10,
            },
        ),
    )
    .expect("prepared");
    h.handle.reconnect().await.expect("reopen");
    wait_until(|| h.handle.generation() > a.target.generation.get()).await;
    assert!(work.run().await.is_err());
    assert!(commands(&h).is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_takeoff_never_sends_the_followup_arm() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    *h.command_peer.as_ref().expect("peer").result.lock() =
        mavlink::common::MavResult::MAV_RESULT_DENIED;
    let session = attach(&h, "main");
    let reply = execute(
        &h,
        &session,
        FlightCommandIntent::Takeoff {
            plan_state: Some(PlanState::default()),
            acknowledged_warnings: Vec::new(),
        },
    )
    .await;
    assert_eq!(reply.steps.len(), 1);
    assert_eq!(reply.steps[0].step, FlightCommandStep::Takeoff);
    assert_eq!(reply.steps[0].result, CommandResult::Denied);
    let sent = commands(&h);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].command, MavCmd::MAV_CMD_NAV_TAKEOFF);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_takeoff_arms_in_the_same_owned_operation() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let session = attach(&h, "main");
    let reply = execute(
        &h,
        &session,
        FlightCommandIntent::Takeoff {
            plan_state: Some(PlanState::default()),
            acknowledged_warnings: Vec::new(),
        },
    )
    .await;
    assert_eq!(
        reply
            .steps
            .iter()
            .map(|step| (step.step, step.result))
            .collect::<Vec<_>>(),
        vec![
            (FlightCommandStep::Takeoff, CommandResult::Accepted),
            (FlightCommandStep::Arm, CommandResult::Accepted),
        ]
    );
    let sent = commands(&h);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].command, MavCmd::MAV_CMD_NAV_TAKEOFF);
    assert_eq!(sent[1].command, MavCmd::MAV_CMD_COMPONENT_ARM_DISARM);
    assert_eq!(sent[1].param1, 1.0);
    assert_eq!(sent[1].param2, 0.0, "never use PX4's force-arm magic code");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forced_arm_audit_failure_happens_before_any_command_write() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let session = attach(&h, "main");
    let result = begin(
        h.app.handle(),
        "main",
        request(
            &session,
            FlightCommandIntent::Arm {
                force: Some(ForceArmRequest {
                    reason: ForceArmReason::SafetyOverride,
                    note: Some("isolated fixture".into()),
                }),
                plan_state: Some(PlanState {
                    drape: Some(DrapeCheckFacts {
                        dem_covers_route: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                acknowledged_warnings: Vec::new(),
            },
        ),
    )
    .expect("prepared")
    .run()
    .await;
    assert!(matches!(result, Err(FlightCommandError::Preflight { .. })));
    assert!(commands(&h).is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn emergency_rtl_preempts_an_owned_normal_command_across_clients() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    h.command_peer
        .as_ref()
        .expect("peer")
        .hold_ack
        .store(true, Ordering::Release);
    let normal = begin(
        h.app.handle(),
        "a",
        request(
            &a,
            FlightCommandIntent::Goto {
                latitude_deg: 49.25,
                longitude_deg: -123.10,
            },
        ),
    )
    .expect("normal");
    let normal_task = tokio::spawn(normal.run());
    wait_until(|| {
        commands(&h)
            .iter()
            .any(|command| command.command == MavCmd::MAV_CMD_DO_REPOSITION)
    })
    .await;
    let emergency = begin(
        h.app.handle(),
        "b",
        request(&b, FlightCommandIntent::ReturnToLaunch),
    )
    .expect("emergency");
    let emergency_task = tokio::spawn(emergency.run());
    wait_until(|| {
        commands(&h)
            .iter()
            .any(|command| command.command == MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH)
    })
    .await;
    assert!(matches!(
        normal_task.await.expect("normal driver"),
        Err(FlightCommandError::Command {
            error: CommandOperationError::ReconciliationRequired { .. }
        })
    ));
    h.command_peer
        .as_ref()
        .expect("peer")
        .hold_ack
        .store(false, Ordering::Release);
    let reply = emergency_task
        .await
        .expect("emergency driver")
        .expect("emergency ACK");
    assert_eq!(reply.steps[0].result, CommandResult::Accepted);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn borrowed_motor_target_blocks_normal_command_but_not_rtl() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let session = attach(&h, "main");
    h.attach().await;
    assert!(begin(
        h.app.handle(),
        "main",
        request(
            &session,
            FlightCommandIntent::SetMode {
                main_mode: 4,
                sub_mode: 3,
            },
        ),
    )
    .is_err());
    let reply = execute(&h, &session, FlightCommandIntent::ReturnToLaunch).await;
    assert_eq!(reply.steps[0].result, CommandResult::Accepted);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn blocked_arm_and_takeoff_send_zero_commands() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    h.command_peer
        .as_ref()
        .expect("peer")
        .airborne
        .store(true, Ordering::Release);
    wait_until(|| {
        h.state()
            .hub()
            .expect("hub")
            .snapshot()
            .extended_sys_state
            .is_some_and(|state| state.landed_state == maggcs_core::telemetry::LandedState::InAir)
    })
    .await;
    let session = attach(&h, "main");
    for intent in [
        FlightCommandIntent::Arm {
            force: None,
            plan_state: Some(PlanState::default()),
            acknowledged_warnings: Vec::new(),
        },
        FlightCommandIntent::Takeoff {
            plan_state: Some(PlanState::default()),
            acknowledged_warnings: Vec::new(),
        },
    ] {
        let result = begin(h.app.handle(), "main", request(&session, intent))
            .expect("prepare")
            .run()
            .await;
        assert!(
            matches!(result, Err(FlightCommandError::Preflight { .. })),
            "{result:?}"
        );
    }
    assert!(commands(&h).is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn preflight_is_rechecked_after_takeoff_ack_before_followup_arm() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let peer = h.command_peer.as_ref().expect("peer");
    peer.hold_ack.store(true, Ordering::Release);
    let session = attach(&h, "main");
    let work = begin(
        h.app.handle(),
        "main",
        request(
            &session,
            FlightCommandIntent::Takeoff {
                plan_state: Some(PlanState::default()),
                acknowledged_warnings: Vec::new(),
            },
        ),
    )
    .expect("prepare takeoff");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| !commands(&h).is_empty()).await;
    peer.airborne.store(true, Ordering::Release);
    wait_until(|| {
        h.state()
            .hub()
            .expect("hub")
            .snapshot()
            .extended_sys_state
            .is_some_and(|state| state.landed_state == maggcs_core::telemetry::LandedState::InAir)
    })
    .await;
    peer.hold_ack.store(false, Ordering::Release);
    assert!(matches!(
        task.await.expect("driver"),
        Err(FlightCommandError::Command {
            error: CommandOperationError::ReconciliationRequired { .. }
        })
    ));
    let sent = commands(&h);
    assert_eq!(
        sent.len(),
        1,
        "newly blocked preflight must prevent the arm"
    );
    assert_eq!(sent[0].command, MavCmd::MAV_CMD_NAV_TAKEOFF);
    let retained = status(h.app.handle(), &operation).expect("retained operation");
    assert!(matches!(
        retained.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert_eq!(retained.cleanup, CleanupOutcome::Confirmed);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rtl_between_takeoff_steps_prevents_later_arm() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    let work = begin(
        h.app.handle(),
        "a",
        request(
            &a,
            FlightCommandIntent::Takeoff {
                plan_state: Some(PlanState::default()),
                acknowledged_warnings: Vec::new(),
            },
        ),
    )
    .expect("takeoff");
    let operation = work.operation().clone();
    let (primary, arm_after_takeoff) = work.primary_spec().await.expect("preflight");
    work.lease.mark_running().expect("running");
    let (accepted, _) = work.execute_step(primary).await.ok().expect("takeoff ACK");
    assert_eq!(accepted.result, CommandResult::Accepted);
    begin(
        h.app.handle(),
        "b",
        request(&b, FlightCommandIntent::ReturnToLaunch),
    )
    .expect("RTL")
    .run()
    .await
    .expect("RTL ACK");
    assert!(matches!(
        work.complete_steps(accepted, arm_after_takeoff).await,
        Err(FlightCommandError::Command {
            error: CommandOperationError::ReconciliationRequired { .. }
        })
    ));
    let sent = commands(&h);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].command, MavCmd::MAV_CMD_NAV_TAKEOFF);
    assert_eq!(sent[1].command, MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH);
    assert_eq!(
        status(h.app.handle(), &operation)
            .expect("status")
            .stop_reason,
        Some(maggcs_core::runtime::operations::OperationStopReason::Superseded)
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_cancel_and_idle_native_close_leave_b_flight_and_primary_cache_alive() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    let params = h.state().primary_params().expect("parameters");
    let cached = params.read("MR_SHARED").await.expect("initial read");
    h.command_peer
        .as_ref()
        .expect("peer")
        .hold_ack
        .store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "b",
        request(
            &b,
            FlightCommandIntent::Goto {
                latitude_deg: 49.25,
                longitude_deg: -123.10,
            },
        ),
    )
    .expect("B command");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| !commands(&h).is_empty()).await;
    assert!(matches!(
        crate::command_operations::cancel(h.app.handle(), "a", &a, &operation),
        Err(CommandOperationError::Operation {
            error: OperationError::NotOwner
        })
    ));
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("native close A");
    assert!(!h.handle.is_shutdown_requested());
    assert_eq!(params.cache().get("MR_SHARED"), Some(cached));
    h.command_peer
        .as_ref()
        .expect("peer")
        .hold_ack
        .store(false, Ordering::Release);
    assert_eq!(
        task.await.expect("B driver").expect("B ACK").steps[0].result,
        CommandResult::Accepted
    );
    assert_eq!(commands(&h).len(), 1);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_cancel_retires_retries_and_old_cancel_cannot_stop_later_work() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let session = attach(&h, "main");
    let peer = h.command_peer.as_ref().expect("peer");
    peer.hold_ack.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "main",
        request(
            &session,
            FlightCommandIntent::Goto {
                latitude_deg: 49.25,
                longitude_deg: -123.10,
            },
        ),
    )
    .expect("first command");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| !commands(&h).is_empty()).await;
    crate::command_operations::cancel(h.app.handle(), "main", &session, &operation)
        .expect("owner cancel");
    let result = tokio::time::timeout(maggcs_core::commands::OWNED_CANCEL_TIMEOUT, task)
        .await
        .expect("bounded retirement")
        .expect("driver");
    assert!(matches!(
        result,
        Err(FlightCommandError::Command {
            error: CommandOperationError::ReconciliationRequired { .. }
        })
    ));
    let retained = status(h.app.handle(), &operation).expect("status");
    assert!(matches!(
        retained.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert_eq!(retained.cleanup, CleanupOutcome::Confirmed);
    let retired_count = commands(&h).len();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        commands(&h).len(),
        retired_count,
        "cancelled retries do not resume"
    );
    let later = begin(
        h.app.handle(),
        "main",
        request(
            &session,
            FlightCommandIntent::SetMode {
                main_mode: 4,
                sub_mode: 3,
            },
        ),
    )
    .expect("later operation");
    let later_task = tokio::spawn(later.run());
    wait_until(|| commands(&h).len() > retired_count).await;
    crate::command_operations::cancel(h.app.handle(), "main", &session, &operation)
        .expect("old cancel is idempotent");
    peer.hold_ack.store(false, Ordering::Release);
    assert_eq!(
        later_task
            .await
            .expect("later driver")
            .expect("later ACK")
            .steps[0]
            .result,
        CommandResult::Accepted
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn native_close_retires_emergency_owner_without_expiring_a_reopened_window() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let old = attach(&h, "main");
    let peer = h.command_peer.as_ref().expect("peer");
    peer.hold_ack.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "main",
        request(&old, FlightCommandIntent::ReturnToLaunch),
    )
    .expect("RTL");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| !commands(&h).is_empty()).await;
    h.state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("native close");
    let result = tokio::time::timeout(maggcs_core::commands::OWNED_CANCEL_TIMEOUT, task)
        .await
        .expect("bounded close retirement")
        .expect("driver");
    assert!(matches!(
        result,
        Err(FlightCommandError::Command {
            error: CommandOperationError::ReconciliationRequired { .. }
        })
    ));
    let retained = status(h.app.handle(), &operation).expect("retained");
    assert_eq!(retained.resources, [OperationResource::EmergencyCommand]);
    assert_eq!(retained.cleanup, CleanupOutcome::Confirmed);
    assert!(!h.handle.is_shutdown_requested());
    let reopened = attach(&h, "main");
    assert_ne!(old.owner, reopened.owner);
    h.state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("late close callback");
    peer.hold_ack.store(false, Ordering::Release);
    execute(
        &h,
        &reopened,
        FlightCommandIntent::SetMode {
            main_mode: 4,
            sub_mode: 3,
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        commands(&h).len(),
        2,
        "closed RTL does not retry against the reopened window"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_owner_loss_and_duplicate_request_never_replay_intent() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let old = attach(&h, "main");
    let prepared = begin(
        h.app.handle(),
        "main",
        request(&old, FlightCommandIntent::ReturnToLaunch),
    )
    .expect("prepared");
    h.state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("close before dispatch");
    assert!(prepared.run().await.is_err());
    assert!(commands(&h).is_empty());
    let session = attach(&h, "main");
    let original = request(&session, FlightCommandIntent::ReturnToLaunch);
    let work = begin(h.app.handle(), "main", original.clone()).expect("original");
    assert!(matches!(
        begin(h.app.handle(), "main", original.clone()),
        Err(FlightCommandError::Command {
            error: CommandOperationError::Operation {
                error: OperationError::DuplicateRequest { .. }
            }
        })
    ));
    work.run().await.expect("original ACK");
    assert!(begin(h.app.handle(), "main", original).is_err());
    assert_eq!(commands(&h).len(), 1);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_ack_retains_quarantine_but_releases_the_local_flight_protocol_claim() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let session = attach(&h, "main");
    let peer = h.command_peer.as_ref().expect("peer");
    peer.hold_ack.store(true, Ordering::Release);
    let intent = FlightCommandIntent::Goto {
        latitude_deg: 49.25,
        longitude_deg: -123.10,
    };
    let work = begin(h.app.handle(), "main", request(&session, intent.clone())).expect("goto");
    let operation = work.operation().clone();
    assert!(matches!(
        work.run().await,
        Err(FlightCommandError::Command {
            error: CommandOperationError::ReconciliationRequired { .. }
        })
    ));
    let retained = status(h.app.handle(), &operation).expect("retained");
    assert!(matches!(
        retained.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert_eq!(retained.cleanup, CleanupOutcome::Confirmed);
    let writes = commands(&h).len();
    let repeated =
        begin(h.app.handle(), "main", request(&session, intent)).expect("local claim released");
    assert!(matches!(
        repeated.run().await,
        Err(FlightCommandError::Command {
            error: CommandOperationError::Protocol {
                error: CommandError::AmbiguousAck
            }
        })
    ));
    assert_eq!(
        commands(&h).len(),
        writes,
        "ambiguous normal intent is not written"
    );
    peer.hold_ack.store(false, Ordering::Release);
    assert_eq!(
        execute(&h, &session, FlightCommandIntent::ReturnToLaunch)
            .await
            .steps[0]
            .result,
        CommandResult::Accepted
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_reopen_retires_dispatched_work_and_never_retries_on_new_generation() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let old = attach(&h, "main");
    let peer = h.command_peer.as_ref().expect("peer");
    peer.hold_ack.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "main",
        request(
            &old,
            FlightCommandIntent::Goto {
                latitude_deg: 49.25,
                longitude_deg: -123.10,
            },
        ),
    )
    .expect("old goto");
    let task = tokio::spawn(work.run());
    wait_until(|| !commands(&h).is_empty()).await;
    h.handle.reconnect().await.expect("reopen");
    assert!(matches!(
        task.await.expect("driver"),
        Err(FlightCommandError::Command {
            error: CommandOperationError::ReconciliationRequired { .. }
        })
    ));
    wait_until(|| {
        RuntimeFacade::new(h.app.handle())
            .targets()
            .expect("targets")
            .first()
            .is_some_and(|target| {
                target
                    .target
                    .as_ref()
                    .is_some_and(|target| target.generation > old.target.generation)
            })
    })
    .await;
    let new = attach(&h, "main");
    assert_ne!(new.target, old.target);
    peer.hold_ack.store(false, Ordering::Release);
    execute(
        &h,
        &new,
        FlightCommandIntent::SetMode {
            main_mode: 4,
            sub_mode: 3,
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        commands(&h)
            .iter()
            .filter(|command| command.command == MavCmd::MAV_CMD_DO_REPOSITION)
            .count(),
        1
    );
    h.shutdown().await;
}

/// `DO_CHANGE_ALTITUDE` / `DO_CHANGE_SPEED` are the only intents that depend on
/// the armed state and on a loaded DTM; every refusal must happen before a
/// `COMMAND_LONG` leaves the app.
#[tokio::test(flavor = "multi_thread")]
async fn speed_and_altitude_changes_guard_the_envelope_arm_state_and_dtm() {
    let h = Harness::commands().await;
    wait_ready(&h).await;
    let session = attach(&h, "main");

    // Disarmed: prepared, then refused by the re-check before any write.
    for intent in [
        FlightCommandIntent::ChangeSpeed { speed_m_s: 6.0 },
        FlightCommandIntent::ChangeAltitude { agl_m: 20.0 },
    ] {
        let error = begin(h.app.handle(), "main", request(&session, intent))
            .expect("prepared")
            .run()
            .await
            .expect_err("disarmed");
        assert!(format!("{error}").contains("must be armed"), "got {error}");
    }
    assert!(commands(&h).is_empty());

    // Outside the envelope the request never reaches the FC at all.
    for speed_m_s in [0.0, 0.5, 15.5, 200.0, f64::NAN] {
        assert!(begin(
            h.app.handle(),
            "main",
            request(&session, FlightCommandIntent::ChangeSpeed { speed_m_s })
        )
        .is_err());
    }
    for agl_m in [0.0, 0.5, 121.0, f64::INFINITY] {
        assert!(begin(
            h.app.handle(),
            "main",
            request(&session, FlightCommandIntent::ChangeAltitude { agl_m })
        )
        .is_err());
    }
    assert!(commands(&h).is_empty());

    // Armed: the cruising speed reaches the FC in PX4's `param2` slot.
    h.command_peer
        .as_ref()
        .expect("peer")
        .armed
        .store(true, Ordering::Release);
    wait_until(|| {
        h.state().hub().is_some_and(|hub| {
            hub.snapshot()
                .heartbeat
                .as_ref()
                .is_some_and(|heartbeat| heartbeat.base_mode.safety_armed)
        })
    })
    .await;
    let reply = execute(
        &h,
        &session,
        FlightCommandIntent::ChangeSpeed { speed_m_s: 6.5 },
    )
    .await;
    assert_eq!(reply.command, FlightCommandKind::ChangeSpeed);
    assert_eq!(reply.steps[0].result, CommandResult::Accepted);
    let sent = commands(&h);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].command, MavCmd::MAV_CMD_DO_CHANGE_SPEED);
    assert_eq!(
        (sent[0].param1, sent[0].param2, sent[0].param3),
        (1.0, 6.5, -1.0),
        "groundspeed in param2, no throttle change"
    );

    // Armed altitude still refuses without a DTM: no AMSL guess is ever sent.
    let error = begin(
        h.app.handle(),
        "main",
        request(&session, FlightCommandIntent::ChangeAltitude { agl_m: 20.0 }),
    )
    .expect("prepared")
    .run()
    .await
    .expect_err("no DTM");
    assert!(format!("{error}").contains("no DTM is loaded"), "got {error}");
    assert_eq!(commands(&h).len(), 1, "the refused altitude wrote nothing");
    h.shutdown().await;
}

/// The AGL→AMSL conversion is the only place this app decides what PX4's
/// `DO_CHANGE_ALTITUDE` `param1` means; it must follow the project's datum
/// chain (`H + N(CGG2013) = h`, `h - N(EGM96) = AMSL`) and stay fail-closed.
#[test]
fn dtm_heights_convert_to_px4_amsl() {
    use maggcs_core::height::GeoidModel;

    let (lat, lon) = (49.2, -123.1);
    let amsl = dtm_height_to_amsl(Height::new(HeightDatum::AmslEgm96, 42.0), lat, lon)
        .expect("AMSL is already the target datum");
    assert_eq!(amsl.datum(), HeightDatum::AmslEgm96);
    assert!((amsl.meters() - 42.0).abs() < 1e-9);

    let offset = crate::terrain_service::vertical_chain_offset(lat, lon).expect("geoid chain");
    let cgvd = dtm_height_to_amsl(Height::new(HeightDatum::OrthometricCgvd2013, 42.0), lat, lon)
        .expect("chain");
    assert_eq!(cgvd.datum(), HeightDatum::AmslEgm96);
    assert!((cgvd.meters() - (42.0 - offset)).abs() < 1e-9, "H - offset");

    let n_egm96 = maggcs_core::height::egm96_model()
        .undulation_m(lat, lon, HeightDatum::AmslEgm96)
        .expect("EGM96 grid");
    let ellipsoid = dtm_height_to_amsl(Height::new(HeightDatum::EllipsoidWgs84, 60.0), lat, lon)
        .expect("ellipsoid");
    assert!((ellipsoid.meters() - (60.0 - n_egm96)).abs() < 1e-9, "h - N");
}
