//! Target-shell SD-file capture control (Task 6).
//!
//! The target PX4's MAVLink shell is a link-scoped service on the *dedicated*
//! motor-test target, not the primary link. The pure command/response state
//! machine lives in `core::logs::capture`; this module owns the
//! `SERIAL_CONTROL` transport, the Tauri commands and the event fan-out.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Runtime, State};

use maggcs_core::logs::capture::{
    CapturePhase, SdCapture, SdCaptureCommand, SdCaptureError, SdCaptureStatus, SdCaptureStep,
};
use maggcs_core::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::mavlink::MavMessage;
use maggcs_core::params::model::ParamValue;
use maggcs_core::params::ParamService;

use crate::motor_test_state::{
    ActuatorCapture, ActuatorCaptureRuntime, CaptureRequest, MotorTestTargetRef,
    TARGET_COMPONENT_ID,
};
use crate::state::AppState;

const SERIAL_CONTROL_ID: u32 = 126;
const SHELL_PAYLOAD: usize = 70;
const CAPTURE_TICK: Duration = Duration::from_millis(100);
const CAPTURE_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const SHELL_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const SHELL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const SHELL_MAX_OUTPUT: usize = 256 * 1024;
const LOG_DIRECTORY: &str = "/fs/microsd/log";

fn shell_flags() -> ::mavlink::common::SerialControlFlag {
    ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_RESPOND
        | ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_EXCLUSIVE
}

/// Frame a complete NuttShell line without truncating long file paths.
fn shell_frames(line: &str, target_system_id: u8) -> Vec<MavMessage> {
    let mut bytes = line.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes
        .chunks(SHELL_PAYLOAD)
        .map(|chunk| {
            let mut data = [0u8; SHELL_PAYLOAD];
            data[..chunk.len()].copy_from_slice(chunk);
            MavMessage::SERIAL_CONTROL(::mavlink::common::SERIAL_CONTROL_DATA {
                device: ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_SHELL,
                flags: shell_flags(),
                timeout: 0,
                baudrate: 0,
                count: chunk.len() as u8,
                data,
                target_system: target_system_id,
                target_component: TARGET_COMPONENT_ID,
            })
        })
        .collect()
}

async fn send_lines(
    handle: &ConnectionHandle,
    target_system_id: u8,
    lines: Vec<String>,
) -> Result<(), SdCaptureError> {
    tokio::time::timeout(SHELL_WRITE_TIMEOUT, async {
        for line in lines {
            for frame in shell_frames(&line, target_system_id) {
                handle
                    .send_confirmed(frame)
                    .await
                    .map_err(|error| SdCaptureError::Transport {
                        message: error.to_string(),
                    })?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| SdCaptureError::Transport {
        message: "timed out confirming target shell writes".into(),
    })?
}

static SHELL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Execute through the sole shell owner, so logger and file commands cannot
/// interleave their responses. Completion requires the marker output line.
async fn shell_exchange(
    handle: &ConnectionHandle,
    target_system_id: u8,
    route: &mut maggcs_core::mavlink::router::RoutedEvents,
    command: &str,
) -> Result<String, SdCaptureError> {
    let marker = format!(
        "__MAGDEL_{:08x}_{}__",
        std::process::id(),
        SHELL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    send_lines(
        handle,
        target_system_id,
        vec![String::new(), command.to_string(), format!("echo {marker}")],
    )
    .await?;

    let deadline = Instant::now() + SHELL_RESPONSE_TIMEOUT;
    let mut output = String::new();
    let mut line_buffer = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SdCaptureError::ShellTimeout { command: command.into() });
        }
        let event = tokio::time::timeout(remaining, route.recv())
            .await
            .map_err(|_| SdCaptureError::ShellTimeout { command: command.into() })?;
        match event {
            Ok(ConnectionEvent::Message(envelope)) => {
                let MavMessage::SERIAL_CONTROL(message) = envelope.message else {
                    continue;
                };
                let Some(chunk) =
                    shell_reply(&message, handle.config().system_id, handle.config().component_id)
                else {
                    continue;
                };
                line_buffer.push_str(&chunk);
                if output.len() + line_buffer.len() > SHELL_MAX_OUTPUT {
                    return Err(SdCaptureError::UnexpectedResponse {
                        output: "target shell output exceeded the limit; no complete response".into(),
                    });
                }
                while let Some(pos) = line_buffer.find('\n') {
                    let line: String = line_buffer.drain(..=pos).collect();
                    let line = clean_shell_line(&line);
                    if line.trim() == marker {
                        return Ok(output);
                    }
                    output.push_str(&line);
                    output.push('\n');
                }
            }
            Ok(ConnectionEvent::LinkError(_))
            | Ok(ConnectionEvent::Failed(_))
            | Ok(ConnectionEvent::HeartbeatLost { .. }) => {
                return Err(SdCaptureError::TargetUnavailable);
            }
            Ok(ConnectionEvent::Connected { .. }) => return Err(SdCaptureError::TargetChanged),
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                return Err(SdCaptureError::UnexpectedResponse {
                    output: "target shell response lost frames; result is unconfirmed".into(),
                });
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(SdCaptureError::TargetUnavailable);
            }
        }
    }
}

