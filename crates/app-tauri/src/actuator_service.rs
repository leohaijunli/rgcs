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
use maggcs_core::motor_test::scope::{self, ScopeBus};
use maggcs_core::motor_test::service::{MotorTestCommand, MotorTestEvent, MotorTestService};
use maggcs_core::motor_test::source::ForwardSlot;
use maggcs_core::motor_test::{SessionState, DEFAULT_TICK_HZ};

/// Maximum time to observe the release pass before closing an actuator link.
const MOTOR_TEST_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MOTOR_TEST_TARGET_SYSTEM_ID: u8 = 1;

use crate::motor_test_state::limits_for_load;
use crate::motor_test_state::{
    AuxiliaryLinkRuntime, MotorTestTarget, SitlLinkRuntime, TargetLinkRuntime,
};
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
    pub target_system_id: Option<u8>,
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
    start_scope_flusher(&app);
    let state = app.state::<AppState>();
    let target_handle = state.motor_test.target.lock().as_ref().map(|runtime| {
        (
            runtime.target.handle.clone(),
            runtime.target.target_system_id,
        )
    });
    if let Some((handle, target_system_id)) = target_handle {
        attach_motor_telemetry(&app, handle, target_system_id);
    }
    // Closing the window must stop the motors (plan §8.3).
    let app_for_cleanup = app.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            let app = app_for_cleanup.clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<AppState>();
                let _ops = state.ops().lock().await;
                if let Err(error) = stop_active_session(&state).await {
                    let _ = app.emit("actuator_stop_warning", error);
                    return;
                }
                state.motor_test.running.lock().take();
                let sitl = state.motor_test.sitl.lock().take();
                if let Some(sitl) = sitl {
                    sitl.stop().await;
                }
                detach_motor_telemetry(&state).await;
                let target = state.motor_test.target.lock().take();
                if let Some(target) = target {
                    target.stop().await;
                }
                stop_scope_flusher(&state);
            });
        }
    });
    Ok(())
}

/// Close the motor-test window and run the stop path.
#[tauri::command]
pub async fn actuator_close(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    stop_active_session(&state).await?;
    state.motor_test.running.lock().take();
    detach_motor_telemetry(&state).await;
    stop_auxiliary_links(&state).await;
    stop_scope_flusher(&state);
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
    let _ops = state.ops().lock().await;
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
    let target_system_id = motor_test_target_system_id(&state);
    let (svc, evt_rx, output_rx) =
        MotorTestService::spawn(handle, limits_for_load(&load, target_system_id));
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
    let _ops = state.ops().lock().await;
    // Same reuse-or-respawn logic as the manual start: a live session takes
    // the command; a dead one is re-spawned on the current link.
    if send_session(&state, MotorTestCommand::StartPreset { preset })
        .await
        .is_ok()
    {
        return Ok(());
    }
    let handle = fc_link(&state).ok_or("no link — connect the serial PX4 first")?;
    let target_system_id = motor_test_target_system_id(&state);
    let (svc, evt_rx, output_rx) =
        MotorTestService::spawn(handle, limits_for_load("none", target_system_id));
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

    let _ops = state.ops().lock().await;
    let existing_sitl = state.motor_test.sitl.lock().is_some();
    let old_sitl = stop_before_auxiliary_close(
        async {
            if existing_sitl {
                stop_active_session(&state).await?;
            }
            Ok(())
        },
        async {
            let old_sitl = state.motor_test.sitl.lock().take();
            if let Some(old) = old_sitl {
                old.stop().await;
            }
            Ok::<(), String>(())
        },
    )
    .await;
    if let Err(error) = old_sitl {
        handle.shutdown().await;
        return Err(error);
    }
    let slot: ForwardSlot = std::sync::Arc::new(std::sync::Mutex::new(
        maggcs_core::motor_test::source::ForwardState::new(4),
    ));
    let bus = state.motor_test.scope.clone();
    let tap = tauri::async_runtime::spawn(run_sitl_tap(handle.clone(), slot.clone(), bus));
    *state.motor_test.sitl.lock() = Some(SitlLinkRuntime {
        slot,
        endpoint,
        runtime: AuxiliaryLinkRuntime::new(handle, [tap]),
    });
    Ok(())
}

