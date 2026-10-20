//! Log download service (issues O1 / operator request): runs the core
//! `LogProtocol` task on the primary link (spawned on connect like the
//! command/params services), forwards events onto `logs_event`, and
//! persists finished downloads through a save-dialog path.

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
    last_error: std::sync::Mutex<Option<String>>,
    last_list: std::sync::Mutex<Option<Vec<maggcs_core::logs::LogEntryInfo>>>,
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
    /// Spawn a protocol runtime bound to one active link.
    pub fn spawn<R: Runtime>(
        self: &Arc<Self>,
        app: AppHandle<R>,
        handle: ConnectionHandle,
    ) -> LogsRuntime {
        *self.last_error.lock().expect("logs err lock") = None;
        *self.last_list.lock().expect("logs list lock") = None;
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<LogCommand>(4);

        let mut route: RoutedEvents = handle.subscribe_log_messages();
        let state = Arc::clone(self);
        let task = tauri::async_runtime::spawn(async move {
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
                    }
                    msg = route.recv() => {
                        let Some(m) = msg_ok(msg) else { return };
                        let (events, frames) = match protocol.handle(&m.message) {
                            Ok(v) => v,
                            Err(e) => {
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

// Non-Debug fields are behind the Mutexes; a manual Debug keeps State usable
// in AppState's derive.
impl std::fmt::Debug for LogsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogsState").finish()
    }
}

/// A `RoutedEvents` recv result: `None` = the link bus closed.
fn msg_ok(
    msg: Result<ConnectionEvent, tokio::sync::broadcast::error::RecvError>,
) -> Option<maggcs_core::mavlink::MessageEnvelope> {
    match msg {
        Ok(ConnectionEvent::Message(env)) => Some(*env),
        Ok(_) => None,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
        Err(_) => None,
    }
}

/// Request the FC's log list.
///
/// # Errors
///
/// "not connected" when the link is down.
#[tauri::command]
pub async fn logs_request_list(state: State<'_, AppState>) -> Result<(), String> {
    let Some(tx) = sender(&state) else {
        return Err("not connected".into());
    };
    tx.send(LogCommand::List)
        .await
        .map_err(|_| "service stopped".to_string())
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
    let Some(tx) = sender(&state) else {
        return Err("not connected".into());
    };
    tx.send(LogCommand::Download {
        id,
        size,
        save_path: path.display().to_string(),
    })
    .await
    .map_err(|_| "service stopped".to_string())
}

/// Cancel any running list/download.
#[tauri::command]
pub async fn logs_cancel(state: State<'_, AppState>) -> Result<(), String> {
    let Some(tx) = sender(&state) else {
        return Err("not connected".into());
    };
    tx.send(LogCommand::Cancel)
        .await
        .map_err(|_| "service stopped".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
