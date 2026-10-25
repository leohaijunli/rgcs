//! Log download service (issues O1 / operator request): runs the core
//! `LogProtocol` task on the primary link (spawned on connect like the
//! command/params services), forwards events onto `logs_event`, and
//! persists finished downloads through a save-dialog path.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use maggcs_core::lifecycle::StopOutcome;
use maggcs_core::logs::protocol::{LogEvent, LogProtocol};
use maggcs_core::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::RoutedEvents;

use crate::state::AppState;

#[derive(Default)]
pub struct LogsState {
    busy: AtomicBool,
    last_error: std::sync::Mutex<Option<String>>,
    last_list: std::sync::Mutex<Option<Vec<maggcs_core::logs::LogEntryInfo>>>,
}

struct ClearBusy(Arc<LogsState>);
impl Drop for ClearBusy {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

/// Runtime owned by one active link; its task is joined before link shutdown.
pub struct LogsRuntime {
    commands: Option<tokio::sync::mpsc::Sender<LogCommand>>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl LogsRuntime {
    pub(crate) fn sender(&self) -> Option<tokio::sync::mpsc::Sender<LogCommand>> {
        self.commands.clone()
    }

    /// Close the command channel and wait for the log protocol task to exit.
    pub async fn stop(mut self, deadline: Duration) -> StopOutcome {
        let started = Instant::now();
        self.commands.take();
        let Some(mut task) = self.task.take() else {
            return StopOutcome::NotRunning;
        };
        match tokio::time::timeout(deadline, &mut task).await {
            Ok(Ok(())) => StopOutcome::Stopped {
                elapsed_ms: elapsed_ms(started),
            },
            Ok(Err(error)) => StopOutcome::Failed {
                message: format!("log service task failed: {error}"),
            },
            Err(_) => {
                task.abort();
                let _ = task.await;
                StopOutcome::TimedOut {
                    elapsed_ms: elapsed_ms(started),
                }
            }
        }
    }
}

impl Drop for LogsRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

pub(crate) enum LogCommand {
    List,
    Download {
        id: u16,
        size: u32,
        save_path: String,
    },
    Cancel,
}

/// Serializable event payloads (`logs_event`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LogsEventPayload {
    #[serde(rename_all = "snake_case")]
    List {
        entries: Vec<maggcs_core::logs::LogEntryInfo>,
    },
    #[serde(rename_all = "snake_case")]
    Progress { id: u16, fetched: u32, total: u32 },
    /// The log finished downloading and was written to `path`.
    #[serde(rename_all = "snake_case")]
    Downloaded { id: u16, path: String },
    #[serde(rename_all = "snake_case")]
    Failed { message: String },
}

impl LogsState {
    pub(crate) fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Spawn a protocol runtime bound to one active link.
    pub fn spawn<R: Runtime>(
        self: &Arc<Self>,
        app: AppHandle<R>,
        handle: ConnectionHandle,
    ) -> LogsRuntime {
        *self.last_error.lock().expect("logs err lock") = None;
        *self.last_list.lock().expect("logs list lock") = None;
        self.busy.store(false, Ordering::Release);
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<LogCommand>(4);

        let mut route: RoutedEvents = handle.subscribe_log_messages();
        let state = Arc::clone(self);
        let task = tauri::async_runtime::spawn(async move {
            let _clear_busy = ClearBusy(state.clone());
            let mut protocol = LogProtocol::new(1, 1);
            let mut save_paths: std::collections::HashMap<u16, String> =
                std::collections::HashMap::new();
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(200));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let emit_failed = |state: &LogsState, app: &AppHandle<R>, message: String| {
                *state.last_error.lock().expect("logs err lock") = Some(message.clone());
                let _ = app.emit("logs_event", &LogsEventPayload::Failed { message });
            };

            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        let Some(cmd) = cmd else { return };
                        let frames = match cmd {
                            LogCommand::List => protocol.begin_list(),
                            LogCommand::Download { id, size, save_path } => {
                                save_paths.insert(id, save_path);
                                protocol.begin_download(id, size)
                            }
                            LogCommand::Cancel => protocol.cancel(),
                        };
                        for f in frames {
                            if handle.send(f).await.is_err() { return; }
                        }
                        if !protocol.is_busy() { state.busy.store(false, Ordering::Release); }
                    }
                    msg = route.recv() => {
                        let envelope = match classify_route(msg) {
                            RouteStep::Message(envelope) => envelope,
                            // Ordinary lifecycle reports and broadcast lag are
                            // not download failures; keep serving the link.
                            RouteStep::Skip => continue,
                            RouteStep::Closed => return,
                        };
                        let was_busy = protocol.is_busy();
                        let (events, frames) = match protocol.handle(
                            &envelope.message,
                            std::time::Instant::now(),
                        ) {
                            Ok(v) => v,
                            Err(e) => {
                                protocol.cancel();
                                if was_busy { state.busy.store(false, Ordering::Release); }
                                emit_failed(&state, &app, e.to_string());
                                continue;
                            }
                        };
                        for f in frames {
                            if handle.send(f).await.is_err() { return; }
                        }
                        for e in events {
                            match e {
                                LogEvent::List { entries } => {
                                    *state.last_list.lock().expect("logs list lock") = Some(entries.clone());
                                    let _ = app.emit("logs_event", &LogsEventPayload::List { entries });
                                }
                                LogEvent::Progress { id, fetched, total } => {
                                    let _ = app.emit("logs_event", &LogsEventPayload::Progress { id, fetched, total });
                                }
                                LogEvent::Downloaded { id, bytes } => {
                                    let path = save_paths.get(&id).cloned()
                                        .unwrap_or_else(|| format!("log{id}.ulg"));
                                    let payload = match std::fs::write(&path, &bytes) {
                                        Ok(()) => LogsEventPayload::Downloaded { id, path },
                                        Err(e) => LogsEventPayload::Failed { message: format!("write {}: {e}", path) },
                                    };
                                    let _ = app.emit("logs_event", &payload);
                                }
                            }
                        }
                        if was_busy && !protocol.is_busy() { state.busy.store(false, Ordering::Release); }
                    }
                    _ = tick.tick() => {
                        match protocol.tick(std::time::Instant::now()) {
                            Ok(frames) => {
                                for f in frames {
                                    if handle.send(f).await.is_err() { return; }
                                }
                            }
                            Err(e) => {
                                let _ = protocol.cancel();
                                state.busy.store(false, Ordering::Release);
                                emit_failed(&state, &app, e.to_string());
                            }
                        }
                    }
                }
            }
        });
        LogsRuntime {
            commands: Some(cmd_tx),
            task: Some(task),
        }
    }
}

