//! MAVLink console (like QGC's MAVLink Console): a PX4 NuttShell session
//! over `SERIAL_CONTROL` (device = `SERIAL_CONTROL_DEV_SHELL`), no serial
//! port configuration needed — the flight stack multiplexes the shell onto
//! the MAVLink bus (SITL and real hardware alike).
//!
//! Commands are framed one per `SERIAL_CONTROL` message (command + `\n`),
//! `flags = RESPOND | EXCLUSIVE`. Shell output arrives as reply frames the
//! watcher forwards to the frontend as `console_output` events (each frame
//! is a raw chunk; the UI reassembles lines).

use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use maggcs_core::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::mavlink::MavMessage;

use crate::state::AppState;

/// `SERIAL_CONTROL` message id (used by the message route).
const SERIAL_CONTROL_ID: u32 = 126;
/// `SERIAL_CONTROL_DEV_SHELL` — the flight stack's MAVLink NuttShell.
const DEV_SHELL: ::mavlink::common::SerialControlDev =
    ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_SHELL;

/// `SERIAL_CONTROL` flags for an interactive shell session: ask for a reply
/// and hold the shell exclusively. (Not a `const` — bitflags `|` is not a
/// const operator.)
fn shell_flags() -> ::mavlink::common::SerialControlFlag {
    ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_RESPOND
        | ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_EXCLUSIVE
}

/// One console watcher: a route subscription on a specific connection.
pub struct ConsoleWatcher {
    connection: ConnectionHandle,
    task: tauri::async_runtime::JoinHandle<()>,
}

/// Process-wide console state: at most one active watcher (on the primary
/// link). Re-attached lazily when the link changes.
#[derive(Default)]
pub struct ConsoleState {
    watcher: Mutex<Option<ConsoleWatcher>>,
}

impl ConsoleState {
    /// Ensure a watcher is running on the *current* primary link: rebuild it
    /// only when the link handle changed (or the task died). The old task
    /// exits on its own when its event bus closes (link swap / disconnect).
    async fn attach(self: &Arc<Self>, app: &AppHandle, state: &AppState) {
        let Some(connection) = state.connection() else {
            self.detach().await;
            return;
        };
        let should_rebuild = {
            let guard = self.watcher.lock().await;
            guard
                .as_ref()
                .map(|w| w.connection.handle_id() != connection.handle_id())
                .unwrap_or(true)
        };
        if !should_rebuild {
            return;
        }
        self.detach().await;
        let app = app.clone();
        let conn = connection.clone();
        let task = tauri::async_runtime::spawn(async move {
            let mut events = conn.subscribe_route(MessageRoute::messages(&[SERIAL_CONTROL_ID]));
            loop {
                match events.recv().await {
                    Ok(ConnectionEvent::Message(env)) => {
                        if let MavMessage::SERIAL_CONTROL(msg) = env.message {
                            let count = msg.count.min(70) as usize;
                            if count == 0 {
                                continue;
                            }
                            let chunk = String::from_utf8_lossy(&msg.data[..count]).into_owned();
                            if app.emit("console_output", chunk).is_err() {
                                return; // UI gone
                            }
                        }
                    }
                    Ok(_) => {} // lifecycle events pass the route unchanged
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
        });
        *self.watcher.lock().await = Some(ConsoleWatcher { connection, task });
    }

    /// Stop the current watcher, if any.
    async fn detach(&self) {
        let mut guard = self.watcher.lock().await;
        if let Some(w) = guard.take() {
            w.task.abort();
        }
    }
}

/// Frame one command as a `SERIAL_CONTROL` payload: command bytes plus the
/// `\n` NuttShell line terminator, clamped to the 70-byte payload. The
/// shell's line terminator is part of the command, so a command at the
/// payload limit loses its newline rather than its tail.
fn shell_frame(command: &str) -> ::mavlink::common::SERIAL_CONTROL_DATA {
    let mut line = command.as_bytes().to_vec();
    line.push(b'\n');
    let count = line.len().min(70);
    let mut data = [0u8; 70];
    data[..count].copy_from_slice(&line[..count]);
    ::mavlink::common::SERIAL_CONTROL_DATA {
        device: DEV_SHELL,
        flags: shell_flags(),
        timeout: 0,
        baudrate: 0,
        count: count as u8,
        data,
        ..Default::default()
    }
}

/// Send one console command to the FC's NuttShell (MAVLink console). Frames
/// the command as a `SERIAL_CONTROL` message on the primary link; the shell
/// echoes the output back as reply frames (the watcher forwards them).
///
/// # Errors
///
/// A stringified error when there is no live link.
#[tauri::command]
pub async fn console_send(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    console: tauri::State<'_, Arc<ConsoleState>>,
    command: String,
) -> Result<(), String> {
    console.attach(&app, &state).await;
    let connection = state
        .connection()
        .ok_or_else(|| "no live link".to_string())?;

    let payload = shell_frame(&command);
    connection
        .send(MavMessage::SERIAL_CONTROL(payload))
        .await
        .map_err(|e| format!("console send failed: {e}"))
}

/// Stop the console session (drop the exclusive shell claim). The watcher
/// stays attached so a later command re-establishes it.
#[tauri::command]
pub async fn console_close(
    state: tauri::State<'_, AppState>,
    console: tauri::State<'_, Arc<ConsoleState>>,
) -> Result<(), String> {
    let connection = state
        .connection()
        .ok_or_else(|| "no live link".to_string())?;
    let payload = ::mavlink::common::SERIAL_CONTROL_DATA {
        device: DEV_SHELL,
        flags: ::mavlink::common::SerialControlFlag::empty(),
        timeout: 0,
        baudrate: 0,
        count: 0,
        data: [0u8; 70],
        ..Default::default()
    };
    connection
        .send(MavMessage::SERIAL_CONTROL(payload))
        .await
        .map_err(|e| format!("console close failed: {e}"))?;
    console.detach().await;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use ::mavlink::common::{SerialControlDev, SerialControlFlag};

    /// A short command frames verbatim with the trailing newline; the
    /// unused payload tail stays zeroed.
    #[test]
    fn shell_frame_wraps_the_command_with_a_newline() {
        let f = shell_frame("ver");
        assert_eq!(f.device, SerialControlDev::SERIAL_CONTROL_DEV_SHELL);
        assert_eq!(
            f.flags,
            SerialControlFlag::SERIAL_CONTROL_FLAG_RESPOND
                | SerialControlFlag::SERIAL_CONTROL_FLAG_EXCLUSIVE
        );
        assert_eq!(f.count, 4);
        assert_eq!(&f.data[..4], b"ver\n");
        assert!(f.data[4..].iter().all(|&b| b == 0));
        assert_eq!(f.timeout, 0);
        assert_eq!(f.baudrate, 0);
    }

    /// A command at the payload limit is clamped to 70 bytes — the newline
    /// is dropped, never the command tail (a truncated tail would run a
    /// different command on the shell).
    #[test]
    fn shell_frame_clamps_to_the_payload_keeping_the_command_head() {
        let long = "x".repeat(120);
        let f = shell_frame(&long);
        assert_eq!(f.count, 70);
        assert_eq!(&f.data[..69], &[b'x'; 69]);
        assert_eq!(f.data[69], b'x', "tail kept, newline dropped");
    }
}