/// Drop the SITL source link.
#[tauri::command]
pub async fn actuator_disconnect_sitl(state: State<'_, AppState>) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    let existing_sitl = state.motor_test.sitl.lock().is_some();
    stop_before_auxiliary_close(
        async {
            if existing_sitl {
                stop_active_session(&state).await?;
            }
            Ok(())
        },
        async {
            // Take the entry before awaiting; never hold its state lock across
            // link shutdown.
            let old = state.motor_test.sitl.lock().take();
            if let Some(old) = old {
                old.stop().await;
            }
            Ok::<(), String>(())
        },
    )
    .await
}

/// The link the motor test sends to: its dedicated serial target (the real
/// PX4) when one is connected, else the app's primary link (backward
/// compatible with the old single-link setup).
fn validate_target_system_id(target_system_id: u8) -> Result<u8, String> {
    if target_system_id == 0 {
        Err("target system ID must be between 1 and 255".into())
    } else {
        Ok(target_system_id)
    }
}

fn motor_test_target_system_id(state: &AppState) -> u8 {
    state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| runtime.target.target_system_id)
        .unwrap_or(DEFAULT_MOTOR_TEST_TARGET_SYSTEM_ID)
}

/// The motor test's command link: **only** the dedicated serial target
/// (`actuator_connect_target`). Never the primary link (A2): without a
/// target, the start commands refuse with "no link — connect the serial PX4
/// first" instead of silently sending `ACTUATOR_TEST` to whatever is on the
/// main link — e.g. a flying SITL, whose armed heartbeats would stop the
/// session instantly (the operator saw Start bounce straight back to idle).
fn fc_link(state: &AppState) -> Option<maggcs_core::mavlink::ConnectionHandle> {
    state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| runtime.target.handle.clone())
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
    target_system_id: u8,
) -> Result<(), String> {
    let target_system_id = validate_target_system_id(target_system_id)?;
    let _ops = state.ops().lock().await;
    let config = maggcs_core::mavlink::ConnectionConfig {
        endpoint: maggcs_core::mavlink::Endpoint::try_from(endpoint.as_str())
            .map_err(|e| e.to_string())?,
        system_id: 250,
        component_id: 250,
        target_system_id,
        target_component_id: 1,
        auto_discover_target: false,
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
    // A session retains the old target handle. Release it before replacing
    // that target; on timeout preserve the old target and close the new link.
    if let Err(error) = stop_before_auxiliary_close(
        async {
            stop_active_session(&state).await?;
            Ok(())
        },
        async {
            detach_motor_telemetry(&state).await;
            state.motor_test.running.lock().take();
            let old = state.motor_test.target.lock().take();
            if let Some(old) = old {
                old.stop().await;
            }
            Ok::<(), String>(())
        },
    )
    .await
    {
        handle.shutdown().await;
        return Err(error);
    }
    // The mag/ESC alarm reads HIGHRES_IMU / ESC_STATUS from the target FC.
    // Stream-rate requests and the parameter read are link-scoped tasks;
    // publish their owner before starting any observers.
    let com_mot_test_en = std::sync::Arc::new(std::sync::Mutex::new(None));
    let telemetry_handle = handle.clone();
    *state.motor_test.target.lock() = Some(TargetLinkRuntime {
        target: MotorTestTarget {
            handle,
            endpoint,
            target_system_id,
            com_mot_test_en: com_mot_test_en.clone(),
        },
        runtime: AuxiliaryLinkRuntime::new(telemetry_handle.clone(), []),
    });
    // The scope needs these at useful rates; the FC defaults are low or
    // absent (ESC_STATUS only streams when asked). Stream rates only —
    // nothing persistent is written and a reboot reverts them.
    let stream_requests = {
        let handle = telemetry_handle.clone();
        tauri::async_runtime::spawn(async move {
            for (msg_id, hz) in TARGET_STREAMS {
                request_message_interval(&handle, target_system_id, msg_id, hz).await;
            }
        })
    };
    if let Some(target) = state.motor_test.target.lock().as_mut() {
        target.runtime.add_task(stream_requests);
    } else {
        stream_requests.abort();
    }
    attach_motor_telemetry(&app, telemetry_handle.clone(), target_system_id);
    // Read COM_MOT_TEST_EN from the PX4 (PX4 denies ACTUATOR_TEST while it is
    // not 1). The read is spawned so a quiet FC never stalls the connect.
    let slot = com_mot_test_en;
    let h = telemetry_handle;
    let task = tauri::async_runtime::spawn(async move {
        if let Some(v) = read_param(&h, target_system_id, "COM_MOT_TEST_EN").await {
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(v);
            }
        }
    });
    if let Some(target) = state.motor_test.target.lock().as_mut() {
        target.runtime.add_task(task);
    } else {
        task.abort();
    }
    Ok(())
}

