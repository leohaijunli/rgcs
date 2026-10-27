use super::*;
use crate::main_link_service::tests::{wait_until, Harness};
use crate::runtime_facade::RuntimeFacade;
use maggcs_core::runtime::operations::{OperationRequest, OperationResource};
use maggcs_core::runtime::RequestId;

async fn setup() -> Harness {
    let h = Harness::logs().await;
    let logs = h
        .state()
        .logs
        .spawn(h.app.handle().clone(), h.handle.clone());
    h.state().set_primary_logs(logs);
    h.attach().await;
    h
}
fn attach(h: &Harness, window: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(h.app.handle())
        .targets()
        .expect("targets")
        .into_iter()
        .find(|target| {
            target
                .roles
                .contains(&maggcs_core::runtime::TargetRole::MotorTest)
        })
        .expect("Motor target")
        .target
        .expect("target");
    h.state()
        .runtime_clients
        .attach(window, "motor-test".into(), target)
        .expect("attach")
        .0
}
fn context(session: &RuntimeClientSession) -> LogRequestContext {
    LogRequestContext {
        session: session.clone(),
        request_id: RequestId::fresh().expect("request"),
    }
}
fn log_frames(h: &Harness) -> Vec<maggcs_core::mavlink::MavMessage> {
    h.trace
        .lock()
        .iter()
        .filter(|frame| {
            matches!(
                frame,
                maggcs_core::mavlink::MavMessage::LOG_REQUEST_LIST(_)
                    | maggcs_core::mavlink::MavMessage::LOG_REQUEST_DATA(_)
                    | maggcs_core::mavlink::MavMessage::LOG_REQUEST_END(_)
                    | maggcs_core::mavlink::MavMessage::LOG_ERASE(_)
            )
        })
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_motor_list_and_durable_download_reuse_the_borrowed_primary_worker() {
    let h = setup().await;
    let session = attach(&h, "a");
    let work = begin(h.app.handle(), "a", context(&session), Action::List).expect("begin list");
    let listed = tokio::time::timeout(Duration::from_secs(10), execute(work))
        .await
        .expect("list deadline")
        .expect("list");
    let LogResult::Listed { entries } = listed.result else {
        panic!("list result")
    };
    assert_eq!(entries, *h.log_peer.as_ref().expect("peer").entries.lock());
    let path = std::env::temp_dir().join(format!(
        "maggcs-motor-log-{}.ulg",
        OperationId::fresh().expect("id")
    ));
    let downloaded = execute(
        begin(
            h.app.handle(),
            "a",
            context(&session),
            Action::Download {
                entry: entries[0].clone(),
                path: path.to_string_lossy().into(),
            },
        )
        .expect("begin download"),
    )
    .await
    .expect("download");
    assert!(
        matches!(downloaded.result, LogResult::Downloaded { entry, .. } if entry == entries[0])
    );
    assert_eq!(
        std::fs::read(&path).expect("durable file"),
        (0..entries[0].size)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>()
    );
    std::fs::remove_file(path).expect("cleanup");
    assert!(
        h.state()
            .motor_test
            .target
            .lock()
            .as_ref()
            .is_some_and(|target| target.origin()
                == crate::motor_test_state::TargetLinkOrigin::MainSerial
                && target.target.handle.handle_id() == h.handle.handle_id()),
        "Motor borrowing reuses the primary transport"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn exact_cancel_sends_end_and_foreign_or_old_cancel_cannot_stop_new_work() {
    let h = setup().await;
    let peer = h.log_peer.as_ref().expect("peer");
    peer.hold_list.store(true, Ordering::Release);
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    let work = begin(h.app.handle(), "a", context(&a), Action::List).expect("list");
    let operation = work.operation().clone();
    let task = tauri::async_runtime::spawn(work.run());
    wait_until(|| {
        log_frames(&h)
            .iter()
            .any(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_REQUEST_LIST(_)))
    })
    .await;
    assert!(owned(h.app.handle(), "b", &b, &operation).is_err());
    h.state()
        .runtime_clients
        .operations
        .cancel(&a.owner, &operation)
        .expect("cancel A");
    assert!(task.await.expect("task").is_err());
    assert_eq!(
        retained(h.app.handle(), &operation)
            .expect("status")
            .cleanup,
        CleanupOutcome::Confirmed
    );
    assert!(log_frames(&h)
        .iter()
        .any(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_REQUEST_END(_))));
    peer.hold_list.store(false, Ordering::Release);
    let next = begin(h.app.handle(), "b", context(&b), Action::List).expect("B");
    h.state()
        .runtime_clients
        .operations
        .cancel(&a.owner, &operation)
        .expect("old cancel");
    assert!(next.run().await.is_ok());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_and_erase_require_idle_logger_proof_and_verified_readback() {
    let h = setup().await;
    let session = attach(&h, "a");
    let entry = h.log_peer.as_ref().expect("peer").entries.lock()[0].clone();
    let deleted = execute(
        begin(
            h.app.handle(),
            "a",
            context(&session),
            Action::Delete { entry },
        )
        .expect("delete"),
    )
    .await
    .expect("verified deletion");
    assert!(matches!(deleted.result, LogResult::Deleted { entries } if entries.len() == 1));
    let erased =
        execute(begin(h.app.handle(), "a", context(&session), Action::Erase).expect("erase"))
            .await
            .expect("verified erase");
    assert_eq!(erased.result, LogResult::Erased);
    assert!(h.log_peer.as_ref().expect("peer").entries.lock().is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_local_write_preserves_existing_destination_and_leaves_no_partial() {
    let h = setup().await;
    let session = attach(&h, "a");
    let lease = h
        .state()
        .runtime_clients
        .operations
        .prepare(OperationRequest {
            request_id: RequestId::fresh().expect("request"),
            owner: session.owner.clone(),
            target: session.target.clone(),
            resources: vec![OperationResource::LogTransfer],
            budget_ms: 1000,
        })
        .expect("lease");
    let cancellation = lease.cancellation();
    h.state()
        .runtime_clients
        .operations
        .cancel(&session.owner, lease.operation())
        .expect("cancel");
    let directory = std::env::temp_dir().join(format!(
        "maggcs-motor-local-{}",
        OperationId::fresh().expect("id")
    ));
    std::fs::create_dir(&directory).expect("directory");
    let path = directory.join("existing.ulg");
    std::fs::write(&path, b"existing").expect("existing");
    assert!(atomic_write(
        path.to_str().expect("path"),
        vec![7; FILE_CHUNK * 2],
        "cancelled",
        cancellation,
        Arc::new(AtomicBool::new(false)),
        h.handle.clone(),
        h.handle.generation()
    )
    .await
    .is_err());
    assert_eq!(std::fs::read(&path).expect("preserved"), b"existing");
    assert_eq!(std::fs::read_dir(&directory).expect("entries").count(), 1);
    std::fs::remove_dir_all(directory).expect("cleanup");
    h.shutdown().await;
}
