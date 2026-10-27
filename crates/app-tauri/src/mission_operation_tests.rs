//! Production native Mission/serial/ledger fixtures; the FC is a test-created PTY peer.
#![allow(deprecated)]

use super::*;
use crate::main_link_service::tests::{wait_until, Harness};
use maggcs_core::mavlink::MavMessage;
use maggcs_core::mission::service::MissionServiceEvent;
use maggcs_core::mission::MissionFrame;
use maggcs_core::runtime::{RequestId, TransportGeneration};
use std::sync::{atomic::Ordering, Arc};
use std::time::Instant;
use tauri::Listener;

fn attach(h: &Harness, window: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(h.app.handle())
        .targets()
        .expect("targets")
        .remove(0)
        .target
        .expect("target");
    h.state()
        .runtime_clients
        .attach(window, "mission".into(), target)
        .expect("attach")
        .0
}

fn context(session: &RuntimeClientSession) -> MissionRequestContext {
    MissionRequestContext {
        session: session.clone(),
        request_id: RequestId::fresh().expect("request id"),
    }
}

fn items(count: u16) -> Vec<MissionItem> {
    (0..count)
        .map(|seq| {
            let mut item = MissionItem::waypoint(
                49.25 + f64::from(seq) * 0.001,
                -123.1,
                50.0,
                MissionFrame::GlobalInt,
            );
            item.seq = seq;
            item
        })
        .collect()
}

fn mission_writes(h: &Harness) -> Vec<MavMessage> {
    h.trace
        .lock()
        .iter()
        .filter(|message| {
            matches!(
                message,
                MavMessage::MISSION_COUNT(_)
                    | MavMessage::MISSION_ITEM_INT(_)
                    | MavMessage::MISSION_REQUEST_LIST(_)
                    | MavMessage::MISSION_REQUEST_INT(_)
                    | MavMessage::MISSION_ACK(_)
                    | MavMessage::MISSION_CLEAR_ALL(_)
                    | MavMessage::MISSION_SET_CURRENT(_)
            )
        })
        .cloned()
        .collect()
}

