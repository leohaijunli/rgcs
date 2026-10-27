use super::*;
use crate::main_link_service::tests::{wait_until, Harness};
use maggcs_core::runtime::operations::{OperationOutcome, OperationPhase};
use tauri::Listener;

fn attach(harness: &Harness, window: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(harness.app.handle())
        .targets()
        .expect("targets")
        .remove(0)
        .target
        .expect("target");
    harness
        .state()
        .runtime_clients
        .attach(window, "console".into(), target)
        .expect("attach")
        .0
}

fn context(session: &RuntimeClientSession) -> ShellRequestContext {
    ShellRequestContext {
        session: session.clone(),
        request_id: RequestId::fresh().expect("request"),
    }
}

fn request(
    session: &RuntimeClientSession,
    operation: &OperationRef,
    command: &str,
    confirmed: bool,
) -> ShellCommandRequest {
    ShellCommandRequest {
        session: session.clone(),
        operation: operation.clone(),
        request_id: RequestId::fresh().expect("request"),
        command: command.into(),
        confirmation: confirmed.then(|| "RUN".into()),
    }
}

fn shell_frames(harness: &Harness) -> Vec<::mavlink::common::SERIAL_CONTROL_DATA> {
    harness
        .trace
        .lock()
        .iter()
        .filter_map(|message| match message {
            MavMessage::SERIAL_CONTROL(frame) => Some(frame.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn long_command_is_complete_output_is_scoped_and_close_releases_exact_target() {
    let harness = Harness::commands().await;
    let console = Arc::new(ConsoleState::default());
    let session = attach(&harness, "main");
    let output = Arc::new(ParkingMutex::new(Vec::<ShellOutputEnvelope>::new()));
    let captured = output.clone();
    harness.app.listen("console_output", move |event| {
        captured
            .lock()
            .push(serde_json::from_str(event.payload()).expect("output envelope"));
    });
    let opened = open_owned(
        harness.app.handle(),
        "main",
        console.clone(),
        context(&session),
    )
    .await
    .expect("open");
    let command = "x".repeat(200);
    let receipt = send_owned(
        harness.app.handle(),
        "main",
        &console,
        request(&session, &opened.operation, &command, true),
    )
    .await
    .expect("full write");
    assert_eq!(receipt.bytes, 200);
    wait_until(|| {
        shell_frames(&harness)
            .iter()
            .filter(|frame| frame.count > 0)
            .count()
            >= 3
    })
    .await;
    let bytes = shell_frames(&harness)
        .into_iter()
        .filter(|frame| frame.count > 0)
        .flat_map(|frame| frame.data[..usize::from(frame.count)].to_vec())
        .collect::<Vec<_>>();
    assert_eq!(
        bytes,
        format!("{command}\n").into_bytes(),
        "never truncate or alter the line"
    );
    wait_until(|| !output.lock().is_empty()).await;
    assert!(output
        .lock()
        .iter()
        .all(|event| event.operation == opened.operation
            && event.target == session.target
            && event.sequence.get() > 0));
    let closed = close_owned(
        harness.app.handle(),
        "main",
        &console,
        &session,
        &opened.operation,
    )
    .await
    .expect("close receipt");
    assert_eq!(closed.phase, OperationPhase::Finished);
    assert_eq!(closed.outcome, Some(OperationOutcome::Succeeded));
    wait_until(|| shell_frames(&harness).iter().any(|frame| frame.count == 0)).await;
    let release = shell_frames(&harness)
        .into_iter()
        .find(|frame| frame.count == 0)
        .expect("release");
    assert_eq!((release.target_system, release.target_component), (42, 1));
    assert!(console.active.lock().is_none());
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_caller_unconfirmed_free_form_and_duplicate_request_write_zero_frames() {
    let harness = Harness::commands().await;
    let console = Arc::new(ConsoleState::default());
    let a = attach(&harness, "a");
    let b = attach(&harness, "b");
    assert!(matches!(
        open_owned(harness.app.handle(), "a", console.clone(), context(&b)).await,
        Err(ShellOperationError::Operation {
            error: OperationError::NotOwner
        })
    ));
    let opened = open_owned(harness.app.handle(), "a", console.clone(), context(&a))
        .await
        .expect("open A");
    assert!(matches!(
        send_owned(
            harness.app.handle(),
            "a",
            &console,
            request(&a, &opened.operation, "reboot", false)
        )
        .await,
        Err(ShellOperationError::Protocol {
            error: ShellProtocolError::ConfirmationRequired
        })
    ));
    assert!(shell_frames(&harness).is_empty());
    let id = RequestId::fresh().expect("request");
    let first = ShellCommandRequest {
        session: a.clone(),
        operation: opened.operation.clone(),
        request_id: id.clone(),
        command: "ver".into(),
        confirmation: None,
    };
    send_owned(harness.app.handle(), "a", &console, first.clone())
        .await
        .expect("first");
    assert!(matches!(
        send_owned(harness.app.handle(), "a", &console, first).await,
        Err(ShellOperationError::Protocol {
            error: ShellProtocolError::InvalidCommand { .. }
        })
    ));
    wait_until(|| shell_frames(&harness).iter().any(|frame| frame.count > 0)).await;
    let before = shell_frames(&harness)
        .iter()
        .filter(|frame| frame.count > 0)
        .count();
    assert_eq!(before, 1);
    assert!(matches!(
        open_owned(
            harness.app.handle(),
            "b",
            Arc::new(ConsoleState::default()),
            context(&b)
        )
        .await,
        Err(ShellOperationError::Operation {
            error: OperationError::ResourceBusy { .. }
        })
    ));
    close_owned(harness.app.handle(), "a", &console, &a, &opened.operation)
        .await
        .expect("close");
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_expiry_releases_old_session_and_late_close_cannot_stop_reopened_owner() {
    let harness = Harness::commands().await;
    let console = Arc::new(ConsoleState::default());
    let old = attach(&harness, "main");
    let opened = open_owned(harness.app.handle(), "main", console.clone(), context(&old))
        .await
        .expect("open");
    harness
        .state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("owner close");
    wait_until(|| console.active.lock().is_none()).await;
    let status = retained_status(harness.app.handle(), &opened.operation).expect("retained");
    assert_eq!(status.cleanup, CleanupOutcome::Confirmed);
    assert_eq!(status.outcome, Some(OperationOutcome::Cancelled));
    assert!(!harness.handle.is_shutdown_requested());
    let reopened = attach(&harness, "main");
    let next = open_owned(
        harness.app.handle(),
        "main",
        console.clone(),
        context(&reopened),
    )
    .await
    .expect("reopen");
    harness
        .state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("late old close");
    send_owned(
        harness.app.handle(),
        "main",
        &console,
        request(&reopened, &next.operation, "ver", false),
    )
    .await
    .expect("new owner retained");
    close_owned(
        harness.app.handle(),
        "main",
        &console,
        &reopened,
        &next.operation,
    )
    .await
    .expect("close new");
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn target_replacement_retires_session_and_old_close_or_output_has_no_new_authority() {
    let harness = Harness::commands().await;
    let console = Arc::new(ConsoleState::default());
    let old = attach(&harness, "main");
    let opened = open_owned(harness.app.handle(), "main", console.clone(), context(&old))
        .await
        .expect("open");
    harness.handle.reconnect().await.expect("reconnect");
    wait_until(|| harness.handle.generation() > old.target.generation.get()).await;
    wait_until(|| console.active.lock().is_none()).await;
    let status = retained_status(harness.app.handle(), &opened.operation).expect("retained");
    assert!(matches!(
        status.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert_eq!(status.cleanup, CleanupOutcome::Uncertain);
    assert!(close_owned(
        harness.app.handle(),
        "main",
        &console,
        &old,
        &opened.operation
    )
    .await
    .is_err());
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn active_console_blocks_motor_borrow_and_foreign_cancel() {
    let harness = Harness::commands().await;
    let console = Arc::new(ConsoleState::default());
    let a = attach(&harness, "a");
    let b = attach(&harness, "b");
    let opened = open_owned(harness.app.handle(), "a", console.clone(), context(&a))
        .await
        .expect("open");
    assert!(matches!(
        cancel_owned(harness.app.handle(), "b", &b, &opened.operation),
        Err(ShellOperationError::Operation {
            error: OperationError::NotOwner
        })
    ));
    let offer = crate::main_link_service::candidate(&harness.state()).expect("offer");
    let app = harness.app.handle().clone();
    let state = harness.state();
    app.manage(console.clone());
    assert!(
        crate::main_link_service::actuator_use_main_target(app, state, offer)
            .await
            .is_err()
    );
    cancel_owned(harness.app.handle(), "a", &a, &opened.operation).expect("owner cancel");
    wait_until(|| console.active.lock().is_none()).await;
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn priority_close_retires_queued_writes_before_a_contended_shell_lock() {
    let h = Harness::commands().await;
    let console = Arc::new(ConsoleState::default());
    let session = attach(&h, "main");
    let opened = open_owned(h.app.handle(), "main", console.clone(), context(&session))
        .await
        .expect("open");
    let lock = console.shell_lock.lock().await;
    let mut sends = Vec::new();
    for _ in 0..COMMAND_CAPACITY + 2 {
        let app = h.app.handle().clone();
        let console = console.clone();
        let request = request(&session, &opened.operation, "ver", false);
        sends.push(tauri::async_runtime::spawn(async move {
            send_owned(&app, "main", &console, request).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let app = h.app.handle().clone();
    let closing_console = console.clone();
    let closing_session = session.clone();
    let operation = opened.operation.clone();
    let close = tauri::async_runtime::spawn(async move {
        close_owned(&app, "main", &closing_console, &closing_session, &operation).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(lock);
    let status = tokio::time::timeout(Duration::from_millis(500), close)
        .await
        .expect("priority receipt")
        .expect("task")
        .expect("closed");
    assert_eq!(status.cleanup, CleanupOutcome::Confirmed);
    for send in sends {
        assert!(send.await.expect("sender").is_err());
    }
    wait_until(|| shell_frames(&h).iter().any(|frame| frame.count == 0)).await;
    assert!(
        shell_frames(&h).iter().all(|frame| frame.count == 0),
        "queued commands must not outlive close"
    );
    assert_eq!(shell_frames(&h).len(), 1);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lifecycle_releases_recorded_console_before_link_shutdown() {
    let h = Harness::commands().await;
    let console = Arc::new(ConsoleState::default());
    h.app.manage(console.clone());
    let session = attach(&h, "main");
    let opened = open_owned(h.app.handle(), "main", console.clone(), context(&session))
        .await
        .expect("open");
    send_owned(
        h.app.handle(),
        "main",
        &console,
        request(&session, &opened.operation, "ver", false),
    )
    .await
    .expect("send");
    assert!(matches!(
        stop(h.app.handle(), Duration::from_secs(1)).await,
        maggcs_core::lifecycle::StopOutcome::Stopped { .. }
    ));
    assert!(!h.handle.is_shutdown_requested());
    wait_until(|| shell_frames(&h).iter().any(|frame| frame.count == 0)).await;
    assert_eq!(
        shell_frames(&h)
            .iter()
            .filter(|frame| frame.count == 0)
            .count(),
        1
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_drops_a_contended_write_waiter_before_owned_release() {
    let h = Harness::commands().await;
    let console = Arc::new(ConsoleState::default());
    let session = attach(&h, "main");
    let opened = open_owned(h.app.handle(), "main", console.clone(), context(&session))
        .await
        .expect("open");
    let lock = console.shell_lock.lock().await;
    let app = h.app.handle().clone();
    let sending_console = console.clone();
    let request = request(&session, &opened.operation, "ver", false);
    let sending = tauri::async_runtime::spawn(async move {
        send_owned(&app, "main", &sending_console, request).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel_owned(h.app.handle(), "main", &session, &opened.operation).expect("cancel own session");
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(lock);
    tokio::time::timeout(Duration::from_millis(500), async {
        wait_until(|| !console.is_active()).await
    })
    .await
    .expect("bounded cleanup");
    assert!(sending.await.expect("sender").is_err());
    assert_eq!(
        retained_status(h.app.handle(), &opened.operation)
            .expect("status")
            .cleanup,
        CleanupOutcome::Confirmed
    );
    wait_until(|| shell_frames(&h).iter().any(|frame| frame.count == 0)).await;
    assert!(
        shell_frames(&h).iter().all(|frame| frame.count == 0),
        "cancelled lock waiter cannot write a command"
    );
    h.shutdown().await;
}
