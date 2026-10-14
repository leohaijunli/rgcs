//! Motor / mag interference test window service (ADR-009 helper window; motor-test features).
//!
//! A thin adapter like `inspector_service` (ADR-001): the window lifecycle
//! and the command surface live here; the safety state machine and the
//! high-rate sender live in `core::motor_test` (A2). Start re-spawns the
//! service on the current link (it stops with its link); events flow back
//! to the window, and the window is pinned on top while the test runs.

use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use tauri::State;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tauri::WindowEvent;

use maggcs_core::mavlink::connection::{spawn_connection, ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::mavlink::MavMessage;
use maggcs_core::motor_test::service::{MotorTestCommand, MotorTestEvent, MotorTestService};
use maggcs_core::motor_test::source::ForwardSlot;
use maggcs_core::motor_test::{SessionState, DEFAULT_TICK_HZ};

use crate::motor_test_state::limits_for_load;
use crate::motor_test_state::MotorTestTarget;
use crate::state::AppState;

/// Status snapshot for the window's header and right panel (polled ~1 Hz by
/// the UI; safety-relevant signals are low-rate).
#[derive(Debug, Clone, Serialize)]
pub struct ActuatorStatus {
    pub connected: bool,
    pub fc_alive: bool,
    /// The one hard interlock: sending is only ever allowed while disarmed.
    pub armed: bool,
    /// Raw PX4 custom-mode bitmask (kept for diagnostics).
    pub custom_mode: u32,
    pub autopilot_px4: bool,
    /// PX4 custom mode pre-decoded in core (`telemetry::px4_mode`, ADR-017);
    /// the window must not decode `custom_mode` itself.
    pub mode_label: String,
    pub endpoint: Option<String>,
    /// Sending rate the service is configured for (10 Hz until M0 says more).
    pub tick_hz: f64,
    /// `idle` | `running` | `stopping` | `emergency`, from the last event.
    pub session: String,
    /// SITL source link (revised D3): connected + feed freshness.
    pub sitl_connected: bool,
    pub sitl_endpoint: Option<String>,
    /// Seconds since the last SERVO_OUTPUT_RAW sample; null = none yet.
    pub sitl_feed_age_s: Option<f64>,
    /// The motor test's own target link (real PX4, serial): connected + endpoint.
    pub target_connected: bool,
    pub target_endpoint: Option<String>,
    /// `COM_MOT_TEST_EN` read from the target PX4 (`None` = unknown): PX4
    /// denies ACTUATOR_TEST while this is not 1.
    pub com_mot_test_en: Option<f32>,
    /// Normalized SITL servo outputs 1–4 (the forward INPUT), latest sample.
    pub input_values: Vec<f32>,
    /// Values actually sent to the FC as ACTUATOR_TEST (the OUTPUT), 0 when idle.
    pub output_values: Vec<f32>,
}

/// Open the motor-test window, or focus it if it already exists.
#[tauri::command]
pub fn actuator_open(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("actuator") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, "actuator", WebviewUrl::App("actuator.html".into()))
        .devtools(true)
        .title("Motor / Mag Interference Test")
        .maximized(true)
        .min_inner_size(1024.0, 640.0)
        .build()
        .map_err(|e| e.to_string())?;
    // Closing the window must stop the motors (plan §8.3).
    let app_for_cleanup = app.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            let state = app_for_cleanup.state::<AppState>();
            stop_running_session(&state);
            state.motor_test.running.lock().take();
            detach_motor_telemetry(&state);
        }
    });
    Ok(())
}

/// Close the motor-test window and run the stop path.
#[tauri::command]
pub fn actuator_close(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    stop_running_session(&state);
    state.motor_test.running.lock().take();
    detach_motor_telemetry(&state);
    if let Some(win) = app.get_webview_window("actuator") {
        let _ = win.close();
    }
    Ok(())
}

/// Live link/FC status for the window's panels.
#[tauri::command]
pub fn actuator_status(state: State<'_, AppState>) -> Result<ActuatorStatus, String> {
    Ok(state.actuator_status())
}

