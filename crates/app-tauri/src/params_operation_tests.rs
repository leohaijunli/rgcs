//! Native owner/driver integration through production serial on Unix PTYs.

use super::*;
use crate::main_link_service::tests::{wait_until, Harness};
use maggcs_core::mavlink::MavMessage;
use maggcs_core::params::codec::decode_param_id;
use maggcs_core::runtime::operations::OperationPhase;
use maggcs_core::runtime::RequestId;
use std::sync::atomic::Ordering;

fn attach(h: &Harness, window: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(h.app.handle())
        .targets()
        .expect("targets")
        .remove(0)
        .target
        .expect("verified target");
    h.state()
        .runtime_clients
        .attach(window, "parameters".into(), target)
        .expect("client")
        .0
}

fn context(session: &RuntimeClientSession) -> ParameterRequestContext {
    ParameterRequestContext {
        session: session.clone(),
        request_id: RequestId::fresh().expect("request"),
    }
}

fn saw_set(h: &Harness, name: &str) -> bool {
    h.trace.lock().iter().any(|frame| matches!(frame, MavMessage::PARAM_SET(set) if decode_param_id(&set.param_id) == name))
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_idle_client_a_cannot_cancel_client_b_write_or_clear_primary_io() {
    let h = Harness::parameters().await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    h.hold_set_echo.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "b",
        context(&b),
        ParameterAction::Set {
            name: "B".into(),
            value: ParamValue::Int(22),
            ptype: ParamType::I32,
        },
    )
    .expect("work");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| saw_set(&h, "B")).await;
    assert!(matches!(
        cancel(h.app.handle(), "a", &a, &operation),
        Err(ParameterCommandError::Operation {
            error: OperationError::NotOwner
        })
    ));
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("native close a");
    assert!(!task.is_finished());
    h.hold_set_echo.store(false, Ordering::Release);
    assert!(matches!(
        task.await.expect("driver").expect("write"),
        ParameterResult::Set(SetResult::Confirmed(ParamValue::Int(22)))
    ));
    assert_eq!(
        owned_status(h.app.handle(), "b", &b, &operation)
            .expect("status")
            .outcome,
        Some(OperationOutcome::Succeeded)
    );
    assert_eq!(
        h.handle.state().await,
        maggcs_core::mavlink::LinkState::Connected
    );
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("late repeated close");
    assert!(h.state().runtime_clients.authorize("b", &b).is_ok());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn client_loss_cancels_and_reconciles_only_its_dispatched_parameter_write() {
    let h = Harness::parameters().await;
    let a = attach(&h, "a");
    h.hold_set_echo.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "a",
        context(&a),
        ParameterAction::Set {
            name: "A".into(),
            value: ParamValue::Int(17),
            ptype: ParamType::I32,
        },
    )
    .expect("work");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| saw_set(&h, "A")).await;
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("native close a");
    assert!(task.await.expect("driver").is_err());
    let status = h
        .state()
        .runtime_clients
        .operations
        .status(&a.owner, &operation)
        .expect("retained evidence");
    assert_eq!(status.phase, OperationPhase::Finished);
    assert!(matches!(
        status.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert_eq!(status.cleanup, CleanupOutcome::Confirmed);
    assert_eq!(
        h.state()
            .primary_params()
            .expect("same worker")
            .cache()
            .get("A")
            .expect("read-back")
            .value,
        ParamValue::Int(17)
    );
    let b = attach(&h, "b");
    h.hold_set_echo.store(false, Ordering::Release);
    let result = begin(
        h.app.handle(),
        "b",
        context(&b),
        ParameterAction::Set {
            name: "B".into(),
            value: ParamValue::Int(22),
            ptype: ParamType::I32,
        },
    )
    .expect("b work")
    .run()
    .await;
    assert!(matches!(
        result,
        Ok(ParameterResult::Set(SetResult::Confirmed(ParamValue::Int(
            22
        ))))
    ));
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unconfirmed_readback_preserves_evidence_and_the_write_resource_claim() {
    let h = Harness::parameters().await;
    let a = attach(&h, "a");
    h.hold_set_echo.store(true, Ordering::Release);
    h.hold_reads.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "a",
        context(&a),
        ParameterAction::Set {
            name: "A".into(),
            value: ParamValue::Int(17),
            ptype: ParamType::I32,
        },
    )
    .expect("work");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| saw_set(&h, "A")).await;
    cancel(h.app.handle(), "a", &a, &operation).expect("cancel");
    assert!(matches!(
        task.await.expect("driver"),
        Err(ParameterCommandError::ReconciliationRequired { .. })
    ));
    let status = owned_status(h.app.handle(), "a", &a, &operation).expect("status");
    assert_eq!(status.cleanup, CleanupOutcome::Uncertain);
    assert!(matches!(
        status.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    let b = attach(&h, "b");
    assert!(matches!(
        begin(
            h.app.handle(),
            "b",
            context(&b),
            ParameterAction::Set {
                name: "B".into(),
                value: ParamValue::Int(22),
                ptype: ParamType::I32
            }
        ),
        Err(ParameterCommandError::Operation {
            error: OperationError::ResourceBusy { .. }
        })
    ));
    assert!(!saw_set(&h, "B"));
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn old_target_or_expired_owner_cannot_dispatch_a_prepared_write() {
    let h = Harness::parameters().await;
    let a = attach(&h, "a");
    let work = begin(
        h.app.handle(),
        "a",
        context(&a),
        ParameterAction::Set {
            name: "NO_SEND".into(),
            value: ParamValue::Int(1),
            ptype: ParamType::I32,
        },
    )
    .expect("prepared");
    let operation = work.operation().clone();
    h.handle.reconnect().await.expect("reopen");
    wait_until(|| h.handle.generation() > a.target.generation.get()).await;
    assert!(work.run().await.is_err());
    assert!(!saw_set(&h, "NO_SEND"));
    assert_eq!(
        h.state()
            .runtime_clients
            .operations
            .status(&a.owner, &operation)
            .expect("status")
            .phase,
        OperationPhase::Finished
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_pull_cancel_does_not_end_a_later_clients_read() {
    let h = Harness::parameters().await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    h.hold_pull.store(true, Ordering::Release);
    let work = begin(h.app.handle(), "a", context(&a), ParameterAction::Pull).expect("pull");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| {
        h.trace
            .lock()
            .iter()
            .any(|frame| matches!(frame, MavMessage::PARAM_REQUEST_LIST(_)))
    })
    .await;
    cancel(h.app.handle(), "a", &a, &operation).expect("cancel pull");
    assert!(task.await.expect("pull driver").is_err());
    let work = begin(
        h.app.handle(),
        "b",
        context(&b),
        ParameterAction::Read {
            name: "MR_SHARED".into(),
        },
    )
    .expect("b read");
    let b_operation = work.operation().clone();
    let read = tokio::spawn(work.run());
    cancel(h.app.handle(), "a", &a, &operation).expect("idle old cancel");
    assert!(matches!(
        read.await.expect("b driver"),
        Ok(ParameterResult::Read(ParamEntry {
            value: ParamValue::Int(17),
            ..
        }))
    ));
    assert_eq!(
        owned_status(h.app.handle(), "b", &b, &b_operation)
            .expect("b status")
            .outcome,
        Some(OperationOutcome::Succeeded)
    );
    h.shutdown().await;
}