fn sender(state: &AppState) -> Option<tokio::sync::mpsc::Sender<LogCommand>> {
    state.primary_logs_sender()
}

async fn submit(state: &AppState, command: LogCommand) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    crate::main_link_service::ensure_primary_tools_available(state)?;
    let tx = sender(state).ok_or("not connected")?;
    let starts = !matches!(command, LogCommand::Cancel);
    if !starts && !state.logs.is_busy() {
        return Ok(());
    }
    if starts && state.logs.busy.swap(true, Ordering::AcqRel) {
        return Err("a main-window log operation is already active; cancel it first".into());
    }
    if tx.send(command).await.is_err() {
        state.logs.busy.store(false, Ordering::Release);
        return Err("service stopped".into());
    }
    Ok(())
}

// Non-Debug fields are behind the Mutexes; a manual Debug keeps State usable
// in AppState's derive.
impl std::fmt::Debug for LogsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogsState").finish()
    }
}

/// What one `RoutedEvents` wake means for the download task. Only a closed
/// bus ends the task; a lagged subscriber simply misses frames, and link
/// lifecycle reports are irrelevant to the protocol.
pub(crate) enum RouteStep {
    Message(Box<maggcs_core::mavlink::MessageEnvelope>),
    Skip,
    Closed,
}

pub(crate) fn classify_route(
    msg: Result<ConnectionEvent, tokio::sync::broadcast::error::RecvError>,
) -> RouteStep {
    match msg {
        Ok(ConnectionEvent::Message(envelope)) => RouteStep::Message(envelope),
        Ok(_) => RouteStep::Skip,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => RouteStep::Skip,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => RouteStep::Closed,
    }
}

/// Request the FC's log list.
///
/// # Errors
///
/// "not connected" when the link is down.
#[tauri::command]
pub async fn logs_request_list(state: State<'_, AppState>) -> Result<(), String> {
    submit(&state, LogCommand::List).await
}

/// Download one log into a save-dialog path.
///
/// # Errors
///
/// "not connected" when the link is down; a cancelled save dialog is not an
/// error (returns Ok without starting).
#[tauri::command]
pub async fn logs_download(
    id: u16,
    size: u32,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    use tauri_plugin_dialog::DialogExt;
    let path = app
        .dialog()
        .file()
        .add_filter("ULog", &["ulg"])
        .set_file_name(format!("log{id}.ulg"))
        .blocking_save_file()
        .and_then(|p| p.into_path().ok());
    let Some(path) = path else { return Ok(()) };
    submit(
        &state,
        LogCommand::Download {
            id,
            size,
            save_path: path.display().to_string(),
        },
    )
    .await
}

/// Cancel any running list/download.
#[tauri::command]
pub async fn logs_cancel(state: State<'_, AppState>) -> Result<(), String> {
    submit(&state, LogCommand::Cancel).await
}