fn clean_shell_line(raw: &str) -> String {
    let mut chars = raw.chars().peekable();
    let mut line = String::new();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for character in chars.by_ref() {
                if ('@'..='~').contains(&character) {
                    break;
                }
            }
        } else if !character.is_control() || character == '\t' {
            line.push(character);
        }
    }
    line
}

async fn target_shell(state: &AppState, command: &str) -> Result<String, String> {
    let (target, _) = target_params(state)?;
    let tx = capture_sender(state, target)?;
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(CaptureRequest::Shell { command: command.into(), reply })
        .await
        .map_err(|_| "target shell service stopped".to_string())?;
    response.await
        .map_err(|_| "target shell service stopped".to_string())?
        .map_err(|error| error.to_string())
}

fn shell_command_error(output: &str) -> Option<&str> {
    output.lines().find(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("nsh:")
            || lower.contains("unknown command")
            || lower.contains("command not found")
            || lower.contains("permission denied")
    })
}

/// Silence on LOG_REQUEST_LIST is not proof of an empty SD card. Require a
/// complete recursive directory listing with no file entries instead.
pub(crate) async fn logs_directory_is_empty(state: &AppState) -> Result<bool, String> {
    let output = target_shell(state, &format!("ls -R {LOG_DIRECTORY}")).await?;
    parse_empty_log_directory(&output)
}

fn parse_empty_log_directory(output: &str) -> Result<bool, String> {
    if let Some(error) = shell_command_error(output) {
        return Err(format!("cannot verify SD logs: {error}"));
    }
    let mut saw_root = false;
    let mut has_file = false;
    for raw in output.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("nsh>") || line.starts_with("ls -R ") {
            continue;
        }
        if line == format!("{LOG_DIRECTORY}:") {
            saw_root = true;
            continue;
        }
        if line.starts_with(&format!("{LOG_DIRECTORY}/")) && line.ends_with(':') {
            continue;
        }
        if line.starts_with("echo __MAGDEL_") || line.starts_with("NuttShell ") {
            continue;
        }
        if !saw_root {
            return Err(format!("unexpected SD-directory response: {line}"));
        }
        for entry in line.split_whitespace() {
            if !entry.ends_with('/') {
                has_file = true;
            }
        }
    }
    if !saw_root {
        return Err("target did not confirm the SD log directory".into());
    }
    Ok(!has_file)
}

/// Refuse SD-log deletion while the target logger is recording. A fresh
/// `logger status` observation is required: the cached phase can be stale
/// after a reconnect. This is read-only and never starts/stops the logger.
pub(crate) async fn ensure_logger_not_recording(state: &AppState) -> Result<(), String> {
    let (target, _) = target_params(state)?;
    let tx = capture_sender(state, target)?;
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(CaptureRequest::Check {
        sdlog_mode: None,
        sdlog_backend: None,
        reply,
    })
    .await
    .map_err(|_| "target shell service stopped".to_string())?;
    let status = response
        .await
        .map_err(|_| "target shell service stopped".to_string())?
        .map_err(|error| error.to_string())?;
    if status.logger.as_ref().is_some_and(|logger| logger.file_logging) {
        return Err(
            "the target logger is recording; stop the SD capture before deleting logs".into(),
        );
    }
    Ok(())
}