/// Start manual sending. `load` is the declared prop load (`none` | `plate`
/// | `props`) — it picks the output envelope (`limits_for_load`); the
/// risk-scaled confirmation happened in the UI before this call.
#[tauri::command]
pub async fn actuator_start_manual(
    app: AppHandle,
    state: State<'_, AppState>,
    load: String,
    values: Vec<f32>,
) -> Result<(), String> {
    // Reuse a live session if its channel takes the values (the channel
    // dies with the run task, so a failed send means the old link is gone);
    // otherwise re-spawn on the current link.
    if send_session(&state, MotorTestCommand::SetValues(values.clone()))
        .await
        .is_ok()
    {
        return send_session(&state, MotorTestCommand::StartManual)
            .await
            .map_err(|e| e.to_string());
    }
    let handle = fc_link(&state).ok_or("no link — connect the serial PX4 first")?;
    let (svc, evt_rx, output_rx) = MotorTestService::spawn(handle, limits_for_load(&load));
    let forwarder = tauri::async_runtime::spawn(forward_events(app.clone(), evt_rx));
    *state.motor_test.running.lock() = Some(crate::motor_test_state::RunningMotorTest {
        svc: svc.clone(),
        forwarder,
        output: output_rx,
    });
    let _ = svc.send(MotorTestCommand::SetValues(values)).await;
    svc.send(MotorTestCommand::StartManual)
        .await
        .map_err(|e| e.to_string())
}

/// Start a preset waveform (all motors in sync for its duration; the
/// session stops itself with `Completed` when it ends).
#[tauri::command]
pub async fn actuator_start_preset(
    app: AppHandle,
    state: State<'_, AppState>,
    preset: maggcs_core::motor_test::source::WaveformPreset,
) -> Result<(), String> {
    // Same reuse-or-respawn logic as the manual start: a live session takes
    // the command; a dead one is re-spawned on the current link.
    if send_session(&state, MotorTestCommand::StartPreset { preset })
        .await
        .is_ok()
    {
        return Ok(());
    }
    let handle = fc_link(&state).ok_or("no link — connect the serial PX4 first")?;
    let (svc, evt_rx, output_rx) = MotorTestService::spawn(handle, limits_for_load("none"));
    let forwarder = tauri::async_runtime::spawn(forward_events(app.clone(), evt_rx));
    *state.motor_test.running.lock() = Some(crate::motor_test_state::RunningMotorTest {
        svc: svc.clone(),
        forwarder,
        output: output_rx,
    });
    svc.send(MotorTestCommand::StartPreset { preset })
        .await
        .map_err(|e| e.to_string())
}

/// Latest-value-wins slider update (the UI throttles to ~30 Hz).
#[tauri::command]
pub async fn actuator_set_values(
    state: State<'_, AppState>,
    values: Vec<f32>,
) -> Result<(), String> {
    send_session(&state, MotorTestCommand::SetValues(values)).await
}

/// Operator stop. Idempotent: fine with no session (link already died).
#[tauri::command]
pub async fn actuator_stop(state: State<'_, AppState>) -> Result<(), String> {
    let _ = send_session(&state, MotorTestCommand::Stop).await;
    Ok(())
}

/// Emergency stop: latched in the session; the UI mirrors it.
#[tauri::command]
pub async fn actuator_estop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _ = send_session(&state, MotorTestCommand::Emergency).await;
    pin_window(&app, false);
    Ok(())
}

/// Explicit emergency reset (never automatic).
#[tauri::command]
pub async fn actuator_reset_emergency(state: State<'_, AppState>) -> Result<(), String> {
    send_session(&state, MotorTestCommand::ResetEmergency).await
}

/// The SITL feed's stream rate for `SERVO_OUTPUT_RAW` (10 Hz: enough head
/// room for the 10 Hz sender tick without flooding the link).
const SITL_STREAM_HZ: f64 = 10.0;