// ---------------------------------------------------------------------------
// SD-log deletion (primary link). Both commands confirm over the primary
// link's NuttShell, sharing the MAVLink console's exclusive shell.
// ---------------------------------------------------------------------------

/// Refuse SD-log deletion while the FC logger is writing. A fresh observation
/// is required because a cached state can be stale after a reconnect.
async fn ensure_primary_logger_not_recording(
    console: &crate::console_service::ConsoleState,
    app: &AppHandle,
    state: &AppState,
) -> Result<(), String> {
    let output = crate::console_service::exec(console, app, state, "logger status").await?;
    let status = maggcs_core::logs::parse_logger_status(&output).map_err(|e| e.to_string())?;
    if status.file_logging {
        return Err(
            "the FC logger is recording; stop logging on the FC before deleting logs".into(),
        );
    }
    Ok(())
}

/// SD-log deletion drives the FC's NuttShell (`cat`, `rm`, `ls`, `echo`).
/// PX4's POSIX shell (`pxh`, used by SITL) lacks all of those, so a marker
/// exchange would only ever time out. Refuse it with an actionable message
/// first; NuttShell (or an inconclusive probe) falls through to the
/// logger-recording gate.
async fn ensure_shell_supports_delete(
    console: &crate::console_service::ConsoleState,
    app: &AppHandle,
    state: &AppState,
) -> Result<(), String> {
    if crate::console_service::probe_shell(console, app, state).await?
        == crate::px4_shell::ShellKind::Pxh
    {
        return Err(
            "SD-log deletion needs a NuttX flight controller; this link's shell is PX4's POSIX shell (SITL)"
                .into(),
        );
    }
    ensure_primary_logger_not_recording(console, app, state).await
}

fn publish_primary_log_list<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    entries: Vec<maggcs_core::logs::LogEntryInfo>,
) {
    *state.logs.last_list.lock().expect("logs list lock") = Some(entries.clone());
    *state.logs.last_error.lock().expect("logs err lock") = None;
    let _ = app.emit("logs_event", &LogsEventPayload::List { entries });
}

