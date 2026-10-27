//! Native owner/target admission and command retirement over production serial PTYs.

use super::*;
use crate::main_link_service::tests::{wait_until, Harness};
use maggcs_core::commands::CommandResult;
use maggcs_core::mavlink::{LinkState, MavMessage};
use maggcs_core::runtime::operations::OperationPhase;
use maggcs_core::runtime::RequestId;
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
        .attach(window, "parameters".into(), target)
        .expect("client")
        .0
}

fn request(session: &RuntimeClientSession) -> CommandRequestContext {
    CommandRequestContext {
        session: session.clone(),
        request_id: RequestId::fresh().expect("request"),
    }
}

fn reboot_count(h: &Harness) -> usize {
    h.trace
        .lock()
        .iter()
        .filter(|frame| {
            matches!(frame,
        MavMessage::COMMAND_LONG(command) if command.command == REBOOT_COMMAND)
        })
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_cancel_and_native_close_a_leave_b_reboot_and_primary_services_alive() {
    let h = Harness::commands().await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    let peer = h.command_peer.as_ref().expect("command peer");
    peer.hold_ack.store(true, Ordering::Release);
    let work = begin_reboot(h.app.handle(), "b", request(&b)).expect("b reboot");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| reboot_count(&h) > 0).await;
    assert!(matches!(
        cancel(h.app.handle(), "a", &a, &operation),
        Err(CommandOperationError::Operation {
            error: OperationError::NotOwner
        })
    ));
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("native close a");
    assert!(!task.is_finished());
    peer.hold_ack.store(false, Ordering::Release);
    let reply = task.await.expect("driver").expect("b ACK");
    assert_eq!(reply.operation, operation);
    assert_eq!(reply.result, CommandResult::Accepted);
    assert_eq!(
        owned_status(h.app.handle(), "b", &b, &operation)
            .expect("status")
            .outcome,
        Some(OperationOutcome::Succeeded)
    );
    let value = h
        .state()
        .primary_params()
        .expect("original params")
        .read("MR_SHARED")
        .await
        .expect("primary read");
    assert_eq!(value.value, maggcs_core::params::model::ParamValue::Int(17));
    assert_eq!(h.handle.state().await, LinkState::Connected);
    assert!(matches!(
        RuntimeFacade::new(h.app.handle()).bind(&b.target),
        Err(RuntimeError::TargetUnavailable)
    ));
    assert!(begin_reboot(h.app.handle(), "b", request(&b)).is_err());
    assert!(h.state().runtime_clients.authorize("b", &b).is_ok());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_reboot_owner_stops_retries_and_retains_uncertainty_and_claims() {
    let h = Harness::commands().await;
    let a = attach(&h, "a");
    h.command_peer
        .as_ref()
        .expect("peer")
        .hold_ack
        .store(true, Ordering::Release);
    let work = begin_reboot(h.app.handle(), "a", request(&a)).expect("reboot");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| reboot_count(&h) > 0).await;
    let started = std::time::Instant::now();
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("owner close");
    assert!(matches!(
        task.await.expect("driver"),
        Err(CommandOperationError::ReconciliationRequired { .. })
    ));
    assert!(started.elapsed() < maggcs_core::commands::OWNED_CANCEL_TIMEOUT);
    let status = retained_status(h.app.handle(), &operation).expect("retained");
    assert_eq!(status.phase, OperationPhase::Finished);
    assert_eq!(status.cleanup, CleanupOutcome::Uncertain);
    assert!(matches!(
        status.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    let b = attach(&h, "b");
    assert!(matches!(
        begin_reboot(h.app.handle(), "b", request(&b)),
        Err(CommandOperationError::Operation {
            error: OperationError::ResourceBusy { .. }
        })
    ));
    let sent = reboot_count(&h);
    tokio::time::sleep(maggcs_core::commands::COMMAND_RETRY_TIMEOUT + TARGET_POLL).await;
    assert_eq!(reboot_count(&h), sent, "cancelled reboot must not retry");
    // A late FC acceptance is evidence for the retired request, not success for
    // a later operation or permission to clear its retained uncertainty.
    h.command_peer
        .as_ref()
        .expect("peer")
        .hold_ack
        .store(false, Ordering::Release);
    tokio::time::sleep(TARGET_POLL).await;
    let late = retained_status(h.app.handle(), &operation).expect("late status");
    assert_eq!(late.operation, status.operation);
    assert_eq!(late.outcome, status.outcome);
    assert_eq!(late.cleanup, status.cleanup);
    assert_eq!(late.phase, status.phase);
    assert_eq!(late.stop_reason, status.stop_reason);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_caller_and_expired_prepared_owner_send_zero_reboots() {
    let h = Harness::commands().await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    assert!(matches!(
        begin_reboot(h.app.handle(), "a", request(&b)),
        Err(CommandOperationError::Operation {
            error: OperationError::NotOwner
        })
    ));
    let work = begin_reboot(h.app.handle(), "a", request(&a)).expect("prepare");
    let operation = work.operation().clone();
    h.state()
        .runtime_clients
        .detach("a", &a)
        .expect("expire module");
    assert!(work.run().await.is_err());
    assert_eq!(reboot_count(&h), 0);
    assert_eq!(
        retained_status(h.app.handle(), &operation)
            .expect("status")
            .cleanup,
        CleanupOutcome::NotRequired
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_reboot_cannot_follow_a_reopened_transport() {
    let h = Harness::commands().await;
    let a = attach(&h, "a");
    let work = begin_reboot(h.app.handle(), "a", request(&a)).expect("prepare");
    h.handle.reconnect().await.expect("reopen");
    wait_until(|| h.handle.generation() > a.target.generation.get()).await;
    assert!(work.run().await.is_err());
    assert_eq!(reboot_count(&h), 0);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn parameter_claim_blocks_reboot_and_denied_ack_releases_its_own_claim() {
    let h = Harness::commands().await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    let parameter = crate::params_operations::begin(
        h.app.handle(),
        "a",
        maggcs_core::params::operation_types::ParameterRequestContext {
            session: a.clone(),
            request_id: RequestId::fresh().expect("request"),
        },
        crate::params_operations::ParameterAction::Read {
            name: "MR_SHARED".into(),
        },
    )
    .expect("read claim");
    assert!(matches!(
        begin_reboot(h.app.handle(), "b", request(&b)),
        Err(CommandOperationError::Operation {
            error: OperationError::ResourceBusy { .. }
        })
    ));
    assert_eq!(reboot_count(&h), 0);
    drop(parameter);
    *h.command_peer.as_ref().expect("peer").result.lock() =
        mavlink::common::MavResult::MAV_RESULT_DENIED;
    let work = begin_reboot(h.app.handle(), "b", request(&b)).expect("b reboot");
    let operation = work.operation().clone();
    assert_eq!(
        work.run().await.expect("FC verdict").result,
        CommandResult::Denied
    );
    let status = owned_status(h.app.handle(), "b", &b, &operation).expect("status");
    assert!(matches!(
        status.outcome,
        Some(OperationOutcome::Failed { .. })
    ));
    assert_eq!(status.cleanup, CleanupOutcome::NotRequired);
    let next = begin_reboot(h.app.handle(), "a", request(&a)).expect("released claim");
    drop(next);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn armed_or_borrowed_motor_target_refuses_reboot_before_writing() {
    let h = Harness::commands().await;
    let a = attach(&h, "a");
    h.command_peer
        .as_ref()
        .expect("peer")
        .armed
        .store(true, Ordering::Release);
    wait_until(|| {
        h.state().hub().is_some_and(|hub| {
            hub.snapshot()
                .heartbeat
                .is_some_and(|hb| hb.base_mode.safety_armed)
        })
    })
    .await;
    assert!(begin_reboot(h.app.handle(), "a", request(&a)).is_err());
    h.command_peer
        .as_ref()
        .expect("peer")
        .armed
        .store(false, Ordering::Release);
    wait_until(|| {
        h.state().hub().is_some_and(|hub| {
            hub.snapshot()
                .heartbeat
                .is_some_and(|hb| !hb.base_mode.safety_armed)
        })
    })
    .await;
    h.attach().await;
    assert!(begin_reboot(h.app.handle(), "a", request(&a)).is_err());
    assert_eq!(reboot_count(&h), 0);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_landed_state_refuses_reboot_before_writing() {
    let h = Harness::parameters().await;
    let a = attach(&h, "a");
    assert!(matches!(
        begin_reboot(h.app.handle(), "a", request(&a)),
        Err(CommandOperationError::Operation {
            error: OperationError::InvalidRequest { .. }
        })
    ));
    assert_eq!(reboot_count(&h), 0);
    h.shutdown().await;
}