/// Connect the SITL source link (revised D3): a secondary MAVLink
/// connection whose `SERVO_OUTPUT_RAW` main outputs 1–4 feed the forward
/// slot, normalized to 0..=1. Idempotent: a previous SITL link is dropped.
#[tauri::command]
pub async fn actuator_connect_sitl(
    state: State<'_, AppState>,
    endpoint: String,
) -> Result<(), String> {
    let config = maggcs_core::mavlink::ConnectionConfig {
        endpoint: maggcs_core::mavlink::Endpoint::try_from(endpoint.as_str())
            .map_err(|e| e.to_string())?,
        system_id: 250,
        component_id: 250,
        target_system_id: 1,
        target_component_id: 1,
        heartbeat_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let (handle, _events_rx, first_result) =
        spawn_connection(config).await.map_err(|e| e.to_string())?;
    match tokio::time::timeout(Duration::from_secs(3), first_result).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(failure))) => {
            handle.shutdown().await;
            return Err(failure.message.to_string());
        }
        _ => {
            handle.shutdown().await;
            return Err("timed out waiting for the SITL link".into());
        }
    }

    // Ask the SITL to stream its actuator outputs (fire-and-forget; PX4
    // accepts MAV_CMD_SET_MESSAGE_INTERVAL while disarmed).
    let interval_us = (1_000_000.0 / SITL_STREAM_HZ) as f32;
    let _ = handle
        .send(MavMessage::COMMAND_LONG(
            ::mavlink::common::COMMAND_LONG_DATA {
                param1: 36.0, // SERVO_OUTPUT_RAW
                param2: interval_us,
                param3: 0.0,
                param4: 0.0,
                param5: 0.0,
                param6: 0.0,
                param7: 0.0,
                command: ::mavlink::common::MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL,
                target_system: 1,
                target_component: 1,
                confirmation: 0,
            },
        ))
        .await;

    let slot: ForwardSlot = std::sync::Arc::new(std::sync::Mutex::new(
        maggcs_core::motor_test::source::ForwardState::new(4),
    ));
    let tap = tauri::async_runtime::spawn(run_sitl_tap(handle.clone(), slot.clone()));
    let old = state
        .motor_test
        .sitl
        .lock()
        .replace(crate::motor_test_state::SitlLink {
            handle,
            tap,
            slot,
            endpoint,
        });
    if let Some(old) = old {
        old.tap.abort();
        old.handle.shutdown().await;
    }
    Ok(())
}

/// Drop the SITL source link.
#[tauri::command]
pub async fn actuator_disconnect_sitl(state: State<'_, AppState>) -> Result<(), String> {
    // Take the entry out before awaiting: never hold the lock across the
    // link shutdown (Send).
    let old = state.motor_test.sitl.lock().take();
    if let Some(old) = old {
        old.tap.abort();
        old.handle.shutdown().await;
    }
    Ok(())
}

/// The link the motor test sends to: its dedicated serial target (the real
/// PX4) when one is connected, else the app's primary link (backward
/// compatible with the old single-link setup).
fn fc_link(state: &AppState) -> Option<maggcs_core::mavlink::ConnectionHandle> {
    state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|t| t.handle.clone())
        .or_else(|| state.connection())
}

/// Connect the motor test to the **real PX4** over a serial interface: a
/// dedicated MAVLink link whose every `ACTUATOR_TEST` output goes to the
/// hardware. The main GCS keeps its own link (e.g. a UDP SITL) untouched.
/// Idempotent: a previous target link is dropped.
#[tauri::command]
pub async fn actuator_connect_target(
    app: AppHandle,
    state: State<'_, AppState>,
    endpoint: String,
) -> Result<(), String> {
    let config = maggcs_core::mavlink::ConnectionConfig {
        endpoint: maggcs_core::mavlink::Endpoint::try_from(endpoint.as_str())
            .map_err(|e| e.to_string())?,
        system_id: 250,
        component_id: 250,
        target_system_id: 1,
        target_component_id: 1,
        heartbeat_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let (handle, _events_rx, first_result) =
        spawn_connection(config).await.map_err(|e| e.to_string())?;
    match tokio::time::timeout(Duration::from_secs(3), first_result).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(failure))) => {
            handle.shutdown().await;
            return Err(failure.message.to_string());
        }
        _ => {
            handle.shutdown().await;
            return Err("timed out waiting for the PX4 link".into());
        }
    }
    // The mag/ESC alarm reads HIGHRES_IMU / ESC_STATUS from the target FC.
    attach_motor_telemetry(&app, handle.clone());
    // Read COM_MOT_TEST_EN from the PX4 (PX4 denies ACTUATOR_TEST while it is
    // not 1). The read is spawned so a quiet FC never stalls the connect;
    // the value lands in the shared slot as soon as the PARAM_VALUE arrives.
    let com_mot_test_en = std::sync::Arc::new(std::sync::Mutex::new(None));
    {
        let slot = com_mot_test_en.clone();
        let h = handle.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(v) = read_param(&h, "COM_MOT_TEST_EN").await {
                if let Ok(mut guard) = slot.lock() {
                    *guard = Some(v);
                }
            }
        });
    }
    let old = state.motor_test.target.lock().replace(MotorTestTarget {
        handle,
        endpoint,
        com_mot_test_en,
    });
    if let Some(old) = old {
        old.handle.shutdown().await;
    }
    Ok(())
}