/// Parse PX4's `logdata.txt` lines (`mtime size path`) into safe log paths.
/// PX4 writes these lines in the same order it assigns MAVLink log ids.
fn log_path_from_listing(listing: &str, entry: &maggcs_core::logs::LogEntryInfo) -> Result<String, String> {
    if let Some(error) = shell_command_error(listing) {
        return Err(format!("cannot read PX4 log index: {error}"));
    }
    let mut index = 0u16;
    for line in listing.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(first) = fields.first() else { continue };
        let Ok(time_utc) = first.parse::<u32>() else { continue };
        if fields.len() != 3 {
            return Err("PX4 log index contains a malformed entry; refresh the list".into());
        }
        let size = fields[1].parse::<u32>()
            .map_err(|_| "PX4 log index contains an invalid size".to_string())?;
        if index == entry.id {
            if time_utc != entry.time_utc || size != entry.size {
                return Err("SD log identity changed; refresh the list before deleting".into());
            }
            let path = fields[2];
            let relative = path.strip_prefix(&format!("{LOG_DIRECTORY}/"));
            let valid = relative.is_some_and(|relative| {
                relative.split('/').count() == 2
                    && relative.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
            }) && path.ends_with(".ulg")
                && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'));
            if !valid {
                return Err("PX4 returned an unsafe SD log path; deletion refused".into());
            }
            return Ok(path.into());
        }
        index = index.checked_add(1).ok_or("PX4 log index is too large")?;
    }
    Err(format!("log #{} is no longer in the PX4 log index; refresh the list", entry.id))
}

/// Delete one log file by its MAVLink log id. `logdata.txt` is regenerated by
/// PX4's list request in exactly the id order, so the id is its line index.
pub(crate) async fn delete_log_by_id(
    state: &AppState,
    entry: &maggcs_core::logs::LogEntryInfo,
) -> Result<(), String> {
    let listing = target_shell(state, "cat /fs/microsd/logdata.txt").await?;
    let path = log_path_from_listing(&listing, entry)?;
    let output = target_shell(state, &format!("rm {path}")).await?;
    if let Some(error) = shell_command_error(&output) {
        return Err(format!("SD log deletion failed: {error}"));
    }
    let output = target_shell(state, &format!("ls -l {path}")).await?;
    if !output.lines().any(|line| {
        line.starts_with("nsh: ls: stat failed:") && line.contains("No such file or directory")
    }) {
        return Err("PX4 has not confirmed the SD log was removed; refresh the list".into());
    }
    Ok(())
}

enum WaitKind {
    Check,
    Start,
    Stop,
}

struct Wait {
    kind: WaitKind,
    reply: tokio::sync::oneshot::Sender<Result<SdCaptureStatus, SdCaptureError>>,
}

fn wait_done(kind: &WaitKind, status: &SdCaptureStatus) -> bool {
    match kind {
        WaitKind::Check => matches!(
            status.phase,
            CapturePhase::Ready
                | CapturePhase::Recording
                | CapturePhase::ExistingRecording
                | CapturePhase::Failed
        ),
        WaitKind::Start => matches!(
            status.phase,
            CapturePhase::Recording | CapturePhase::ExistingRecording | CapturePhase::Failed
        ),
        WaitKind::Stop => matches!(status.phase, CapturePhase::Complete | CapturePhase::Failed),
    }
}

fn resolve_wait(wait: &mut Option<Wait>, capture: &SdCapture) {
    let Some(pending) = wait.as_ref() else {
        return;
    };
    let status = capture.status();
    if !wait_done(&pending.kind, &status) {
        return;
    }
    let Some(pending) = wait.take() else {
        return;
    };
    let result = if let Some(error) = status.last_error.clone() {
        Err(error)
    } else if status.phase == CapturePhase::Failed {
        Err(SdCaptureError::UnexpectedResponse {
            output: "capture failed without a logger observation".into(),
        })
    } else {
        Ok(status)
    };
    let _ = pending.reply.send(result);
}

fn publish<R: Runtime>(
    app: &AppHandle<R>,
    capture: &SdCapture,
    status: &Arc<Mutex<SdCaptureStatus>>,
    step: SdCaptureStep,
) {
    *status.lock() = capture.status();
    for event in step.events {
        let _ = app.emit("actuator_capture", &event);
    }
}

async fn apply_step<R: Runtime>(
    app: &AppHandle<R>,
    handle: &ConnectionHandle,
    target_system_id: u8,
    capture: &mut SdCapture,
    status: &Arc<Mutex<SdCaptureStatus>>,
    mut step: SdCaptureStep,
) {
    if let Err(error) =
        send_lines(handle, target_system_id, std::mem::take(&mut step.outbound)).await
    {
        step = capture.invalidate(error);
    }
    publish(app, capture, status, step);
}

