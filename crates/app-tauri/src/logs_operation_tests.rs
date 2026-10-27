use super::*;
use crate::main_link_service::tests::{wait_until, Harness};
use maggcs_core::runtime::RequestId;

#[derive(Clone)]
pub(crate) struct LogPeer {
    pub entries: Arc<Mutex<Vec<LogEntryInfo>>>,
    pub hold_list: Arc<AtomicBool>,
    pub hold_data: Arc<AtomicBool>,
    pub flood: Arc<AtomicBool>,
    pub recording: Arc<AtomicBool>,
    pub reject_absence: Arc<AtomicBool>,
    pub pxh: Arc<AtomicBool>,
    pub index: Arc<Mutex<Option<String>>>,
    pub lines: Arc<Mutex<Vec<String>>>,
    buffer: Arc<Mutex<Vec<u8>>>,
}
impl Default for LogPeer {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(vec![
                LogEntryInfo {
                    id: 0,
                    size: 360,
                    time_utc: 1_700_000_000,
                },
                LogEntryInfo {
                    id: 1,
                    size: 90,
                    time_utc: 1_700_000_001,
                },
            ])),
            hold_list: Arc::new(AtomicBool::new(false)),
            hold_data: Arc::new(AtomicBool::new(false)),
            flood: Arc::new(AtomicBool::new(false)),
            recording: Arc::new(AtomicBool::new(false)),
            reject_absence: Arc::new(AtomicBool::new(false)),
            pxh: Arc::new(AtomicBool::new(false)),
            index: Arc::new(Mutex::new(None)),
            lines: Arc::new(Mutex::new(Vec::new())),
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
impl LogPeer {
    pub(crate) fn respond(
        &self,
        message: &maggcs_core::mavlink::MavMessage,
        gcs: u8,
        component: u8,
    ) -> Vec<maggcs_core::mavlink::MavMessage> {
        use ::mavlink::common::*;
        use maggcs_core::mavlink::MavMessage;
        match message {
            MavMessage::LOG_REQUEST_LIST(_) if !self.hold_list.load(Ordering::Acquire) => {
                let entries = self.entries.lock();
                if entries.is_empty() {
                    return vec![MavMessage::LOG_ENTRY(LOG_ENTRY_DATA {
                        num_logs: 0,
                        ..Default::default()
                    })];
                }
                entries
                    .iter()
                    .map(|entry| {
                        MavMessage::LOG_ENTRY(LOG_ENTRY_DATA {
                            id: entry.id,
                            size: entry.size,
                            time_utc: entry.time_utc,
                            num_logs: entries.len() as u16,
                            last_log_num: entries.len() as u16 - 1,
                        })
                    })
                    .collect()
            }
            MavMessage::LOG_REQUEST_DATA(request) if !self.hold_data.load(Ordering::Acquire) => {
                let size = self
                    .entries
                    .lock()
                    .iter()
                    .find(|entry| entry.id == request.id)
                    .map(|entry| entry.size)
                    .unwrap_or(0);
                let end = request.ofs.saturating_add(request.count).min(size);
                (request.ofs..end)
                    .step_by(90)
                    .map(|offset| {
                        let count = (end - offset).min(90) as u8;
                        let mut data = [0; 90];
                        for (index, byte) in data[..usize::from(count)].iter_mut().enumerate() {
                            *byte = ((offset as usize + index) % 251) as u8;
                        }
                        MavMessage::LOG_DATA(LOG_DATA_DATA {
                            id: request.id,
                            ofs: offset,
                            count,
                            data,
                        })
                    })
                    .collect()
            }
            MavMessage::LOG_ERASE(_) => {
                self.entries.lock().clear();
                Vec::new()
            }
            MavMessage::SERIAL_CONTROL(frame) if frame.count > 0 => {
                let mut buffer = self.buffer.lock();
                buffer.extend_from_slice(&frame.data[..usize::from(frame.count)]);
                let mut text = String::new();
                while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                    let bytes: Vec<_> = buffer.drain(..=end).collect();
                    let line = String::from_utf8(bytes).expect("fixture UTF-8");
                    let line = line.trim();
                    self.lines.lock().push(line.into());
                    match line {
                        "" | "# maggcs-shell-probe" => text.push_str(if self.pxh.load(Ordering::Acquire) { "pxh>\n" } else { "nsh>\n" }),
                        "logger status" => text.push_str(if self.recording.load(Ordering::Acquire) {
                            "INFO [logger] Running in mode: file\nINFO [logger] Full File Logging Running:\nINFO [logger] Log file: /fs/microsd/log/session/log000.ulg\n"
                        } else { "INFO [logger] Running in mode: file\nINFO [logger] Not logging\n" }),
                        "cat /fs/microsd/logdata.txt" => {
                            let index = self.index.lock().clone().unwrap_or_else(|| self.entries.lock().iter().map(|entry|
                                format!("{} {} /fs/microsd/log/session/log{:03}.ulg\n", entry.time_utc, entry.size, entry.id)).collect());
                            text.push_str(&index);
                        }
                        command if command.starts_with("rm ") => {
                            let mut entries = self.entries.lock(); entries.remove(0);
                            for (index, entry) in entries.iter_mut().enumerate() { entry.id = index as u16; }
                        }
                        command if command.starts_with("ls -l ") => text.push_str(if self.reject_absence.load(Ordering::Acquire) {
                            "still-present.ulg\n"
                        } else { "nsh: ls: stat failed: No such file or directory\n" }),
                        command if command.starts_with("ls -R ") => {
                            text.push_str("/fs/microsd/log:\n");
                            if !self.entries.lock().is_empty() { text.push_str("log000.ulg\n"); }
                        }
                        command if command.starts_with("echo __MAGDEL_") => { text.push_str(&command[5..]); text.push('\n'); }
                        _ => text.push_str("shell-output\n"),
                    }
                }
                text.as_bytes()
                    .chunks(70)
                    .map(|chunk| {
                        let mut data = [0; 70];
                        data[..chunk.len()].copy_from_slice(chunk);
                        MavMessage::SERIAL_CONTROL(SERIAL_CONTROL_DATA {
                            device: SerialControlDev::SERIAL_CONTROL_DEV_SHELL,
                            flags: SerialControlFlag::SERIAL_CONTROL_FLAG_REPLY,
                            count: chunk.len() as u8,
                            data,
                            target_system: gcs,
                            target_component: component,
                            ..Default::default()
                        })
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}

fn attach(h: &Harness, window: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(h.app.handle())
        .targets()
        .expect("targets")
        .remove(0)
        .target
        .expect("target");
    h.state()
        .runtime_clients
        .attach(window, "logs".into(), target)
        .expect("attach")
        .0
}
fn context(session: &RuntimeClientSession) -> LogRequestContext {
    LogRequestContext {
        session: session.clone(),
        request_id: RequestId::fresh().expect("request"),
    }
}
fn install(h: &Harness) {
    let runtime = h
        .state()
        .logs
        .spawn(h.app.handle().clone(), h.handle.clone());
    h.state().set_primary_logs(runtime);
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_prepared_owner_loss_and_idle_old_cancel_have_no_write_authority() {
    let h = Harness::commands().await;
    install(&h);
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    assert!(matches!(
        begin(h.app.handle(), "a", context(&b), LogAction::List, None),
        Err(LogOperationError::Operation {
            error: OperationError::NotOwner
        })
    ));
    let work = begin(h.app.handle(), "a", context(&a), LogAction::List, None).expect("prepare");
    let operation = work.operation().clone();
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("close A");
    assert!(work.run().await.is_err());
    assert!(!h.trace.lock().iter().any(|frame| matches!(
        frame,
        maggcs_core::mavlink::MavMessage::LOG_REQUEST_LIST(_)
            | maggcs_core::mavlink::MavMessage::LOG_REQUEST_END(_)
    )));
    assert!(h
        .state()
        .runtime_clients
        .operations
        .cancel(&b.owner, &operation)
        .is_err());
    let next = begin(h.app.handle(), "b", context(&b), LogAction::List, None).expect("prepare B");
    assert!(h
        .state()
        .runtime_clients
        .operations
        .cancel(&a.owner, &operation)
        .is_ok());
    assert!(next.lease.check().is_ok());
    drop(next);
    wait_until(|| !h.state().logs.is_busy()).await;
    assert!(!h.handle.is_shutdown_requested());
    h.shutdown().await;
}

#[tokio::test]
async fn atomic_local_write_preserves_destination_and_removes_owned_partial_on_rejection() {
    let directory =
        std::env::temp_dir().join(format!("maggcs-log-{}", OperationId::fresh().expect("id")));
    std::fs::create_dir(&directory).expect("directory");
    let destination = directory.join("log.ulg");
    std::fs::write(&destination, b"existing").expect("existing file");
    let result = atomic_write(
        destination.to_string_lossy().into(),
        vec![1; FILE_CHUNK_BYTES * 2],
        OperationId::fresh().expect("id"),
        Arc::new(FileControl::default()),
        || Err(OperationError::Cancelled.into()),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(std::fs::read(&destination).expect("preserved"), b"existing");
    assert_eq!(std::fs::read_dir(&directory).expect("entries").count(), 1);
    atomic_write(
        destination.to_string_lossy().into(),
        b"complete".to_vec(),
        OperationId::fresh().expect("id"),
        Arc::new(FileControl::default()),
        || Ok(()),
    )
    .await
    .expect("durable write");
    assert_eq!(std::fs::read(&destination).expect("complete"), b"complete");
    assert_eq!(std::fs::read_dir(&directory).expect("entries").count(), 1);
    std::fs::remove_dir_all(directory).expect("cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn listed_downloaded_bytes_and_progress_bind_the_exact_owner_and_durable_file() {
    let h = Harness::logs().await;
    install(&h);
    let a = attach(&h, "a");
    let listed = begin(h.app.handle(), "a", context(&a), LogAction::List, None)
        .expect("list")
        .run()
        .await
        .expect("listed");
    let LogResult::Listed { entries } = listed.result else {
        panic!("listed result");
    };
    assert_eq!(entries, *h.log_peer.as_ref().expect("peer").entries.lock());
    let path = std::env::temp_dir().join(format!(
        "maggcs-download-{}.ulg",
        OperationId::fresh().expect("id")
    ));
    let work = begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::Download {
            entry: entries[0].clone(),
            save_path: path.to_string_lossy().into(),
        },
        None,
    )
    .expect("download");
    let operation = work.operation().clone();
    let reply = work.run().await.expect("durable download");
    assert_eq!(reply.operation, operation);
    assert_eq!(
        std::fs::read(&path).expect("saved"),
        (0..360)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>()
    );
    std::fs::remove_file(path).expect("cleanup");
    assert!(!h.state().logs.is_busy());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn exact_cancel_stops_retries_and_old_cancel_cannot_retire_a_later_owner() {
    let h = Harness::logs().await;
    install(&h);
    let peer = h.log_peer.as_ref().expect("peer");
    peer.hold_list.store(true, Ordering::Release);
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    let work = begin(h.app.handle(), "a", context(&a), LogAction::List, None).expect("list");
    let operation = work.operation().clone();
    let task = tauri::async_runtime::spawn(work.run());
    wait_until(|| {
        h.trace
            .lock()
            .iter()
            .any(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_REQUEST_LIST(_)))
    })
    .await;
    assert!(owned_status(h.app.handle(), "b", &b, &operation).is_err());
    h.state()
        .runtime_clients
        .operations
        .cancel(&a.owner, &operation)
        .expect("cancel A");
    assert!(task.await.expect("task").is_err());
    let status = retained_status(h.app.handle(), &operation).expect("status");
    assert_eq!(status.cleanup, CleanupOutcome::Confirmed);
    assert!(h
        .trace
        .lock()
        .iter()
        .any(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_REQUEST_END(_))));
    peer.hold_list.store(false, Ordering::Release);
    let next = begin(h.app.handle(), "b", context(&b), LogAction::List, None).expect("B");
    h.state()
        .runtime_clients
        .operations
        .cancel(&a.owner, &operation)
        .expect("old cancel");
    assert!(next.run().await.is_ok());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nonquiet_cleanup_retains_uncertain_claim_instead_of_accepting_a_new_transfer() {
    let h = Harness::logs().await;
    install(&h);
    let peer = h.log_peer.as_ref().expect("peer");
    peer.hold_list.store(true, Ordering::Release);
    peer.flood.store(true, Ordering::Release);
    let a = attach(&h, "a");
    let work = begin(h.app.handle(), "a", context(&a), LogAction::List, None).expect("list");
    let operation = work.operation().clone();
    let task = tauri::async_runtime::spawn(work.run());
    wait_until(|| {
        h.trace
            .lock()
            .iter()
            .any(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_REQUEST_LIST(_)))
    })
    .await;
    h.state()
        .runtime_clients
        .operations
        .cancel(&a.owner, &operation)
        .expect("cancel");
    assert!(matches!(
        task.await.expect("task"),
        Err(LogOperationError::ReconciliationRequired { .. })
    ));
    assert_eq!(
        retained_status(h.app.handle(), &operation)
            .expect("status")
            .cleanup,
        CleanupOutcome::Uncertain
    );
    assert!(begin(h.app.handle(), "a", context(&a), LogAction::List, None).is_err());
    peer.flood.store(false, Ordering::Release);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deletion_rechecks_identity_logger_and_readback_and_erase_confirms_empty_directory() {
    let h = Harness::logs().await;
    install(&h);
    let a = attach(&h, "a");
    let console = Arc::new(crate::console_service::ConsoleState::default());
    let entry = h.log_peer.as_ref().expect("peer").entries.lock()[0].clone();
    let removed = begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::Delete { entry },
        Some(console.clone()),
    )
    .expect("delete")
    .run()
    .await
    .expect("deleted");
    assert!(
        matches!(removed.result, LogResult::Deleted { entries } if entries.len() == 1 && entries[0].id == 0)
    );
    let erased = begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::Erase,
        Some(console),
    )
    .expect("erase")
    .run()
    .await
    .expect("verified erase");
    assert_eq!(erased.result, LogResult::Erased);
    assert!(h.log_peer.as_ref().expect("peer").entries.lock().is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn armed_airborne_recording_posix_and_stale_paths_send_zero_destructive_writes() {
    let h = Harness::logs().await;
    install(&h);
    let a = attach(&h, "a");
    let console = Arc::new(crate::console_service::ConsoleState::default());
    let peer = h.log_peer.as_ref().expect("peer");
    let entry = peer.entries.lock()[0].clone();
    for scenario in ["recording", "pxh", "unsafe", "stale"] {
        peer.recording
            .store(scenario == "recording", Ordering::Release);
        peer.pxh.store(scenario == "pxh", Ordering::Release);
        *peer.index.lock() = match scenario {
            "unsafe" => Some(format!(
                "{} {} /fs/microsd/log/../bad.ulg\n",
                entry.time_utc, entry.size
            )),
            "stale" => Some(format!(
                "{} {} /fs/microsd/log/session/log000.ulg\n",
                entry.time_utc + 1,
                entry.size
            )),
            _ => None,
        };
        let work = begin(
            h.app.handle(),
            "a",
            context(&a),
            LogAction::Delete {
                entry: entry.clone(),
            },
            Some(console.clone()),
        )
        .expect("admission");
        assert!(work.run().await.is_err(), "{scenario}");
    }
    let commands = h.command_peer.as_ref().expect("commands");
    commands.armed.store(true, Ordering::Release);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::Erase,
        Some(console.clone())
    )
    .is_err());
    commands.armed.store(false, Ordering::Release);
    commands.airborne.store(true, Ordering::Release);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::Erase,
        Some(console)
    )
    .is_err());
    assert!(!peer.lines.lock().iter().any(|line| line.starts_with("rm ")));
    assert!(!h
        .trace
        .lock()
        .iter()
        .any(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_ERASE(_))));
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn native_close_and_target_replacement_retire_only_the_recorded_log_operation() {
    let h = Harness::logs().await;
    install(&h);
    let peer = h.log_peer.as_ref().expect("peer");
    peer.hold_list.store(true, Ordering::Release);
    let old = attach(&h, "main");
    let work = begin(h.app.handle(), "main", context(&old), LogAction::List, None).expect("list");
    let operation = work.operation().clone();
    let task = tauri::async_runtime::spawn(work.run());
    wait_until(|| {
        h.trace
            .lock()
            .iter()
            .any(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_REQUEST_LIST(_)))
    })
    .await;
    h.state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("close");
    assert!(task.await.expect("task").is_err());
    assert_eq!(
        retained_status(h.app.handle(), &operation)
            .expect("status")
            .cleanup,
        CleanupOutcome::Confirmed
    );
    let fresh = attach(&h, "main");
    let work = begin(
        h.app.handle(),
        "main",
        context(&fresh),
        LogAction::List,
        None,
    )
    .expect("fresh list");
    let current = work.operation().clone();
    let task = tauri::async_runtime::spawn(work.run());
    wait_until(|| {
        h.trace
            .lock()
            .iter()
            .filter(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_REQUEST_LIST(_)))
            .count()
            >= 2
    })
    .await;
    h.handle.reconnect().await.expect("replace generation");
    assert!(matches!(
        task.await.expect("task"),
        Err(LogOperationError::ReconciliationRequired { .. })
    ));
    let status = retained_status(h.app.handle(), &current).expect("status");
    assert!(matches!(
        status.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert!(owned_status(h.app.handle(), "main", &old, &current).is_err());
    h.shutdown().await;
}

#[tokio::test]
async fn cancellation_before_rename_removes_partial_and_never_replaces_existing_file() {
    let directory =
        std::env::temp_dir().join(format!("maggcs-file-{}", OperationId::fresh().expect("id")));
    std::fs::create_dir(&directory).expect("directory");
    let path = directory.join("old.ulg");
    std::fs::write(&path, b"old").expect("old");
    let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = checks.clone();
    let result = atomic_write(
        path.to_string_lossy().into(),
        vec![1; FILE_CHUNK_BYTES],
        OperationId::fresh().expect("id"),
        Arc::new(FileControl::default()),
        move || {
            if calls.fetch_add(1, Ordering::AcqRel) >= 2 {
                Err(OperationError::Cancelled.into())
            } else {
                Ok(())
            }
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(std::fs::read(path).expect("preserved"), b"old");
    assert_eq!(std::fs::read_dir(&directory).expect("entries").count(), 1);
    std::fs::remove_dir_all(directory).expect("cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn silence_is_empty_only_after_owned_directory_proof_and_delete_failure_needs_readback() {
    let h = Harness::logs().await;
    install(&h);
    let a = attach(&h, "a");
    let console = Arc::new(crate::console_service::ConsoleState::default());
    let peer = h.log_peer.as_ref().expect("peer");
    let entry = peer.entries.lock()[0].clone();
    peer.reject_absence.store(true, Ordering::Release);
    let removed = begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::Delete { entry },
        Some(console.clone()),
    )
    .expect("delete")
    .run()
    .await;
    assert!(matches!(
        removed,
        Err(LogOperationError::ReconciliationRequired { .. })
    ));
    assert!(begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::Erase,
        Some(console.clone())
    )
    .is_err());
    begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::List,
        Some(console.clone()),
    )
    .expect("readback")
    .run()
    .await
    .expect("verified list");
    assert!(h.state().logs.reconciliation.lock().is_none());
    peer.entries.lock().clear();
    peer.hold_list.store(true, Ordering::Release);
    let empty = begin(
        h.app.handle(),
        "a",
        context(&a),
        LogAction::List,
        Some(console),
    )
    .expect("empty list")
    .run()
    .await
    .expect("directory proof");
    assert_eq!(
        empty.result,
        LogResult::Listed {
            entries: Vec::new()
        }
    );
    assert!(peer
        .lines
        .lock()
        .iter()
        .any(|line| line == "ls -R /fs/microsd/log"));
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lifecycle_cancels_download_actor_before_worker_and_primary_io() {
    let h = Harness::logs().await;
    install(&h);
    let session = attach(&h, "main");
    let peer = h.log_peer.as_ref().expect("peer");
    peer.hold_data.store(true, Ordering::Release);
    let entry = peer.entries.lock()[0].clone();
    let work = begin(
        h.app.handle(),
        "main",
        context(&session),
        LogAction::Download {
            entry,
            save_path: "never-written.ulg".into(),
        },
        None,
    )
    .expect("download");
    let task = tauri::async_runtime::spawn(work.run());
    wait_until(|| {
        h.trace
            .lock()
            .iter()
            .any(|frame| matches!(frame, maggcs_core::mavlink::MavMessage::LOG_REQUEST_DATA(_)))
    })
    .await;
    assert!(matches!(
        h.state()
            .logs
            .stop_operations(h.app.handle(), Duration::from_secs(1))
            .await,
        StopOutcome::Stopped { .. }
    ));
    assert!(task.await.expect("task").is_err());
    assert!(!h.handle.is_shutdown_requested());
    assert!(!Path::new("never-written.ulg").exists());
    h.shutdown().await;
}