/// Request the primary link's log list and wait for a completed `List` event.
/// `Ok(None)` means the FC stayed silent (flat `num_logs == 0`), so the caller
/// must verify emptiness over the shell instead.
async fn primary_refresh_list(
    connection: &ConnectionHandle,
) -> Result<Option<Vec<maggcs_core::logs::LogEntryInfo>>, String> {
    let mut route = connection.subscribe_log_messages();
    let mut protocol = LogProtocol::new(
        connection.config().target_system_id,
        connection.config().target_component_id,
    );
    for frame in protocol.begin_list() {
        connection
            .send_confirmed(frame)
            .await
            .map_err(|error| error.to_string())?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        match tokio::time::timeout(remaining, route.recv()).await {
            Err(_) => return Ok(None),
            Ok(Ok(ConnectionEvent::Message(envelope))) => {
                match protocol.handle(&envelope.message, Instant::now()) {
                    Ok((events, frames)) => {
                        for frame in frames {
                            let _ = connection.send(frame).await;
                        }
                        for event in events {
                            if let LogEvent::List { entries } = event {
                                return Ok(Some(entries));
                            }
                        }
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => return Err("log list failed: the connection closed".into()),
        }
    }
}

/// Shell-verify the SD log directory is empty after a delete that left no
/// protocol list.
async fn confirm_directory_empty(
    console: &crate::console_service::ConsoleState,
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, String> {
    let output = crate::console_service::exec(
        console,
        app,
        state,
        &format!("ls -R {}", crate::px4_shell::LOG_DIRECTORY),
    )
    .await?;
    crate::px4_shell::parse_empty_log_directory(&output)
}

/// Permanently delete every SD log on the primary link's FC. The UI performs
/// two confirmations; this command additionally requires a fresh observation
/// that the logger is not recording, then confirms the empty directory.
#[tauri::command]
pub async fn logs_erase_all(
    app: AppHandle,
    state: State<'_, AppState>,
    console: State<'_, Arc<crate::console_service::ConsoleState>>,
) -> Result<Vec<maggcs_core::logs::LogEntryInfo>, String> {
    let _ops = state.ops().lock().await;
    let connection = state
        .connection()
        .ok_or_else(|| "not connected".to_string())?;
    ensure_shell_supports_delete(&console, &app, &state).await?;
    let mut protocol = LogProtocol::new(
        connection.config().target_system_id,
        connection.config().target_component_id,
    );
    for frame in protocol.begin_erase() {
        connection
            .send_confirmed(frame)
            .await
            .map_err(|error| error.to_string())?;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if confirm_directory_empty(&console, &app, &state).await? {
            publish_primary_log_list(&app, &state, Vec::new());
            tracing::info!("all SD logs deleted and empty directory confirmed");
            return Ok(Vec::new());
        }
        if Instant::now() >= deadline {
            let entries = primary_refresh_list(&connection).await?.unwrap_or_default();
            publish_primary_log_list(&app, &state, entries);
            return Err(
                "the FC has not confirmed deletion of every SD log; inspect the refreshed list"
                    .into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Permanently delete one SD log by id on the primary link's FC. The id comes
/// from the current list; `logdata.txt` (id order) maps it to a safe path, and
/// the entry's identity is re-checked before removal.
#[tauri::command]
pub async fn logs_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    console: State<'_, Arc<crate::console_service::ConsoleState>>,
    id: u16,
    size: u32,
    time_utc: u32,
) -> Result<Vec<maggcs_core::logs::LogEntryInfo>, String> {
    let _ops = state.ops().lock().await;
    let connection = state
        .connection()
        .ok_or_else(|| "not connected".to_string())?;
    ensure_shell_supports_delete(&console, &app, &state).await?;
    let expected = maggcs_core::logs::LogEntryInfo { id, size, time_utc };
    let listing =
        crate::console_service::exec(&console, &app, &state, "cat /fs/microsd/logdata.txt").await?;
    let path = crate::px4_shell::log_path_from_listing(&listing, &expected)?;
    let output =
        crate::console_service::exec(&console, &app, &state, &format!("rm {path}")).await?;
    if let Some(error) = crate::px4_shell::command_error(&output) {
        return Err(format!("SD log deletion failed: {error}"));
    }
    let output =
        crate::console_service::exec(&console, &app, &state, &format!("ls -l {path}")).await?;
    if !output.lines().any(|line| {
        line.starts_with("nsh: ls: stat failed:") && line.contains("No such file or directory")
    }) {
        return Err("the FC has not confirmed the SD log was removed; refresh the list".into());
    }
    // Re-list so ids renumber exactly as PX4 assigns them after a deletion.
    let entries = match primary_refresh_list(&connection).await? {
        Some(entries) => entries,
        None => {
            if confirm_directory_empty(&console, &app, &state).await? {
                Vec::new()
            } else {
                return Err(
                    "the FC did not return the refreshed log list; refresh the list".into(),
                );
            }
        }
    };
    publish_primary_log_list(&app, &state, entries.clone());
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maggcs_core::mavlink::{MavMessage, MessageEnvelope};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn stopping_link_owned_runtime_closes_and_joins_its_task() {
        let (commands, mut receiver) = tokio::sync::mpsc::channel::<LogCommand>(1);
        let exited = Arc::new(AtomicBool::new(false));
        let task_exited = Arc::clone(&exited);
        let task = tauri::async_runtime::spawn(async move {
            let _ = receiver.recv().await;
            task_exited.store(true, Ordering::SeqCst);
        });
        let runtime = LogsRuntime {
            commands: Some(commands),
            task: Some(task),
        };

        let outcome = runtime.stop(Duration::from_secs(1)).await;

        assert!(matches!(outcome, StopOutcome::Stopped { .. }));
        assert!(exited.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn dropping_a_replaced_runtime_stops_its_task() {
        struct Signal(Arc<AtomicBool>);
        impl Drop for Signal {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let signal = Signal(Arc::clone(&dropped));
        let (commands, _receiver) = tokio::sync::mpsc::channel::<LogCommand>(1);
        let task = tauri::async_runtime::spawn(async move {
            let _signal = signal;
            std::future::pending::<()>().await;
        });
        let runtime = LogsRuntime {
            commands: Some(commands),
            task: Some(task),
        };

        // Replacement drops the previous runtime without a graceful stop.
        drop(runtime);
        for _ in 0..50 {
            if dropped.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(
            dropped.load(Ordering::SeqCst),
            "the replaced runtime's task must be aborted"
        );
    }

    #[test]
    fn route_classifier_skips_lag_and_lifecycle_events() {
        assert!(matches!(
            classify_route(Ok(ConnectionEvent::HeartbeatRestored)),
            RouteStep::Skip
        ));
        assert!(matches!(
            classify_route(Err(tokio::sync::broadcast::error::RecvError::Lagged(3))),
            RouteStep::Skip
        ));
        assert!(matches!(
            classify_route(Err(tokio::sync::broadcast::error::RecvError::Closed)),
            RouteStep::Closed
        ));

        let envelope = MessageEnvelope {
            header: Default::default(),
            message: MavMessage::HEARTBEAT(Default::default()),
            received_at: std::time::Instant::now(),
        };
        assert!(matches!(
            classify_route(Ok(ConnectionEvent::Message(Box::new(envelope)))),
            RouteStep::Message(_)
        ));
    }
}