/// Only replies to this GCS's shell request can advance the exchange. PX4
/// serial-passthrough and another GCS's shell output are different streams.
fn shell_reply(
    message: &::mavlink::common::SERIAL_CONTROL_DATA,
    gcs_system_id: u8,
    gcs_component_id: u8,
) -> Option<String> {
    if message.device != ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_SHELL
        || !message
            .flags
            .contains(::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_REPLY)
        || (message.target_system != 0 && message.target_system != gcs_system_id)
        || (message.target_component != 0 && message.target_component != gcs_component_id)
        || message.count == 0
        || usize::from(message.count) > SHELL_PAYLOAD
    {
        return None;
    }
    Some(String::from_utf8_lossy(&message.data[..usize::from(message.count)]).into_owned())
}

async fn run_capture<R: Runtime>(
    app: AppHandle<R>,
    handle: ConnectionHandle,
    target_system_id: u8,
    status: Arc<Mutex<SdCaptureStatus>>,
    mut commands: tokio::sync::mpsc::Receiver<CaptureRequest>,
) {
    let mut capture = SdCapture::default();
    let mut route = handle.subscribe_route(
        MessageRoute::messages(&[SERIAL_CONTROL_ID]).from(target_system_id, TARGET_COMPONENT_ID),
    );
    let mut ticker = tokio::time::interval(CAPTURE_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut wait: Option<Wait> = None;
    loop {
        tokio::select! {
            request = commands.recv() => {
                let Some(request) = request else { break };
                let now = std::time::Instant::now();
                let (command, kind, reply) = match request {
                    CaptureRequest::Shell { command, reply } => {
                        let busy = wait.is_some() || matches!(capture.status().phase,
                            CapturePhase::Verifying | CapturePhase::Starting |
                            CapturePhase::Stopping | CapturePhase::Finalizing);
                        let result = if busy {
                            Err(SdCaptureError::Busy)
                        } else {
                            shell_exchange(&handle, target_system_id, &mut route, &command).await
                        };
                        let _ = reply.send(result);
                        continue;
                    }
                    CaptureRequest::Check { sdlog_mode, sdlog_backend, reply } => (
                        SdCaptureCommand::Check { sdlog_mode, sdlog_backend }, WaitKind::Check, reply,
                    ),
                    CaptureRequest::Start { test_id, sdlog_mode, sdlog_backend, reply } => (
                        SdCaptureCommand::Start { test_id, sdlog_mode, sdlog_backend }, WaitKind::Start, reply,
                    ),
                    CaptureRequest::Stop { reply } => (SdCaptureCommand::Stop, WaitKind::Stop, reply),
                };
                match capture.command(command, now) {
                    Ok(step) => {
                        wait = Some(Wait { kind, reply });
                        apply_step(&app, &handle, target_system_id, &mut capture, &status, step).await;
                        resolve_wait(&mut wait, &capture);
                    }
                    Err(error) => {
                        publish(&app, &capture, &status, SdCaptureStep {
                            events: vec![maggcs_core::logs::capture::SdCaptureEvent::Status { status: capture.status() }],
                            ..Default::default()
                        });
                        let _ = reply.send(Err(error));
                    }
                }
            }
            event = route.recv() => {
                match event {
                    Ok(ConnectionEvent::Message(envelope)) => {
                        if let MavMessage::SERIAL_CONTROL(message) = envelope.message {
                            let Some(chunk) = shell_reply(&message, handle.config().system_id, handle.config().component_id) else {
                                continue;
                            };
                            let step = capture.ingest_shell(&chunk, std::time::Instant::now());
                            apply_step(&app, &handle, target_system_id, &mut capture, &status, step).await;
                            resolve_wait(&mut wait, &capture);
                        }
                    }
                    Ok(ConnectionEvent::LinkError(_))
                        | Ok(ConnectionEvent::Failed(_))
                        | Ok(ConnectionEvent::HeartbeatLost { .. }) => {
                        let step = capture.invalidate(SdCaptureError::TargetUnavailable);
                        publish(&app, &capture, &status, step);
                        resolve_wait(&mut wait, &capture);
                    }
                    Ok(ConnectionEvent::Connected { .. }) if capture.status().phase != CapturePhase::Idle => {
                        let step = capture.invalidate(SdCaptureError::TargetChanged);
                        publish(&app, &capture, &status, step);
                        resolve_wait(&mut wait, &capture);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = ticker.tick() => {
                let step = capture.tick(std::time::Instant::now());
                apply_step(&app, &handle, target_system_id, &mut capture, &status, step).await;
                resolve_wait(&mut wait, &capture);
            }
        }
    }
    if let Some(wait) = wait {
        let _ = wait.reply.send(Err(SdCaptureError::TargetUnavailable));
    }
    let mut current = status.lock();
    if !matches!(current.phase, CapturePhase::Idle | CapturePhase::Complete) {
        current.phase = CapturePhase::Failed;
        current.last_error = Some(SdCaptureError::TargetUnavailable);
    }
}

/// Start the shell service for the current target.
pub(crate) fn spawn_actuator_capture<R: Runtime>(
    app: AppHandle<R>,
    state: &AppState,
    handle: ConnectionHandle,
    target_system_id: u8,
) {
    let target = MotorTestTargetRef {
        connection_id: handle.handle_id(),
        target_system_id,
        target_component_id: TARGET_COMPONENT_ID,
    };
    // A replaced task retains only its own slot, never the replacement's status.
    let status = Arc::new(Mutex::new(SdCaptureStatus::default()));
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let task = tauri::async_runtime::spawn(run_capture(
        app,
        handle,
        target_system_id,
        status.clone(),
        rx,
    ));
    *state.motor_test.capture.lock() = ActuatorCapture {
        status,
        runtime: Some(ActuatorCaptureRuntime {
            target,
            commands: Some(tx),
            task: Some(task),
        }),
    };
}

/// Stop the capture service and reset the visible status (target teardown).
pub(crate) async fn stop_actuator_capture(state: &AppState) {
    let old = { state.motor_test.capture.lock().runtime.take() };
    if let Some(old) = old {
        old.stop(CAPTURE_STOP_TIMEOUT).await;
    }
    let status = state.motor_test.capture.lock().status.clone();
    *status.lock() = SdCaptureStatus::default();
}

pub(crate) fn capture_sender(
    state: &AppState,
    target: MotorTestTargetRef,
) -> Result<tokio::sync::mpsc::Sender<CaptureRequest>, String> {
    let capture = state.motor_test.capture.lock();
    let runtime = capture
        .runtime
        .as_ref()
        .ok_or_else(|| "target shell is not connected — reconnect the serial PX4".to_string())?;
    if runtime.target != target {
        return Err(SdCaptureError::TargetChanged.to_string());
    }
    runtime
        .commands
        .clone()
        .ok_or_else(|| SdCaptureError::TargetUnavailable.to_string())
}

fn target_params(state: &AppState) -> Result<(MotorTestTargetRef, ParamService), String> {
    state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| (runtime.target.target_ref(), runtime.target.params.clone()))
        .ok_or_else(|| "no serial PX4 target — connect it first".to_string())
}

async fn read_sdlog_params(params: &ParamService) -> (Option<i32>, Option<i32>) {
    let mode = read_int(params, "SDLOG_MODE").await;
    let backend = read_int(params, "SDLOG_BACKEND").await;
    (mode, backend)
}

async fn read_int(params: &ParamService, name: &str) -> Option<i32> {
    match params.read(name).await.ok()?.value {
        ParamValue::Int(value) => i32::try_from(value).ok(),
        ParamValue::Real(_) => None,
    }
}

/// Read the hardware's SDLOG_MODE/BACKEND and actual logger status. An
/// existing file capture is a valid observation, not a failed start.
#[tauri::command]
pub async fn actuator_capture_verify(
    state: State<'_, AppState>,
) -> Result<SdCaptureStatus, String> {
    let _ops = state.ops().lock().await;
    let (target, params) = target_params(&state)?;
    let tx = capture_sender(&state, target)?;
    let (mode, backend) = read_sdlog_params(&params).await;
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(CaptureRequest::Check {
        sdlog_mode: mode,
        sdlog_backend: backend,
        reply,
    })
    .await
    .map_err(|_| "target shell service stopped".to_string())?;
    response
        .await
        .map_err(|_| "target shell service stopped".to_string())?
        .map_err(|error| error.to_string())
}

/// Manually start a unique SD file. Capture remains independent of motor tests.
#[tauri::command]
pub async fn actuator_capture_start(
    state: State<'_, AppState>,
    test_id: String,
) -> Result<SdCaptureStatus, String> {
    let _ops = state.ops().lock().await;
    crate::run_service::ensure_no_active_run(&state)?;
    crate::run_service::validate_test_id(&test_id)?;
    start_capture_locked(&state, test_id).await
}

/// Start the capture and wait for the confirmed recording. The caller holds
/// the ops lock; the run coordinator is not started here.
pub(crate) async fn start_capture_locked(
    state: &AppState,
    test_id: String,
) -> Result<SdCaptureStatus, String> {
    crate::actuator_service::ensure_idle_session(state)?;
    let (target, params) = target_params(state)?;
    crate::actuator_service::ensure_target_disarmed(state, target)?;
    let tx = capture_sender(state, target)?;
    let (mode, backend) = read_sdlog_params(&params).await;
    crate::actuator_service::ensure_target_disarmed(state, target)?;
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(CaptureRequest::Start {
        test_id,
        sdlog_mode: mode,
        sdlog_backend: backend,
        reply,
    })
    .await
    .map_err(|_| "target shell service stopped".to_string())?;
    response
        .await
        .map_err(|_| "target shell service stopped".to_string())?
        .map_err(|error| error.to_string())
}

/// Stop the active capture and wait for a stable "Not logging" observation.
#[tauri::command]
pub async fn actuator_capture_stop(state: State<'_, AppState>) -> Result<SdCaptureStatus, String> {
    let _ops = state.ops().lock().await;
    stop_capture_locked(&state).await
}

/// Release the motors first, then stop the capture. The caller holds the ops
/// lock.
pub(crate) async fn stop_capture_locked(state: &AppState) -> Result<SdCaptureStatus, String> {
    let (target, _) = target_params(state)?;
    let tx = capture_sender(state, target)?;
    crate::actuator_service::ensure_idle_session(state)?;
    crate::actuator_service::ensure_target_disarmed(state, target)?;
    request_stop_with(tx).await
}

/// Coordinator action: `logger off` after a confirmed motor release. No ops
/// lock and no second motor release — the ticker already observed the stop.
pub(crate) async fn request_stop_for_target(
    state: &AppState,
    target: MotorTestTargetRef,
) -> Result<SdCaptureStatus, String> {
    let tx = capture_sender(state, target)?;
    request_stop_with(tx).await
}

async fn request_stop_with(
    tx: tokio::sync::mpsc::Sender<CaptureRequest>,
) -> Result<SdCaptureStatus, String> {
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(CaptureRequest::Stop { reply })
        .await
        .map_err(|_| "target shell service stopped".to_string())?;
    response
        .await
        .map_err(|_| "target shell service stopped".to_string())?
        .map_err(|error| error.to_string())
}

/// Read-only logger observation for recovery reconciliation.
pub(crate) async fn check_capture_for_target(
    state: &AppState,
    target: MotorTestTargetRef,
) -> Result<SdCaptureStatus, String> {
    let tx = capture_sender(state, target)?;
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(CaptureRequest::Check {
        sdlog_mode: None,
        sdlog_backend: None,
        reply,
    })
    .await
    .map_err(|_| "target shell service stopped".to_string())?;
    response
        .await
        .map_err(|_| "target shell service stopped".to_string())?
        .map_err(|error| error.to_string())
}

/// Current capture state for the UI (also embedded in `actuator_status`).
#[tauri::command]
pub fn actuator_capture_status(state: State<'_, AppState>) -> SdCaptureStatus {
    capture_status_blocking(&state)
}

pub(crate) fn capture_status_blocking(state: &AppState) -> SdCaptureStatus {
    state.motor_test.capture.lock().status.lock().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logdata_listing_maps_ids_to_safe_absolute_paths() {
        let listing = "1700000000 580665 /fs/microsd/log/2026-10-22/21_02_22.ulg\n1700000100 1032480 /fs/microsd/log/2026-10-22/21_26_00.ulg\n   broken line\n1700000200 1 /etc/passwd\n1700000300 1 /fs/microsd/log/../../etc/passwd.ulg\n";
        let mut entry = maggcs_core::logs::LogEntryInfo { id: 1, size: 1032480, time_utc: 1700000100 };
        assert_eq!(log_path_from_listing(listing, &entry).expect("log 1"), "/fs/microsd/log/2026-10-22/21_26_00.ulg");
        entry.size = 42;
        assert!(log_path_from_listing(listing, &entry).is_err(), "changed identity");
        entry.id = 3;
        entry.size = 1;
        entry.time_utc = 1700000300;
        assert!(log_path_from_listing(listing, &entry).is_err(), "path traversal");
    }

    #[test]
    fn shell_frame_terminates_the_line_and_sets_shell_flags() {
        let frames = shell_frames("logger status", 42);
        let MavMessage::SERIAL_CONTROL(frame) = &frames[0] else {
            panic!("serial control frame");
        };
        assert_eq!(frame.count as usize, "logger status\n".len());
        assert_eq!(&frame.data[..frame.count as usize], b"logger status\n");
        assert_eq!(frame.target_system, 42);
        assert_eq!(frame.target_component, TARGET_COMPONENT_ID);
        assert_eq!(
            frame.flags,
            ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_RESPOND
                | ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_EXCLUSIVE
        );
    }

    #[test]
    fn shell_frames_preserve_long_paths_and_one_final_newline() {
        let long = "x".repeat(100);
        let frames = shell_frames(&long, 42);
        assert_eq!(frames.len(), 2);
        let mut bytes = Vec::new();
        for frame in frames {
            let MavMessage::SERIAL_CONTROL(frame) = frame else { panic!("serial control") };
            assert!(usize::from(frame.count) <= SHELL_PAYLOAD);
            bytes.extend_from_slice(&frame.data[..usize::from(frame.count)]);
        }
        assert_eq!(bytes, format!("{long}\n").as_bytes());
    }

    #[test]
    fn empty_sd_directory_requires_complete_positive_evidence() {
        assert!(parse_empty_log_directory("nsh> ls -R /fs/microsd/log\n/fs/microsd/log:\nnsh> echo __MAGDEL_1__\n").expect("empty root"));
        assert!(parse_empty_log_directory("/fs/microsd/log:\n 2026-10-22/\n/fs/microsd/log/2026-10-22:\n").expect("empty subdirectory"));
        assert!(!parse_empty_log_directory("/fs/microsd/log:\n 2026-10-22/\n/fs/microsd/log/2026-10-22:\n 21_00_00.ulg\n").expect("file"));
        assert!(parse_empty_log_directory("").is_err());
        assert!(parse_empty_log_directory("nsh: ls: stat failed: No such file or directory\n").is_err());
    }

    #[test]
    fn shell_errors_and_ansi_prompt_are_not_success() {
        assert_eq!(clean_shell_line("\u{1b}[K__MAGDEL_1__\r\n"), "__MAGDEL_1__");
        assert!(shell_command_error("nsh: rm: unlink failed: No such file or directory\n").is_some());
        assert!(shell_command_error("nsh> rm /fs/microsd/log/2026-10-22/21_00_00.ulg\n").is_none());
    }

    fn reply(
        target_system: u8,
        target_component: u8,
        text: &str,
    ) -> ::mavlink::common::SERIAL_CONTROL_DATA {
        let mut data = [0u8; SHELL_PAYLOAD];
        data[..text.len()].copy_from_slice(text.as_bytes());
        ::mavlink::common::SERIAL_CONTROL_DATA {
            device: ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_SHELL,
            flags: ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_REPLY,
            count: text.len() as u8,
            data,
            target_system,
            target_component,
            ..Default::default()
        }
    }

    #[test]
    fn only_this_gcs_receives_target_shell_replies() {
        let accepted = reply(250, 250, "INFO [logger] Not logging\r\n");
        assert_eq!(
            shell_reply(&accepted, 250, 250).as_deref(),
            Some("INFO [logger] Not logging\r\n")
        );
        assert!(shell_reply(&accepted, 42, 250).is_none(), "another GCS");
        let mut foreign = accepted.clone();
        foreign.flags = ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_EXCLUSIVE;
        assert!(shell_reply(&foreign, 250, 250).is_none(), "not a reply");
        let mut other_device = accepted;
        other_device.device = ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_TELEM1;
        assert!(
            shell_reply(&other_device, 250, 250).is_none(),
            "another stream"
        );
    }

    #[test]
    fn check_and_start_waits_accept_an_existing_recording() {
        let status = SdCaptureStatus {
            phase: CapturePhase::ExistingRecording,
            ..Default::default()
        };
        assert!(wait_done(&WaitKind::Check, &status));
        assert!(wait_done(&WaitKind::Start, &status));
        assert!(!wait_done(&WaitKind::Stop, &status));
    }
}