/// Messages the scope asks the target FC to stream: `(MAVLink id, Hz)` —
/// `SERVO_OUTPUT_RAW`, `HIGHRES_IMU` (mag), `ESC_STATUS`.
const TARGET_STREAMS: [(u32, f64); 3] = [(36, 20.0), (105, 50.0), (291, 10.0)];

/// Fire-and-forget `MAV_CMD_SET_MESSAGE_INTERVAL` (PX4 accepts it while
/// disarmed; a FC that lacks the stream simply never sends it).
async fn request_message_interval(
    handle: &ConnectionHandle,
    target_system_id: u8,
    msg_id: u32,
    hz: f64,
) {
    let _ = handle
        .send(MavMessage::COMMAND_LONG(
            ::mavlink::common::COMMAND_LONG_DATA {
                param1: msg_id as f32,
                param2: (1_000_000.0 / hz) as f32,
                param3: 0.0,
                param4: 0.0,
                param5: 0.0,
                param6: 0.0,
                param7: 0.0,
                command: ::mavlink::common::MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL,
                target_system: target_system_id,
                target_component: 1,
                confirmation: 0,
            },
        ))
        .await;
}

/// Drop the motor test's dedicated target link (real PX4). The session (if
/// any) is stopped first so no frames are left addressed to a dead link.
#[tauri::command]
pub async fn actuator_disconnect_target(state: State<'_, AppState>) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    stop_before_auxiliary_close(
        async {
            stop_active_session(&state).await?;
            Ok(())
        },
        async {
            detach_motor_telemetry(&state).await;
            state.motor_test.running.lock().take();
            let old = state.motor_test.target.lock().take();
            if let Some(old) = old {
                old.stop().await;
            }
            Ok::<(), String>(())
        },
    )
    .await
}

