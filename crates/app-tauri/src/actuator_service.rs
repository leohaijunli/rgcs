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

use maggcs_core::logs::capture::SdCaptureStatus;
use maggcs_core::mavlink::connection::{spawn_connection, ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::mavlink::{MavMessage, MessageEnvelope};
use maggcs_core::motor_test::mapping::{
    build_output_map, mapped_feedback_pwm, OutputMap, ServoPort, SERVO_CHANNELS_PER_PORT,
};
use maggcs_core::motor_test::pwm::PwmModel;
use maggcs_core::motor_test::scope::{self, ScopeBus};
use maggcs_core::motor_test::service::{MotorTestCommand, MotorTestEvent, MotorTestService};
use maggcs_core::motor_test::source::{ForwardSlot, ForwardState};
use maggcs_core::motor_test::{SessionState, DEFAULT_TICK_HZ};
use maggcs_core::params::model::{ParamType, ParamValue};
use maggcs_core::params::{ParamEntry, ParamObservation, ParamService};

/// Maximum time to observe the release pass before closing an actuator link.
const MOTOR_TEST_STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// How old the last target-FC heartbeat may be before the window shows the
/// serial target as "heartbeat lost". PX4 streams HEARTBEAT at 1 Hz.
const TARGET_HEARTBEAT_STALE: Duration = Duration::from_secs(3);
/// Motor functions the window displays and the forward slot carries.
const MOTOR_TEST_MOTOR_COUNT: u8 = 4;
/// Older SERVO_OUTPUT_RAW samples are not shown as live feedback.
const FC_OUTPUT_STALE: Duration = Duration::from_secs(2);

use crate::motor_test_state::limits_for_load;
use crate::motor_test_state::{
    AuxiliaryLinkRuntime, FcPortOutputs, MotorTestLoad, MotorTestTarget, MotorTestTargetRef,
    SitlLinkRuntime, SitlSourceConfig, TargetLinkRuntime,
};
use crate::state::AppState;

/// Status snapshot for the window's header and right panel (polled ~1 Hz by
/// the UI; safety-relevant signals are low-rate).
#[derive(Debug, Clone, Serialize)]
pub struct ActuatorStatus {
    pub connected: bool,
    pub fc_alive: bool,
    /// The serial target PX4's heartbeat is fresh (`false` when no target is
    /// connected). Distinct from `fc_alive`, which describes the primary link.
    pub target_fc_alive: bool,
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
    /// Node and PWM bank selected for the SITL source.
    pub sitl_source: Option<SitlSourceConfig>,
    /// Seconds since the last SERVO_OUTPUT_RAW sample; null = none yet.
    pub sitl_feed_age_s: Option<f64>,
    /// The motor test's own target link (real PX4, serial): connected + endpoint.
    pub target_connected: bool,
    pub target_endpoint: Option<String>,
    pub target_system_id: Option<u8>,
    /// `COM_MOT_TEST_EN` read from the target PX4 (`None` = unknown): PX4
    /// denies ACTUATOR_TEST while this is not 1.
    pub com_mot_test_en: Option<i32>,
    /// `SDLOG_MODE` read from the target PX4 (`None` = unknown): 0 = armed
    /// until disarm. This is startup policy, not the logger's recording state.
    pub sdlog_mode: Option<i32>,
    pub sdlog_backend: Option<i32>,
    pub parameter_restart_pending: bool,
    /// Normalized SITL servo outputs 1–4 (the forward INPUT), latest sample.
    pub input_values: Vec<f32>,
    /// The SITL's raw `SERVO_OUTPUT_RAW` 1–4 in µs (what `input_values` was
    /// normalized from). Empty when no SITL link is attached.
    pub input_pwm: Vec<u16>,
    /// Values actually sent to the FC as ACTUATOR_TEST (the OUTPUT), 0 when idle.
    pub output_values: Vec<f32>,
    /// Converted queued command PWM in µs; None when a motor is released.
    /// These are predictions from the sender's models, not FC measurements.
    pub output_pwm: Vec<Option<f32>>,
    /// The target FC's actual PWM for each motor function in µs, selected from
    /// the mapped MAIN/AUX channel. `None` = unmapped or no sample yet.
    pub fc_output_pwm: Vec<Option<u16>>,
    /// Motor function → physical output mapping read from `PWM_*_FUNCx`.
    pub output_map: OutputMap,
    /// Per-motor target PWM model (endpoints, reversal, `THR_MDL_FAC`,
    /// disabled) used to compensate `ACTUATOR_TEST` and forward raw pulses.
    pub pwm_models: Vec<PwmModel>,
    /// Target-shell SD-file capture state (Task 6).
    pub sd_capture: SdCaptureStatus,
    /// Current/last per-test run + recovery manifest (Task 8).
    pub run: crate::run_service::RunSnapshot,
}

/// Open the motor-test window, or focus it if it already exists.
/// Must run asynchronously: synchronous WebView2 creation deadlocks on Windows.
#[tauri::command]
pub async fn actuator_open(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("actuator") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, "actuator", WebviewUrl::App("actuator.html".into()))
        .devtools(true)
        .title("Motor test")
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
    stop_auxiliary_links(&app, &state).await;
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

/// A start that passed target identity, heartbeat and load checks. Holding
/// it binds the new session to the exact connection the checks were made on.
struct PreparedStart {
    handle: ConnectionHandle,
    target: MotorTestTargetRef,
    load: MotorTestLoad,
    limits: maggcs_core::motor_test::SafetyLimits,
    /// Captured with the target identity, so every start sends exactly the
    /// models that were current when readiness was checked.
    pwm_models: Vec<PwmModel>,
}

/// Verify the current target before any session is created or reused. The
/// caller holds [`AppState::ops`], so the target cannot be replaced between
/// these checks and the spawn.
fn prepare_start(state: &AppState, load: MotorTestLoad) -> Result<PreparedStart, String> {
    let (handle, target, pwm_models) = {
        let guard = state.motor_test.target.lock();
        let runtime = guard
            .as_ref()
            .ok_or("no serial PX4 target — connect it first")?;
        let pwm_models = runtime.target.pwm_models.lock().clone();
        (
            runtime.target.handle.clone(),
            runtime.target.target_ref(),
            pwm_models,
        )
    };
    refuse_unread_pwm_ranges(&pwm_models)?;
    ensure_target_disarmed(state, target)?;
    Ok(PreparedStart {
        handle,
        target,
        load,
        limits: limits_for_load(load, target.target_system_id),
        pwm_models,
    })
}

/// Read-only readiness for logger control as well as motor starts. The
/// caller holds AppState::ops; this never sends an arm/disarm command.
pub(crate) fn ensure_target_disarmed(
    state: &AppState,
    target: MotorTestTargetRef,
) -> Result<(), String> {
    if !state
        .motor_test
        .target
        .lock()
        .as_ref()
        .is_some_and(|runtime| runtime.target.target_ref() == target)
    {
        return Err(maggcs_core::logs::capture::SdCaptureError::TargetChanged.to_string());
    }
    let (armed, heartbeat_age) = {
        let telemetry = state.motor_test.telemetry.lock();
        let telemetry = telemetry
            .as_ref()
            .ok_or("target telemetry is not attached — reconnect the serial PX4")?;
        let heartbeat = telemetry
            .hb
            .as_ref()
            .ok_or("waiting for the target PX4 heartbeat — connect it first")?;
        if heartbeat.system_id != target.target_system_id
            || heartbeat.component_id != target.target_component_id
        {
            return Err("target heartbeat identity changed — reconnect the serial PX4".into());
        }
        if heartbeat.autopilot != maggcs_core::telemetry::Autopilot::Px4 {
            return Err("target is not a PX4 autopilot".into());
        }
        let at = telemetry
            .last_heartbeat_at
            .ok_or("waiting for the target PX4 heartbeat — connect it first")?;
        (heartbeat.base_mode.safety_armed, at.elapsed())
    };
    if heartbeat_age > TARGET_HEARTBEAT_STALE {
        return Err("target heartbeat is stale — check the serial PX4 link".into());
    }
    if armed {
        return Err("target is armed — disarm it before starting".into());
    }
    Ok(())
}

/// Refuse a start while any motor's PWM range could not be read from the FC
/// (the fallback 1100–1900 would look real but is not). The operator must
/// refresh the mapping until every launched motor reports its FC range.
fn refuse_unread_pwm_ranges(models: &[PwmModel]) -> Result<(), String> {
    if models.len() != usize::from(MOTOR_TEST_MOTOR_COUNT) {
        return Err(format!(
            "PWM ranges incomplete: expected {MOTOR_TEST_MOTOR_COUNT} FC-read motor models, got {} — refresh the mapping first",
            models.len()
        ));
    }
    let unread = models
        .iter()
        .enumerate()
        .filter(|(_, model)| {
            model.range_source == maggcs_core::motor_test::pwm::PwmRangeSource::Fallback
        })
        .map(|(index, _)| index + 1)
        .collect::<Vec<_>>();
    if unread.is_empty() {
        return Ok(());
    }
    Err(format!(
        "PWM range not read from the PX4 for motor(s) {} — refresh the mapping first \
         (check the serial link and PWM_MAIN/AUX_MINn/MAXn)",
        unread
            .iter()
            .map(|motor| format!("M{motor}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Refuse a new start while the existing session is not idle. A failed
/// release keeps `Stopping`, and the emergency latch needs an explicit reset.
pub(crate) fn ensure_idle_session(state: &AppState) -> Result<(), String> {
    match state.motor_test.session() {
        Some(SessionState::ManualRunning) => Err("motor test is already running".into()),
        Some(SessionState::Stopping) => {
            Err("motor-test release is still pending — retry Stop first".into())
        }
        Some(SessionState::Emergency) => {
            Err("reset the emergency latch before starting again".into())
        }
        _ => Ok(()),
    }
}

/// Reuse only a session bound to the same target and load. An idle session
/// from another load/target is stopped (release confirmed) before replacement;
/// a running one is never silently re-bounded.
async fn reusable_session(
    state: &AppState,
    prepared: &PreparedStart,
) -> Result<Option<MotorTestService>, String> {
    let running = state.motor_test.running.lock().as_ref().map(|running| {
        (
            running.svc.clone(),
            running.matches(prepared.target, prepared.load),
        )
    });
    let Some((service, matches)) = running else {
        return Ok(None);
    };
    if matches {
        return Ok(Some(service));
    }
    if state
        .motor_test
        .session()
        .is_some_and(|session| session.is_sending())
    {
        return Err("stop the motor test before changing the prop load or target".into());
    }
    service
        .stop_and_wait()
        .await
        .map_err(|error| format!("cannot replace the old motor-test session: {error}"))?;
    state.motor_test.running.lock().take();
    Ok(None)
}

fn spawn_session(app: &AppHandle, state: &AppState, prepared: &PreparedStart) -> MotorTestService {
    let (service, events, output) =
        MotorTestService::spawn(prepared.handle.clone(), prepared.limits.clone());
    let forwarder = tauri::async_runtime::spawn(forward_events(app.clone(), events));
    *state.motor_test.running.lock() = Some(crate::motor_test_state::RunningMotorTest {
        svc: service.clone(),
        forwarder,
        output,
        target: prepared.target,
        load: prepared.load,
    });
    service
}

/// Send the target PWM models before a start. The session applies them to
/// every subsequent value; a failed send aborts before the start command.
async fn send_pwm_models(
    service: &MotorTestService,
    prepared: &PreparedStart,
) -> Result<(), String> {
    service
        .send(MotorTestCommand::SetPwmModels {
            models: prepared.pwm_models.clone(),
        })
        .await
        .map_err(|error| error.to_string())
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
    ensure_idle_session(&state)?;
    let load = MotorTestLoad::parse(&load)?;
    let prepared = prepare_start(&state, load)?;
    crate::run_service::authorize_motor_start(&app, &state)?;
    if let Some(service) = reusable_session(&state, &prepared).await? {
        if send_pwm_models(&service, &prepared).await.is_ok()
            && service
                .send(MotorTestCommand::SetValues(values.clone()))
                .await
                .is_ok()
            && service.send(MotorTestCommand::StartManual).await.is_ok()
        {
            return confirm_start_or_release(&app, &state).await;
        }
        state.motor_test.running.lock().take();
    }
    let service = spawn_session(&app, &state, &prepared);
    send_pwm_models(&service, &prepared).await?;
    service
        .send(MotorTestCommand::SetValues(values))
        .await
        .map_err(|error| error.to_string())?;
    service
        .send(MotorTestCommand::StartManual)
        .await
        .map_err(|error| error.to_string())?;
    confirm_start_or_release(&app, &state).await
}

/// The run manifest may only claim `MotorRunning` after the start was sent.
/// If that bookkeeping fails, stop the session rather than leave motors
/// running without a run.
async fn confirm_start_or_release(app: &AppHandle, state: &AppState) -> Result<(), String> {
    match crate::run_service::confirm_motor_started(app, state) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = stop_active_session(state).await;
            Err(error)
        }
    }
}

/// Start a preset waveform (all motors in sync for its duration; the
/// session stops itself with `Completed` when it ends).
#[tauri::command]
pub async fn actuator_start_preset(
    app: AppHandle,
    state: State<'_, AppState>,
    load: String,
    preset: maggcs_core::motor_test::source::WaveformPreset,
) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    ensure_idle_session(&state)?;
    let load = MotorTestLoad::parse(&load)?;
    let prepared = prepare_start(&state, load)?;
    crate::run_service::authorize_motor_start(&app, &state)?;
    if let Some(service) = reusable_session(&state, &prepared).await? {
        if send_pwm_models(&service, &prepared).await.is_ok()
            && service
                .send(MotorTestCommand::StartPreset { preset })
                .await
                .is_ok()
        {
            return confirm_start_or_release(&app, &state).await;
        }
        state.motor_test.running.lock().take();
    }
    let service = spawn_session(&app, &state, &prepared);
    send_pwm_models(&service, &prepared).await?;
    service
        .send(MotorTestCommand::StartPreset { preset })
        .await
        .map_err(|error| error.to_string())?;
    confirm_start_or_release(&app, &state).await
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
    stop_active_session(&state).await
}

/// Emergency stop: latched in the session; the UI mirrors it.
#[tauri::command]
pub async fn actuator_estop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(service) = service_of(&state) {
        let receipt = service
            .emergency_stop_and_wait()
            .await
            .map_err(|error| error.to_string())?;
        state.motor_test.note_state(receipt.state);
    } else {
        state.motor_test.note_state(SessionState::Emergency);
    }
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
/// connection whose selected node/port `SERVO_OUTPUT_RAW` channels 1–4 feed
/// the forward slot, normalized to 0..=1. Idempotent: a previous SITL link is
/// dropped.
#[tauri::command]
pub async fn actuator_connect_sitl(
    state: State<'_, AppState>,
    source: SitlSourceConfig,
) -> Result<(), String> {
    if source.system_id == 0 || source.component_id == 0 {
        return Err("SITL source system and component IDs must be 1–255".into());
    }
    let config = maggcs_core::mavlink::ConnectionConfig {
        endpoint: maggcs_core::mavlink::Endpoint::try_from(source.endpoint.as_str())
            .map_err(|e| e.to_string())?,
        system_id: 250,
        component_id: 250,
        target_system_id: source.system_id,
        target_component_id: source.component_id,
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
                target_system: source.system_id,
                target_component: source.component_id,
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
        maggcs_core::motor_test::source::ForwardState::new(usize::from(MOTOR_TEST_MOTOR_COUNT)),
    ));
    let bus = state.motor_test.scope.clone();
    let tap = tauri::async_runtime::spawn(run_sitl_tap(
        handle.clone(),
        source.clone(),
        slot.clone(),
        bus,
    ));
    *state.motor_test.sitl.lock() = Some(SitlLinkRuntime {
        slot,
        source,
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
            crate::run_service::interrupt_active(&app, &state, "serial target link replaced");
            crate::capture_service::stop_actuator_capture(&state).await;
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
    let (params, _param_events, parameter_task) =
        ParamService::spawn_with_task(handle.clone(), target_system_id, 1);
    let initial_params = params.clone();
    let output_map = std::sync::Arc::new(parking_lot::Mutex::new(OutputMap::unknown(
        MOTOR_TEST_MOTOR_COUNT,
    )));
    let mapping_slot = output_map.clone();
    let pwm_models = std::sync::Arc::new(parking_lot::Mutex::new(vec![
        PwmModel::default();
        MOTOR_TEST_MOTOR_COUNT
            as usize
    ]));
    let pwm_models_slot = pwm_models.clone();
    let telemetry_handle = handle.clone();
    *state.motor_test.target.lock() = Some(TargetLinkRuntime {
        target: MotorTestTarget {
            handle,
            endpoint,
            target_system_id,
            params,
            restart_pending: Default::default(),
            output_map,
            pwm_models,
        },
        runtime: AuxiliaryLinkRuntime::new(
            telemetry_handle.clone(),
            [tauri::async_runtime::spawn(async move {
                let _ = parameter_task.await;
            })],
        ),
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
    // Discovery is read-only. Permission changes use an explicit verified set.
    let task = tauri::async_runtime::spawn(async move {
        let _ = initial_params.request_fc_version().await;
        for name in [
            "COM_MOT_TEST_EN",
            "SDLOG_MODE",
            "SDLOG_BACKEND",
            "THR_MDL_FAC",
        ] {
            let _ = initial_params.read(name).await;
        }
        let map = read_output_map(&initial_params).await;
        let models = read_pwm_models(&initial_params, &map).await;
        *mapping_slot.lock() = map;
        *pwm_models_slot.lock() = models;
    });
    if let Some(target) = state.motor_test.target.lock().as_mut() {
        target.runtime.add_task(task);
    } else {
        task.abort();
    }
    // Rebuild the SD-log runtime on the new target handle (feature 2).
    respawn_actuator_logs(app.clone(), &state);
    // Task 6: one shell controller bound to this target/shell.
    crate::capture_service::spawn_actuator_capture(
        app.clone(),
        &state,
        telemetry_handle,
        target_system_id,
    );
    // Task 8: close runs that a previous process left pending on this PX4.
    if let Some(target) = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| runtime.target.target_ref())
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            crate::run_service::reconcile_target(app, target).await;
        });
    }
    Ok(())
}

/// Messages the scope asks the target FC to stream: `(MAVLink id, Hz)` —
/// `SERVO_OUTPUT_RAW`, `HIGHRES_IMU` (mag), `BATTERY_STATUS` (current),
/// `ESC_STATUS`.
const TARGET_STREAMS: [(u32, f64); 4] = [(36, 20.0), (105, 50.0), (147, 5.0), (291, 10.0)];

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
pub async fn actuator_disconnect_target(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    stop_before_auxiliary_close(
        async {
            stop_active_session(&state).await?;
            Ok(())
        },
        async {
            detach_motor_telemetry(&state).await;
            crate::run_service::interrupt_active(&app, &state, "serial target disconnected");
            crate::capture_service::stop_actuator_capture(&state).await;
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
/// normalized into the slot by the tap) drive the real FC's motors. The
/// declared load still caps the forwarded demand.
#[tauri::command]
pub async fn actuator_start_forward(
    app: AppHandle,
    state: State<'_, AppState>,
    load: String,
) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    ensure_idle_session(&state)?;
    let load = MotorTestLoad::parse(&load)?;
    // The slot is fed by the SITL tap; forwarding without it is meaningless.
    let slot = state
        .motor_test
        .sitl
        .lock()
        .as_ref()
        .map(|s| s.slot.clone())
        .ok_or("connect the SITL source first")?;
    let prepared = prepare_start(&state, load)?;
    crate::run_service::authorize_motor_start(&app, &state)?;
    if let Some(service) = reusable_session(&state, &prepared).await? {
        if send_pwm_models(&service, &prepared).await.is_ok()
            && service
                .send(MotorTestCommand::StartForward { slot: slot.clone() })
                .await
                .is_ok()
        {
            return confirm_start_or_release(&app, &state).await;
        }
        state.motor_test.running.lock().take();
    }
    let service = spawn_session(&app, &state, &prepared);
    send_pwm_models(&service, &prepared).await?;
    service
        .send(MotorTestCommand::StartForward { slot })
        .await
        .map_err(|error| error.to_string())?;
    confirm_start_or_release(&app, &state).await
}

/// Standard PPM range of `SERVO_OUTPUT_RAW`: 1000 µs = 0 %, 2000 µs = 100 %.
const PWM_MIN_US: f32 = 1000.0;
const PWM_MAX_US: f32 = 2000.0;

/// Map a `SERVO_OUTPUT_RAW` value to 0..=1. Values outside the PPM range are
/// clamped, so a non-standard output can never forward more than 100 %.
fn pwm_to_norm(pwm: u16) -> f32 {
    ((f32::from(pwm) - PWM_MIN_US) / (PWM_MAX_US - PWM_MIN_US)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod ppm_tests {
    use super::pwm_to_norm;

    #[test]
    fn servo_pwm_maps_1000_to_2000_onto_zero_to_one() {
        assert_eq!(pwm_to_norm(1000), 0.0);
        assert_eq!(pwm_to_norm(1500), 0.5);
        assert_eq!(pwm_to_norm(2000), 1.0);
    }

    #[test]
    fn out_of_range_pwm_is_clamped_not_forwarded_above_100_percent() {
        assert_eq!(pwm_to_norm(0), 0.0);
        assert_eq!(pwm_to_norm(999), 0.0);
        assert_eq!(pwm_to_norm(2001), 1.0);
        assert_eq!(pwm_to_norm(u16::MAX), 1.0);
    }
}

/// Apply one routed frame to the forward slot. Returns the raw channel values
/// when the frame belongs to the selected node and port; foreign frames,
/// wrong ports and other message types return `None` without touching state.
fn apply_sitl_frame(
    source: &SitlSourceConfig,
    env: &MessageEnvelope,
    state: &mut ForwardState,
    now_s: f64,
) -> Option<[u16; 4]> {
    if env.header.system_id != source.system_id || env.header.component_id != source.component_id {
        return None;
    }
    let MavMessage::SERVO_OUTPUT_RAW(servo) = &env.message else {
        return None;
    };
    if ServoPort::from_wire(servo.port) != Some(source.port) {
        return None;
    }
    let pwm = [
        servo.servo1_raw,
        servo.servo2_raw,
        servo.servo3_raw,
        servo.servo4_raw,
    ];
    state.values = pwm.map(pwm_to_norm).to_vec();
    state.raw_pwm = pwm.to_vec();
    state.last_update_s = Some(now_s);
    Some(pwm)
}

/// Tap task: the selected SITL node/port `SERVO_OUTPUT_RAW` channels 1–4 →
/// normalized 0..=1 into the forward slot (latest value wins; the tap never
/// blocks the sender).
async fn run_sitl_tap(
    handle: maggcs_core::mavlink::ConnectionHandle,
    source: SitlSourceConfig,
    slot: ForwardSlot,
    bus: ScopeBus,
) {
    let mut events = handle
        .subscribe_route(MessageRoute::messages(&[36]).from(source.system_id, source.component_id));
    loop {
        match events.recv().await {
            Ok(ConnectionEvent::Message(env)) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                let pwm = {
                    let Ok(mut state) = slot.lock() else {
                        continue;
                    };
                    apply_sitl_frame(&source, &env, &mut state, now)
                };
                if let Some(pwm) = pwm {
                    // The scope plots the raw microseconds, not the clamped
                    // 0..1 the forward slot gets.
                    bus.push_all(scope::servo_samples("sitl", now, pwm));
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

fn target_params(state: &AppState) -> Result<(usize, ParamService), String> {
    state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| {
            (
                runtime.target.handle.handle_id(),
                runtime.target.params.clone(),
            )
        })
        .ok_or_else(|| "no serial PX4 target — connect it first".into())
}

fn ensure_parameter_target(state: &AppState, link_id: usize) -> Result<(), String> {
    if state
        .motor_test
        .target
        .lock()
        .as_ref()
        .is_some_and(|runtime| runtime.target.handle.handle_id() == link_id)
    {
        Ok(())
    } else {
        Err("parameter target changed during transaction".into())
    }
}

#[tauri::command]
pub async fn actuator_param_read(
    state: State<'_, AppState>,
    name: String,
) -> Result<ParamObservation, String> {
    let (id, svc) = target_params(&state)?;
    let entry = svc.read(&name).await.map_err(|error| error.to_string())?;
    ensure_parameter_target(&state, id)?;
    Ok(ParamObservation { entry, age_ms: 0 })
}

#[tauri::command]
pub async fn actuator_param_set(
    state: State<'_, AppState>,
    name: String,
    value: ParamValue,
    ptype: ParamType,
) -> Result<ParamEntry, String> {
    let _ops = state.ops().lock().await;
    if state
        .motor_test
        .session()
        .is_some_and(|session| session.is_sending())
    {
        return Err("stop the motor test before changing target parameters".into());
    }
    let (id, svc) = target_params(&state)?;
    let current = svc.read(&name).await.map_err(|error| error.to_string())?;
    if current.ptype != ptype {
        return Err(format!(
            "{name} type mismatch: expected {:?}",
            current.ptype
        ));
    }
    let entry = svc
        .set_confirmed(&name, value, ptype)
        .await
        .map_err(|error| error.to_string())?;
    ensure_parameter_target(&state, id)?;
    if matches!(
        name.as_str(),
        "SDLOG_MODE" | "SDLOG_BACKEND" | "SDLOG_PROFILE"
    ) {
        if let Some(target) = state.motor_test.target.lock().as_ref() {
            target
                .target
                .restart_pending
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    Ok(entry)
}

/// Enable or disable `COM_MOT_TEST_EN` on the target PX4. PX4 refuses
/// `MAV_CMD_ACTUATOR_TEST` while this is not 1, so the window enables it
/// automatically on connect; this command lets the operator flip it back.
#[tauri::command]
pub async fn actuator_set_mot_test_en(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<i32, String> {
    let value = i32::from(enabled);
    actuator_param_set(
        state,
        "COM_MOT_TEST_EN".into(),
        ParamValue::Int(value as i64),
        ParamType::I32,
    )
    .await?;
    Ok(value)
}

/// Rebuild the SD-log runtime for the current serial target PX4. Idempotent:
/// a previous runtime is stopped and its last list/error kept for the UI.
fn respawn_actuator_logs<R: tauri::Runtime>(app: AppHandle<R>, state: &AppState) {
    let Some(target) = state.motor_test.target.lock().as_ref().map(|runtime| {
        (
            runtime.target.handle.clone(),
            runtime.target.target_system_id,
            1,
        )
    }) else {
        return;
    };
    let (handle, target_system_id, target_component_id) = target;
    let mut logs = state.motor_test.logs.lock();
    if let Some(old) = logs.runtime.take() {
        // Abort in the background; the current handle is gone (or replaced).
        drop(old);
    }
    let last_list = logs.last_list.clone();
    let last_error = logs.last_error.clone();
    let task_app = app.clone();
    let (cmd_tx, mut cmd_rx) =
        tokio::sync::mpsc::channel::<crate::motor_test_state::ActuatorLogCommand>(4);
    let mut route = handle.subscribe_log_messages();
    let task = tauri::async_runtime::spawn(async move {
        use maggcs_core::logs::protocol::{LogEvent, LogProtocol};
        let mut protocol = LogProtocol::new(target_system_id, target_component_id);
        let mut list_reply: Option<
            tokio::sync::oneshot::Sender<Result<Vec<maggcs_core::logs::LogEntryInfo>, String>>,
        > = None;
        let mut save_paths: std::collections::HashMap<u16, String> =
            std::collections::HashMap::new();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(200));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let emit_failed = |error: &str| {
            eprintln!("actuator_logs error: {error}");
            *last_error.lock().expect("logs err lock") = Some(error.to_string());
            let _ = task_app.emit(
                "actuator_logs",
                &actuator_logs_payload_failed(error.to_string()),
            );
        };
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    let Some(cmd) = cmd else { return };
                    let frames = match cmd {
                        crate::motor_test_state::ActuatorLogCommand::List { reply } => {
                            if protocol.is_busy() {
                                let _ = reply.send(Err("an SD log operation is already active; cancel it first".into()));
                                continue;
                            }
                            *last_list.lock().expect("logs list lock") = None;
                            *last_error.lock().expect("logs err lock") = None;
                            list_reply = Some(reply);
                            protocol.begin_list()
                        }
                        crate::motor_test_state::ActuatorLogCommand::Download { id, size, save_path } => {
                            if protocol.is_busy() {
                                emit_failed("an SD log operation is already active; cancel it first");
                                continue;
                            }
                            *last_error.lock().expect("logs err lock") = None;
                            save_paths.insert(id, save_path);
                            protocol.begin_download(id, size)
                        }
                        crate::motor_test_state::ActuatorLogCommand::Cancel => {
                            if let Some(reply) = list_reply.take() {
                                let _ = reply.send(Err("SD log operation canceled".into()));
                            }
                            save_paths.clear();
                            protocol.cancel()
                        }
                    };
                    for f in frames {
                        if let Err(error) = handle.send_confirmed(f).await {
                            emit_failed(&error.to_string());
                            return;
                        }
                    }
                }
                msg = route.recv() => {
                    let envelope = match crate::logs_service::classify_route(msg) {
                        crate::logs_service::RouteStep::Message(envelope) => envelope,
                        crate::logs_service::RouteStep::Skip => continue,
                        crate::logs_service::RouteStep::Closed => return,
                    };
                    let (events, frames) = match protocol.handle(
                        &envelope.message,
                        std::time::Instant::now(),
                    ) {
                        Ok(v) => v,
                        Err(e) => {
                            for f in protocol.cancel() { let _ = handle.send_confirmed(f).await; }
                            save_paths.clear();
                            emit_failed(&e.to_string());
                            continue;
                        }
                    };
                    for f in frames {
                        if let Err(error) = handle.send_confirmed(f).await {
                            emit_failed(&error.to_string());
                            return;
                        }
                    }
                    for e in events {
                        match e {
                            LogEvent::List { entries } => {
                                *last_list.lock().expect("logs list lock") = Some(entries.clone());
                                *last_error.lock().expect("logs err lock") = None;
                                if let Some(reply) = list_reply.take() {
                                    let _ = reply.send(Ok(entries.clone()));
                                }
                                let _ = app.emit("actuator_logs", &ActuatorLogsEvent::List { entries });
                            }
                            LogEvent::Progress { id, fetched, total } => {
                                let _ = app.emit("actuator_logs", &ActuatorLogsEvent::Progress { id, fetched, total });
                            }
                            LogEvent::Downloaded { id, bytes } => {
                                let path = save_paths.remove(&id)
                                    .unwrap_or_else(|| format!("log{id}.ulg"));
                                let payload = match tokio::fs::write(&path, &bytes).await {
                                    Ok(()) => {
                                        tracing::info!(id, bytes = bytes.len(), path, "SD log download completed");
                                        ActuatorLogsEvent::Downloaded { id, path }
                                    },
                                    Err(e) => ActuatorLogsEvent::Failed { message: format!("write {}: {e}", path) },
                                };
                                let _ = app.emit("actuator_logs", &payload);
                            }
                        }
                    }
                }
                _ = tick.tick() => {
                    match protocol.tick(std::time::Instant::now()) {
                        Ok(frames) => {
                            for f in frames {
                                if let Err(error) = handle.send_confirmed(f).await {
                                    emit_failed(&error.to_string());
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            for f in protocol.cancel() { let _ = handle.send_confirmed(f).await; }
                            if let Some(reply) = list_reply.take() {
                                // The command will verify PX4's silent-empty case
                                // against the SD directory before publishing success.
                                let _ = reply.send(Err(e.to_string()));
                            } else {
                                emit_failed(&e.to_string());
                            }
                            save_paths.clear();
                        }
                    }
                }
            }
        }
    });
    logs.runtime = Some(crate::motor_test_state::ActuatorLogsRuntime {
        commands: Some(cmd_tx),
        task: Some(task),
    });
}

/// Stop the SD-log runtime for the serial target (link teardown / window
/// close). Keeps the last list/error for a re-open.
pub(crate) async fn stop_actuator_logs(state: &AppState) {
    let old = { state.motor_test.logs.lock().runtime.take() };
    if let Some(old) = old {
        old.stop(std::time::Duration::from_secs(2)).await;
    }
}

/// Serializable SD-log events for the motor-test window (`actuator_logs`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum ActuatorLogsEvent {
    #[serde(rename_all = "snake_case")]
    List {
        entries: Vec<maggcs_core::logs::LogEntryInfo>,
    },
    #[serde(rename_all = "snake_case")]
    Progress { id: u16, fetched: u32, total: u32 },
    #[serde(rename_all = "snake_case")]
    Downloaded { id: u16, path: String },
    #[serde(rename_all = "snake_case")]
    Failed { message: String },
}

fn actuator_logs_payload_failed(message: String) -> ActuatorLogsEvent {
    ActuatorLogsEvent::Failed { message }
}

fn actuator_logs_sender(
    state: &AppState,
) -> Option<tokio::sync::mpsc::Sender<crate::motor_test_state::ActuatorLogCommand>> {
    state
        .motor_test
        .logs
        .lock()
        .runtime
        .as_ref()
        .and_then(crate::motor_test_state::ActuatorLogsRuntime::sender)
}

fn publish_actuator_log_list<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    entries: Vec<maggcs_core::logs::LogEntryInfo>,
) {
    let logs = state.motor_test.logs.lock();
    *logs.last_list.lock().expect("logs list lock") = Some(entries.clone());
    *logs.last_error.lock().expect("logs err lock") = None;
    let _ = app.emit("actuator_logs", ActuatorLogsEvent::List { entries });
}

fn invalidate_actuator_log_list(state: &AppState) {
    let logs = state.motor_test.logs.lock();
    *logs.last_list.lock().expect("logs list lock") = None;
    *logs.last_error.lock().expect("logs err lock") = None;
}

/// Caller holds ops, so no target/capture/deletion mutation can change the
/// index between its request and the verified result.
async fn refresh_actuator_log_list<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
) -> Result<Vec<maggcs_core::logs::LogEntryInfo>, String> {
    if actuator_logs_sender(state).is_none() {
        respawn_actuator_logs(app.clone(), state);
    }
    let tx = actuator_logs_sender(state)
        .ok_or_else(|| "no serial PX4 target — connect it first".to_string())?;
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(crate::motor_test_state::ActuatorLogCommand::List { reply })
        .await
        .map_err(|_| "log service stopped".to_string())?;
    let result = response
        .await
        .map_err(|_| "log service stopped".to_string())?;
    match result {
        Ok(entries) => Ok(entries),
        Err(error) if error.contains("no reply to log list") => {
            if crate::capture_service::logs_directory_is_empty(state).await? {
                publish_actuator_log_list(app, state, Vec::new());
                Ok(Vec::new())
            } else {
                Err(format!(
                    "{error}; SD files still exist, so refresh the list before downloading"
                ))
            }
        }
        Err(error) => Err(error),
    }
}

fn same_log_entry(
    expected: &maggcs_core::logs::LogEntryInfo,
    entries: &[maggcs_core::logs::LogEntryInfo],
) -> Result<maggcs_core::logs::LogEntryInfo, String> {
    entries
        .iter()
        .find(|entry| {
            entry.id == expected.id
                && entry.time_utc == expected.time_utc
                && entry.size == expected.size
        })
        .cloned()
        .ok_or_else(|| {
            "SD log list changed; use the refreshed list before downloading or deleting".into()
        })
}

/// Request the serial target PX4's SD log list (feature 2). The runtime is
/// (re)spawned on the current target handle if it is not running.
#[tauri::command]
pub async fn actuator_logs_request_list(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<maggcs_core::logs::LogEntryInfo>, String> {
    let _ops = state.ops().lock().await;
    refresh_actuator_log_list(&app, &state).await
}

/// Download one SD log from the serial target PX4 into a save-dialog path
/// (feature 2). A cancelled save dialog returns Ok without starting.
#[tauri::command]
pub async fn actuator_logs_download(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u16,
    size: u32,
    time_utc: u32,
) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    if size == 0 {
        return Err("this log is empty (0 bytes); nothing to download".into());
    }
    let expected = maggcs_core::logs::LogEntryInfo { id, size, time_utc };
    let path = app
        .dialog()
        .file()
        .add_filter("ULog", &["ulg"])
        .set_file_name(format!("log{id}.ulg"))
        .blocking_save_file()
        .and_then(|p| p.into_path().ok());
    let Some(path) = path else { return Ok(false) };
    let _ops = state.ops().lock().await;
    crate::capture_service::ensure_logger_not_recording(&state).await?;
    let entries = refresh_actuator_log_list(&app, &state).await?;
    let entry = same_log_entry(&expected, &entries)?;
    let tx = actuator_logs_sender(&state)
        .ok_or_else(|| "no serial PX4 target — connect it first".to_string())?;
    tx.send(crate::motor_test_state::ActuatorLogCommand::Download {
        id,
        size: entry.size,
        save_path: path.display().to_string(),
    })
    .await
    .map_err(|_| "log service stopped".to_string())?;
    Ok(true)
}

/// Permanently delete every SD log on the target. The UI performs two
/// confirmations; this command additionally requires a fresh observation that
/// the logger is not recording.
#[tauri::command]
pub async fn actuator_logs_erase_all(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<maggcs_core::logs::LogEntryInfo>, String> {
    let _ops = state.ops().lock().await;
    ensure_idle_session(&state)?;
    crate::capture_service::ensure_logger_not_recording(&state).await?;
    let handle = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| runtime.target.handle.clone())
        .ok_or("no serial PX4 target — connect it first")?;
    stop_actuator_logs(&state).await;
    invalidate_actuator_log_list(&state);
    let mut protocol = maggcs_core::logs::LogProtocol::new(
        handle.config().target_system_id,
        handle.config().target_component_id,
    );
    for frame in protocol.begin_erase() {
        handle
            .send_confirmed(frame)
            .await
            .map_err(|error| error.to_string())?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if crate::capture_service::logs_directory_is_empty(&state).await? {
            publish_actuator_log_list(&app, &state, Vec::new());
            respawn_actuator_logs(app, &state);
            tracing::info!("all SD logs deleted and empty directory confirmed");
            return Ok(Vec::new());
        }
        if tokio::time::Instant::now() >= deadline {
            respawn_actuator_logs(app.clone(), &state);
            let entries = refresh_actuator_log_list(&app, &state).await?;
            publish_actuator_log_list(&app, &state, entries);
            return Err(
                "PX4 has not confirmed deletion of every SD log; inspect the refreshed list".into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Permanently delete one SD log by id. The id must be from the current list;
/// PX4 assigns it from `logdata.txt` order, which this command reads directly.
#[tauri::command]
pub async fn actuator_logs_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u16,
    size: u32,
    time_utc: u32,
) -> Result<Vec<maggcs_core::logs::LogEntryInfo>, String> {
    let _ops = state.ops().lock().await;
    ensure_idle_session(&state)?;
    crate::capture_service::ensure_logger_not_recording(&state).await?;
    let expected = maggcs_core::logs::LogEntryInfo { id, size, time_utc };
    let entries = refresh_actuator_log_list(&app, &state).await?;
    let entry = same_log_entry(&expected, &entries)?;
    crate::capture_service::delete_log_by_id(&state, &entry).await?;
    invalidate_actuator_log_list(&state);
    if entries.len() == 1 && crate::capture_service::logs_directory_is_empty(&state).await? {
        publish_actuator_log_list(&app, &state, Vec::new());
        return Ok(Vec::new());
    }
    refresh_actuator_log_list(&app, &state).await
}

/// Cancel any running SD-log list/download on the serial target.
#[tauri::command]
pub async fn actuator_logs_cancel(state: State<'_, AppState>) -> Result<(), String> {
    let tx = actuator_logs_sender(&state).ok_or_else(|| "no serial PX4 target".to_string())?;
    tx.send(crate::motor_test_state::ActuatorLogCommand::Cancel)
        .await
        .map_err(|_| "log service stopped".to_string())
}

/// Latest SD-log list + error for the motor-test window.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ActuatorLogsStatus {
    pub entries: Vec<maggcs_core::logs::LogEntryInfo>,
    pub error: Option<String>,
}

#[tauri::command]
pub fn actuator_logs_status(state: State<'_, AppState>) -> ActuatorLogsStatus {
    let logs = state.motor_test.logs.lock();
    let entries = logs
        .last_list
        .lock()
        .expect("logs list lock")
        .clone()
        .unwrap_or_default();
    let error = logs.last_error.lock().expect("logs err lock").clone();
    ActuatorLogsStatus { entries, error }
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
pub(crate) async fn stop_auxiliary_links(app: &AppHandle, state: &AppState) {
    detach_motor_telemetry(state).await;
    crate::run_service::interrupt_active(app, state, "auxiliary links closed");
    // Feature 2: stop the serial-target SD-log and capture runtimes (both are
    // link-scoped). The last list/error survive for a re-open.
    stop_actuator_logs(state).await;
    crate::capture_service::stop_actuator_capture(state).await;
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

pub(crate) async fn stop_active_session(state: &AppState) -> Result<(), String> {
    let Some(service) = service_of(state) else {
        return Ok(());
    };
    let receipt = tokio::time::timeout(MOTOR_TEST_STOP_TIMEOUT, service.stop_and_wait())
        .await
        .map_err(|_| "timed out waiting for motor-test release; link kept open".to_string())?
        .map_err(|error| format!("motor-test release failed; link kept open: {error}"))?;
    state.motor_test.note_state(receipt.state);
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
                let _ = app.emit("actuator_event", SessionPayload::State { state: s });
            }
            MotorTestEvent::Stopped(reason) => {
                eprintln!("motor test stopped: {reason:?}");
                if let Some(state) = app.try_state::<AppState>() {
                    crate::run_service::note_motor_released(&app, &state, reason);
                }
                pin_window(&app, false);
                let _ = app.emit("actuator_event", SessionPayload::Stopped { reason });
            }
            MotorTestEvent::ReleaseFailed(error) => {
                let _ = app.emit("actuator_stop_warning", error.to_string());
            }
        }
    }
}

/// Serializable event payloads for the window (`actuator_event`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SessionPayload {
    State {
        state: SessionState,
    },
    Stopped {
        reason: maggcs_core::motor_test::service::StopReason,
    },
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
        // The serial target's heartbeat age, so the window can show the link
        // it actually commands instead of only the primary link.
        let target_fc_alive = self
            .motor_test
            .telemetry
            .lock()
            .as_ref()
            .and_then(|t| t.last_heartbeat_at)
            .is_some_and(|at| at.elapsed() <= TARGET_HEARTBEAT_STALE);
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
        let (sitl_connected, sitl_endpoint, sitl_source, sitl_feed_age_s) = {
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
                    (
                        true,
                        Some(link.source.endpoint.clone()),
                        Some(link.source.clone()),
                        age,
                    )
                }
                None => (false, None, None, None),
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
        // The same sample's raw microseconds, so the window shows the exact
        // source value the percentage came from.
        let input_pwm = self
            .motor_test
            .sitl
            .lock()
            .as_ref()
            .and_then(|link| link.slot.lock().ok().map(|st| st.raw_pwm.clone()))
            .unwrap_or_default();
        // The OUTPUT: the values the session sent to the FC as ACTUATOR_TEST.
        let (output_values, output_pwm) = self
            .motor_test
            .running
            .lock()
            .as_ref()
            .map(|r| ((*r.output.borrow()).clone(), r.svc.pwm_output()))
            .unwrap_or_default();
        // The target FC's actual SERVO_OUTPUT_RAW (what PX4 drives), captured
        // by the serial telemetry tap, separately per bank.
        let (main_pwm, aux_pwm) = {
            let telemetry = self.motor_test.telemetry.lock();
            match telemetry.as_ref() {
                Some(t) => (
                    t.fc_outputs[ServoPort::Main.wire() as usize]
                        .filter(|o| o.at.elapsed() <= FC_OUTPUT_STALE)
                        .map(|o| o.pwm),
                    t.fc_outputs[ServoPort::Aux.wire() as usize]
                        .filter(|o| o.at.elapsed() <= FC_OUTPUT_STALE)
                        .map(|o| o.pwm),
                ),
                None => (None, None),
            }
        };
        let (
            target_connected,
            target_endpoint,
            target_system_id,
            com_mot_test_en,
            sdlog_mode,
            output_map,
            pwm_models,
            sd_capture,
        ) = {
            let guard = self.motor_test.target.lock();
            match guard.as_ref() {
                Some(runtime) => (
                    true,
                    Some(runtime.target.endpoint.clone()),
                    Some(runtime.target.target_system_id),
                    cached_int(&runtime.target.params, "COM_MOT_TEST_EN"),
                    cached_int(&runtime.target.params, "SDLOG_MODE"),
                    runtime.target.output_map.lock().clone(),
                    runtime.target.pwm_models.lock().clone(),
                    self.motor_test.capture.lock().status.lock().clone(),
                ),
                None => (
                    false,
                    None,
                    None,
                    None,
                    None,
                    OutputMap::unknown(MOTOR_TEST_MOTOR_COUNT),
                    Vec::new(),
                    self.motor_test.capture.lock().status.lock().clone(),
                ),
            }
        };
        let fc_output_pwm = (1..=MOTOR_TEST_MOTOR_COUNT)
            .map(|motor| {
                mapped_feedback_pwm(&output_map, main_pwm.as_ref(), aux_pwm.as_ref(), motor)
            })
            .collect();
        // Both fields come from the same target entry. `parking_lot::Mutex`
        // is not re-entrant, so they must be read under one guard: locking
        // twice in the `ActuatorStatus` literal deadlocks the status poll
        // (the window then never finishes loading).
        let (sdlog_backend, parameter_restart_pending) = {
            let guard = self.motor_test.target.lock();
            match guard.as_ref() {
                Some(runtime) => (
                    cached_int(&runtime.target.params, "SDLOG_BACKEND"),
                    runtime
                        .target
                        .restart_pending
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
                None => (None, false),
            }
        };
        ActuatorStatus {
            connected: link
                .as_ref()
                .map(|l| l.link_state == maggcs_core::mavlink::LinkState::Connected)
                .unwrap_or(false),
            fc_alive: link.as_ref().map(|l| l.fc_alive).unwrap_or(false),
            target_fc_alive,
            armed,
            custom_mode,
            autopilot_px4,
            mode_label,
            endpoint: link.map(|l| l.endpoint),
            tick_hz: DEFAULT_TICK_HZ,
            session: session.to_string(),
            sitl_connected,
            sitl_endpoint,
            sitl_source,
            sitl_feed_age_s,
            target_connected,
            target_endpoint,
            target_system_id,
            input_values,
            input_pwm,
            output_values,
            output_pwm,
            com_mot_test_en,
            sdlog_mode,
            sdlog_backend,
            parameter_restart_pending,
            fc_output_pwm,
            output_map,
            pwm_models,
            sd_capture,
            run: crate::run_service::snapshot(&self.motor_test.run),
        }
    }
}

fn cached_int(service: &ParamService, name: &str) -> Option<i32> {
    match service.cache().get(name)?.value {
        ParamValue::Int(value) => i32::try_from(value).ok(),
        _ => None,
    }
}

/// Read `PWM_MAIN_FUNC1..8` and `PWM_AUX_FUNC1..8` from the serial target and
/// build the Motor-function → physical-output map. Missing parameters are
/// reported as unreadable, not mistaken for "disabled".
async fn read_output_map(params: &ParamService) -> OutputMap {
    let mut main = [None; SERVO_CHANNELS_PER_PORT as usize];
    let mut aux = [None; SERVO_CHANNELS_PER_PORT as usize];
    for port in [ServoPort::Main, ServoPort::Aux] {
        let values = match port {
            ServoPort::Main => &mut main,
            ServoPort::Aux => &mut aux,
        };
        for channel in 1..=SERVO_CHANNELS_PER_PORT {
            let name = format!("{}_FUNC{channel}", port.param_prefix());
            if let Ok(entry) = params.read(&name).await {
                if let ParamValue::Int(value) = entry.value {
                    values[usize::from(channel - 1)] = Some(value);
                }
            }
        }
    }
    build_output_map(MOTOR_TEST_MOTOR_COUNT, &main, &aux)
}

/// Read one integer parameter, if it is currently readable.
async fn read_int_param(params: &ParamService, name: &str) -> Option<i32> {
    match params.read(name).await.ok()?.value {
        ParamValue::Int(value) => i32::try_from(value).ok(),
        ParamValue::Real(value) => Some(value as i32),
    }
}

/// Build one model from the raw `PWM_*_` parameter reads. Unmapped motors or
/// missing/invalid endpoints fall back explicitly (PwmRangeSource::Fallback
/// plus the 1100–1900 default) — never silently to a plausible-looking range.
fn pwm_model_from_raw(
    min: Option<i32>,
    max: Option<i32>,
    reversed: bool,
    disabled: bool,
    thrust_factor: f32,
    mapped: bool,
) -> PwmModel {
    if !mapped {
        return PwmModel::default();
    }
    min.zip(max)
        .and_then(|(min, max)| {
            Some(PwmModel::new(
                u16::try_from(min).ok()?,
                u16::try_from(max).ok()?,
                reversed,
                thrust_factor,
            ))
        })
        .and_then(Result::ok)
        .unwrap_or_default()
        .with_disabled(disabled)
}

/// Read the mapped channel's `PWM_*_MIN/MAX/REV/DISn` plus the global
/// `THR_MDL_FAC` and build one model per motor. Missing/invalid ranges and
/// unmapped motors are marked fallback; the start gate rejects them.
async fn read_pwm_models(params: &ParamService, map: &OutputMap) -> Vec<PwmModel> {
    let factor = match params.read("THR_MDL_FAC").await.map(|entry| entry.value) {
        Ok(ParamValue::Real(value)) => value,
        Ok(ParamValue::Int(value)) => value as f32,
        Err(_) => 0.0,
    };
    let factor = if factor.is_finite() {
        factor.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut models = Vec::with_capacity(usize::from(MOTOR_TEST_MOTOR_COUNT));
    for motor in 1..=MOTOR_TEST_MOTOR_COUNT {
        let Some(channel) = map.channel_for(motor) else {
            models.push(PwmModel::default());
            continue;
        };
        let prefix = channel.port.param_prefix();
        let n = channel.channel;
        let min = read_int_param(params, &format!("{prefix}_MIN{n}")).await;
        let max = read_int_param(params, &format!("{prefix}_MAX{n}")).await;
        let reversed = read_int_param(params, &format!("{prefix}_REV{n}"))
            .await
            .is_some_and(|value| value != 0);
        let disabled = read_int_param(params, &format!("{prefix}_DIS{n}"))
            .await
            .is_some_and(|value| value != 0);
        let mapped = map.channel_for(motor).is_some();
        models.push(pwm_model_from_raw(
            min, max, reversed, disabled, factor, mapped,
        ));
    }
    models
}

/// Re-read the target's output assignments and PWM models. Safe to call while
/// idle; the UI freezes the action while a test is sending.
#[tauri::command]
pub async fn actuator_refresh_mapping(state: State<'_, AppState>) -> Result<OutputMap, String> {
    let (params, slot, models_slot) = {
        let guard = state.motor_test.target.lock();
        let runtime = guard
            .as_ref()
            .ok_or("no serial PX4 target — connect it first")?;
        (
            runtime.target.params.clone(),
            runtime.target.output_map.clone(),
            runtime.target.pwm_models.clone(),
        )
    };
    let map = read_output_map(&params).await;
    let models = read_pwm_models(&params, &map).await;
    *slot.lock() = map.clone();
    *models_slot.lock() = models;
    Ok(map)
}

/// Request the streams the motor-test window plots from the FC on `handle`:
/// `HIGHRES_IMU` (105, the mag field), `BATTERY_STATUS` (147, total current),
/// `ESC_STATUS` (291, ESC current/RPM) and `SERVO_OUTPUT_RAW` (36, the servo
/// curves) at useful rates. A bare serial link streams only the default set
/// otherwise, so the curves would stay flat.
fn request_motor_test_streams(
    handle: &maggcs_core::mavlink::ConnectionHandle,
    target_system_id: u8,
) -> tauri::async_runtime::JoinHandle<()> {
    let h = handle.clone();
    tauri::async_runtime::spawn(async move {
        for (msg_id, interval_us) in [
            (105u32, 100_000u32), // HIGRES_IMU @ 10 Hz (mag total)
            (147u32, 200_000u32), // BATTERY_STATUS @ 5 Hz (total current)
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

/// Attach the mag/ESC/battery telemetry tap to a link (A4 wiring): subscribes
/// to `HIGHRES_IMU` (105, mag triple), `BATTERY_STATUS` (147, total current) and
/// `ESC_STATUS` (291), applies the loaded calibration, drives the alarm state
/// machine, and emits `actuator_telemetry` events for the window's curves.
/// Called from `connect` after the link comes up.
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
        fc_outputs: [None, None],
        last_heartbeat_at: None,
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

/// Record one bank without disturbing the other: MAIN 1 and AUX 1 are
/// different physical outputs.
fn store_fc_output(
    outputs: &mut [Option<FcPortOutputs>; 2],
    port: ServoPort,
    pwm: [u16; SERVO_CHANNELS_PER_PORT as usize],
    at: std::time::Instant,
) {
    outputs[port.wire() as usize] = Some(FcPortOutputs { pwm, at });
}

/// The telemetry tap: `HIGHRES_IMU` (105) → calibrated `mag_total` → alarm;
/// `ESC_STATUS` (291) → the ESC aggregator; `BATTERY_STATUS` (147) → the total
/// battery current trace; `SERVO_OUTPUT_RAW` (36) → the target-FC servo trace.
/// Emits `actuator_telemetry` (aggregated snapshot) at ~10 Hz for the
/// alarm/status readout, and pushes every sample (not a snapshot) to the scope
/// bus.
async fn run_motor_telemetry_tap(
    app: AppHandle,
    handle: maggcs_core::mavlink::ConnectionHandle,
    target_system_id: u8,
) {
    // Only telemetry from the configured target may gate motor-test safety
    // or feed the target's plots and alarm.
    let mut events = handle
        .subscribe_route(MessageRoute::messages(&[0, 36, 105, 147, 291]).from(target_system_id, 1));
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
                        telemetry.last_heartbeat_at = Some(std::time::Instant::now());
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
                    MavMessage::BATTERY_STATUS(bat) => {
                        // Total battery current (PX4 sends a negative value
                        // when it does not measure current); the scope shows
                        // one `bat{k}.current` trace per battery id.
                        bus.push_all(scope::battery_samples(
                            bat.id,
                            now_epoch_s(),
                            bat.current_battery,
                        ));
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
                        // Keep MAIN and AUX separate: the same channel number
                        // is a different physical output on each bank.
                        let Some(port) = ServoPort::from_wire(servo.port) else {
                            continue;
                        };
                        let pwm = [
                            servo.servo1_raw,
                            servo.servo2_raw,
                            servo.servo3_raw,
                            servo.servo4_raw,
                            servo.servo5_raw,
                            servo.servo6_raw,
                            servo.servo7_raw,
                            servo.servo8_raw,
                        ];
                        store_fc_output(
                            &mut telemetry.fc_outputs,
                            port,
                            pwm,
                            std::time::Instant::now(),
                        );
                        bus.push_all(scope::servo_samples(
                            "fc",
                            now_epoch_s(),
                            [pwm[0], pwm[1], pwm[2], pwm[3]],
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
        // Emit the final release gap once after the last active PWM command.
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
            let pwm_us = state
                .motor_test
                .running
                .lock()
                .as_ref()
                .map(|r| r.svc.pwm_output());
            if let Some(pwm_us) = pwm_us {
                let live = pwm_us.iter().any(Option::is_some);
                if live || cmd_active {
                    batch.extend(scope::command_samples(now_epoch_s(), &pwm_us));
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

    use super::{prepare_start, MotorTestLoad};

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
        assert_eq!(
            prepare_start(&state, MotorTestLoad::None).map(|_| ()),
            Err("no serial PX4 target — connect it first".to_string())
        );

        // `prepare_start` is the guard every start command runs before it can
        // spawn a session, so the refusal is enforced by the production path
        // (not inferred from a test-only helper).
        assert!(
            state.motor_test.running.lock().is_none(),
            "no session may be spawned on the primary link"
        );

        // Teardown of the test's own primary link.
        let _ = crate::lifecycle_service::run(app.handle(), StopReason::Disconnect).await;
    }
}

#[cfg(test)]
mod start_readiness_tests {
    use std::time::{Duration, Instant};

    use maggcs_core::logs::capture::CapturePhase;
    use maggcs_core::mavlink::{ConnectionConfig, ConnectionHandle, Endpoint};
    use maggcs_core::motor_test::mag_alarm::{MagAlarm, MagAlarmConfig};
    use maggcs_core::motor_test::mapping::OutputMap;
    use maggcs_core::motor_test::pwm::PwmModel;
    use maggcs_core::motor_test::SessionState;
    use maggcs_core::params::ParamService;
    use maggcs_core::signals::derived::EscAggregator;
    use maggcs_core::telemetry::{Autopilot, BaseMode, FlightState, Heartbeat, VehicleType};

    use crate::motor_test_state::{
        AuxiliaryLinkRuntime, MotorTelemetry, MotorTestLoad, MotorTestTarget, TargetLinkRuntime,
    };
    use crate::state::AppState;

    use super::{ensure_idle_session, prepare_start, TARGET_HEARTBEAT_STALE};

    fn mark_capture_recording(state: &AppState) {
        state.motor_test.capture.lock().status.lock().phase = CapturePhase::Recording;
    }

    fn refusal(state: &AppState, load: MotorTestLoad) -> String {
        prepare_start(state, load)
            .err()
            .expect("start must be refused")
    }

    /// Models that look like they were read from the FC (the start gate passes).
    fn fc_models() -> Vec<PwmModel> {
        vec![PwmModel::new(1100, 1900, false, 0.0).expect("model"); 4]
    }

    async fn install_target(state: &AppState, system_id: u8) -> ConnectionHandle {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").expect("reserve UDP port");
        let port = probe.local_addr().expect("probe address").port();
        drop(probe);
        let (handle, _events, first) =
            maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
                endpoint: Endpoint::UdpListener {
                    addr: format!("127.0.0.1:{port}")
                        .parse()
                        .expect("listener address"),
                },
                ..Default::default()
            })
            .await
            .expect("spawn target link");
        tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .expect("listener startup timed out")
            .expect("first result channel")
            .expect("listener bound");
        let (params, _param_events) = ParamService::spawn(handle.clone(), system_id, 1);
        let runtime_handle = handle.clone();
        *state.motor_test.target.lock() = Some(TargetLinkRuntime {
            target: MotorTestTarget {
                handle: handle.clone(),
                endpoint: format!("udp:127.0.0.1:{port}"),
                target_system_id: system_id,
                params,
                restart_pending: Default::default(),
                output_map: std::sync::Arc::new(parking_lot::Mutex::new(OutputMap::unknown(4))),
                pwm_models: std::sync::Arc::new(parking_lot::Mutex::new(fc_models())),
            },
            runtime: AuxiliaryLinkRuntime::new(runtime_handle, []),
        });
        handle
    }

    fn heartbeat(system_id: u8, armed: bool) -> Heartbeat {
        Heartbeat {
            system_id,
            component_id: 1,
            vehicle_type: VehicleType::Quadrotor,
            autopilot: Autopilot::Px4,
            base_mode: BaseMode::from_raw(if armed { BaseMode::BIT_SAFETY_ARMED } else { 0 }),
            custom_mode: 0,
            mode_label: String::new(),
            flight_state: FlightState::Standby,
            mavlink_version: 3,
        }
    }

    fn install_telemetry(state: &AppState, hb: Option<Heartbeat>, age: Option<Duration>) {
        *state.motor_test.telemetry.lock() = Some(MotorTelemetry {
            esc: EscAggregator::new(8),
            alarm: MagAlarm::new(MagAlarmConfig::default()),
            calib: None,
            last_mag_total_nt: None,
            hb,
            fc_outputs: [None, None],
            last_heartbeat_at: age.map(|age| Instant::now() - age),
        });
    }

    /// Task 3 acceptance: every start is bound to the current target and only
    /// passes with a fresh, matching, disarmed PX4 heartbeat.
    #[tokio::test(flavor = "multi_thread")]
    async fn prepare_start_requires_a_fresh_matching_disarmed_px4() {
        let state = AppState::default();
        assert_eq!(
            refusal(&state, MotorTestLoad::None),
            "no serial PX4 target — connect it first"
        );

        let handle = install_target(&state, 1).await;
        assert_eq!(
            refusal(&state, MotorTestLoad::None),
            "target telemetry is not attached — reconnect the serial PX4"
        );

        install_telemetry(&state, Some(heartbeat(1, false)), None);
        assert_eq!(
            refusal(&state, MotorTestLoad::None),
            "waiting for the target PX4 heartbeat — connect it first"
        );

        install_telemetry(&state, Some(heartbeat(1, true)), Some(Duration::ZERO));
        assert_eq!(
            refusal(&state, MotorTestLoad::None),
            "target is armed — disarm it before starting"
        );

        install_telemetry(
            &state,
            Some(heartbeat(1, false)),
            Some(TARGET_HEARTBEAT_STALE + Duration::from_millis(1)),
        );
        assert_eq!(
            refusal(&state, MotorTestLoad::None),
            "target heartbeat is stale — check the serial PX4 link"
        );

        install_telemetry(&state, Some(heartbeat(99, false)), Some(Duration::ZERO));
        assert_eq!(
            refusal(&state, MotorTestLoad::None),
            "target heartbeat identity changed — reconnect the serial PX4"
        );

        let mut wrong_component = heartbeat(1, false);
        wrong_component.component_id = 2;
        install_telemetry(&state, Some(wrong_component), Some(Duration::ZERO));
        assert_eq!(
            refusal(&state, MotorTestLoad::None),
            "target heartbeat identity changed — reconnect the serial PX4"
        );

        let mut ardupilot = heartbeat(1, false);
        ardupilot.autopilot = Autopilot::ArduPilot;
        install_telemetry(&state, Some(ardupilot), Some(Duration::ZERO));
        assert_eq!(
            refusal(&state, MotorTestLoad::None),
            "target is not a PX4 autopilot"
        );

        install_telemetry(&state, Some(heartbeat(1, false)), Some(Duration::ZERO));
        let custom_model = PwmModel::new(1100, 1900, true, 0.5)
            .expect("model")
            .with_disabled(true);
        if let Some(runtime) = state.motor_test.target.lock().as_mut() {
            *runtime.target.pwm_models.lock() = vec![custom_model; 4];
        }
        mark_capture_recording(&state);
        let prepared = prepare_start(&state, MotorTestLoad::Props).expect("ready target");
        assert_eq!(prepared.pwm_models, vec![custom_model; 4]);
        assert_eq!(prepared.target.connection_id, handle.handle_id());
        assert_eq!(prepared.target.target_system_id, 1);
        assert_eq!(prepared.target.target_component_id, 1);
        assert_eq!(prepared.load, MotorTestLoad::Props);
        assert_eq!(prepared.limits.max_value, 0.3);
        assert_eq!(prepared.limits.max_slew_per_s, 2.0);
        assert_eq!(
            prepared.limits.heartbeat_timeout,
            Duration::from_millis(1500)
        );

        let target = state.motor_test.target.lock().take();
        if let Some(target) = target {
            target.stop().await;
        }
        *state.motor_test.telemetry.lock() = None;
    }

    /// Manual SD capture is optional, even after a failed or existing capture.
    #[tokio::test(flavor = "multi_thread")]
    async fn prepare_start_does_not_require_an_sd_capture() {
        let state = AppState::default();
        install_target(&state, 1).await;
        install_telemetry(&state, Some(heartbeat(1, false)), Some(Duration::ZERO));
        for phase in [
            CapturePhase::Idle,
            CapturePhase::Ready,
            CapturePhase::Recording,
            CapturePhase::ExistingRecording,
            CapturePhase::Failed,
            CapturePhase::Complete,
        ] {
            state.motor_test.capture.lock().status.lock().phase = phase;
            assert!(
                prepare_start(&state, MotorTestLoad::None).is_ok(),
                "{phase:?}"
            );
        }

        let target = state.motor_test.target.lock().take();
        if let Some(target) = target {
            target.stop().await;
        }
        *state.motor_test.telemetry.lock() = None;
    }

    /// Acceptance: a start is refused while any motor's PWM range could not
    /// be read from the FC (the explicit fallback) — the operator must
    /// refresh the mapping first.
    #[tokio::test(flavor = "multi_thread")]
    async fn unread_pwm_ranges_refuse_a_start() {
        let state = AppState::default();
        install_target(&state, 1).await;
        install_telemetry(&state, Some(heartbeat(1, false)), Some(Duration::ZERO));
        {
            let runtime = state.motor_test.target.lock();
            *runtime.as_ref().expect("target").target.pwm_models.lock() =
                vec![PwmModel::default(); 4];
        }
        let error = refusal(&state, MotorTestLoad::None);
        assert!(
            error.contains("M1, M2, M3, M4") && error.contains("refresh the mapping"),
            "the error lists the unread motors: {error}"
        );
        {
            let runtime = state.motor_test.target.lock();
            *runtime.as_ref().expect("target").target.pwm_models.lock() = fc_models();
        }
        assert!(
            prepare_start(&state, MotorTestLoad::None).is_ok(),
            "FC-read ranges pass again"
        );
        let target = state.motor_test.target.lock().take();
        if let Some(target) = target {
            target.stop().await;
        }
        *state.motor_test.telemetry.lock() = None;
    }

    /// A replacement serial target gets a new connection ID; a start prepared
    /// for the old connection must not be treated as the same target.
    #[tokio::test(flavor = "multi_thread")]
    async fn replacing_the_target_changes_the_bound_identity() {
        let state = AppState::default();
        install_target(&state, 1).await;
        install_telemetry(&state, Some(heartbeat(1, false)), Some(Duration::ZERO));
        mark_capture_recording(&state);
        let first = prepare_start(&state, MotorTestLoad::Plate).expect("first target");

        let second_handle = install_target(&state, 1).await;
        let second = prepare_start(&state, MotorTestLoad::Plate).expect("replacement target");
        assert_ne!(first.target.connection_id, second.target.connection_id);
        assert_eq!(second.target.connection_id, second_handle.handle_id());

        let target = state.motor_test.target.lock().take();
        if let Some(target) = target {
            target.stop().await;
        }
    }

    /// Starts are refused while an earlier session is still sending, releasing,
    /// or latched in emergency.
    #[test]
    fn non_idle_sessions_refuse_a_new_start() {
        let state = AppState::default();
        assert!(ensure_idle_session(&state).is_ok());

        for session in [
            SessionState::ManualRunning,
            SessionState::Stopping,
            SessionState::Emergency,
        ] {
            *state.motor_test.last_state.lock() = Some(session);
            assert!(ensure_idle_session(&state).is_err(), "{session:?}");
        }

        *state.motor_test.last_state.lock() = Some(SessionState::Idle);
        assert!(ensure_idle_session(&state).is_ok());
    }
}

#[cfg(test)]
mod status_deadlock_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use maggcs_core::motor_test::pwm::PwmModel;

    use crate::state::AppState;

    /// Regression (user report 2026-10-22: the motor-test window froze on
    /// open): `actuator_status` took the non-reentrant target mutex twice in
    /// one expression, so the second `.lock()` never returned. The status
    /// poll must complete promptly, even with no target connected.
    #[test]
    fn pwm_model_from_raw_reads_endpoints_and_flags() {
        let model = super::pwm_model_from_raw(Some(1100), Some(1900), true, true, 0.5, true);
        assert_eq!(model.min_us, 1100);
        assert_eq!(model.max_us, 1900);
        assert!(model.reversed);
        assert!(model.disabled);
        assert!((model.thrust_factor - 0.5).abs() < 1e-6);
        assert_eq!(
            model.range_source,
            maggcs_core::motor_test::pwm::PwmRangeSource::Fc
        );
    }

    #[test]
    fn pwm_model_from_raw_falls_back_to_identity_on_bad_values() {
        let expected = PwmModel::default();
        let model = super::pwm_model_from_raw(Some(2000), Some(1000), false, false, 0.0, true);
        assert_eq!(model, expected);
        let model = super::pwm_model_from_raw(None, Some(1900), false, false, 0.0, true);
        assert_eq!(model, expected);
    }

    #[test]
    fn an_unmapped_motor_gets_the_explicit_fallback_model() {
        let model = super::pwm_model_from_raw(None, None, false, false, 0.0, false);
        assert_eq!(model, PwmModel::default());
        assert_eq!(
            model.range_source,
            maggcs_core::motor_test::pwm::PwmRangeSource::Fallback
        );
        assert_eq!(model.min_us, 1100);
        assert_eq!(model.max_us, 1900);
    }

    #[test]
    fn missing_motor_models_cannot_pass_the_fc_range_gate() {
        assert!(super::refuse_unread_pwm_ranges(&[]).is_err());
        let model = PwmModel::new(1100, 1900, false, 0.0).expect("model");
        assert!(super::refuse_unread_pwm_ranges(&[model; 3]).is_err());
        assert!(super::refuse_unread_pwm_ranges(&[model; 4]).is_ok());
    }

    #[test]
    fn actuator_status_does_not_deadlock_on_repeated_target_locks() {
        let state = Arc::new(AppState::default());
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = state.clone();
        let worker = std::thread::spawn(move || {
            let _ = worker.actuator_status();
            let _ = done_tx.send(());
        });
        match done_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(()) => worker.join().expect("status thread panicked"),
            Err(error) => {
                // The worker is wedged on the mutex; detach it so the test
                // process can still report the failure instead of hanging.
                std::mem::forget(worker);
                panic!("actuator_status did not complete: {error}");
            }
        }
    }
}

#[cfg(test)]
mod sitl_mapping_tests {
    use maggcs_core::mavlink::{MavHeader, MavMessage, MessageEnvelope};
    use maggcs_core::motor_test::mapping::ServoPort;
    use maggcs_core::motor_test::source::ForwardState;

    use crate::motor_test_state::{FcPortOutputs, SitlSourceConfig};

    use super::{apply_sitl_frame, store_fc_output};

    fn source(port: ServoPort) -> SitlSourceConfig {
        SitlSourceConfig {
            endpoint: "udp:127.0.0.1:14540".into(),
            system_id: 1,
            component_id: 1,
            port,
        }
    }

    fn servo_frame(system_id: u8, component_id: u8, port: u8) -> MessageEnvelope {
        MessageEnvelope {
            header: MavHeader {
                system_id,
                component_id,
                sequence: 0,
            },
            message: MavMessage::SERVO_OUTPUT_RAW(::mavlink::common::SERVO_OUTPUT_RAW_DATA {
                time_usec: 0,
                servo1_raw: 1100,
                servo2_raw: 1200,
                servo3_raw: 1300,
                servo4_raw: 1400,
                port,
                ..Default::default()
            }),
            received_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn foreign_node_or_port_does_not_touch_the_forward_slot() {
        let source = source(ServoPort::Main);
        let mut state = ForwardState::new(4);

        assert!(apply_sitl_frame(&source, &servo_frame(2, 1, 0), &mut state, 10.0).is_none());
        assert!(apply_sitl_frame(&source, &servo_frame(1, 2, 0), &mut state, 10.0).is_none());
        assert!(apply_sitl_frame(&source, &servo_frame(1, 1, 1), &mut state, 10.0).is_none());
        assert_eq!(state.raw_pwm, vec![0, 0, 0, 0]);
        assert!(state.last_update_s.is_none());
    }

    #[test]
    fn selected_node_and_port_updates_the_forward_slot() {
        let source = source(ServoPort::Aux);
        let mut state = ForwardState::new(4);

        let pwm = apply_sitl_frame(&source, &servo_frame(1, 1, 1), &mut state, 42.0)
            .expect("selected frame must apply");
        assert_eq!(pwm, [1100, 1200, 1300, 1400]);
        assert_eq!(state.raw_pwm, pwm.to_vec());
        assert!(state.values.iter().all(|v| *v > 0.0));
        assert_eq!(state.last_update_s, Some(42.0));
    }

    #[test]
    fn main_and_aux_frames_are_kept_separate() {
        let mut outputs: [Option<FcPortOutputs>; 2] = [None, None];
        let at = std::time::Instant::now();
        store_fc_output(&mut outputs, ServoPort::Main, [1, 2, 3, 4, 5, 6, 7, 8], at);
        store_fc_output(
            &mut outputs,
            ServoPort::Aux,
            [11, 12, 13, 14, 15, 16, 17, 18],
            at,
        );

        assert_eq!(outputs[0].expect("main sample").pwm[0], 1);
        assert_eq!(outputs[1].expect("aux sample").pwm[0], 11);
    }
}
#[cfg(test)]
mod serial_bench_tests {
    //! Real-PX4 bench runs, `#[ignore]`d by default. The propeller must be
    //! removed and the operator must consciously enable them:
    //!   MAGGCS_BENCH=1 MAGGCS_BENCH_UART=/dev/ttyACM0 \
    //!   cargo test -p maggcs-app serial_bench -- --ignored --test-threads=1
    //!
    //! Risk analysis (repo rule 5): the bench runs disarmed with the
    //! propeller removed; starts only pass the FC-read PWM-range gate; the
    //! demand stays inside `limits_for_load(None)`; every run ends with a
    //! confirmed full release pass (`StopAndWait` receipt `Idle`).

    use std::path::PathBuf;
    use std::time::Instant;

    use maggcs_core::mavlink::connection::{spawn_connection, ConnectionConfig, ConnectionHandle};
    use maggcs_core::mavlink::router::MessageRoute;
    use maggcs_core::mavlink::{ConnectionEvent, Endpoint, MavMessage};
    use maggcs_core::motor_test::mapping::OutputMap;
    use maggcs_core::motor_test::pwm::PwmModel;
    use maggcs_core::motor_test::{
        MotorTestCommand, MotorTestEvent, MotorTestService, SessionState,
    };

    use crate::motor_test_state::{limits_for_load, MotorTestLoad};

    use super::{refuse_unread_pwm_ranges, MOTOR_TEST_MOTOR_COUNT};

    /// Shared (t, value) buffers for the log/curve samplers.
    type MagCurve = std::sync::Arc<parking_lot::Mutex<Vec<(f64, f64)>>>;
    type ServoCurve = std::sync::Arc<parking_lot::Mutex<Vec<(f64, [f32; 4])>>>;

    /// Target PX4 identity on the bench serial link.
    const BENCH_SYSTEM_ID: u8 = 1;
    const BENCH_COMPONENT_ID: u8 = 1;
    const BENCH_GCS_SYSTEM_ID: u8 = 250;
    const BENCH_GCS_COMPONENT_ID: u8 = 250;
    const BENCH_UART: &str = "/dev/ttyACM0";
    const BENCH_BAUD: u32 = 115200;
    /// MAVLink message id of `HIGHRES_IMU` (mag triple, ~50 Hz default here).
    const MAG_MESSAGE_ID: u32 = 105;
    /// MAVLink message id of `SERVO_OUTPUT_RAW` (port 0 = MAIN 1-8 pulses).
    const SERVO_MESSAGE_ID: u32 = 36;
    /// Sine coefficient: the raised cosine peaks at twice this fraction.
    const BENCH_AMPLITUDE: f32 = 0.2;
    const BENCH_SINE_HZ: f32 = 0.5;
    const BENCH_SPIN_S: f64 = 30.0;
    /// MAG stream rate for the flight plot.
    const MAG_STREAM_HZ: f64 = 50.0;
    const BENCH_PREROLL_S: f64 = 3.0;
    const LINK_TIMEOUT_S: u64 = 10;
    /// Sine update rate of the bench driver (the UI throttles similarly).
    const SINE_UPDATE_MS: u64 = 50;
    const BENCH_OUT_DIR: &str = "/tmp/opencode";

    fn bench_uart() -> PathBuf {
        std::env::var("MAGGCS_BENCH_UART")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(BENCH_UART))
    }

    fn bench_enabled() -> bool {
        std::env::var("MAGGCS_BENCH").is_ok_and(|value| value == "1")
    }

    async fn connect_bench() -> ConnectionHandle {
        let config = ConnectionConfig {
            endpoint: Endpoint::Serial {
                port: bench_uart(),
                baudrate: BENCH_BAUD,
            },
            system_id: BENCH_GCS_SYSTEM_ID,
            component_id: BENCH_GCS_COMPONENT_ID,
            target_system_id: BENCH_SYSTEM_ID,
            target_component_id: BENCH_COMPONENT_ID,
            ..Default::default()
        };
        let (handle, _events, first) = spawn_connection(config).await.expect("spawn bench link");
        tokio::time::timeout(std::time::Duration::from_secs(LINK_TIMEOUT_S), first)
            .await
            .expect("link startup timed out")
            .expect("first connection result")
            .expect("link opened");
        handle
    }

    async fn mot_test_enabled(params: &maggcs_core::params::ParamService) -> i32 {
        params
            .read("COM_MOT_TEST_EN")
            .await
            .map(|entry| entry.value)
            .map(|value| match value {
                maggcs_core::params::ParamValue::Int(value) => {
                    i32::try_from(value).unwrap_or_default()
                }
                _ => -2,
            })
            .unwrap_or_default()
    }

    async fn ensure_mot_test_en(params: &maggcs_core::params::ParamService) {
        let value = mot_test_enabled(params).await;
        assert_eq!(
            value, 1,
            "COM_MOT_TEST_EN must be 1 on the bench FC (got {value}); \
             enable it or every ACTUATOR_TEST is denied"
        );
    }

    /// Read the Motor 1–4 configuration and print it: `FUNCx` → MAIN/AUX +
    /// channel, then MIN/MAX/REV/DIS + range source of the mapped output.
    async fn read_configuration(
        params: &maggcs_core::params::ParamService,
    ) -> (OutputMap, Vec<PwmModel>) {
        ensure_mot_test_en(params).await;
        let map = super::read_output_map(params).await;
        println!("output map: {map:?}");
        let models = super::read_pwm_models(params, &map).await;
        for (motor, model) in models.iter().enumerate() {
            println!(
                "M{}: {}–{} µs reversed={} disabled={} range_source={:?}",
                motor + 1,
                model.min_us,
                model.max_us,
                model.reversed,
                model.disabled,
                model.range_source
            );
        }
        (map, models)
    }

    /// Stream-rate diagnostic: which message ids the bench FC streams.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "bench diagnostics: MAGGCS_BENCH=1 cargo test -p maggcs-app -- --ignored"]
    async fn bench_list_streamed_messages() {
        if !bench_enabled() {
            panic!("set MAGGCS_BENCH=1 to run");
        }
        let handle = connect_bench().await;
        for msg_id in [291u32, 292u32] {
            super::request_message_interval(&handle, BENCH_SYSTEM_ID, msg_id, 10.0).await;
        }
        let mut events = handle.subscribe_route(MessageRoute::all());
        let deadline = Instant::now() + std::time::Duration::from_secs(6);
        let mut counts: std::collections::BTreeMap<String, u32> = Default::default();
        while Instant::now() < deadline {
            if let Ok(Ok(ConnectionEvent::Message(env))) =
                tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await
            {
                let name = format!("{:?}", env.message)
                    .split('(')
                    .next()
                    .unwrap_or("?")
                    .to_string();
                *counts.entry(name).or_default() += 1;
            }
        }
        let mut names: Vec<String> = counts.into_keys().collect();
        names.sort();
        println!("streamed messages: {names:?}");
        handle.shutdown().await;
    }

    /// Harness state for the mag curve: the sampler task is abort-joinable
    /// and the curve is shared with the test.
    struct MagBench {
        handle: ConnectionHandle,
        curve: MagCurve,
        sampler: tauri::async_runtime::JoinHandle<()>,
    }

    impl MagBench {
        /// Abort and join the sampler, return the curve; loggers first, the
        /// link shutdown is owned by the last cleanup call.
        async fn stop(self) -> Vec<(f64, f64)> {
            self.sampler.abort();
            let _ = self.sampler.await;
            self.handle.shutdown().await;
            self.curve.lock().clone()
        }
    }

    /// Record `(run-relative seconds, [MAIN1..MAIN4 pulse µs])` per
    /// `SERVO_OUTPUT_RAW` frame — the FC-view of every channel's commanded
    /// pulse (no ESC telemetry exists on this bench).
    struct ServoBench {
        curve: ServoCurve,
        sampler: tauri::async_runtime::JoinHandle<()>,
    }

    impl ServoBench {
        /// Abort and join the sampler, return the curve (no link shutdown).
        async fn stop(self) -> Vec<(f64, [f32; 4])> {
            self.sampler.abort();
            let _ = self.sampler.await;
            self.curve.lock().clone()
        }
    }

    /// Record `(run-relative seconds, total field nT)` per HIGHRES_IMU frame.
    fn spawn_mag_sampler(handle: ConnectionHandle, t0: Instant) -> MagBench {
        let curve = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<(f64, f64)>::new()));
        let sink = curve.clone();
        let mut events = handle.subscribe_route(
            MessageRoute::messages(&[MAG_MESSAGE_ID]).from(BENCH_SYSTEM_ID, BENCH_COMPONENT_ID),
        );
        let sampler = tauri::async_runtime::spawn(async move {
            let started = t0;
            loop {
                match events.recv().await {
                    Ok(ConnectionEvent::Message(env)) => {
                        if let MavMessage::HIGHRES_IMU(imu) = &env.message {
                            // Same conversion + norm as the motor-test scope
                            // (raw pass-through, gauss → nanotesla).
                            let g = maggcs_core::signals::GAUSS_TO_NANOTESLA;
                            let raw_nt = [
                                f64::from(imu.xmag) * g,
                                f64::from(imu.ymag) * g,
                                f64::from(imu.zmag) * g,
                            ];
                            let total = (raw_nt[0] * raw_nt[0]
                                + raw_nt[1] * raw_nt[1]
                                + raw_nt[2] * raw_nt[2])
                                .sqrt();
                            sink.lock().push((started.elapsed().as_secs_f64(), total));
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        });
        MagBench {
            handle,
            curve,
            sampler,
        }
    }

    /// Record the MAIN1..4 pulses (`servo1_raw..servo4_raw` of port 0).
    fn spawn_servo_sampler(handle: ConnectionHandle, t0: Instant) -> ServoBench {
        let curve = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<(f64, [f32; 4])>::new()));
        let sink = curve.clone();
        let mut events = handle.subscribe_route(
            MessageRoute::messages(&[SERVO_MESSAGE_ID]).from(BENCH_SYSTEM_ID, BENCH_COMPONENT_ID),
        );
        let sampler = tauri::async_runtime::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(ConnectionEvent::Message(env)) => {
                        if let MavMessage::SERVO_OUTPUT_RAW(servo) = &env.message {
                            if servo.port == 0 {
                                let pulses = [
                                    f32::from(servo.servo1_raw),
                                    f32::from(servo.servo2_raw),
                                    f32::from(servo.servo3_raw),
                                    f32::from(servo.servo4_raw),
                                ];
                                sink.lock().push((t0.elapsed().as_secs_f64(), pulses));
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        });
        ServoBench { curve, sampler }
    }

    fn write_servo_csv(path: &PathBuf, curve: &[(f64, [f32; 4])]) {
        use std::io::Write;
        let mut file = std::fs::File::create(path).expect("create CSV");
        writeln!(file, "t_s,main1_us,main2_us,main3_us,main4_us").expect("header");
        for (t, pulses) in curve {
            writeln!(
                file,
                "{t:.3},{:.0},{:.0},{:.0},{:.0}",
                pulses[0], pulses[1], pulses[2], pulses[3]
            )
            .expect("row");
        }
    }

    fn write_curve_csv(path: &PathBuf, curve: &[(f64, f64)]) {
        use std::io::Write;
        let mut file = std::fs::File::create(path).expect("create CSV");
        writeln!(file, "t_s,mag_total_nt").expect("header");
        for (t, total) in curve {
            writeln!(file, "{t:.3},{total:.1}").expect("row");
        }
    }

    /// Spin Motor 1 inside the FC-read range, then stop and release.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "benches the real PX4 on serial: MAGGCS_BENCH=1 MAGGCS_BENCH_UART=/dev/ttyACM0"]
    async fn bench_reads_configuration_and_spins_motor_one() {
        if !bench_enabled() {
            panic!("set MAGGCS_BENCH=1 (propeller removed, FC disarmed) to run");
        }
        let handle = connect_bench().await;
        let (params, _param_events) = maggcs_core::params::ParamService::spawn(
            handle.clone(),
            BENCH_SYSTEM_ID,
            BENCH_COMPONENT_ID,
        );
        let (map, models) = read_configuration(&params).await;
        assert!(map.channel_for(1).is_some(), "Motor 1 must be mapped");
        refuse_unread_pwm_ranges(&models).expect("every launched motor needs an FC-read range");

        let limits = limits_for_load(MotorTestLoad::None, BENCH_SYSTEM_ID);
        let (service, mut events, output) = MotorTestService::spawn(handle.clone(), limits);
        service
            .send(MotorTestCommand::SetPwmModels { models })
            .await
            .expect("models accepted");
        let mut values = vec![0.0; usize::from(MOTOR_TEST_MOTOR_COUNT)];
        values[0] = BENCH_AMPLITUDE;
        service
            .send(MotorTestCommand::SetValues(values.clone()))
            .await
            .expect("slider values accepted");
        service
            .send(MotorTestCommand::StartManual)
            .await
            .expect("start accepted");

        let spin_until = Instant::now() + std::time::Duration::from_secs_f64(BENCH_SPIN_S);
        while Instant::now() < spin_until {
            if let Ok(Some(event)) = tokio::time::timeout(
                spin_until.saturating_duration_since(Instant::now()),
                events.recv(),
            )
            .await
            {
                match event {
                    MotorTestEvent::Stopped(reason) => panic!("stopped during spin: {reason:?}"),
                    MotorTestEvent::ReleaseFailed(error) => panic!("release failed: {error:?}"),
                    MotorTestEvent::StateChanged(state) => println!("state: {state:?}"),
                }
            } else {
                break;
            }
        }
        let sent = output.borrow().clone();
        assert!(
            sent.first().copied().unwrap_or(0.0) > 0.0,
            "ACTUATOR_TEST commands must flow while spinning"
        );

        let receipt = service
            .stop_and_wait()
            .await
            .expect("stop within the deadline");
        println!("stop receipt: {receipt:?}");
        assert_eq!(receipt.state, SessionState::Idle);
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("stopped event within the deadline")
            .expect("events alive");
        assert!(
            matches!(stopped, MotorTestEvent::Stopped(_)),
            "waiting for Stopped, got {stopped:?}"
        );
        handle.shutdown().await;
    }

    /// Latest-value-wins 20 Hz sine updates, Motor 1 and 3 in phase,
    /// Motors 2 and 4 held at 0.
    async fn drive_sine(
        service: &MotorTestService,
        phase_start: Instant,
        until: Instant,
    ) -> Vec<(f64, f64)> {
        let mut recorded = Vec::<(f64, f64)>::new();
        let period = std::time::Duration::from_millis(SINE_UPDATE_MS);
        let mut next = phase_start;
        while Instant::now() < until {
            let until_next = next.saturating_duration_since(Instant::now());
            tokio::time::sleep(until_next).await;
            let t = next.duration_since(phase_start).as_secs_f32();
            // 0 → twice the coefficient, preserving the original bench run.
            let demand =
                BENCH_AMPLITUDE * (1.0 - (2.0 * std::f32::consts::PI * BENCH_SINE_HZ * t).cos());
            let mut values = vec![0.0; usize::from(MOTOR_TEST_MOTOR_COUNT)];
            values[0] = demand;
            values[2] = demand;
            service
                .send(MotorTestCommand::SetValues(values))
                .await
                .expect("slider values accepted during the sine run");
            recorded.push((f64::from(t), f64::from(demand)));
            next += period;
        }
        recorded
    }

    /// Acceptance: sine on Motor 1 **and** Motor 3 for 30 s with the whole
    /// total-magnetic-field curve recorded to CSV (bench, propeller removed).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "benches the real PX4 on serial: MAGGCS_BENCH=1 MAGGCS_BENCH_UART=/dev/ttyACM0"]
    async fn bench_sine_on_motors_1_and_3_records_mag_field() {
        if !bench_enabled() {
            panic!("set MAGGCS_BENCH=1 (propeller removed, FC disarmed) to run");
        }
        let handle = connect_bench().await;
        request_mag_stream(&handle).await;
        let t0 = Instant::now();
        let bench = spawn_mag_sampler(handle.clone(), t0);
        // Servo recorder before the config reads: it runs through the whole
        // test and its samples share the mag curve time base.
        let servo = spawn_servo_sampler(handle.clone(), t0);

        let (params, _param_events) = maggcs_core::params::ParamService::spawn(
            handle.clone(),
            BENCH_SYSTEM_ID,
            BENCH_COMPONENT_ID,
        );
        let (map, models) = read_configuration(&params).await;
        assert!(
            map.channel_for(1).is_some() && map.channel_for(3).is_some(),
            "Motor 1 and Motor 3 must both be mapped"
        );
        refuse_unread_pwm_ranges(&models).expect("every launched motor needs an FC-read range");
        for motor in 1..=MOTOR_TEST_MOTOR_COUNT {
            let channel = map.channel_for(motor).expect("mapped bench motor");
            assert!(
                channel.port == maggcs_core::motor_test::mapping::ServoPort::Main
                    && channel.channel <= 4,
                "this recorder requires motors mapped to MAIN1..4"
            );
        }
        let analysis_models = models.clone();
        let mut dis = [0u16; 4];
        for channel in 1..=4u8 {
            let pulse = super::read_int_param(&params, &format!("PWM_MAIN_DIS{channel}")).await;
            dis[usize::from(channel - 1)] = pulse.unwrap_or_default().try_into().unwrap_or(0);
        }
        println!("PWM_MAIN_DIS1..4 = {dis:?}");

        let limits = limits_for_load(MotorTestLoad::None, BENCH_SYSTEM_ID);
        let (service, mut events, output) = MotorTestService::spawn(handle.clone(), limits);
        service
            .send(MotorTestCommand::SetPwmModels { models })
            .await
            .expect("models accepted");

        // Idle initial values, then start: the sine loop owns the values after.
        let idle = vec![0.0; usize::from(MOTOR_TEST_MOTOR_COUNT)];
        service
            .send(MotorTestCommand::SetValues(idle.clone()))
            .await
            .expect("idle values accepted");
        service
            .send(MotorTestCommand::StartManual)
            .await
            .expect("start accepted");
        // Pre-roll sample window (baseline field, motors released).
        tokio::time::sleep(std::time::Duration::from_secs_f64(BENCH_PREROLL_S)).await;
        assert!(
            matches!(
                events.recv().await,
                Some(MotorTestEvent::StateChanged(SessionState::ManualRunning))
            ),
            "session must be running before the sine drive"
        );

        // 30 s sine on M1 + M3.
        let spin_started = Instant::now();
        let record = drive_sine(
            &service,
            spin_started,
            spin_started + std::time::Duration::from_secs_f64(BENCH_SPIN_S),
        )
        .await;
        let spin_finished = Instant::now();
        let sent = output.borrow().clone();

        // Stop: confirmed full release pass.
        let receipt = service
            .stop_and_wait()
            .await
            .expect("stop within the deadline");
        println!("stop receipt: {receipt:?}");
        assert_eq!(receipt.state, SessionState::Idle);
        assert!(
            record.iter().any(|(_, value)| *value > 0.0),
            "sine drove a nonzero demand"
        );
        assert!(
            sent.get(1) == Some(&0.0) && sent.get(3) == Some(&0.0),
            "M2 and M4 stayed undemanded"
        );

        // Post-roll field samples settle before the sampler stops.
        tokio::time::sleep(std::time::Duration::from_secs_f64(BENCH_PREROLL_S)).await;
        let servo_curve = servo.stop().await;
        let curve = bench.stop().await;

        // Curve sanity: enough samples covering the whole process.
        let minimum_samples =
            usize::try_from((MAG_STREAM_HZ * (BENCH_SPIN_S + 2.0 * BENCH_PREROLL_S)) as u64)
                .unwrap();
        assert!(
            curve.len() >= minimum_samples,
            "the mag curve must cover the run: {} samples < {minimum_samples}",
            curve.len()
        );
        let last = curve.last().expect("curve").0;
        assert!(
            last >= BENCH_PREROLL_S * 2.0 + BENCH_SPIN_S,
            "the curve must span pre-roll, spin and post-roll: last t = {last}"
        );
        let out_path = PathBuf::from(BENCH_OUT_DIR).join("bench_sine_mag.csv");
        write_curve_csv(&out_path, &curve);
        let spin_start_s = spin_started.duration_since(t0).as_secs_f64();
        let spin_end_s = spin_finished.duration_since(t0).as_secs_f64();
        let timing_path = PathBuf::from(BENCH_OUT_DIR).join("bench_sine_timing.csv");
        {
            use std::io::Write;
            let mut timing = std::fs::File::create(timing_path).expect("timing CSV");
            writeln!(timing, "phase,t_s").expect("timing header");
            writeln!(timing, "spin_start,{spin_start_s:.3}").expect("start marker");
            writeln!(timing, "spin_end,{spin_end_s:.3}").expect("end marker");
        }
        let baseline: Vec<_> = curve
            .iter()
            .filter(|(t, _)| *t >= spin_start_s - BENCH_PREROLL_S && *t < spin_start_s)
            .collect();
        let baseline_mean =
            baseline.iter().map(|(_, v)| v).sum::<f64>() / f64::from(baseline.len().max(1) as u32);
        println!(
            "mag curve: {} samples → {} (baseline ≈ {baseline_mean:.0} nT)",
            curve.len(),
            out_path.display()
        );
        println!("sine updates recorded: {}", record.len());

        // Servo analysis: MAIN1=M2, MAIN2=M4, MAIN3=M3, MAIN4=M1.
        let servo_path = PathBuf::from(BENCH_OUT_DIR).join("bench_sine_servo.csv");
        write_servo_csv(&servo_path, &servo_curve);
        let window_start = spin_start_s + 1.0;
        let window_end = spin_end_s - 1.0;
        let in_window: Vec<&(f64, [f32; 4])> = servo_curve
            .iter()
            .filter(|(t, _)| *t >= window_start && *t <= window_end)
            .collect();
        assert!(
            !in_window.is_empty(),
            "SERVO_OUTPUT_RAW frames must cover the spin window"
        );
        let mut spans = [[f32::INFINITY, f32::NEG_INFINITY]; 4];
        for (_, pulses) in &in_window {
            for (ch, pulse) in pulses.iter().enumerate() {
                spans[ch][0] = spans[ch][0].min(*pulse);
                spans[ch][1] = spans[ch][1].max(*pulse);
            }
        }
        println!(
            "spin window MAIN pulses (µs): 1={:?} 2={:?} 3={:?} 4={:?}",
            spans[0], spans[1], spans[2], spans[3]
        );
        // M2 (MAIN1) and M4 (MAIN2) were demanded 0 the whole run: pinned at
        // `PWM_MAIN_MIN` 1100 (the idle-spin mechanism) must not occur; a
        // real release shows the DIS pulse or stays below the function min.
        for motor in [2u8, 4u8] {
            let ch = usize::from(map.channel_for(motor).expect("mapped").channel - 1);
            let dis_pulse = dis[ch];
            let [lo, hi] = spans[ch];
            let released = hi < f32::from(analysis_models[usize::from(motor - 1)].min_us)
                || (lo == f32::from(dis_pulse) && hi == f32::from(dis_pulse));
            assert!(
                released,
                "M{motor} must stay released (DIS {dis_pulse} µs), observed {lo}..{hi} µs"
            );
        }
        // The benchmark's raised cosine peaks at twice BENCH_AMPLITUDE.
        for motor in [1u8, 3u8] {
            let ch = usize::from(map.channel_for(motor).expect("mapped").channel - 1);
            let model = analysis_models[usize::from(motor - 1)];
            let [lo, hi] = spans[ch];
            let range_hi = f32::from(model.min_us) + 2.0 * BENCH_AMPLITUDE * model.span_us();
            assert!(
                lo <= f32::from(model.min_us) + 0.05 * model.span_us()
                    && hi >= range_hi - 0.05 * model.span_us() && hi <= range_hi + 5.0,
                "M{motor} must sweep the configured sine band, observed {lo}..{hi} µs (peak ≈ {range_hi:.0})"
            );
        }
        println!(
            "servo curve: {} samples → {}",
            servo_curve.len(),
            servo_path.display()
        );

        handle.shutdown().await;
    }

    /// Diagnostic (bench finding 2026-10-23): does the real FC answer
    /// `MAV_CMD_REQUEST_MESSAGE` (148) with `AUTOPILOT_VERSION` on the
    /// saturated serial link?
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "bench diagnostics: MAGGCS_BENCH=1 cargo test -p maggcs-app --ignored"]
    async fn bench_autopilot_version_diag() {
        if !bench_enabled() {
            panic!("set MAGGCS_BENCH=1 to run");
        }
        let handle = connect_bench().await;
        let mut events = handle.subscribe_route(MessageRoute::all());
        super::request_message_interval(&handle, BENCH_SYSTEM_ID, 148u32, 0.5).await;
        let deadline = Instant::now() + std::time::Duration::from_secs(6);
        while Instant::now() < deadline {
            if let Ok(Ok(ConnectionEvent::Message(env))) =
                tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await
            {
                match env.message {
                    MavMessage::AUTOPILOT_VERSION(v) => {
                        let f = v.flight_sw_version;
                        println!(
                            "AUTOPILOT_VERSION: {}.{}.{} capabilities={:#x}",
                            (f >> 24) & 0xFF,
                            (f >> 16) & 0xFF,
                            (f >> 8) & 0xFF,
                            v.capabilities.bits()
                        );
                    }
                    MavMessage::COMMAND_LONG(cmd)
                        if cmd.command == ::mavlink::common::MavCmd::MAV_CMD_REQUEST_MESSAGE =>
                    {
                        println!("echoed REQUEST_MESSAGE: {}", cmd.param1);
                    }
                    MavMessage::COMMAND_ACK(ack) => {
                        println!("ACK: command={:?} result={:?}", ack.command, ack.result);
                    }
                    _ => {}
                }
            }
        }
        handle.shutdown().await;
    }

    /// Diagnostic (bench finding 2026-10-23): replicates the app's params
    /// runtime path on the real serial FC — `spawn_with_task` + the bounded
    /// `wait_fc_version` — to prove the convergence (or expose the break).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "bench diagnostics: MAGGCS_BENCH=1 cargo test -p maggcs-app --ignored"]
    async fn bench_params_fc_version_converges() {
        if !bench_enabled() {
            panic!("set MAGGCS_BENCH=1 to run");
        }
        let handle = connect_bench().await;
        let (svc, mut evt, task) =
            maggcs_core::params::ParamService::spawn_with_task(handle.clone(), 1, 1);
        let started = Instant::now();
        let version = svc
            .wait_fc_version(2, std::time::Duration::from_millis(1200))
            .await;
        println!(
            "wait_fc_version -> {version:?} after {:.1?}",
            started.elapsed()
        );
        if let Ok(Ok(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(200), evt.recv()).await
        {
            println!("event: {ev:?}");
        }
        drop(svc);
        task.abort();
        handle.shutdown().await;
        assert!(
            version.is_some(),
            "wait_fc_version must converge on the real serial FC"
        );
    }

    /// Diagnostic (bench finding 2026-10-23): does the real PX4 1.17.0 FC
    /// expose `USB_MAV_MODE`? Single lazy reads for the suspected names.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "bench diagnostics: MAGGCS_BENCH=1 cargo test -p maggcs-app --ignored"]
    async fn bench_read_usb_mav_mode_variants() {
        if !bench_enabled() {
            panic!("set MAGGCS_BENCH=1 to run");
        }
        let handle = connect_bench().await;
        let (params, _param_events) =
            maggcs_core::params::ParamService::spawn(handle.clone(), 1, 1);
        for name in ["USB_MAV_MODE", "SYS_USB_MAV_MODE"] {
            let entry = params.read(name).await;
            match entry {
                Ok(e) => println!("param {name}: value={:?} type={:?}", e.value, e.ptype),
                Err(err) => println!("param {name}: NOT READ ({err:?})"),
            }
        }
        drop(params);
        handle.shutdown().await;
    }

    /// Request the mag stream rate for the curve (the FC already streams
    /// HIGHRES_IMU, but the rate must be ≥ half the accepted minimum).
    async fn request_mag_stream(handle: &ConnectionHandle) {
        super::request_message_interval(handle, BENCH_SYSTEM_ID, MAG_MESSAGE_ID, MAG_STREAM_HZ)
            .await;
    }
}