async fn ready(h: &Harness) {
    wait_until(|| {
        h.state()
            .hub()
            .expect("hub")
            .snapshot()
            .extended_sys_state
            .is_some()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn all_native_mission_actions_return_exact_identity_and_terminal_results() {
    let h = Harness::missions().await;
    ready(&h).await;
    let session = attach(&h, "main");
    let upload = begin(
        h.app.handle(),
        "main",
        context(&session),
        MissionAction::Upload(items(3)),
    )
    .expect("upload")
    .run()
    .await
    .expect("accepted upload");
    assert_eq!(upload.result, MissionResult::Uploaded { count: 3 });
    assert_eq!(upload.operation.owner, session.owner);
    assert_eq!(upload.operation.target, session.target);
    let download = begin(
        h.app.handle(),
        "main",
        context(&session),
        MissionAction::Download,
    )
    .expect("download")
    .run()
    .await
    .expect("download result");
    assert_eq!(
        download.result,
        MissionResult::Downloaded { items: items(3) }
    );
    let current = begin(
        h.app.handle(),
        "main",
        context(&session),
        MissionAction::SetCurrent(2),
    )
    .expect("current")
    .run()
    .await
    .expect("read-back");
    assert_eq!(current.result, MissionResult::Current { seq: 2 });
    let clear = begin(
        h.app.handle(),
        "main",
        context(&session),
        MissionAction::Clear,
    )
    .expect("clear")
    .run()
    .await
    .expect("clear ACK");
    assert_eq!(clear.result, MissionResult::Cleared);
    assert!(h
        .mission_peer
        .as_ref()
        .expect("peer")
        .items
        .lock()
        .is_empty());
    assert!(h.state().missions.active.lock().is_empty());
    assert_eq!(
        owned_status(h.app.handle(), "main", &session, &upload.operation)
            .expect("retained")
            .outcome,
        Some(OperationOutcome::Succeeded)
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_caller_invalid_snapshot_and_replaced_prepared_target_write_zero_frames() {
    let h = Harness::missions().await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    assert!(matches!(
        begin(h.app.handle(), "a", context(&b), MissionAction::Clear),
        Err(MissionOperationError::Operation {
            error: OperationError::NotOwner
        })
    ));
    let mut invalid = items(1);
    invalid[0].seq = 1;
    assert!(matches!(
        begin(
            h.app.handle(),
            "a",
            context(&a),
            MissionAction::Upload(invalid)
        ),
        Err(MissionOperationError::Protocol {
            error: MissionProtocolError::InvalidRequest { .. }
        })
    ));
    let work = begin(h.app.handle(), "a", context(&a), MissionAction::Download).expect("prepared");
    h.handle.reconnect().await.expect("reopen");
    wait_until(|| h.handle.generation() > a.target.generation.get()).await;
    assert!(work.run().await.is_err());
    assert!(mission_writes(&h).is_empty());
    assert!(h.state().missions.active.lock().is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_cancel_and_idle_native_close_preserve_b_transfer_and_primary_cache() {
    let h = Harness::missions().await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    let params = h.state().primary_params().expect("parameters");
    let cached = params.read("MR_SHARED").await.expect("cache read");
    let peer = h.mission_peer.as_ref().expect("peer");
    peer.hold_upload.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "b",
        context(&b),
        MissionAction::Upload(items(2)),
    )
    .expect("B upload");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| !mission_writes(&h).is_empty()).await;
    assert!(matches!(
        cancel(h.app.handle(), "a", &a, &operation),
        Err(MissionOperationError::Operation {
            error: OperationError::NotOwner
        })
    ));
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("native close A");
    assert!(!h.handle.is_shutdown_requested());
    assert_eq!(params.cache().get("MR_SHARED"), Some(cached));
    peer.hold_upload.store(false, Ordering::Release);
    assert_eq!(
        task.await.expect("B driver").expect("B ACK").result,
        MissionResult::Uploaded { count: 2 }
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_cancel_retains_fc_uncertainty_and_old_cancel_cannot_stop_new_generation() {
    let h = Harness::missions().await;
    let old = attach(&h, "main");
    let peer = h.mission_peer.as_ref().expect("peer");
    peer.hold_ack.store(true, Ordering::Release);
    let work = begin(h.app.handle(), "main", context(&old), MissionAction::Clear).expect("clear");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| !mission_writes(&h).is_empty()).await;
    let started = Instant::now();
    cancel(h.app.handle(), "main", &old, &operation).expect("own cancellation");
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("bounded retirement")
        .expect("driver");
    assert!(matches!(
        result,
        Err(MissionOperationError::ReconciliationRequired { .. })
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    let retained = owned_status(h.app.handle(), "main", &old, &operation).expect("retained");
    assert_eq!(retained.cleanup, CleanupOutcome::Confirmed);
    assert!(matches!(
        retained.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert!(
        peer.items.lock().is_empty(),
        "cancel does not invent a rollback of an already-cleared FC"
    );
    let count = mission_writes(&h).len();
    assert!(matches!(
        begin(
            h.app.handle(),
            "main",
            context(&old),
            MissionAction::Download
        )
        .expect("claim released")
        .run()
        .await,
        Err(MissionOperationError::Protocol {
            error: MissionProtocolError::AmbiguousReply
        })
    ));
    assert_eq!(mission_writes(&h).len(), count);
    h.handle.reconnect().await.expect("new generation");
    wait_until(|| {
        RuntimeFacade::new(h.app.handle())
            .targets()
            .expect("targets")
            .first()
            .is_some_and(|snapshot| {
                snapshot
                    .target
                    .as_ref()
                    .is_some_and(|target| target.generation > old.target.generation)
            })
    })
    .await;
    let new = attach(&h, "main");
    let work = begin(
        h.app.handle(),
        "main",
        context(&new),
        MissionAction::Upload(items(1)),
    )
    .expect("new upload");
    let task = tokio::spawn(work.run());
    wait_until(|| {
        mission_writes(&h)
            .iter()
            .filter(|message| matches!(message, MavMessage::MISSION_ITEM_INT(_)))
            .count()
            == 1
    })
    .await;
    cancel(h.app.handle(), "main", &old, &operation).expect("old cancellation");
    peer.hold_ack.store(false, Ordering::Release);
    assert_eq!(
        task.await.expect("new driver").expect("new ACK").result,
        MissionResult::Uploaded { count: 1 }
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn native_close_retires_owned_download_and_late_close_cannot_expire_reopened_client() {
    let h = Harness::missions().await;
    let old = attach(&h, "main");
    let peer = h.mission_peer.as_ref().expect("peer");
    peer.hold_download.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "main",
        context(&old),
        MissionAction::Download,
    )
    .expect("download");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| !mission_writes(&h).is_empty()).await;
    h.state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("native owner close");
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("close budget")
        .expect("driver");
    assert!(matches!(
        result,
        Err(MissionOperationError::ReconciliationRequired { .. })
    ));
    assert!(matches!(
        retained_status(h.app.handle(), &operation)
            .expect("retained")
            .outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert!(!h.handle.is_shutdown_requested());
    h.handle.reconnect().await.expect("reopen transport");
    wait_until(|| {
        RuntimeFacade::new(h.app.handle())
            .targets()
            .expect("targets")
            .first()
            .is_some_and(|snapshot| {
                snapshot
                    .target
                    .as_ref()
                    .is_some_and(|target| target.generation > old.target.generation)
            })
    })
    .await;
    let new = attach(&h, "main");
    assert_ne!(new.owner.client_id, old.owner.client_id);
    peer.hold_current.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "main",
        context(&new),
        MissionAction::SetCurrent(1),
    )
    .expect("new current");
    let task = tokio::spawn(work.run());
    wait_until(|| {
        mission_writes(&h)
            .iter()
            .any(|message| matches!(message, MavMessage::MISSION_SET_CURRENT(_)))
    })
    .await;
    h.state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("captured old close callback");
    peer.hold_current.store(false, Ordering::Release);
    assert_eq!(
        task.await.expect("new driver").expect("new reply").result,
        MissionResult::Current { seq: 1 }
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_close_duplicate_request_and_foreign_operation_type_cannot_write_or_cancel() {
    let h = Harness::missions().await;
    let session = attach(&h, "main");
    let original = context(&session);
    let work = begin(
        h.app.handle(),
        "main",
        original.clone(),
        MissionAction::Download,
    )
    .expect("prepared");
    assert!(matches!(
        begin(h.app.handle(), "main", original, MissionAction::Download),
        Err(MissionOperationError::Operation {
            error: OperationError::DuplicateRequest { .. }
        })
    ));
    h.state()
        .runtime_clients
        .expire_window(&session.owner.client_id)
        .expect("close before write");
    assert!(work.run().await.is_err());
    assert!(mission_writes(&h).is_empty());
    let session = attach(&h, "main");
    let command = crate::flight_command_operations::begin(
        h.app.handle(),
        "main",
        maggcs_core::commands::operation_types::FlightCommandRequest {
            context: maggcs_core::commands::operation_types::CommandRequestContext {
                session: session.clone(),
                request_id: RequestId::fresh().expect("request"),
            },
            intent: maggcs_core::commands::operation_types::FlightCommandIntent::ReturnToLaunch,
        },
    )
    .expect("command operation");
    assert!(matches!(
        cancel(h.app.handle(), "main", &session, command.operation()),
        Err(MissionOperationError::Operation {
            error: OperationError::InvalidRequest { .. }
        })
    ));
    drop(command);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mission_claim_blocks_reboot_and_borrowed_motor_target_blocks_mission_admission() {
    let h = Harness::missions().await;
    ready(&h).await;
    let session = attach(&h, "main");
    let work = begin(
        h.app.handle(),
        "main",
        context(&session),
        MissionAction::Download,
    )
    .expect("mission claim");
    assert!(matches!(
        crate::command_operations::begin_reboot(
            h.app.handle(),
            "main",
            maggcs_core::commands::operation_types::CommandRequestContext {
                session: session.clone(),
                request_id: RequestId::fresh().expect("request"),
            }
        ),
        Err(
            maggcs_core::commands::operation_types::CommandOperationError::Operation {
                error: OperationError::ResourceBusy { .. }
            }
        )
    ));
    drop(work);
    h.attach().await;
    assert!(matches!(
        begin(
            h.app.handle(),
            "main",
            context(&session),
            MissionAction::Clear
        ),
        Err(MissionOperationError::Operation {
            error: OperationError::InvalidRequest { .. }
        })
    ));
    assert!(mission_writes(&h).is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_ack_and_wrong_current_readback_never_report_success() {
    let h = Harness::missions().await;
    let session = attach(&h, "main");
    let peer = h.mission_peer.as_ref().expect("peer");
    *peer.result.lock() = mavlink::common::MavMissionResult::MAV_MISSION_DENIED;
    let result = begin(
        h.app.handle(),
        "main",
        context(&session),
        MissionAction::Clear,
    )
    .expect("clear")
    .run()
    .await;
    assert!(matches!(
        result,
        Err(MissionOperationError::Protocol {
            error: MissionProtocolError::AckDenied { .. }
        })
    ));
    h.handle.reconnect().await.expect("reopen");
    wait_until(|| {
        RuntimeFacade::new(h.app.handle())
            .targets()
            .expect("targets")
            .first()
            .is_some_and(|snapshot| {
                snapshot
                    .target
                    .as_ref()
                    .is_some_and(|target| target.generation > session.target.generation)
            })
    })
    .await;
    let session = attach(&h, "main");
    peer.hold_current.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "main",
        context(&session),
        MissionAction::SetCurrent(3),
    )
    .expect("current");
    let task = tokio::spawn(work.run());
    wait_until(|| {
        mission_writes(&h)
            .iter()
            .any(|message| matches!(message, MavMessage::MISSION_SET_CURRENT(_)))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(!task.is_finished());
    peer.hold_current.store(false, Ordering::Release);
    assert_eq!(
        task.await
            .expect("driver")
            .expect("matching read-back")
            .result,
        MissionResult::Current { seq: 3 }
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_and_current_events_keep_their_target_and_retire_obsolete_operation_authority() {
    let h = Harness::missions().await;
    let session = attach(&h, "main");
    let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let output = captured.clone();
    h.app.listen("mission_progress", move |event| {
        output
            .lock()
            .push(serde_json::from_str(event.payload()).expect("progress JSON"));
    });
    let peer = h.mission_peer.as_ref().expect("peer");
    peer.hold_ack.store(true, Ordering::Release);
    let work = begin(
        h.app.handle(),
        "main",
        context(&session),
        MissionAction::Upload(items(1)),
    )
    .expect("upload");
    let operation = work.operation().clone();
    let task = tokio::spawn(work.run());
    wait_until(|| !captured.lock().is_empty()).await;
    assert_eq!(
        captured.lock()[0]["operation"],
        serde_json::to_value(&operation).expect("operation")
    );
    let count = captured.lock().len();
    crate::mission_service::forward_event(
        h.app.handle(),
        session.target.link_id,
        MissionServiceEvent::OwnedProgress {
            operation_id: operation.id.clone(),
            generation: TransportGeneration::new(session.target.generation.get() + 1)
                .expect("wrong generation"),
            sent: 99,
            total: 99,
        },
    );
    assert_eq!(captured.lock().len(), count);
    cancel(h.app.handle(), "main", &session, &operation).expect("cancel");
    assert!(task.await.expect("driver").is_err());
    crate::mission_service::forward_event(
        h.app.handle(),
        session.target.link_id,
        MissionServiceEvent::OwnedProgress {
            operation_id: operation.id,
            generation: session.target.generation,
            sent: 99,
            total: 99,
        },
    );
    assert_eq!(captured.lock().len(), count);
    assert!(h.state().missions.active.lock().is_empty());
    let current = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let output = current.clone();
    h.app.listen("mission_current", move |event| {
        output
            .lock()
            .push(serde_json::from_str(event.payload()).expect("current JSON"));
    });
    peer.publish_current.store(false, Ordering::Release);
    crate::mission_service::forward_event(
        h.app.handle(),
        session.target.link_id,
        MissionServiceEvent::CurrentObserved {
            seq: 999,
            generation: TransportGeneration::new(session.target.generation.get() + 1)
                .expect("wrong generation"),
            received_at: Instant::now(),
        },
    );
    assert!(current.lock().iter().all(|event| event["seq"] != 999));
    crate::mission_service::forward_event(
        h.app.handle(),
        session.target.link_id,
        MissionServiceEvent::CurrentObserved {
            seq: 5,
            generation: session.target.generation,
            received_at: Instant::now(),
        },
    );
    assert!(current.lock().iter().any(|event| event["seq"] == 5
        && event["target"] == serde_json::to_value(&session.target).expect("target")));
    h.shutdown().await;
}