/// Start realtime forwarding: the live SITL actuator outputs (already
/// normalized into the slot by the tap) drive the real FC's motors.
#[tauri::command]
pub async fn actuator_start_forward(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _ops = state.ops().lock().await;
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
    let target_system_id = motor_test_target_system_id(&state);
    let (svc, evt_rx, output_rx) =
        MotorTestService::spawn(handle, limits_for_load("none", target_system_id));
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
async fn run_sitl_tap(
    handle: maggcs_core::mavlink::ConnectionHandle,
    slot: ForwardSlot,
    bus: ScopeBus,
) {
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
                    // The scope plots the raw microseconds, not the clamped
                    // 0..1 the forward slot gets.
                    bus.push_all(scope::servo_samples(
                        "sitl",
                        now,
                        [
                            servo.servo1_raw,
                            servo.servo2_raw,
                            servo.servo3_raw,
                            servo.servo4_raw,
                        ],
                    ));
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
async fn read_param(handle: &ConnectionHandle, target_system_id: u8, name: &str) -> Option<f32> {
    // Subscribe before sending and accept only the configured FC's reply.
    let mut events =
        handle.subscribe_route(MessageRoute::messages(&[22]).from(target_system_id, 1));
    let req = MavMessage::PARAM_REQUEST_READ(::mavlink::common::PARAM_REQUEST_READ_DATA {
        target_system: target_system_id,
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

/// Stop the auxiliary owners outside any state lock; their tasks are
/// cancelled/joined before their connection workers are shut down.
pub(crate) async fn stop_auxiliary_links(state: &AppState) {
    detach_motor_telemetry(state).await;
    let sitl = state.motor_test.sitl.lock().take();
    if let Some(sitl) = sitl {
        sitl.stop().await;
    }
    let target = state.motor_test.target.lock().take();
    if let Some(target) = target {
        target.stop().await;
    }
}

/// Run an auxiliary-link mutation only after the active motor-test stop path
/// has completed successfully. The second future is not polled on stop failure.
async fn stop_before_auxiliary_close<T, Stop, Close>(stop: Stop, close: Close) -> Result<T, String>
where
    Stop: std::future::Future<Output = Result<(), String>>,
    Close: std::future::Future<Output = Result<T, String>>,
{
    stop.await?;
    close.await
}

async fn stop_active_session(state: &AppState) -> Result<(), String> {
    let Some(service) = service_of(state) else {
        return Ok(());
    };
    let stopped_sending = tokio::time::timeout(MOTOR_TEST_STOP_TIMEOUT, service.stop_and_wait())
        .await
        .map_err(|_| "timed out waiting for motor-test release; link kept open".to_string())?
        .map_err(|error| format!("motor-test release failed; link kept open: {error}"))?;
    if stopped_sending {
        state.motor_test.note_state(SessionState::Idle);
    }
    Ok(())
}

/// Push session events to the window, remember the state for the status
/// poll, and pin the window while sending.
async fn forward_events(app: AppHandle, mut rx: tokio::sync::mpsc::Receiver<MotorTestEvent>) {
    while let Some(ev) = rx.recv().await {
        match ev {
            MotorTestEvent::StateChanged(s) => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.motor_test.note_state(s);
                    // Exact send start/stop instants for the scope's shaded
                    // "motor test sending" band.
                    state.motor_test.scope.push_all(vec![scope::ScopeSample {
                        id: "session.running".into(),
                        t: now_epoch_s(),
                        v: if s.is_sending() { 1.0 } else { 0.0 },
                    }]);
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
        let target_system_id = self
            .motor_test
            .target
            .lock()
            .as_ref()
            .map(|runtime| runtime.target.target_system_id);
        let target_hb = self
            .motor_test
            .telemetry
            .lock()
            .as_ref()
            .and_then(|t| t.hb.clone())
            .filter(|heartbeat| {
                target_system_id.is_none_or(|system_id| heartbeat.system_id == system_id)
            });
        let (armed, custom_mode, autopilot_px4, mode_label) = match target_hb {
            Some(hb) => (
                hb.base_mode.safety_armed,
                hb.custom_mode,
                hb.autopilot == maggcs_core::telemetry::Autopilot::Px4,
                hb.mode_label,
            ),
            None => match self.hub().and_then(|h| h.snapshot().heartbeat) {
                Some(hb) if target_system_id.is_none() => (
                    hb.base_mode.safety_armed,
                    hb.custom_mode,
                    hb.autopilot == maggcs_core::telemetry::Autopilot::Px4,
                    hb.mode_label,
                ),
                _ => (false, 0, false, String::new()),
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
            .and_then(|link| link.slot.lock().ok().map(|st| st.values.clone()))
            .unwrap_or_default();
        // The OUTPUT: the values the session sent to the FC as ACTUATOR_TEST.
        let output_values = self
            .motor_test
            .running
            .lock()
            .as_ref()
            .map(|r| (*r.output.borrow()).clone())
            .unwrap_or_default();
        let (target_connected, target_endpoint, target_system_id, com_mot_test_en) = {
            let guard = self.motor_test.target.lock();
            match guard.as_ref() {
                Some(runtime) => (
                    true,
                    Some(runtime.target.endpoint.clone()),
                    Some(runtime.target.target_system_id),
                    runtime.target.com_mot_test_en.lock().ok().and_then(|g| *g),
                ),
                None => (false, None, None, None),
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
            target_system_id,
            input_values,
            output_values,
            com_mot_test_en,
        }
    }
}

/// Request the streams the motor-test window plots from the FC on `handle`:
/// `HIGHRES_IMU` (105, the mag field), `ESC_STATUS` (291, ESC current/RPM) and
/// `SERVO_OUTPUT_RAW` (36, the servo curves) at useful rates. A bare serial
/// link streams only the default set otherwise, so the curves would stay flat.
fn request_motor_test_streams(
    handle: &maggcs_core::mavlink::ConnectionHandle,
    target_system_id: u8,
) -> tauri::async_runtime::JoinHandle<()> {
    let h = handle.clone();
    tauri::async_runtime::spawn(async move {
        for (msg_id, interval_us) in [
            (105u32, 100_000u32), // HIGRES_IMU @ 10 Hz (mag total)
            (291u32, 100_000u32), // ESC_STATUS @ 10 Hz (current, RPM)
            (36u32, 40_000u32),   // SERVO_OUTPUT_RAW @ 25 Hz (servo curves)
        ] {
            let _ = h
                .send(MavMessage::COMMAND_LONG(
                    ::mavlink::common::COMMAND_LONG_DATA {
                        param1: msg_id as f32,
                        param2: interval_us as f32,
                        param3: 0.0,
                        param4: 0.0,
                        param5: 0.0,
                        param6: 0.0,
                        param7: 0.0,
                        command: ::mavlink::common::MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL,
                        target_system: target_system_id,
                        target_component: 1,
                        confirmation: 0,
                    },
                ))
                .await;
        }
    })
}

/// Attach the mag/ESC telemetry tap to a link (A4 wiring): subscribes to
/// `HIGHRES_IMU` (105, mag triple) and `ESC_STATUS` (291), applies the loaded
/// calibration, drives the alarm state machine, and emits
/// `actuator_telemetry` events for the window's curves. Called from
/// `connect` after the link comes up.
pub fn attach_motor_telemetry(
    app: &AppHandle,
    handle: maggcs_core::mavlink::ConnectionHandle,
    target_system_id: u8,
) {
    // Ask the FC to stream the messages the motor-test window plots: the mag
    // (HIGRES_IMU), the ESC telemetry (ESC_STATUS) and the servo outputs.
    // PX4 accepts MAV_CMD_SET_MESSAGE_INTERVAL while disarmed; fire-and-forget
    // (a busy FC just misses this attach — the next one retries).
    let stream_task = request_motor_test_streams(&handle, target_system_id);
    let state = app.state::<AppState>();
    // (Re)attach resets the alarm clock; the loaded calibration survives
    // (it is a file, not link state). The target runtime owns the task.
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
    let telemetry_task =
        tauri::async_runtime::spawn(run_motor_telemetry_tap(tap_app, handle, target_system_id));
    {
        let mut target = state.motor_test.target.lock();
        if let Some(target) = target.as_mut() {
            target.runtime.add_tasks([stream_task, telemetry_task]);
        } else {
            stream_task.abort();
            telemetry_task.abort();
        }
    }
}

/// The telemetry tap: `HIGHRES_IMU` (105) → calibrated `mag_total` → alarm;
/// `ESC_STATUS` (291) → the ESC aggregator; `SERVO_OUTPUT_RAW` (36) → the
/// target-FC servo trace. Emits `actuator_telemetry` (aggregated snapshot)
/// at ~10 Hz for the alarm/status readout, and pushes every sample (not a
/// snapshot) to the scope bus.
async fn run_motor_telemetry_tap(
    app: AppHandle,
    handle: maggcs_core::mavlink::ConnectionHandle,
    target_system_id: u8,
) {
    // Only telemetry from the configured target may gate motor-test safety
    // or feed the target's plots and alarm.
    let mut events = handle
        .subscribe_route(MessageRoute::messages(&[0, 36, 105, 291]).from(target_system_id, 1));
    let mut last_emit = tokio::time::Instant::now();
    let Some(bus) = app
        .try_state::<AppState>()
        .map(|s| s.motor_test.scope.clone())
    else {
        return;
    };
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
                        // Raw total always; the calibrated total (A·(raw−b))
                        // only while a calibration is loaded.
                        let t = now_epoch_s();
                        let totals = scope::mag_totals(raw, telemetry.calib.as_ref());
                        bus.push_all(scope::mag_samples(t, totals));
                        // Alarm + readout keep watching the calibrated total
                        // (raw pass-through when none is loaded).
                        let total = totals.effective();
                        telemetry.last_mag_total_nt = Some(total);
                        if telemetry.alarm.feed(total, now_epoch_s()) {
                            // Transition into Alarm: fire the audio + flash.
                            let _ = app.emit("actuator_alarm", total);
                        }
                    }
                    MavMessage::ESC_STATUS(esc) => {
                        telemetry
                            .esc
                            .feed(esc.index, &esc.rpm, &esc.voltage, &esc.current);
                        bus.push_all(scope::esc_samples(
                            esc.index,
                            now_epoch_s(),
                            &esc.rpm,
                            &esc.voltage,
                            &esc.current,
                        ));
                    }
                    MavMessage::SERVO_OUTPUT_RAW(servo) => {
                        // What the target FC reports on its main outputs —
                        // the serial-side counterpart of the SITL servos.
                        bus.push_all(scope::servo_samples(
                            "fc",
                            now_epoch_s(),
                            [
                                servo.servo1_raw,
                                servo.servo2_raw,
                                servo.servo3_raw,
                                servo.servo4_raw,
                            ],
                        ));
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
pub async fn detach_motor_telemetry(state: &AppState) {
    let tasks = state
        .motor_test
        .target
        .lock()
        .as_mut()
        .map(|target| target.runtime.take_tasks())
        .unwrap_or_default();
    crate::motor_test_state::stop_auxiliary_tasks(tasks).await;
    *state.motor_test.telemetry.lock() = None;
}

/// How often the flusher drains the scope bus into one `actuator_samples`
/// event (20 Hz keeps the IPC light while the plot still looks continuous).
const SCOPE_FLUSH: Duration = Duration::from_millis(50);

/// Start the scope flusher (idempotent): drains the bus, adds the sender's
/// command readback, and emits `actuator_samples` batches while the window
/// is open. It stops with the window.
fn start_scope_flusher(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut slot = state.motor_test.scope_flusher.lock();
    if slot.is_some() {
        return;
    }
    state.motor_test.scope.clear();
    let app = app.clone();
    *slot = Some(tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(SCOPE_FLUSH);
        // After a stall, do not fire a burst of catch-up ticks.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // True while the last command sample was non-zero, so the fall back
        // to 0 after STOP is recorded once and then the trace goes quiet.
        let mut cmd_active = false;
        // Keep re-asking the target PX4 for the scope streams: the one-shot
        // request at connect can be lost (busy, link settling), and the
        // curves would stay silent forever (the reported "no mag data").
        let mut last_stream_req = tokio::time::Instant::now() - Duration::from_secs(3);
        loop {
            tick.tick().await;
            if last_stream_req.elapsed() >= Duration::from_secs(3) {
                last_stream_req = tokio::time::Instant::now();
                // Clone the target handle first: the `State` guard is not Send
                // and must not live across the awaited sends below.
                let target = app.try_state::<AppState>().and_then(|state| {
                    state.motor_test.target.lock().as_ref().map(|runtime| {
                        (
                            runtime.target.handle.clone(),
                            runtime.target.target_system_id,
                        )
                    })
                });
                if let Some((target, target_system_id)) = target {
                    for (msg_id, hz) in TARGET_STREAMS {
                        request_message_interval(&target, target_system_id, msg_id, hz).await;
                    }
                }
            }
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let mut batch = state.motor_test.scope.drain();
            let values = state
                .motor_test
                .running
                .lock()
                .as_ref()
                .map(|r| (*r.output.borrow()).clone());
            if let Some(values) = values {
                let live = values.iter().any(|v| *v > 0.0);
                if live || cmd_active {
                    batch.extend(scope::command_samples(now_epoch_s(), &values));
                }
                cmd_active = live;
            } else {
                cmd_active = false;
            }
            if !batch.is_empty() {
                let _ = app.emit("actuator_samples", &batch);
            }
        }
    }));
}

/// Stop the scope flusher (window closed).
fn stop_scope_flusher(state: &AppState) {
    if let Some(task) = state.motor_test.scope_flusher.lock().take() {
        task.abort();
    }
}

#[cfg(test)]
mod auxiliary_link_lifecycle_tests {
    use super::{stop_before_auxiliary_close, validate_target_system_id};

    #[test]
    fn target_system_id_must_not_be_broadcast() {
        assert_eq!(
            validate_target_system_id(0),
            Err("target system ID must be between 1 and 255".to_string())
        );
        assert_eq!(validate_target_system_id(1), Ok(1));
        assert_eq!(validate_target_system_id(255), Ok(255));
    }
    use maggcs_core::mavlink::connection::{spawn_connection, ConnectionConfig, ConnectionHandle};
    use maggcs_core::mavlink::endpoint::Endpoint;
    use maggcs_core::mavlink::{MavHeader, MavMessage};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::timeout;

    type Peer = Box<dyn ::mavlink::AsyncMavConnection<::mavlink::common::MavMessage> + Sync + Send>;

    async fn loopback_link() -> (ConnectionHandle, Peer) {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").expect("reserve UDP port");
        let port = probe.local_addr().expect("port address").port();
        drop(probe);
        let config = ConnectionConfig {
            endpoint: Endpoint::UdpListener {
                addr: format!("127.0.0.1:{port}")
                    .parse()
                    .expect("listener address"),
            },
            ..Default::default()
        };
        let (handle, _events, first) = spawn_connection(config).await.expect("spawn link");
        timeout(Duration::from_secs(2), first)
            .await
            .expect("listener startup timed out")
            .expect("first result channel")
            .expect("listener bind");
        let mut peer = ::mavlink::connect_async::<::mavlink::common::MavMessage>(&format!(
            "udpout:127.0.0.1:{port}"
        ))
        .await
        .expect("connect fake FC");
        peer.set_protocol_version(::mavlink::MavlinkVersion::V2);
        let heartbeat = MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
            autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: ::mavlink::common::MavModeFlag::empty(),
            system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        });
        peer.send(
            &MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            &heartbeat,
        )
        .await
        .expect("fake FC heartbeat");
        (handle, peer)
    }

    fn probe_message() -> MavMessage {
        MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: ::mavlink::common::MavType::MAV_TYPE_GCS,
            autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_GENERIC,
            base_mode: ::mavlink::common::MavModeFlag::empty(),
            system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        })
    }

    #[tokio::test]
    async fn stop_failure_preserves_the_auxiliary_link() {
        let close_ran = Arc::new(AtomicBool::new(false));
        let close_flag = close_ran.clone();
        let result = stop_before_auxiliary_close(
            async { Err::<(), _>("release confirmation failed".to_string()) },
            async move {
                close_flag.store(true, Ordering::SeqCst);
                Ok::<_, String>(())
            },
        )
        .await;

        assert_eq!(result, Err("release confirmation failed".to_string()));
        assert!(
            !close_ran.load(Ordering::SeqCst),
            "failed release must leave the old auxiliary link open"
        );

        let (old_link, fake_fc) = loopback_link().await;
        let old_for_close = old_link.clone();
        let result = stop_before_auxiliary_close(
            async { Err::<(), _>("release confirmation failed".to_string()) },
            async move {
                old_for_close.shutdown().await;
                Ok::<_, String>(())
            },
        )
        .await;
        assert!(result.is_err());
        timeout(
            Duration::from_secs(2),
            old_link.send_confirmed(probe_message()),
        )
        .await
        .expect("old link write confirmation timed out")
        .expect("failed stop must leave old link writable");
        timeout(Duration::from_secs(2), fake_fc.recv())
            .await
            .expect("fake FC did not receive frame")
            .expect("fake FC receive failed");
        old_link.shutdown().await;
    }

    #[tokio::test]
    async fn auxiliary_link_close_runs_after_release_confirmation() {
        let released = Arc::new(AtomicBool::new(false));
        let stop_flag = released.clone();
        let close_flag = released.clone();
        let result = stop_before_auxiliary_close(
            async move {
                stop_flag.store(true, Ordering::SeqCst);
                Ok::<_, String>(())
            },
            async move {
                assert!(
                    close_flag.load(Ordering::SeqCst),
                    "the stop future must finish before closing the link"
                );
                Ok::<_, String>(())
            },
        )
        .await;

        assert_eq!(result, Ok(()));

        let (old_link, _fake_fc) = loopback_link().await;
        let link_to_close = old_link.clone();
        let result = stop_before_auxiliary_close(async { Ok::<(), String>(()) }, async move {
            link_to_close.shutdown().await;
            Ok::<_, String>(())
        })
        .await;
        assert_eq!(result, Ok(()));

        let probe_until = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match timeout(
                Duration::from_millis(250),
                old_link.send_confirmed(probe_message()),
            )
            .await
            {
                Ok(Err(_)) => break,
                Ok(Ok(())) if tokio::time::Instant::now() < probe_until => {
                    tokio::task::yield_now().await;
                }
                Ok(Ok(())) => panic!("closed auxiliary link continued accepting writes"),
                Err(_) => panic!("closed auxiliary link write confirmation hung"),
            }
        }
    }
}

#[cfg(test)]
mod start_refusal_tests {
    use std::time::Duration;

    use maggcs_core::commands::CommandService;
    use maggcs_core::lifecycle::StopReason;
    use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
    use maggcs_core::mission::service::{MissionIds, MissionService};
    use maggcs_core::telemetry::hub::{TelemetryHub, DEFAULT_PUSH_HZ};
    use maggcs_core::telemetry::VehicleId;
    use tauri::test::mock_app;
    use tauri::Manager;

    use crate::state::AppState;
    use crate::state::{ActiveLink, LinkId};

    use super::fc_link;

    /// Regression (user report 2026-10-17): with a **live primary link** (a
    /// flying PX4 SITL) and **no serial motor-test target** connected, Start
    /// must refuse with the connect-the-target error — not silently spawn
    /// the session on the primary link, where the flying vehicle's armed
    /// heartbeats stop it instantly (`Stopped(Armed)`).
    #[tokio::test(flavor = "multi_thread")]
    async fn motor_test_start_refuses_without_a_serial_target_even_with_a_primary_link() {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").expect("reserve UDP port");
        let port = probe.local_addr().expect("probe address").port();
        drop(probe);
        let config = ConnectionConfig {
            endpoint: Endpoint::UdpListener {
                addr: format!("127.0.0.1:{port}")
                    .parse()
                    .expect("listener address"),
            },
            ..Default::default()
        };
        let (handle, events, first) = maggcs_core::mavlink::connection::spawn_connection(config)
            .await
            .expect("spawn primary link");
        tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .expect("listener startup timed out")
            .expect("first result channel")
            .expect("listener bound");

        let app = mock_app();
        app.manage(AppState::default());
        let state = app.state::<AppState>();
        let hub = TelemetryHub::spawn(events, DEFAULT_PUSH_HZ, VehicleId::new(1, 1));
        let (mission, _mission_events) = MissionService::spawn(
            handle.clone(),
            MissionIds {
                self_system: 255,
                self_component: 190,
                target_system: 1,
                target_component: 1,
            },
        );
        let (command, _command_events) = CommandService::spawn(handle.clone(), 1, 1);
        state.set_primary_link(ActiveLink {
            id: LinkId::next(),
            connection: handle,
            hub,
            mission,
            command,
            logs: None,
            params: None,
            rtk: None,
        });

        // The primary link is up, but it must NOT satisfy the motor test.
        assert!(fc_link(&state).is_none());

        // The start commands are `fc_link(..).ok_or("no link — connect
        // the serial PX4 first")`: with `fc_link` returning None the
        // refusal text is produced by construction, and no session can be
        // spawned. (`AppHandle<MockRuntime>` cannot call the Wry-typed
        // command fn directly, so the guard is the fc_link contract.)
        assert!(
            state.motor_test.running.lock().is_none(),
            "no session may be spawned on the primary link"
        );

        // Teardown of the test's own primary link.
        let _ = crate::lifecycle_service::run(app.handle(), StopReason::Disconnect).await;
    }
}