/// Drop the motor test's dedicated target link (real PX4). The session (if
/// any) is stopped first so no frames are left addressed to a dead link.
#[tauri::command]
pub async fn actuator_disconnect_target(state: State<'_, AppState>) -> Result<(), String> {
    let _ = send_session(&state, MotorTestCommand::Stop).await;
    let old = state.motor_test.target.lock().take();
    if let Some(old) = old {
        old.handle.shutdown().await;
    }
    detach_motor_telemetry(&state);
    Ok(())
}

/// Start realtime forwarding: the live SITL actuator outputs (already
/// normalized into the slot by the tap) drive the real FC's motors.
#[tauri::command]
pub async fn actuator_start_forward(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // The slot is fed by the SITL tap; forwarding without it is meaningless.
    let slot = state
        .motor_test
        .sitl
        .lock()
        .as_ref()
        .map(|s| s.slot.clone())
        .ok_or("connect the SITL source first")?;
    if send_session(
        &state,
        MotorTestCommand::StartForward { slot: slot.clone() },
    )
    .await
    .is_ok()
    {
        return Ok(());
    }
    let handle = fc_link(&state).ok_or("no link — connect the serial PX4 first")?;
    let (svc, evt_rx, output_rx) = MotorTestService::spawn(handle, limits_for_load("none"));
    let forwarder = tauri::async_runtime::spawn(forward_events(app.clone(), evt_rx));
    *state.motor_test.running.lock() = Some(crate::motor_test_state::RunningMotorTest {
        svc: svc.clone(),
        forwarder,
        output: output_rx,
    });
    svc.send(MotorTestCommand::StartForward { slot })
        .await
        .map_err(|e| e.to_string())
}

/// Tap task: SITL `SERVO_OUTPUT_RAW` → normalized 0..=1 into the forward
/// slot (latest value wins; the tap never blocks the sender).
async fn run_sitl_tap(handle: maggcs_core::mavlink::ConnectionHandle, slot: ForwardSlot) {
    let mut events = handle.subscribe_route(MessageRoute::messages(&[36]));
    loop {
        match events.recv().await {
            Ok(ConnectionEvent::Message(env)) => {
                if let MavMessage::SERVO_OUTPUT_RAW(servo) = env.message {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    let pwm_to_norm = |pwm: u16| (((pwm as f32) - 1000.0) / 1000.0).clamp(0.0, 1.0);
                    if let Ok(mut st) = slot.lock() {
                        st.values = [
                            pwm_to_norm(servo.servo1_raw),
                            pwm_to_norm(servo.servo2_raw),
                            pwm_to_norm(servo.servo3_raw),
                            pwm_to_norm(servo.servo4_raw),
                        ]
                        .to_vec();
                        st.last_update_s = Some(now);
                    }
                }
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Clone the live service handle out of the state (no lock across awaits).
fn service_of(state: &AppState) -> Option<MotorTestService> {
    state
        .motor_test
        .running
        .lock()
        .as_ref()
        .map(|r| r.svc.clone())
}

/// Request one parameter by name from the FC and wait for its `PARAM_VALUE`
/// (PX4 answers `PARAM_REQUEST_READ` with `param_index = -1` by name). Returns
/// `None` on timeout / a closed link; the caller treats that as "unknown".
async fn read_param(handle: &ConnectionHandle, name: &str) -> Option<f32> {
    // Subscribe before sending: broadcast receivers only see future frames.
    let mut events = handle.subscribe_route(MessageRoute::messages(&[22]));
    let req = MavMessage::PARAM_REQUEST_READ(::mavlink::common::PARAM_REQUEST_READ_DATA {
        target_system: 1,
        target_component: 1,
        param_id: maggcs_core::params::codec::encode_param_id(name).into(),
        param_index: -1, // -1 = by name
    });
    if handle.send(req).await.is_err() {
        return None;
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(ConnectionEvent::Message(env)) => {
                    if let MavMessage::PARAM_VALUE(p) = &env.message {
                        // The requested name echoes back in the reply.
                        if maggcs_core::params::codec::decode_param_id(&p.param_id) == name {
                            return Some(p.param_value);
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
    .await
    .unwrap_or(None)
}

/// Send one command to the stored session; `Err` = no session / dead link.
async fn send_session(state: &AppState, cmd: MotorTestCommand) -> Result<(), String> {
    match service_of(state) {
        Some(svc) => svc.send(cmd).await.map_err(|e| e.to_string()),
        None => Err("no motor-test session".into()),
    }
}

/// Best-effort stop of the stored session (window close, app exit). The
/// buffered channel takes the command without blocking the UI thread.
fn stop_running_session(state: &AppState) {
    let guard = state.motor_test.running.lock();
    if let Some(running) = guard.as_ref() {
        let _ = running.svc.try_send(MotorTestCommand::Stop);
    }
}

/// Push session events to the window, remember the state for the status
/// poll, and pin the window while sending.
async fn forward_events(app: AppHandle, mut rx: tokio::sync::mpsc::Receiver<MotorTestEvent>) {
    while let Some(ev) = rx.recv().await {
        match ev {
            MotorTestEvent::StateChanged(s) => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.motor_test.note_state(s);
                }
                pin_window(&app, s.is_sending());
                let _ = app.emit("actuator_event", SessionPayload::State(s));
            }
            MotorTestEvent::Stopped(reason) => {
                pin_window(&app, false);
                let _ = app.emit("actuator_event", SessionPayload::Stopped(reason));
            }
        }
    }
}

/// Serializable event payloads for the window (`actuator_event`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SessionPayload {
    State(SessionState),
    Stopped(maggcs_core::motor_test::service::StopReason),
}

/// Pin/unpin the motor-test window (operator request: on top while sending).
fn pin_window(app: &AppHandle, on_top: bool) {
    if let Some(win) = app.get_webview_window("actuator") {
        let _ = win.set_always_on_top(on_top);
    }
}

impl AppState {
    /// Snapshot the link + FC state for the actuator window.
    pub fn actuator_status(&self) -> ActuatorStatus {
        let link = self.link_status();
        // Armed/mode gates the START button, so it must come from the link
        // that actually receives the commands: the serial target PX4 when one
        // is connected, else the app's primary link.
        let target_hb = self
            .motor_test
            .telemetry
            .lock()
            .as_ref()
            .and_then(|t| t.hb.clone());
        let (armed, custom_mode, autopilot_px4, mode_label) = match target_hb {
            Some(hb) => (
                hb.base_mode.safety_armed,
                hb.custom_mode,
                hb.autopilot == maggcs_core::telemetry::Autopilot::Px4,
                hb.mode_label,
            ),
            None => match self.hub().and_then(|h| h.snapshot().heartbeat) {
                Some(hb) => (
                    hb.base_mode.safety_armed,
                    hb.custom_mode,
                    hb.autopilot == maggcs_core::telemetry::Autopilot::Px4,
                    hb.mode_label,
                ),
                None => (false, 0, false, String::new()),
            },
        };
        let session = match self.motor_test.session() {
            Some(SessionState::ManualRunning) => "running",
            Some(SessionState::Stopping) => "stopping",
            Some(SessionState::Emergency) => "emergency",
            _ => "idle",
        };
        let (sitl_connected, sitl_endpoint, sitl_feed_age_s) = {
            let guard = self.motor_test.sitl.lock();
            match guard.as_ref() {
                Some(link) => {
                    let age = link
                        .slot
                        .lock()
                        .ok()
                        .and_then(|st| {
                            st.last_update_s.map(|t| {
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs_f64() - t)
                                    .unwrap_or(0.0)
                            })
                        })
                        .filter(|a| *a >= 0.0);
                    (true, Some(link.endpoint.clone()), age)
                }
                None => (false, None, None),
            }
        };
        // The forward INPUT: the SITL's normalized SERVO_OUTPUT_RAW 1–4.
        let input_values = self
            .motor_test
            .sitl
            .lock()
            .as_ref()
            .and_then(|l| l.slot.lock().ok().map(|st| st.values.clone()))
            .unwrap_or_default();
        // The OUTPUT: the values the session sent to the FC as ACTUATOR_TEST.
        let output_values = self
            .motor_test
            .running
            .lock()
            .as_ref()
            .map(|r| (*r.output.borrow()).clone())
            .unwrap_or_default();
        let (target_connected, target_endpoint, com_mot_test_en) = {
            let guard = self.motor_test.target.lock();
            match guard.as_ref() {
                Some(t) => (
                    true,
                    Some(t.endpoint.clone()),
                    t.com_mot_test_en.lock().ok().and_then(|g| *g),
                ),
                None => (false, None, None),
            }
        };
        ActuatorStatus {
            connected: link
                .as_ref()
                .map(|l| l.link_state == maggcs_core::mavlink::LinkState::Connected)
                .unwrap_or(false),
            fc_alive: link.as_ref().map(|l| l.fc_alive).unwrap_or(false),
            armed,
            custom_mode,
            autopilot_px4,
            mode_label,
            endpoint: link.map(|l| l.endpoint),
            tick_hz: DEFAULT_TICK_HZ,
            session: session.to_string(),
            sitl_connected,
            sitl_endpoint,
            sitl_feed_age_s,
            target_connected,
            target_endpoint,
            input_values,
            output_values,
            com_mot_test_en,
        }
    }
}

/// Attach the mag/ESC telemetry tap to a link (A4 wiring): subscribes to
/// `HIGHRES_IMU` (105, mag triple) and `ESC_STATUS` (291), applies the loaded
/// calibration, drives the alarm state machine, and emits
/// `actuator_telemetry` events for the window's curves. Called from
/// `connect` after the link comes up.
pub fn attach_motor_telemetry(app: &AppHandle, handle: maggcs_core::mavlink::ConnectionHandle) {
    let state = app.state::<AppState>();
    // (Re)attach stops a previous tap and resets the alarm clock; the loaded
    // calibration survives (it is a file, not link state).
    if let Some(old) = state.motor_test.telemetry_tap.lock().take() {
        old.abort();
    }
    *state.motor_test.telemetry.lock() = Some(crate::motor_test_state::MotorTelemetry {
        esc: maggcs_core::signals::derived::EscAggregator::new(8),
        alarm: maggcs_core::motor_test::MagAlarm::new(Default::default()),
        // The loaded calibration is process-wide, so it survives link swaps
        // and is re-applied here on every attach.
        calib: state.calibration.get(),
        last_mag_total_nt: None,
        hb: None,
    });
    let tap_app = app.clone();
    let task = tauri::async_runtime::spawn(run_motor_telemetry_tap(tap_app, handle));
    *state.motor_test.telemetry_tap.lock() = Some(task);
}

/// The telemetry tap: `HIGHRES_IMU` (105) → calibrated `mag_total` → alarm;
/// `ESC_STATUS` (291) → the ESC aggregator. Emits `actuator_telemetry`
/// (aggregated snapshot) at ~10 Hz.
async fn run_motor_telemetry_tap(app: AppHandle, handle: maggcs_core::mavlink::ConnectionHandle) {
    // HEARTBEAT (armed/mode gating), HIGHRES_IMU (mag), ESC_STATUS (curves).
    let mut events = handle.subscribe_route(MessageRoute::messages(&[0, 105, 291]));
    let mut last_emit = tokio::time::Instant::now();
    loop {
        match events.recv().await {
            Ok(ConnectionEvent::Message(env)) => {
                let Some(state) = app.try_state::<AppState>() else {
                    return;
                };
                let mut guard = state.motor_test.telemetry.lock();
                let Some(telemetry) = guard.as_mut() else {
                    return; // detached
                };
                match &env.message {
                    MavMessage::HEARTBEAT(_) => {
                        // The FC that gates the START button: the target PX4's
                        // armed/mode state (not the main GCS's link).
                        if let Ok(hb) = maggcs_core::telemetry::Heartbeat::try_from(&*env) {
                            telemetry.hb = Some(hb);
                        }
                    }
                    MavMessage::HIGHRES_IMU(imu) => {
                        // HIGHRES_IMU mag fields are gauss on the wire; the
                        // whole MagGCS mag pipeline (calibration `A·(m−b)`,
                        // `mag_total`, the alarm thresholds) is nanotesla.
                        let raw_gauss = [imu.xmag as f64, imu.ymag as f64, imu.zmag as f64];
                        let raw = raw_gauss.map(|g| g * maggcs_core::signals::GAUSS_TO_NANOTESLA);
                        // Apply the loaded calibration (A·(raw−b), optional
                        // K·I) — none loaded: raw pass-through.
                        let calibrated = match &telemetry.calib {
                            Some(cal) => cal.apply(raw, None),
                            None => raw,
                        };
                        let total = (calibrated[0] * calibrated[0]
                            + calibrated[1] * calibrated[1]
                            + calibrated[2] * calibrated[2])
                            .sqrt();
                        telemetry.last_mag_total_nt = Some(total);
                        if telemetry.alarm.feed(total, now_epoch_s()) {
                            // Transition into Alarm: fire the audio + flash.
                            let _ = app.emit("actuator_alarm", total);
                        }
                    }
                    MavMessage::ESC_STATUS(esc) => {
                        telemetry.esc.feed(
                            esc.index,
                            &[
                                esc.rpm[0] as i16,
                                esc.rpm[1] as i16,
                                esc.rpm[2] as i16,
                                esc.rpm[3] as i16,
                            ],
                            &esc.voltage,
                            &esc.current,
                        );
                    }
                    _ => {}
                }
                // Batch the UI pushes at ~10 Hz.
                if last_emit.elapsed() >= Duration::from_millis(100) {
                    last_emit = tokio::time::Instant::now();
                    let payload = telemetry_payload(telemetry);
                    let _ = app.emit("actuator_telemetry", &payload);
                }
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Serializable telemetry snapshot for the window (mag + ESC + alarm).
#[derive(Debug, Clone, Serialize)]
pub struct MotorTelemetryPayload {
    pub mag_total_nt: Option<f64>,
    pub alarm: &'static str,
    pub esc: Vec<(String, f64)>,
}

fn telemetry_payload(t: &crate::motor_test_state::MotorTelemetry) -> MotorTelemetryPayload {
    MotorTelemetryPayload {
        mag_total_nt: t.last_mag_total_nt,
        alarm: match t.alarm.state() {
            maggcs_core::motor_test::MagAlarmState::Armed => "armed",
            maggcs_core::motor_test::MagAlarmState::Alarm => "alarm",
        },
        esc: t.esc.signals(),
    }
}

fn now_epoch_s() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Update the alarm tunables (threshold/debounce/hysteresis/cooldown/α).
#[tauri::command]
pub fn actuator_set_alarm_config(
    state: State<'_, AppState>,
    config: maggcs_core::motor_test::MagAlarmConfig,
) -> Result<(), String> {
    let mut guard_opt = state.motor_test.telemetry.lock();
    let Some(telemetry) = guard_opt.as_mut() else {
        return Err("no telemetry tap attached".into());
    };
    telemetry.alarm.set_config(config);
    Ok(())
}

/// Operator acknowledge: the audio + flash stop; the state machine keeps
/// its level tracking (re-fires only per §9.1 rules).
#[tauri::command]
pub fn actuator_acknowledge_alarm(state: State<'_, AppState>) -> Result<(), String> {
    // The window silences itself; the state machine has nothing to reset
    // (the hysteresis + cooldown own the re-arm). Reserved for a future
    // acknowledge-required latch mode.
    let _ = state;
    Ok(())
}

/// Detach the telemetry tap (window close / link teardown).
pub fn detach_motor_telemetry(state: &AppState) {
    if let Some(old) = state.motor_test.telemetry_tap.lock().take() {
        old.abort();
    }
    *state.motor_test.telemetry.lock() = None;
}
