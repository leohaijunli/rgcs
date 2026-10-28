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
use tauri::WebviewWindow;
use tauri::WebviewWindowBuilder;

use maggcs_core::logs::capture::SdCaptureStatus;
use maggcs_core::mavlink::connection::{spawn_connection, ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::mavlink::{MavMessage, MessageEnvelope};
use maggcs_core::motor_test::mapping::{
    build_output_map_checked, mapped_feedback_pwm, FunctionRead, MappingIssue, OutputMap,
    ServoPort, SERVO_CHANNELS_PER_PORT,
};
use maggcs_core::motor_test::operation_types::{
    MotorForwardSourceRef, MotorOperationError, MotorParameterReadRequest,
    MotorParameterSetRequest, MotorReleaseReceipt, MotorStartReply, MotorStartRequest,
    MotorStartSource, MotorStopRequest, MotorValuesReceipt, MotorValuesRequest,
};
use maggcs_core::motor_test::protocol::{
    detect_bank_protocol_checked, OutputProtocol, OutputProtocols,
};
use maggcs_core::motor_test::pwm::{PwmModel, PwmRangeSource};
use maggcs_core::motor_test::scope::{
    self, MagScopeSource, OutputScopeError, ScopeBus, ScopeOutputMeta,
};
use maggcs_core::motor_test::service::{MotorTestCommand, MotorTestEvent, MotorTestService};
use maggcs_core::motor_test::source::{ForwardSlot, ForwardState};
use maggcs_core::motor_test::{SessionState, DEFAULT_TICK_HZ};
use maggcs_core::params::model::ParamValue;
use maggcs_core::params::operation_types::{ParameterCommandError, ParameterRequestContext};
use maggcs_core::params::{ParamEntry, ParamObservation, ParamService};
use maggcs_core::runtime::operations::OperationError;
use maggcs_core::runtime::{Revision, TransportGeneration};
use maggcs_core::signals::derived::esc_frame_range;
use maggcs_core::signals::{
    HIGHRES_IMU_ID, MILLIGAUSS_TO_NANOTESLA, SCALED_IMU2_ID, SCALED_IMU3_ID, SCALED_IMU_ID,
};

/// Maximum time to observe the release pass before closing an actuator link.
const MOTOR_TEST_STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// How old the last target-FC heartbeat may be before the window shows the
/// serial target as "heartbeat lost". PX4 streams HEARTBEAT at 1 Hz.
const TARGET_HEARTBEAT_STALE: Duration = Duration::from_secs(3);
const MOTOR_GCS_SYSTEM_ID: u8 = 250;
const MOTOR_GCS_ALTERNATE_SYSTEM_ID: u8 = 251;
const MOTOR_GCS_COMPONENT_ID: u8 = 250;
/// Motor functions the window displays and the forward slot carries.
const MOTOR_TEST_MOTOR_COUNT: u8 = 4;
/// Older SERVO_OUTPUT_RAW samples are not shown as live feedback.
const FC_OUTPUT_STALE: Duration = Duration::from_secs(2);
/// `ESC_STATUS` older than this must not be shown as a "live" RPM value.
const ESC_RPM_STALE: Duration = Duration::from_millis(500);
/// Short per-name probe before a full parameter inventory is available. An
/// absent channel otherwise stalls for the full transaction timeout.
const MAPPING_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

use crate::motor_test_state::motor_test_limits;
use crate::motor_test_state::{
    AuxiliaryLinkRuntime, FcPortOutputs, MainSitlSource, MotorTestTarget, MotorTestTargetRef,
    OutputConfigSnapshot, SitlLinkRuntime, SitlSourceConfig, SitlSourceOrigin, TargetLinkRuntime,
    TARGET_COMPONENT_ID,
};
use crate::state::AppState;

/// Status snapshot for the window's header and right panel (polled ~1 Hz by
/// the UI; safety-relevant signals are low-rate).
#[derive(Debug, Clone, Serialize)]
pub struct ActuatorStatus {
    pub operation: Option<maggcs_core::runtime::operations::OperationRef>,
    pub capture_operation: Option<maggcs_core::runtime::operations::OperationRef>,
    pub config_revision: Option<Revision>,
    pub forward_source_ref: Option<MotorForwardSourceRef>,
    pub motor_control: crate::motor_control_service::MotorControlStatus,
    pub streams: crate::stream_service::StreamSnapshot,
    pub tones: crate::motor_tone_service::MotorToneStatus,
    /// The selected Motor Test FC is attached, on a dedicated or borrowed link.
    pub connected: bool,
    /// A matching PX4 heartbeat from the selected FC is fresh.
    pub fc_alive: bool,
    /// Explicit target alias for `fc_alive`; never describes another FC.
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
    /// Whether the feed owns its connection or borrows the primary link.
    pub sitl_origin: Option<SitlSourceOrigin>,
    /// Seconds since the last SERVO_OUTPUT_RAW sample; null = none yet.
    pub sitl_feed_age_s: Option<f64>,
    /// Offer to use the already-connected primary link as the SITL feed.
    pub main_sitl_source: Option<MainSitlSource>,
    /// The motor test's own target link (real PX4, serial): connected + endpoint.
    pub target_connected: bool,
    pub target_endpoint: Option<String>,
    pub target_system_id: Option<u8>,
    pub target_origin: Option<crate::motor_test_state::TargetLinkOrigin>,
    pub main_serial_target: Option<crate::motor_test_state::MainSerialTarget>,
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
    /// Complete output descriptors from the same configuration snapshot.
    /// Empty until units/mapping are known or while the snapshot is stale.
    pub scope_outputs: Vec<ScopeOutputMeta>,
    /// Output protocol PX4 applies to each bank, detected from `PWM_*_TIMn`.
    pub output_protocols: OutputProtocols,
    /// Target `DSHOT_3D_ENABLE` (null = unread). Nonzero blocks DShot starts.
    pub dshot_3d_enable: Option<i32>,
    /// Target `DSHOT_BIDIR_EN` (null = unread), needed for RPM telemetry.
    pub dshot_bidir_en: Option<i32>,
    /// Per-ESC RPM from `ESC_STATUS`; `rpm` null means that ESC never
    /// reported. `stale` marks a value older than the RPM freshness window.
    pub esc_rpm: Vec<EscRpmStatus>,
    /// Target-shell SD-file capture state (Task 6).
    pub sd_capture: SdCaptureStatus,
    /// Current/last per-test run + recovery manifest (Task 8).
    pub run: crate::run_service::RunSnapshot,
}

/// One ESC's RPM sample in the status snapshot.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct EscRpmStatus {
    /// 1-based display index; the `ESC_STATUS` wire index is zero-based.
    pub esc: u8,
    /// Latest reported RPM; null when this ESC has never reported.
    pub rpm: Option<f64>,
    /// Seconds since the newest frame covering this ESC.
    pub age_s: Option<f64>,
    pub stale: bool,
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
    // Permission belongs to a native-issued client instance. Opening a view
    // cannot revoke another window's grant or stop its current operation.
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
    // runtime_client_attach installs an exact ClientId destruction hook. The
    // owned service observes that cancellation directly and retains its receipts.
    let app_for_cleanup = app.clone();
    win.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Destroyed) {
            return;
        }
        let cleanup_app = app_for_cleanup.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) =
                crate::motor_operations::stop_all(&cleanup_app, MOTOR_TEST_STOP_TIMEOUT).await
            {
                let _ = cleanup_app.emit("actuator_stop_warning", error);
                return;
            }
            if let Err(error) =
                crate::motor_capture_operations::stop_all(&cleanup_app, Duration::from_secs(15))
                    .await
            {
                let _ = cleanup_app.emit("actuator_stop_warning", error);
                return;
            }
            if let Err(error) =
                crate::motor_log_operations::stop_all(&cleanup_app, Duration::from_secs(2)).await
            {
                let _ = cleanup_app.emit("actuator_stop_warning", error);
                return;
            }
            let state = cleanup_app.state::<AppState>();
            crate::motor_control_service::revoke(&state);
            let _ops = state.ops().lock().await;
            state.motor_test.running.lock().take();
            crate::stream_service::stop(&state);
            let sitl = state.motor_test.sitl.lock().take();
            if let Some(sitl) = sitl {
                sitl.stop().await;
            }
            detach_motor_telemetry(&state).await;
            let target = state.motor_test.target.lock().take();
            if let Some(target) = target {
                target.stop().await;
            }
            if let Some(task) = state.motor_test.scope_flusher.lock().take() {
                task.abort();
            };
        });
    });
    Ok(())
}

/// Close the motor-test window and run the stop path.
#[tauri::command]
pub async fn actuator_close(window: WebviewWindow) -> Result<(), String> {
    window.close().map_err(|error| error.to_string())
}

/// Live link/FC status for the window's panels.
#[tauri::command]
pub fn actuator_status(state: State<'_, AppState>) -> Result<ActuatorStatus, String> {
    Ok(state.actuator_status())
}

/// A start that passed target identity and heartbeat checks. Holding it
/// binds the new session to the exact connection the checks were made on.
struct PreparedStart {
    handle: ConnectionHandle,
    target: MotorTestTargetRef,
    limits: maggcs_core::motor_test::SafetyLimits,
    /// Captured with the target identity, so every start sends exactly the
    /// models that were current when readiness was checked.
    pwm_models: Vec<PwmModel>,
    scope_outputs: Vec<ScopeOutputMeta>,
    /// Configured M1..MN prefix from the captured output map.
    active_motors: usize,
}

/// Verify the current target before any session is created or reused. The
/// caller holds [`AppState::ops`], so the target cannot be replaced between
/// these checks and the spawn.
fn prepare_start(state: &AppState) -> Result<PreparedStart, String> {
    let (handle, target, config, config_stale) = {
        let guard = state.motor_test.target.lock();
        let runtime = guard
            .as_ref()
            .ok_or("no serial PX4 target — connect it first")?;
        let handle = runtime.target.handle.clone();
        let target = runtime.target.target_ref();
        let config = runtime.target.config.lock().clone();
        let stale = runtime
            .target
            .config_stale
            .load(std::sync::atomic::Ordering::Relaxed);
        (handle, target, config, stale)
    };
    let config =
        config.ok_or("output configuration has not been read yet — refresh the mapping first")?;
    if config_stale {
        return Err(
            "output configuration is stale after a failed refresh — refresh the mapping again"
                .into(),
        );
    }
    if config.target != target {
        return Err(
            "output configuration belongs to a different target — refresh the mapping first".into(),
        );
    }
    refuse_unread_pwm_ranges(
        &config.map,
        &config.models,
        config.protocols_known,
        config.dshot_3d_enable,
    )?;
    ensure_target_disarmed(state, target)?;
    let scope_outputs = output_scope_metadata(&config).map_err(|error| error.to_string())?;
    Ok(PreparedStart {
        handle,
        target,
        limits: motor_test_limits(target.target_system_id),
        pwm_models: config.models,
        scope_outputs,
        active_motors: usize::from(config.map.active_count()),
    })
}

fn output_scope_metadata(
    config: &OutputConfigSnapshot,
) -> Result<Vec<ScopeOutputMeta>, OutputScopeError> {
    if !config.protocols_known {
        return Err(OutputScopeError::IncompleteConfiguration);
    }
    scope::output_metadata(
        config.target.connection_id,
        config.revision,
        &config.map,
        &config.models,
    )
}

/// Take output metadata before acquiring the telemetry lock. A replaced target
/// or stale configuration cannot lend units/mapping to an old tap.
fn scope_outputs_for_target(state: &AppState, target: MotorTestTargetRef) -> Vec<ScopeOutputMeta> {
    let config = {
        let guard = state.motor_test.target.lock();
        guard
            .as_ref()
            .filter(|runtime| {
                runtime.target.target_ref() == target
                    && !runtime
                        .target
                        .config_stale
                        .load(std::sync::atomic::Ordering::Relaxed)
            })
            .and_then(|runtime| runtime.target.config.lock().clone())
    };
    config
        .as_ref()
        .filter(|config| config.target == target)
        .and_then(|config| output_scope_metadata(config).ok())
        .unwrap_or_default()
}

/// Read-only readiness for logger control as well as motor starts. The
/// caller holds AppState::ops; this never sends an arm/disarm command.
pub(crate) fn ensure_target_disarmed(
    state: &AppState,
    target: MotorTestTargetRef,
) -> Result<(), String> {
    let handle = {
        let selected = state.motor_test.target.lock();
        selected
            .as_ref()
            .filter(|runtime| {
                runtime.target.target_ref() == target && runtime.generation_is_current()
            })
            .map(|runtime| runtime.target.handle.clone())
            .ok_or_else(|| maggcs_core::logs::capture::SdCaptureError::TargetChanged.to_string())?
    };
    let env = handle
        .target_heartbeat()
        .ok_or("waiting for the selected serial PX4 heartbeat")?;
    let heartbeat = maggcs_core::telemetry::Heartbeat::try_from(&env)
        .map_err(|error| format!("cannot decode the selected PX4 heartbeat: {error}"))?;
    validate_target_heartbeat(
        target,
        &heartbeat,
        env.received_at,
        std::time::Instant::now(),
    )
}

fn validate_target_heartbeat(
    target: MotorTestTargetRef,
    heartbeat: &maggcs_core::telemetry::Heartbeat,
    received_at: std::time::Instant,
    now: std::time::Instant,
) -> Result<(), String> {
    if heartbeat.system_id != target.target_system_id
        || heartbeat.component_id != target.target_component_id
    {
        return Err("target heartbeat identity changed — reconnect the serial PX4".into());
    }
    if heartbeat.autopilot != maggcs_core::telemetry::Autopilot::Px4 {
        return Err("target is not a PX4 autopilot".into());
    }
    if now.saturating_duration_since(received_at) > TARGET_HEARTBEAT_STALE {
        return Err("target heartbeat is stale — check the serial PX4 link".into());
    }
    if heartbeat.base_mode.safety_armed {
        return Err("target is armed — disarm it before starting".into());
    }
    Ok(())
}

/// Refuse a start unless every active motor (the contiguous M1..MN prefix)
/// has an FC-read range and a known output protocol. Missing middle motors,
/// duplicates, unknown protocols and unread `DSHOT_MIN` are all blocking: the
/// fallback range would look real but is not.
fn refuse_unread_pwm_ranges(
    map: &OutputMap,
    models: &[PwmModel],
    protocols_known: bool,
    dshot_3d_enable: Option<i32>,
) -> Result<(), String> {
    if map.has_gap() {
        return Err(
            "output mapping has a gap (for example M1 and M3 without M2) —              fix PWM_*_FUNCn and refresh the mapping first"
                .into(),
        );
    }
    if map.state == maggcs_core::motor_test::mapping::OutputMapState::Invalid {
        return Err("output mapping is invalid (a motor function is assigned twice) —              fix PWM_*_FUNCn and refresh the mapping first"
            .into());
    }
    if map.state != maggcs_core::motor_test::mapping::OutputMapState::Complete {
        return Err(format!(
            "output mapping is not complete ({:?}) — a function or timer read may have failed; refresh the mapping and parameters first",
            map.state
        ));
    }
    if !protocols_known {
        return Err(
            "output protocol reads are incomplete — refresh the mapping and parameters first"
                .into(),
        );
    }
    let active = map.active_count();
    if active == 0 {
        return Err(
            "no configured motor outputs found — assign a motor function to              PWM_MAIN/AUX_FUNCn and refresh the mapping first"
                .into(),
        );
    }
    if models.len() < usize::from(active) {
        return Err(format!(
            "output configuration incomplete: expected {active} active motor models, got {} — refresh the mapping first",
            models.len()
        ));
    }
    let mut problems = Vec::new();
    for motor in 1..=active {
        let model = models[usize::from(motor - 1)];
        if model.range_source == PwmRangeSource::Fallback {
            problems.push(format!(
                "M{motor}: output range not read (check the link and PWM_*_MINn/MAXn{})",
                if model.protocol.is_dshot() {
                    " / DSHOT_MIN"
                } else {
                    ""
                }
            ));
        }
        if model.protocol == OutputProtocol::Unknown {
            problems.push(format!(
                "M{motor}: output protocol unknown (check PWM_MAIN/AUX_TIMn)"
            ));
        }
        if model.protocol.is_dshot() && dshot_3d_enable != Some(0) {
            problems.push(match dshot_3d_enable {
                Some(value) => format!(
                    "M{motor}: DSHOT_3D_ENABLE={value}; 3D mode changes the throttle range —                      set it to 0 before using the motor test"
                ),
                None => format!(
                    "M{motor}: DSHOT_3D_ENABLE could not be read; 3D mode would change the                      throttle range — refresh the parameters first"
                ),
            });
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "motor output configuration is not ready — {}; refresh the mapping and parameters first",
        problems.join("; ")
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

/// Reuse only a session bound to the same target and output configuration.
/// An idle session with obsolete metadata is released before replacement; a
/// running one is never silently re-bounded.
async fn reusable_session(
    state: &AppState,
    prepared: &PreparedStart,
) -> Result<Option<MotorTestService>, String> {
    let running = state.motor_test.running.lock().as_ref().map(|running| {
        (
            running.svc.clone(),
            running.matches(prepared.target) && running.scope_outputs == prepared.scope_outputs,
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
        return Err(
            "stop the motor test before changing the target or output configuration".into(),
        );
    }
    confirmed_stop(&service, false)
        .await
        .map_err(|error| format!("cannot replace the old motor-test session: {error}"))?;
    state.motor_test.running.lock().take();
    Ok(None)
}

fn spawn_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    prepared: &PreparedStart,
    tone_generation: u64,
    owned: Option<(
        &crate::motor_operations::MotorScope,
        maggcs_core::runtime::operations::OperationCancellation,
    )>,
) -> Result<MotorTestService, String> {
    let (service, events, output) =
        MotorTestService::spawn(prepared.handle.clone(), prepared.limits.clone());
    if let Some((scope, cancellation)) = &owned {
        service
            .bind_owner(cancellation.clone(), scope.deadline)
            .map_err(|error| error.to_string())?;
        *scope.service.lock() = Some(service.clone());
    }
    let forwarder = tauri::async_runtime::spawn(forward_events(
        app.clone(),
        events,
        prepared.target,
        tone_generation,
    ));
    *state.motor_test.running.lock() = Some(crate::motor_test_state::RunningMotorTest {
        operation: owned.map(|(scope, _)| scope.operation.clone()),
        svc: service.clone(),
        forwarder,
        output,
        target: prepared.target,
        scope_outputs: prepared.scope_outputs.clone(),
        tone_generation,
    });
    Ok(service)
}

/// Send the target PWM models before a start. The session applies them to
/// every subsequent value; a failed send aborts before the start command.
async fn send_pwm_models(
    service: &MotorTestService,
    prepared: &PreparedStart,
) -> Result<(), String> {
    // Width first: a preset/forward start drives exactly the configured
    // M1..MN prefix, never unmapped trailing motor functions.
    service
        .send(MotorTestCommand::SetMotorCount {
            motors: prepared.active_motors,
        })
        .await
        .map_err(|error| error.to_string())?;
    service
        .send(MotorTestCommand::SetPwmModels {
            models: prepared.pwm_models.clone(),
        })
        .await
        .map_err(|error| error.to_string())
}

/// Start manual sending across the full FC-read output range (normalized 0…1,
/// 2.0/s slew). The long-press confirmation happened in the UI.
#[tauri::command]
pub async fn actuator_start_manual(
    app: AppHandle,
    window: WebviewWindow,
    request: MotorStartRequest,
) -> Result<MotorStartReply, MotorOperationError> {
    if !matches!(request.source, MotorStartSource::Manual { .. }) {
        return Err(crate::motor_operations::invalid_source());
    }
    crate::motor_operations::start(&app, window.label(), request).await
}

pub(crate) enum StartSource {
    Manual(Vec<f32>),
    Preset(maggcs_core::motor_test::source::WaveformPreset),
    Forward,
}

/// All sources cross the same cancellable tone fence and atomic start commit.
#[cfg(test)]
pub(crate) async fn start_source<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    source: StartSource,
) -> Result<(), String> {
    start_scoped(app, state, source, None, || Ok(())).await
}

pub(crate) async fn start_scoped<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    source: StartSource,
    owned: Option<(
        &crate::motor_operations::MotorScope,
        maggcs_core::runtime::operations::OperationCancellation,
    )>,
    authorize: impl Fn() -> Result<(), String> + Send + Sync,
) -> Result<(), String> {
    authorize()?;
    crate::motor_control_service::ensure_enabled(state).map_err(|error| error.to_string())?;
    let expected = state.motor_test.tones.generation();
    let _ops = state.ops().lock().await;
    authorize()?;
    crate::motor_control_service::ensure_enabled(state).map_err(|error| error.to_string())?;
    ensure_idle_session(state)?;
    let prepared = prepare_start(state)?;
    crate::run_service::authorize_motor_start(app, state)?;
    let (command, values) = match source {
        StartSource::Manual(values) => (MotorTestCommand::StartManual, Some(values)),
        StartSource::Preset(preset) => (MotorTestCommand::StartPreset { preset }, None),
        StartSource::Forward => {
            let slot = forward_slot(state)?;
            (MotorTestCommand::StartForward { slot }, None)
        }
    };
    // Each run gets an event observer bound to its generation. A delayed stop
    // event from a prior run must never schedule a tone for the new run.
    if let Some(old) = reusable_session(state, &prepared).await? {
        confirmed_stop(&old, false).await?;
    }
    let old = state.motor_test.running.lock().take();
    if let Some(old) = old {
        old.forwarder.abort();
        let _ = old.forwarder.await;
    }
    let revision = prepared
        .scope_outputs
        .first()
        .ok_or("missing output configuration")?
        .revision;
    let mut lease =
        crate::motor_tone_service::begin_start(state, prepared.target, revision, expected)
            .map_err(|error| error.to_string())?;
    crate::motor_tone_service::play_motor_tone(state, lease.request)
        .await
        .map_err(|error| error.to_string())?;
    // Heartbeat and output configuration may have changed during the sound.
    let current = prepare_start(state)?;
    if current.target != prepared.target || current.scope_outputs != prepared.scope_outputs {
        return Err(crate::motor_tone_service::MotorToneError::TargetChanged.to_string());
    }
    authorize()?;
    ensure_idle_session(state)?;
    let service = spawn_session(
        app,
        state,
        &prepared,
        lease.request.session_generation,
        owned.clone(),
    )?;
    send_pwm_models(&service, &prepared).await?;
    if let Some(values) = values {
        service
            .send(MotorTestCommand::SetValues(values))
            .await
            .map_err(|error| error.to_string())?;
    }
    lease.commit(|| {
        authorize()?;
        crate::motor_control_service::with_enabled(state, || {
            service
                .try_send(command)
                .map_err(|error| error.to_string())?;
            state.motor_test.note_state(SessionState::ManualRunning);
            Ok(())
        })
    })?;
    if owned.is_some() {
        Ok(())
    } else {
        confirm_start_or_release(app, state).await
    }
}

/// The run manifest may only claim `MotorRunning` after the start was sent.
/// If that bookkeeping fails, stop the session rather than leave motors
/// running without a run.
async fn confirm_start_or_release<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
) -> Result<(), String> {
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
    window: WebviewWindow,
    request: MotorStartRequest,
) -> Result<MotorStartReply, MotorOperationError> {
    if !matches!(request.source, MotorStartSource::Preset { .. }) {
        return Err(crate::motor_operations::invalid_source());
    }
    crate::motor_operations::start(&app, window.label(), request).await
}

/// Latest-value-wins slider update (the UI throttles to ~30 Hz).
#[tauri::command]
pub async fn actuator_set_values(
    app: AppHandle,
    window: WebviewWindow,
    request: MotorValuesRequest,
) -> Result<MotorValuesReceipt, MotorOperationError> {
    crate::motor_operations::values(&app, window.label(), request)
}

#[cfg(test)]
pub(crate) async fn set_values_legacy(state: &AppState, values: Vec<f32>) -> Result<(), String> {
    crate::motor_control_service::ensure_enabled(state).map_err(|error| error.to_string())?;
    let expected = session_identity(state);
    // Serialize slider updates with ordinary session start/stop operations.
    let _ops = state.ops().lock().await;
    if expected.is_none()
        || session_identity(state) != expected
        || !state
            .motor_test
            .session()
            .is_some_and(|session| session.is_sending())
    {
        return Err(crate::motor_control_service::MotorControlError::Busy.to_string());
    }
    let service = service_of(state).ok_or("no motor-test session")?;
    crate::motor_control_service::with_enabled(state, || {
        service
            .try_send(MotorTestCommand::SetValues(values))
            .map_err(|error| error.to_string())
    })
}

/// Operator stop. Idempotent: fine with no session (link already died).
#[tauri::command]
pub async fn actuator_stop(
    app: AppHandle,
    window: WebviewWindow,
    request: MotorStopRequest,
) -> Result<MotorReleaseReceipt, MotorOperationError> {
    crate::motor_operations::stop(&app, window.label(), request).await
}

/// Emergency stop: latched in the session; the UI mirrors it.
#[tauri::command]
pub async fn actuator_estop(
    app: AppHandle,
    window: WebviewWindow,
    mut request: MotorStopRequest,
) -> Result<MotorReleaseReceipt, MotorOperationError> {
    request.emergency = true;
    let receipt = crate::motor_operations::stop(&app, window.label(), request).await?;
    pin_window(&app, false);
    Ok(receipt)
}

/// Release immediately without waiting for capture or connection-operation locks.
#[cfg(test)]
pub(crate) async fn emergency_stop_active_session(state: &AppState) -> Result<(), String> {
    state.motor_test.tones.cancel();
    let identity = session_identity(state);
    if let Some(service) = service_of(state) {
        let receipt = service
            .emergency_stop_and_wait()
            .await
            .map_err(|error| error.to_string())?;
        state.motor_test.note_state(receipt.state);
        if let Some((target, generation)) = identity {
            crate::motor_tone_service::after_release(state, target, generation);
        }
    } else {
        state.motor_test.note_state(SessionState::Emergency);
    }
    Ok(())
}

/// Explicit emergency reset (never automatic).
#[tauri::command]
pub async fn actuator_reset_emergency(
    app: AppHandle,
    window: WebviewWindow,
    request: MotorStopRequest,
) -> Result<(), MotorOperationError> {
    crate::motor_operations::reset_emergency(&app, window.label(), request).await
}

#[cfg(test)]
pub(crate) async fn reset_emergency_legacy(state: &AppState) -> Result<(), String> {
    if service_of(state).is_some() {
        send_session(state, MotorTestCommand::ResetEmergency).await
    } else {
        state.motor_test.note_state(SessionState::Idle);
        Ok(())
    }
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
    app: AppHandle,
    state: State<'_, AppState>,
    source: SitlSourceConfig,
    expected_serial: Option<maggcs_core::devices::SerialPortRef>,
) -> Result<(), String> {
    state.motor_test.tones.cancel();
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
    let serial = crate::serial_service::ensure(&app, &state).await?;
    let _manual = if matches!(
        config.endpoint,
        maggcs_core::mavlink::Endpoint::Serial { .. }
    ) {
        Some(
            serial
                .begin_manual(maggcs_core::config::serial::AutoConnectRole::PrimaryFc)
                .await?,
        )
    } else {
        None
    };
    let lease = serial
        .reserve_endpoint(
            &config.endpoint,
            expected_serial,
            maggcs_core::devices::SerialOwner::AuxiliarySource,
        )
        .await?;
    let _ops = state.ops().lock().await;
    if let Some(manual) = &_manual {
        manual.check()?;
    }
    crate::rtk_service::ensure_port_not_base(&state, &config.endpoint)?;
    let spawned = match lease {
        Some(lease) => {
            maggcs_core::mavlink::connection::spawn_leased_serial_connection(config, lease).await
        }
        None => spawn_connection(config).await,
    };
    let (handle, _events_rx, first_result) = spawned.map_err(|e| e.to_string())?;
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

    if let Some(manual) = &_manual {
        if let Err(error) = manual.check() {
            let _ = handle
                .shutdown_and_wait(maggcs_core::rtk::base::BASE_STOP_TIMEOUT)
                .await;
            return Err(error);
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
    let runtime = sitl_feed_runtime(&state, handle, source, SitlSourceOrigin::Dedicated)?;
    *state.motor_test.sitl.lock() = Some(runtime);
    Ok(())
}

/// Build the forward slot + tap for a SITL source. A dedicated runtime owns
/// its connection; a main-link runtime only owns the tap task, so detaching
/// or reconnecting never shuts the primary transport down.
fn sitl_feed_runtime(
    state: &AppState,
    handle: ConnectionHandle,
    source: SitlSourceConfig,
    origin: SitlSourceOrigin,
) -> Result<SitlLinkRuntime, String> {
    let source_ref = MotorForwardSourceRef {
        link_id: maggcs_core::runtime::LinkId::next().map_err(|error| error.to_string())?,
        generation: TransportGeneration::new(handle.generation())
            .map_err(|error| error.to_string())?,
        mavlink_target: maggcs_core::telemetry::VehicleId::new(
            source.system_id,
            source.component_id,
        ),
        port: source.port,
    };
    let slot: ForwardSlot = std::sync::Arc::new(std::sync::Mutex::new(ForwardState::new(
        usize::from(MOTOR_TEST_MOTOR_COUNT),
    )));
    let bus = state.motor_test.scope.clone();
    let tap = tauri::async_runtime::spawn(run_sitl_tap(
        handle.clone(),
        source.clone(),
        slot.clone(),
        bus,
    ));
    let runtime = match origin {
        SitlSourceOrigin::Dedicated => AuxiliaryLinkRuntime::new(handle, [tap]),
        SitlSourceOrigin::MainLink => {
            let mut runtime = AuxiliaryLinkRuntime::borrowed(&handle);
            runtime.add_task(tap);
            runtime
        }
    };
    Ok(SitlLinkRuntime {
        source_ref,
        slot,
        source,
        origin,
        runtime,
    })
}

/// Offer the live primary connection as the SITL feed. The tap subscribes to
/// the same frames the main window already receives, so PX4 keeps exactly one
/// UDP partner (PX4 v1.17 pins one GCS per MAVLink instance).
pub(crate) fn main_sitl_candidate(state: &AppState) -> Option<MainSitlSource> {
    let handle = state.connection()?;
    if handle.generation() == 0 {
        return None;
    }
    let link = state.link_status()?;
    if link.link_state != maggcs_core::mavlink::LinkState::Connected || !link.fc_alive {
        return None;
    }
    let identity = handle.target_identity()?;
    if identity.system_id == 0 || identity.component_id == 0 {
        return None;
    }
    Some(MainSitlSource {
        endpoint: handle.config().endpoint.to_address_string(),
        system_id: identity.system_id,
        component_id: identity.component_id,
        transport_generation: handle.generation(),
    })
}

/// Attach the forward feed to the already-connected primary link instead of
/// opening a second UDP stream.
#[tauri::command]
pub async fn actuator_use_main_sitl(
    state: State<'_, AppState>,
    expected: MainSitlSource,
    port: ServoPort,
) -> Result<(), String> {
    state.motor_test.tones.cancel();
    let _ops = state.ops().lock().await;
    let Some(handle) = state.connection() else {
        return Err("the main link is not connected".into());
    };
    if handle.generation() != expected.transport_generation
        || main_sitl_candidate(&state) != Some(expected.clone())
    {
        return Err("the main link changed; choose Use main link again".into());
    }
    let existing_sitl = state.motor_test.sitl.lock().is_some();
    stop_before_auxiliary_close(
        async {
            if existing_sitl {
                stop_active_session(&state).await?;
            }
            Ok(())
        },
        async {
            let old = state.motor_test.sitl.lock().take();
            if let Some(old) = old {
                old.stop().await;
            }
            Ok::<(), String>(())
        },
    )
    .await?;
    let source = SitlSourceConfig {
        endpoint: expected.endpoint,
        system_id: expected.system_id,
        component_id: expected.component_id,
        port,
    };
    let runtime = sitl_feed_runtime(&state, handle, source, SitlSourceOrigin::MainLink)?;
    *state.motor_test.sitl.lock() = Some(runtime);
    Ok(())
}

/// The live forward slot for a start, or the reason it cannot be used.
fn forward_slot(state: &AppState) -> Result<ForwardSlot, String> {
    let primary = state.connection();
    let guard = state.motor_test.sitl.lock();
    let link = guard.as_ref().ok_or("connect the SITL source first")?;
    if !link.generation_is_current(primary.as_ref()) {
        return Err("SITL source link changed; reconnect it".into());
    }
    Ok(link.slot.clone())
}

/// Drop the SITL source link.
#[tauri::command]
pub async fn actuator_disconnect_sitl(state: State<'_, AppState>) -> Result<(), String> {
    state.motor_test.tones.cancel();
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

/// A selected target must have a non-broadcast MAVLink system ID.
fn validate_target_system_id(target_system_id: u8) -> Result<u8, String> {
    if target_system_id == 0 {
        Err("target system ID must be between 1 and 255".into())
    } else {
        Ok(target_system_id)
    }
}

/// Identify the PX4 on the reserved port without writes, then transfer that
/// same transport into the connection worker using its observed IDs.
pub(crate) async fn connect_motor_serial_target(
    config: maggcs_core::mavlink::ConnectionConfig,
    lease: maggcs_core::devices::SerialLease,
    cancel: maggcs_core::devices::SerialCancellation,
) -> Result<maggcs_core::mavlink::connection::SpawnedConnection, String> {
    let maggcs_core::mavlink::Endpoint::Serial { baudrate, .. } = &config.endpoint else {
        return Err("Motor Test's PX4 target must use a serial connection".into());
    };
    let probed = maggcs_core::mavlink::serial::probe_fc_serial(
        lease,
        maggcs_core::mavlink::serial::FcProbeConfig {
            baud: maggcs_core::devices::BaudSelection::Fixed(*baudrate),
            local_system_id: None,
            expected_target: None,
        },
        cancel,
    )
    .await
    .map_err(|error| format!("cannot identify the PX4 on the selected serial port: {error}"))?;
    let config = motor_serial_target_config(config, probed.target(), probed.endpoint())?;
    probed
        .connect(config)
        .await
        .map_err(|error| error.to_string())
}

fn motor_serial_target_config(
    mut config: maggcs_core::mavlink::ConnectionConfig,
    identity: &maggcs_core::mavlink::serial::FcIdentity,
    endpoint: maggcs_core::mavlink::Endpoint,
) -> Result<maggcs_core::mavlink::ConnectionConfig, String> {
    config.target_system_id = validate_target_system_id(identity.system_id)?;
    config.target_component_id = identity.component_id;
    config.system_id = if config.target_system_id == MOTOR_GCS_SYSTEM_ID {
        MOTOR_GCS_ALTERNATE_SYSTEM_ID
    } else {
        MOTOR_GCS_SYSTEM_ID
    };
    config.component_id = MOTOR_GCS_COMPONENT_ID;
    config.endpoint = endpoint;
    config.auto_discover_target = false;
    Ok(config)
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
    expected_serial: Option<maggcs_core::devices::SerialPortRef>,
) -> Result<(), String> {
    crate::motor_control_service::revoke(&state);
    let config = maggcs_core::mavlink::ConnectionConfig {
        endpoint: maggcs_core::mavlink::Endpoint::try_from(endpoint.as_str())
            .map_err(|e| e.to_string())?,
        auto_discover_target: false,
        heartbeat_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    if !matches!(
        &config.endpoint,
        maggcs_core::mavlink::Endpoint::Serial { .. }
    ) {
        return Err("Motor Test's PX4 target must use a serial connection".into());
    }
    crate::main_link_service::ensure_port_not_primary(&state, &config.endpoint)?;
    let serial = crate::serial_service::ensure(&app, &state).await?;
    let _manual = serial
        .begin_manual(maggcs_core::config::serial::AutoConnectRole::PrimaryFc)
        .await?;
    let lease = serial
        .reserve_endpoint(
            &config.endpoint,
            expected_serial,
            maggcs_core::devices::SerialOwner::MotorTestTarget,
        )
        .await?
        .ok_or("Motor Test's PX4 target requires a serial reservation")?;
    let _ops = state.ops().lock().await;
    _manual.check()?;
    crate::main_link_service::ensure_port_not_primary(&state, &config.endpoint)?;
    crate::rtk_service::ensure_port_not_base(&state, &config.endpoint)?;
    let (handle, _events_rx, first_result) =
        connect_motor_serial_target(config, lease, _manual.cancel.clone()).await?;
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
    if let Err(error) = _manual.check() {
        let _ = handle
            .shutdown_and_wait(maggcs_core::rtk::base::BASE_STOP_TIMEOUT)
            .await;
        return Err(error);
    }
    let target_system_id = handle.config().target_system_id;
    let endpoint = handle.config().endpoint.to_address_string();
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
            crate::stream_service::stop(&state);
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
    if let Err(error) = _manual.check() {
        let _ = handle
            .shutdown_and_wait(maggcs_core::rtk::base::BASE_STOP_TIMEOUT)
            .await;
        return Err(error);
    }
    // The mag/ESC alarm reads HIGHRES_IMU / ESC_STATUS from the target FC.
    // Stream-rate requests and the parameter read are link-scoped tasks;
    // publish their owner before starting any observers.
    let (params, _param_events, parameter_task) =
        ParamService::spawn_with_task(handle.clone(), target_system_id, 1);
    let runtime = AuxiliaryLinkRuntime::new(
        handle.clone(),
        [tauri::async_runtime::spawn(async move {
            let _ = parameter_task.await;
        })],
    );
    install_target(
        &app,
        &state,
        handle,
        endpoint,
        target_system_id,
        params,
        runtime,
    );
    Ok(())
}

/// Publish one target and start only its tool-local tasks. A borrowed target
/// supplies the primary ParamService and a non-owning transport runtime.
pub(crate) fn install_target<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    handle: ConnectionHandle,
    endpoint: String,
    target_system_id: u8,
    params: ParamService,
    runtime: AuxiliaryLinkRuntime,
) {
    let initial_params = params.clone();
    let config = std::sync::Arc::new(parking_lot::Mutex::new(None));
    let config_slot = config.clone();
    let config_stale = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let config_stale_slot = config_stale.clone();
    let param_names = std::sync::Arc::new(parking_lot::Mutex::new(None));
    let param_names_slot = param_names.clone();
    let config_target = MotorTestTargetRef {
        connection_id: handle.handle_id(),
        target_system_id,
        target_component_id: TARGET_COMPONENT_ID,
    };
    let telemetry_handle = handle.clone();
    *state.motor_test.target.lock() = Some(TargetLinkRuntime {
        target: MotorTestTarget {
            handle,
            endpoint,
            target_system_id,
            params,
            restart_pending: Default::default(),
            config,
            config_stale,
            param_names,
        },
        runtime,
    });
    crate::stream_service::attach(app, state, telemetry_handle.clone(), target_system_id);
    attach_motor_telemetry(app, telemetry_handle.clone(), target_system_id);
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
        let read = read_output_config(&initial_params, &param_names_slot).await;
        let snapshot = OutputConfigSnapshot {
            revision: 1,
            target: config_target,
            map: read.map,
            models: read.models,
            protocols: read.protocols,
            protocols_known: read.protocols_known,
            dshot_3d_enable: read.dshot_3d_enable,
            dshot_bidir_en: read.dshot_bidir_en,
        };
        *config_slot.lock() = Some(snapshot);
        config_stale_slot.store(false, std::sync::atomic::Ordering::Relaxed);
    });
    if let Some(target) = state.motor_test.target.lock().as_mut() {
        target.runtime.add_task(task);
    } else {
        task.abort();
    }
    // Rebuild the SD-log runtime on the new target handle (feature 2).
    respawn_actuator_logs(app.clone(), state);
    // Task 6: one shell controller bound to this target/shell.
    crate::capture_service::spawn_actuator_capture(
        app.clone(),
        state,
        telemetry_handle,
        target_system_id,
    );
    // Task 8: close runs that a previous process left pending on this PX4.
    let target = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| runtime.target.target_ref());
    if let Some(target) = target {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            crate::run_service::reconcile_target(app, target).await;
        });
    }
}

/// Fire-and-forget `MAV_CMD_SET_MESSAGE_INTERVAL` (PX4 accepts it while
/// disarmed; a FC that lacks the stream simply never sends it).
#[cfg(test)]
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
pub async fn actuator_disconnect_target<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::motor_control_service::revoke(&state);
    crate::motor_operations::stop_all(&app, MOTOR_TEST_STOP_TIMEOUT).await?;
    crate::motor_capture_operations::stop_all(&app, Duration::from_secs(15)).await?;
    crate::motor_log_operations::stop_all(&app, Duration::from_secs(2)).await?;
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
            stop_actuator_logs(&state).await;
            crate::stream_service::stop(&state);
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
/// target model bounds the forwarded demand to its configured output range.
#[tauri::command]
pub async fn actuator_start_forward(
    app: AppHandle,
    window: WebviewWindow,
    request: MotorStartRequest,
) -> Result<MotorStartReply, MotorOperationError> {
    if !matches!(request.source, MotorStartSource::Forward { .. }) {
        return Err(crate::motor_operations::invalid_source());
    }
    crate::motor_operations::start(&app, window.label(), request).await
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

#[tauri::command]
pub async fn actuator_param_read(
    app: AppHandle,
    window: WebviewWindow,
    request: MotorParameterReadRequest,
) -> Result<ParamObservation, ParameterCommandError> {
    crate::motor_operations::authorize_context(&app, window.label(), &request.context)
        .map_err(crate::motor_operations::parameter_error)?;
    let work = crate::params_operations::begin(
        &app,
        window.label(),
        ParameterRequestContext {
            session: request.context.session,
            request_id: request.context.request_id,
        },
        crate::params_operations::ParameterAction::Read { name: request.name },
    )?;
    match work.run().await? {
        crate::params_operations::ParameterResult::Read(entry) => {
            Ok(ParamObservation { entry, age_ms: 0 })
        }
        _ => Err(OperationError::RegistryUnavailable.into()),
    }
}

#[tauri::command]
pub async fn actuator_param_set(
    app: AppHandle,
    window: WebviewWindow,
    request: MotorParameterSetRequest,
) -> Result<ParamEntry, ParameterCommandError> {
    let context = request.context.clone();
    crate::motor_operations::authorize_configuration(
        &app,
        window.label(),
        &context,
        &request.permission,
        None,
    )
    .map_err(crate::motor_operations::parameter_error)?;
    let name = request.name.clone();
    let (_, service) = target_params(&app.state::<AppState>())
        .map_err(|message| OperationError::InvalidRequest { message })?;
    let work = crate::params_operations::begin(
        &app,
        window.label(),
        ParameterRequestContext {
            session: request.context.session,
            request_id: request.context.request_id,
        },
        crate::params_operations::ParameterAction::Set {
            name: request.name,
            value: request.value,
            ptype: request.ptype,
        },
    )?;
    let result = work.run().await?;
    crate::motor_operations::authorize_configuration(
        &app,
        window.label(),
        &context,
        &request.permission,
        None,
    )
    .map_err(crate::motor_operations::parameter_error)?;
    let entry = service
        .cache()
        .get(&name)
        .ok_or(OperationError::RegistryUnavailable)?;
    if matches!(
        name.as_str(),
        "SDLOG_MODE" | "SDLOG_BACKEND" | "SDLOG_PROFILE"
    ) {
        if let Some(target) = app.state::<AppState>().motor_test.target.lock().as_ref() {
            target
                .target
                .restart_pending
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let _ = result;
    Ok(entry)
}

/// Rebuild the SD-log runtime for the current serial target PX4. Idempotent:
/// a previous runtime is stopped and its last list/error kept for the UI.
#[cfg(any())]
fn respawn_actuator_logs_legacy<R: tauri::Runtime>(app: AppHandle<R>, state: &AppState) {
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

fn respawn_actuator_logs<R: tauri::Runtime>(app: AppHandle<R>, state: &AppState) {
    let target = state.motor_test.target.lock().as_ref().map(|runtime| {
        (
            runtime.origin(),
            runtime.target.handle.clone(),
            runtime.target.target_system_id,
        )
    });
    let Some((origin, handle, target_system)) = target else {
        return;
    };
    let mut logs = state.motor_test.logs.lock();
    if let Some(old) = logs.runtime.take() {
        drop(old);
    }
    if origin == crate::motor_test_state::TargetLinkOrigin::MainSerial {
        return; // Borrow the primary link's existing LogService through RuntimeFacade.
    }
    let (service, mut events) =
        maggcs_core::logs::LogService::spawn(handle, target_system, TARGET_COMPONENT_ID);
    let forward_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            crate::motor_log_operations::forward_event(&forward_app, event);
        }
    });
    logs.runtime = Some(crate::motor_test_state::ActuatorLogsRuntime {
        service: Some(service),
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
}

#[cfg(any())]
fn actuator_logs_payload_failed(message: String) -> ActuatorLogsEvent {
    ActuatorLogsEvent::Failed { message }
}

#[cfg(any())]
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

pub(crate) fn publish_actuator_log_list<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    entries: Vec<maggcs_core::logs::LogEntryInfo>,
) {
    let logs = state.motor_test.logs.lock();
    *logs.last_list.lock().expect("logs list lock") = Some(entries.clone());
    *logs.last_error.lock().expect("logs err lock") = None;
    let _ = app.emit("actuator_logs", ActuatorLogsEvent::List { entries });
}

pub(crate) fn invalidate_actuator_log_list(state: &AppState) {
    let logs = state.motor_test.logs.lock();
    *logs.last_list.lock().expect("logs list lock") = None;
    *logs.last_error.lock().expect("logs err lock") = None;
}

/// Caller holds ops, so no target/capture/deletion mutation can change the
/// index between its request and the verified result.
#[cfg(any())]
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

pub(crate) fn same_log_entry(
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
#[cfg(any())]
async fn actuator_logs_request_list_legacy(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<maggcs_core::logs::LogEntryInfo>, String> {
    let _ops = state.ops().lock().await;
    refresh_actuator_log_list(&app, &state).await
}

/// Download one SD log from the serial target PX4 into a save-dialog path
/// (feature 2). A cancelled save dialog returns Ok without starting.
#[tauri::command]
#[cfg(any())]
async fn actuator_logs_download_legacy(
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
#[cfg(any())]
async fn actuator_logs_erase_all_legacy(
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
#[cfg(any())]
async fn actuator_logs_delete_legacy(
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
#[cfg(any())]
async fn actuator_logs_cancel_legacy(state: State<'_, AppState>) -> Result<(), String> {
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
#[cfg(test)]
async fn send_session(state: &AppState, cmd: MotorTestCommand) -> Result<(), String> {
    match service_of(state) {
        Some(svc) => svc.send(cmd).await.map_err(|e| e.to_string()),
        None => Err("no motor-test session".into()),
    }
}

/// Stop the auxiliary owners outside any state lock; their tasks are
/// cancelled/joined before their connection workers are shut down.
pub(crate) async fn stop_auxiliary_links(app: &AppHandle, state: &AppState) {
    crate::motor_control_service::revoke(state);
    for result in [
        crate::motor_operations::stop_all(app, MOTOR_TEST_STOP_TIMEOUT).await,
        crate::motor_capture_operations::stop_all(app, Duration::from_secs(15)).await,
        crate::motor_log_operations::stop_all(app, Duration::from_secs(2)).await,
    ] {
        if let Err(error) = result {
            let _ = app.emit("actuator_stop_warning", error);
            return;
        }
    }
    crate::stream_service::stop(state);
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
    state.motor_test.tones.cancel();
    let identity = session_identity(state);
    let Some(service) = service_of(state) else {
        return Ok(());
    };
    let receipt = tokio::time::timeout(MOTOR_TEST_STOP_TIMEOUT, confirmed_stop(&service, false))
        .await
        .map_err(|_| "timed out waiting for motor-test release; link kept open".to_string())?
        .map_err(|error| format!("motor-test release failed; link kept open: {error}"))?;
    state.motor_test.note_state(receipt.state);
    if let Some((target, generation)) = identity {
        crate::motor_tone_service::after_release(state, target, generation);
    }
    Ok(())
}

pub(crate) async fn confirmed_stop(
    service: &MotorTestService,
    emergency: bool,
) -> Result<maggcs_core::motor_test::StopReceipt, String> {
    let result = if emergency {
        service.emergency_stop_and_wait().await
    } else {
        service.stop_and_wait().await
    };
    match result {
        Ok(receipt) => Ok(receipt),
        Err(error) => match service.release_result() {
            Some(Ok(receipt)) => Ok(receipt),
            _ => Err(error.to_string()),
        },
    }
}

fn session_identity(state: &AppState) -> Option<(MotorTestTargetRef, u64)> {
    state
        .motor_test
        .running
        .lock()
        .as_ref()
        .map(|running| (running.target, running.tone_generation))
}

/// Push session events to the window, remember the state for the status
/// poll, and pin the window while sending.
async fn forward_events<R: tauri::Runtime>(
    app: AppHandle<R>,
    mut rx: tokio::sync::mpsc::Receiver<MotorTestEvent>,
    target: MotorTestTargetRef,
    generation: u64,
) {
    while let Some(ev) = rx.recv().await {
        if let Some(state) = app.try_state::<AppState>() {
            if session_identity(&state) != Some((target, generation)) {
                continue;
            }
        }
        match ev {
            MotorTestEvent::StateChanged(s) => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.motor_test.note_state(s);
                    // Exact send start/stop instants for the scope's shaded
                    // "motor test sending" band.
                    state
                        .motor_test
                        .scope
                        .push_all(vec![scope::ScopeSample::value(
                            "session.running".into(),
                            now_epoch_s(),
                            if s.is_sending() { 1.0 } else { 0.0 },
                        )]);
                }
                pin_window(&app, s.is_sending());
                let _ = app.emit("actuator_event", SessionPayload::State { state: s });
            }
            MotorTestEvent::Stopped(reason) => {
                eprintln!("motor test stopped: {reason:?}");
                if let Some(state) = app.try_state::<AppState>() {
                    crate::run_service::note_motor_released(&app, &state, reason);
                    crate::motor_tone_service::after_release(&state, target, generation);
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
fn pin_window<R: tauri::Runtime>(app: &AppHandle<R>, on_top: bool) {
    if let Some(win) = app.get_webview_window("actuator") {
        let _ = win.set_always_on_top(on_top);
    }
}

impl AppState {
    /// Snapshot the selected Motor Test FC, whether dedicated or borrowed.
    pub fn actuator_status(&self) -> ActuatorStatus {
        let target = self
            .motor_test
            .target
            .lock()
            .as_ref()
            .filter(|runtime| runtime.generation_is_current())
            .map(|runtime| (runtime.target.target_ref(), runtime.target.handle.clone()));
        // FC state comes directly from the selected connection's retained
        // heartbeat, even before the window's mag/ESC observer attaches.
        let target_hb = target.and_then(|(target, handle)| {
            let env = handle.target_heartbeat()?;
            let heartbeat = maggcs_core::telemetry::Heartbeat::try_from(&env).ok()?;
            (heartbeat.system_id == target.target_system_id
                && heartbeat.component_id == target.target_component_id
                && heartbeat.autopilot == maggcs_core::telemetry::Autopilot::Px4)
                .then_some((heartbeat, env.received_at))
        });
        let target_fc_alive = target_hb
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() <= TARGET_HEARTBEAT_STALE);
        let (armed, custom_mode, autopilot_px4, mode_label) = match target_hb {
            Some((hb, _)) => (
                hb.base_mode.safety_armed,
                hb.custom_mode,
                hb.autopilot == maggcs_core::telemetry::Autopilot::Px4,
                hb.mode_label,
            ),
            None => (false, 0, false, String::new()),
        };
        let session = match self.motor_test.session() {
            Some(SessionState::ManualRunning) => "running",
            Some(SessionState::Stopping) => "stopping",
            Some(SessionState::Emergency) => "emergency",
            _ => "idle",
        };
        let (
            sitl_connected,
            sitl_origin,
            sitl_endpoint,
            sitl_source,
            sitl_feed_age_s,
            input_values,
            input_pwm,
        ) = {
            let primary = self.connection();
            let guard = self.motor_test.sitl.lock();
            match guard
                .as_ref()
                .filter(|link| link.generation_is_current(primary.as_ref()))
            {
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
                    // The same sample's normalized and raw values, so the
                    // window shows the exact source percentage.
                    let (values, pwm) = link
                        .slot
                        .lock()
                        .ok()
                        .map(|st| (st.values.clone(), st.raw_pwm.clone()))
                        .unwrap_or_default();
                    (
                        true,
                        Some(link.origin),
                        Some(link.source.endpoint.clone()),
                        Some(link.source.clone()),
                        age,
                        values,
                        pwm,
                    )
                }
                None => (false, None, None, None, None, Vec::new(), Vec::new()),
            }
        };
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
            config,
            config_target,
            config_stale,
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
                    runtime.target.config.lock().clone(),
                    Some(runtime.target.target_ref()),
                    runtime
                        .target
                        .config_stale
                        .load(std::sync::atomic::Ordering::Relaxed),
                    self.motor_test.capture.lock().status.lock().clone(),
                ),
                None => (
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    false,
                    self.motor_test.capture.lock().status.lock().clone(),
                ),
            }
        };
        // One snapshot, cloned once: map, models, protocols and DShot flags
        // always describe the same revision.
        let output_map = config
            .as_ref()
            .map(|snapshot| snapshot.map.clone())
            .unwrap_or_else(|| OutputMap::unknown(MOTOR_TEST_MOTOR_COUNT));
        let pwm_models = config
            .as_ref()
            .map(|snapshot| snapshot.models.clone())
            .unwrap_or_default();
        let output_protocols = config
            .as_ref()
            .map(|snapshot| snapshot.protocols)
            .unwrap_or_default();
        let dshot_3d_enable = config
            .as_ref()
            .and_then(|snapshot| snapshot.dshot_3d_enable);
        let dshot_bidir_en = config.as_ref().and_then(|snapshot| snapshot.dshot_bidir_en);
        let scope_outputs = if config_stale {
            Vec::new()
        } else {
            config
                .as_ref()
                .filter(|config| Some(config.target) == config_target)
                .and_then(|config| output_scope_metadata(config).ok())
                .unwrap_or_default()
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
        let esc_rpm = {
            let telemetry = self.motor_test.telemetry.lock();
            let now = std::time::Instant::now();
            let mut esc_rpm = Vec::new();
            if let Some(telemetry) = telemetry.as_ref() {
                for esc in 1..=telemetry.esc.esc_count() {
                    let at = telemetry.esc_seen_at.get(esc - 1).copied().flatten();
                    let age = at.map(|at| now.saturating_duration_since(at));
                    let rpm = at
                        .and_then(|_| telemetry.esc.sample(esc - 1))
                        .map(|s| s.rpm);
                    esc_rpm.push(EscRpmStatus {
                        esc: esc as u8,
                        rpm,
                        age_s: age.map(|age| age.as_secs_f64()),
                        stale: age.is_some_and(|age| age > ESC_RPM_STALE),
                    });
                }
            }
            esc_rpm
        };
        let target_origin = self
            .motor_test
            .target
            .lock()
            .as_ref()
            .map(TargetLinkRuntime::origin);
        let operation = {
            self.motor_test
                .running
                .lock()
                .as_ref()
                .and_then(|running| running.operation.clone())
        };
        let capture_operation = self.motor_test.capture_operations.current();
        let config_revision = {
            self.motor_test.target.lock().as_ref().and_then(|runtime| {
                runtime
                    .target
                    .config
                    .lock()
                    .as_ref()
                    .and_then(|config| Revision::new(config.revision).ok())
            })
        };
        let forward_source_ref = {
            self.motor_test
                .sitl
                .lock()
                .as_ref()
                .map(|link| link.source_ref.clone())
        };
        let motor_control = crate::motor_control_service::status(self);
        ActuatorStatus {
            operation,
            capture_operation,
            config_revision,
            forward_source_ref,
            motor_control,
            streams: crate::stream_service::snapshot(self),
            tones: self.motor_test.tones.snapshot(),
            connected: target_connected,
            fc_alive: target_fc_alive,
            target_fc_alive,
            armed,
            custom_mode,
            autopilot_px4,
            mode_label,
            endpoint: target_endpoint.clone(),
            tick_hz: DEFAULT_TICK_HZ,
            session: session.to_string(),
            sitl_connected,
            sitl_endpoint,
            sitl_source,
            sitl_origin,
            sitl_feed_age_s,
            main_sitl_source: main_sitl_candidate(self),
            target_connected,
            target_endpoint,
            target_system_id,
            target_origin,
            main_serial_target: crate::main_link_service::candidate(self),
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
            scope_outputs,
            output_protocols,
            dshot_3d_enable,
            dshot_bidir_en,
            esc_rpm,
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
#[cfg(test)]
async fn read_output_map(params: &ParamService) -> OutputMap {
    read_output_map_with_names(params, None).await
}

async fn read_output_map_with_names(
    params: &ParamService,
    names: Option<&std::collections::HashSet<String>>,
) -> OutputMap {
    let mut main = [FunctionRead::Absent; SERVO_CHANNELS_PER_PORT as usize];
    let mut aux = [FunctionRead::Absent; SERVO_CHANNELS_PER_PORT as usize];
    if names.is_none() {
        main = [FunctionRead::Unreadable; SERVO_CHANNELS_PER_PORT as usize];
        aux = [FunctionRead::Unreadable; SERVO_CHANNELS_PER_PORT as usize];
    }
    for port in [ServoPort::Main, ServoPort::Aux] {
        let values = match port {
            ServoPort::Main => &mut main,
            ServoPort::Aux => &mut aux,
        };
        for channel in 1..=SERVO_CHANNELS_PER_PORT {
            let name = format!("{}_FUNC{channel}", port.param_prefix());
            if names.is_some_and(|known| !known.contains(&name)) {
                values[usize::from(channel - 1)] = FunctionRead::Absent;
                continue;
            }
            let result = if names.is_some() {
                params.read(&name).await
            } else {
                params.read_with_timeout(&name, MAPPING_PROBE_TIMEOUT).await
            };
            values[usize::from(channel - 1)] = match result {
                Ok(entry) => match entry.value {
                    ParamValue::Int(value) => FunctionRead::Value(value),
                    _ => FunctionRead::Unreadable,
                },
                Err(_) => FunctionRead::Unreadable,
            };
        }
    }
    build_output_map_checked(MOTOR_TEST_MOTOR_COUNT, &main, &aux)
}

/// One reader pass over the target's output configuration. The caller adds
/// the target identity and revision before publishing it as one snapshot.
struct OutputConfigRead {
    map: OutputMap,
    models: Vec<PwmModel>,
    protocols: OutputProtocols,
    protocols_known: bool,
    dshot_3d_enable: Option<i32>,
    dshot_bidir_en: Option<i32>,
}

impl OutputConfigRead {
    fn is_startable(&self) -> bool {
        self.map.state == maggcs_core::motor_test::mapping::OutputMapState::Complete
            && self.protocols_known
    }

    /// A transient read failed (as opposed to a mapping the FC actually
    /// reports as invalid). Such a result must not replace a usable snapshot.
    fn read_failure(&self) -> bool {
        let active = usize::from(self.map.active_count());
        let models = &self.models;
        self.map
            .issues
            .iter()
            .any(|issue| matches!(issue, MappingIssue::Unreadable { .. }))
            || !self.protocols_known
            || models
                .iter()
                .take(active)
                .any(|model| model.range_source == PwmRangeSource::Fallback)
            || (self.dshot_3d_enable.is_none()
                && models
                    .iter()
                    .take(active)
                    .any(|model| model.protocol.is_dshot()))
    }
}

/// Read map, models, protocols and DShot flags from one link pass. A failed
/// read is preserved as `Partial`; it is never converted into "disabled".
async fn read_output_config(
    params: &ParamService,
    names_slot: &parking_lot::Mutex<Option<std::collections::HashSet<String>>>,
) -> OutputConfigRead {
    let mut names = names_slot.lock().clone();
    let mut map = read_output_map_with_names(params, names.as_ref()).await;
    let (mut protocols, mut protocols_known) =
        read_output_protocols_checked(params, names.as_ref()).await;
    if names.is_none() && (map_has_unreadable(&map) || !protocols_known) {
        // Reduced-channel boards expose fewer FUNC/TIM parameters; only a
        // complete FC inventory can prove that a missing name is absent.
        if let Some(fresh) = ensure_param_names(params, names_slot).await {
            names = Some(fresh);
            map = read_output_map_with_names(params, names.as_ref()).await;
            (protocols, protocols_known) =
                read_output_protocols_checked(params, names.as_ref()).await;
        }
    }
    let models = read_pwm_models(params, &map, protocols).await;
    OutputConfigRead {
        map,
        models,
        protocols,
        protocols_known,
        dshot_3d_enable: read_int_param(params, "DSHOT_3D_ENABLE").await,
        dshot_bidir_en: read_int_param(params, "DSHOT_BIDIR_EN").await,
    }
}

/// Read the FC's full parameter-name inventory if it is not cached yet. A
/// partial pull is not used: absent-channel inference requires the complete
/// declared set. Returns the inventory when already known or freshly read.
async fn ensure_param_names(
    params: &ParamService,
    slot: &parking_lot::Mutex<Option<std::collections::HashSet<String>>>,
) -> Option<std::collections::HashSet<String>> {
    if let Some(names) = slot.lock().clone() {
        return Some(names);
    }
    let report = params.pull_and_wait(Duration::from_secs(60)).await.ok()?;
    if !report.is_complete() {
        return None;
    }
    let names: std::collections::HashSet<String> = params.cache().names().into_iter().collect();
    *slot.lock() = Some(names.clone());
    Some(names)
}

/// A map with any failed read is only partial. The FC's parameter inventory
/// can prove that a channel does not exist, which is not a failure.
fn map_has_unreadable(map: &OutputMap) -> bool {
    map.issues
        .iter()
        .any(|issue| matches!(issue, MappingIssue::Unreadable { .. }))
}

/// Read one integer parameter, if it is currently readable.
async fn read_int_param(params: &ParamService, name: &str) -> Option<i32> {
    read_int_param_bounded(params, name, Duration::from_secs(5)).await
}

async fn read_int_param_bounded(
    params: &ParamService,
    name: &str,
    timeout: Duration,
) -> Option<i32> {
    match params.read_with_timeout(name, timeout).await.ok()?.value {
        ParamValue::Int(value) => i32::try_from(value).ok(),
        ParamValue::Real(value) => Some(value as i32),
    }
}

/// Build one PWM/OneShot model from the raw `PWM_*_` parameter reads.
/// Unmapped motors or missing/invalid endpoints fall back explicitly
/// (`PwmRangeSource::Fallback` plus the 1100–1900 default) — never silently
/// to a plausible-looking range.
fn pwm_model_from_raw(
    min: Option<i32>,
    max: Option<i32>,
    reversed: bool,
    disabled: bool,
    thrust_factor: f32,
    mapped: bool,
    protocol: OutputProtocol,
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
        .with_protocol(protocol)
}

/// Build a DShot/BDShot model. `DSHOT_MIN` is required: an unread value must
/// not masquerade as a valid 0…1999 range, so it stays marked fallback.
fn dshot_model_from_raw(
    dshot_min: Option<f32>,
    reversed: bool,
    disabled: bool,
    thrust_factor: f32,
    protocol: OutputProtocol,
) -> PwmModel {
    match dshot_min {
        Some(value) => PwmModel::dshot(value, reversed, thrust_factor, protocol)
            .map(|model| model.with_disabled(disabled))
            .unwrap_or_else(|_| dshot_fallback_model(reversed, disabled, thrust_factor, protocol)),
        None => dshot_fallback_model(reversed, disabled, thrust_factor, protocol),
    }
}

fn dshot_fallback_model(
    reversed: bool,
    disabled: bool,
    thrust_factor: f32,
    protocol: OutputProtocol,
) -> PwmModel {
    let mut model = PwmModel::dshot(0.0, reversed, thrust_factor, protocol).unwrap_or_default();
    model.range_source = PwmRangeSource::Fallback;
    model.with_disabled(disabled)
}

/// Read `PWM_MAIN_TIMn` / `PWM_AUX_TIMn` and fold each bank into one
/// protocol. MAJOR (MAIN) exposes TIM0..2; AUX exposes TIM0..3 on Pixhawk 6X.
///
/// With a complete parameter inventory, channels the board does not have are
/// skipped instead of counting as failed reads. Any present-but-unreadable
/// timer keeps the snapshot incomplete, so the start gate can refuse it.
async fn read_output_protocols_checked(
    params: &ParamService,
    names: Option<&std::collections::HashSet<String>>,
) -> (OutputProtocols, bool) {
    async fn bank(
        params: &ParamService,
        port: ServoPort,
        names: Option<&std::collections::HashSet<String>>,
    ) -> (OutputProtocol, bool) {
        let count = match port {
            ServoPort::Main => 3,
            ServoPort::Aux => 4,
        };
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            let name = format!("{}_TIM{index}", port.param_prefix());
            if names.is_some_and(|known| !known.contains(&name)) {
                continue;
            }
            let value = if names.is_some() {
                read_int_param(params, &name).await
            } else {
                read_int_param_bounded(params, &name, MAPPING_PROBE_TIMEOUT).await
            };
            values.push(value.map(i64::from));
        }
        detect_bank_protocol_checked(&values)
    }
    let (main, main_known) = bank(params, ServoPort::Main, names).await;
    let (aux, aux_known) = bank(params, ServoPort::Aux, names).await;
    (OutputProtocols { main, aux }, main_known && aux_known)
}

#[cfg(test)]
async fn read_output_protocols(params: &ParamService) -> OutputProtocols {
    read_output_protocols_checked(params, None).await.0
}

/// Read the mapped channel's `PWM_*_MIN/MAX/REV/DISn` plus the global
/// `THR_MDL_FAC` and build one model per motor. DShot/BDShot derive their
/// range from `DSHOT_MIN` instead of the PWM endpoints. Missing/invalid
/// values and unmapped motors are marked fallback; the start gate rejects
/// them.
async fn read_pwm_models(
    params: &ParamService,
    map: &OutputMap,
    protocols: OutputProtocols,
) -> Vec<PwmModel> {
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
    let dshot_min = read_real_param(params, "DSHOT_MIN").await;
    let mut models = Vec::with_capacity(usize::from(MOTOR_TEST_MOTOR_COUNT));
    for motor in 1..=MOTOR_TEST_MOTOR_COUNT {
        let Some(channel) = map.channel_for(motor) else {
            models.push(PwmModel::default());
            continue;
        };
        let prefix = channel.port.param_prefix();
        let n = channel.channel;
        let protocol = match channel.port {
            ServoPort::Main => protocols.main,
            ServoPort::Aux => protocols.aux,
        };
        let reversed = read_int_param(params, &format!("{prefix}_REV{n}"))
            .await
            .is_some_and(|value| value != 0);
        let disabled = read_int_param(params, &format!("{prefix}_DIS{n}"))
            .await
            .is_some_and(|value| value != 0);
        if protocol.is_dshot() {
            models.push(dshot_model_from_raw(
                dshot_min, reversed, disabled, factor, protocol,
            ));
            continue;
        }
        let min = read_int_param(params, &format!("{prefix}_MIN{n}")).await;
        let max = read_int_param(params, &format!("{prefix}_MAX{n}")).await;
        let mapped = map.channel_for(motor).is_some();
        models.push(pwm_model_from_raw(
            min, max, reversed, disabled, factor, mapped, protocol,
        ));
    }
    models
}

/// Read one real-valued parameter, if it is currently readable.
async fn read_real_param(params: &ParamService, name: &str) -> Option<f32> {
    match params.read(name).await.ok()?.value {
        ParamValue::Real(value) => Some(value),
        ParamValue::Int(value) => Some(value as f32),
    }
}

/// Re-read the target's output assignments and PWM models. Safe to call while
/// idle; the UI freezes the action while a test is sending.
#[tauri::command]
pub async fn actuator_refresh_mapping(
    app: AppHandle,
    window: WebviewWindow,
    request: maggcs_core::motor_test::operation_types::MotorRequestContext,
) -> Result<OutputMap, MotorOperationError> {
    crate::motor_operations::authorize_context(&app, window.label(), &request)?;
    let state = app.state::<AppState>();
    let (params, config_slot, config_stale, param_names_slot, target_ref) = {
        let guard = state.motor_test.target.lock();
        let runtime = guard.as_ref().ok_or_else(|| {
            crate::motor_operations::invalid_request("no serial PX4 target — connect it first")
        })?;
        (
            runtime.target.params.clone(),
            runtime.target.config.clone(),
            runtime.target.config_stale.clone(),
            runtime.target.param_names.clone(),
            runtime.target.target_ref(),
        )
    };
    let read = read_output_config(&params, &param_names_slot).await;
    // The read can take seconds (a first pull). Only the validation and the
    // publish are serialized with start/stop, so E-STOP never waits on I/O.
    let _ops = state.ops().lock().await;
    crate::motor_operations::authorize_context(&app, window.label(), &request)?;
    ensure_idle_session(&state).map_err(crate::motor_operations::invalid_request)?;
    let current = {
        let guard = state.motor_test.target.lock();
        let runtime = guard.as_ref().ok_or_else(|| {
            crate::motor_operations::invalid_request("no serial PX4 target — connect it first")
        })?;
        if runtime.target.target_ref() != target_ref {
            return Err(crate::motor_operations::invalid_request(
                "target changed while reading — refresh again",
            ));
        }
        let current = runtime.target.config.lock().clone();
        current
    };
    if !read.is_startable() && read.read_failure() {
        // Keep the last complete snapshot for inspection, but mark it stale
        // so START cannot use it after a failed read.
        config_stale.store(true, std::sync::atomic::Ordering::Relaxed);
        if current
            .as_ref()
            .is_some_and(OutputConfigSnapshot::is_startable)
        {
            let message = refuse_unread_pwm_ranges(
                &read.map,
                &read.models,
                read.protocols_known,
                read.dshot_3d_enable,
            )
            .err()
            .unwrap_or_else(|| "output configuration is incomplete — refresh again".into());
            return Err(crate::motor_operations::invalid_request(message));
        }
    }
    let revision = current
        .as_ref()
        .map_or(1, |snapshot| snapshot.revision.saturating_add(1));
    let snapshot = OutputConfigSnapshot {
        revision,
        target: target_ref,
        map: read.map,
        models: read.models,
        protocols: read.protocols,
        protocols_known: read.protocols_known,
        dshot_3d_enable: read.dshot_3d_enable,
        dshot_bidir_en: read.dshot_bidir_en,
    };
    config_stale.store(
        !snapshot.is_startable(),
        std::sync::atomic::Ordering::Relaxed,
    );
    *config_slot.lock() = Some(snapshot.clone());
    Ok(snapshot.map)
}

/// Attach the mag/ESC/battery telemetry tap to a link (A4 wiring): subscribes
/// to all four supported mag messages, `BATTERY_STATUS` (147, total current)
/// and `ESC_STATUS` (291). Every mag source gets independent scope ids;
/// `HIGHRES_IMU` alone drives the alarm/status readout.
/// Called from `connect` after the link comes up.
pub fn attach_motor_telemetry<R: tauri::Runtime>(
    app: &AppHandle<R>,
    handle: maggcs_core::mavlink::ConnectionHandle,
    target_system_id: u8,
) {
    // The link-scoped stream manager owns rate requests, including defaults.
    let state = app.state::<AppState>();
    // (Re)attach resets the alarm clock; the loaded calibration survives
    // (it is a file, not link state). The target runtime owns the task.
    let events = handle.subscribe_route(
        MessageRoute::messages(&[
            0,
            36,
            SCALED_IMU_ID,
            HIGHRES_IMU_ID,
            SCALED_IMU2_ID,
            SCALED_IMU3_ID,
            147,
            291,
        ])
        .from(target_system_id, TARGET_COMPONENT_ID),
    );
    let retained = handle.target_heartbeat().filter(|env| {
        env.header.system_id == target_system_id && env.header.component_id == TARGET_COMPONENT_ID
    });
    *state.motor_test.telemetry.lock() = Some(crate::motor_test_state::MotorTelemetry {
        esc: maggcs_core::signals::derived::EscAggregator::new(scope::MAX_ESC),
        alarm: maggcs_core::motor_test::MagAlarm::new(Default::default()),
        // The loaded calibration is process-wide, so it survives link swaps
        // and is re-applied here on every attach.
        calib: state.calibration.get(),
        last_mag_total_nt: None,
        hb: retained
            .as_ref()
            .and_then(|env| maggcs_core::telemetry::Heartbeat::try_from(env).ok()),
        fc_outputs: [None, None],
        last_heartbeat_at: retained.as_ref().map(|env| env.received_at),
        esc_seen_at: [None; scope::MAX_ESC],
    });
    let tap_app = app.clone();
    let telemetry_task = tauri::async_runtime::spawn(run_motor_telemetry_tap(
        tap_app,
        handle,
        target_system_id,
        events,
    ));
    {
        let mut target = state.motor_test.target.lock();
        if let Some(target) = target.as_mut() {
            target.runtime.add_tasks([telemetry_task]);
        } else {
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

/// Update only the ESC slots covered by this standard MAVLink frame.
/// `index` identifies its first ESC; other slots keep their samples and ages.
fn store_esc_status(
    telemetry: &mut crate::motor_test_state::MotorTelemetry,
    esc: &::mavlink::common::ESC_STATUS_DATA,
    received_at: std::time::Instant,
) {
    telemetry
        .esc
        .feed(esc.index, &esc.rpm, &esc.voltage, &esc.current);
    for slot in esc_frame_range(esc.index, telemetry.esc_seen_at.len()) {
        telemetry.esc_seen_at[slot] = Some(received_at);
    }
}

/// Decode one complete Motor Test magnetometer frame and normalize it to nT.
/// The source enum is carried beside the values all the way to the scope ids.
fn scope_mag_frame(message: &MavMessage) -> Option<(MagScopeSource, [f64; 3])> {
    match message {
        MavMessage::HIGHRES_IMU(imu) => Some((
            MagScopeSource::HighresImu,
            [imu.xmag, imu.ymag, imu.zmag]
                .map(|gauss| f64::from(gauss) * maggcs_core::signals::GAUSS_TO_NANOTESLA),
        )),
        MavMessage::SCALED_IMU(imu) => Some((
            MagScopeSource::ScaledImu1,
            [imu.xmag, imu.ymag, imu.zmag]
                .map(|milligauss| f64::from(milligauss) * MILLIGAUSS_TO_NANOTESLA),
        )),
        MavMessage::SCALED_IMU2(imu) => Some((
            MagScopeSource::ScaledImu2,
            [imu.xmag, imu.ymag, imu.zmag]
                .map(|milligauss| f64::from(milligauss) * MILLIGAUSS_TO_NANOTESLA),
        )),
        MavMessage::SCALED_IMU3(imu) => Some((
            MagScopeSource::ScaledImu3,
            [imu.xmag, imu.ymag, imu.zmag]
                .map(|milligauss| f64::from(milligauss) * MILLIGAUSS_TO_NANOTESLA),
        )),
        _ => None,
    }
}

/// The telemetry tap: every supported IMU message gets source-specific magnetic
/// traces; `HIGHRES_IMU` additionally drives the calibrated `mag_total` alarm.
/// `ESC_STATUS` (291) → the ESC aggregator; `BATTERY_STATUS` (147) → the total
/// battery current trace; `SERVO_OUTPUT_RAW` (36) → the target-FC servo trace.
/// Emits `actuator_telemetry` (aggregated snapshot) at ~10 Hz for the
/// alarm/status readout, and pushes every sample (not a snapshot) to the scope
/// bus.
async fn run_motor_telemetry_tap<R: tauri::Runtime>(
    app: AppHandle<R>,
    handle: maggcs_core::mavlink::ConnectionHandle,
    target_system_id: u8,
    mut events: maggcs_core::mavlink::router::RoutedEvents,
) {
    let target = MotorTestTargetRef {
        connection_id: handle.handle_id(),
        target_system_id,
        target_component_id: TARGET_COMPONENT_ID,
    };
    // Only telemetry from the configured target may gate motor-test safety
    // or feed the target's plots and alarm.
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
                let scope_outputs = if matches!(&env.message, MavMessage::SERVO_OUTPUT_RAW(_)) {
                    scope_outputs_for_target(&state, target)
                } else {
                    Vec::new()
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
                            telemetry.last_heartbeat_at = Some(env.received_at);
                        }
                    }
                    MavMessage::HIGHRES_IMU(_)
                    | MavMessage::SCALED_IMU(_)
                    | MavMessage::SCALED_IMU2(_)
                    | MavMessage::SCALED_IMU3(_) => {
                        if let Some((source, raw_nt)) = scope_mag_frame(&env.message) {
                            let t = now_epoch_s();
                            let totals = scope::mag_totals(raw_nt, telemetry.calib.as_ref());
                            bus.push_all(scope::mag_samples(source, t, totals));
                            // Scaled-IMU sources are plots only. They must never
                            // perturb the established HIGHRES alarm/readout.
                            if source == MagScopeSource::HighresImu {
                                let total = totals.effective();
                                telemetry.last_mag_total_nt = Some(total);
                                if telemetry.alarm.feed(total, t) {
                                    // Transition into Alarm: fire audio + flash.
                                    let _ = app.emit("actuator_alarm", total);
                                }
                            }
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
                        store_esc_status(telemetry, esc, std::time::Instant::now());
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
                        bus.push_all(scope::mapped_feedback_samples(
                            now_epoch_s(),
                            port,
                            &pwm,
                            &scope_outputs,
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

#[cfg(test)]
mod scope_mag_frame_tests {
    use super::scope_mag_frame;
    use maggcs_core::mavlink::MavMessage;
    use maggcs_core::motor_test::scope::MagScopeSource;
    use mavlink::common::{HIGHRES_IMU_DATA, SCALED_IMU2_DATA, SCALED_IMU3_DATA, SCALED_IMU_DATA};

    #[test]
    fn all_imu_mag_messages_keep_their_source_and_wire_units() {
        let frames = [
            (
                MavMessage::HIGHRES_IMU(HIGHRES_IMU_DATA {
                    xmag: 0.1,
                    ymag: 0.2,
                    zmag: 0.3,
                    ..Default::default()
                }),
                MagScopeSource::HighresImu,
                [10_000.0, 20_000.0, 30_000.0],
            ),
            (
                MavMessage::SCALED_IMU(SCALED_IMU_DATA {
                    xmag: 100,
                    ymag: 200,
                    zmag: 300,
                    ..Default::default()
                }),
                MagScopeSource::ScaledImu1,
                [10_000.0, 20_000.0, 30_000.0],
            ),
            (
                MavMessage::SCALED_IMU2(SCALED_IMU2_DATA {
                    xmag: 200,
                    ymag: 300,
                    zmag: 400,
                    ..Default::default()
                }),
                MagScopeSource::ScaledImu2,
                [20_000.0, 30_000.0, 40_000.0],
            ),
            (
                MavMessage::SCALED_IMU3(SCALED_IMU3_DATA {
                    xmag: 300,
                    ymag: 400,
                    zmag: 500,
                    ..Default::default()
                }),
                MagScopeSource::ScaledImu3,
                [30_000.0, 40_000.0, 50_000.0],
            ),
        ];
        for (message, expected_source, expected_values) in frames {
            let (source, values) = scope_mag_frame(&message).expect("supported IMU frame");
            assert_eq!(source, expected_source);
            for (actual, expected) in values.into_iter().zip(expected_values) {
                assert!((actual - expected).abs() < 0.01);
            }
        }
    }
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
        loop {
            tick.tick().await;
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let mut batch = state.motor_test.scope.drain();
            let command_output = state
                .motor_test
                .running
                .lock()
                .as_ref()
                .map(|r| (r.svc.pwm_output(), r.scope_outputs.clone()));
            if let Some((values, scope_outputs)) = command_output {
                let live = values.iter().any(Option::is_some);
                if live || cmd_active {
                    batch.extend(scope::command_samples(
                        now_epoch_s(),
                        &values,
                        &scope_outputs,
                    ));
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

#[cfg(test)]
mod auxiliary_link_lifecycle_tests {
    use super::{
        reusable_session, stop_before_auxiliary_close, validate_target_system_id, PreparedStart,
    };

    #[test]
    fn target_system_id_must_not_be_broadcast() {
        assert_eq!(
            validate_target_system_id(0),
            Err("target system ID must be between 1 and 255".to_string())
        );
        assert_eq!(validate_target_system_id(1), Ok(1));
        assert_eq!(validate_target_system_id(255), Ok(255));
    }

    #[test]
    fn detected_serial_fc_identity_replaces_the_hint_and_keeps_gcs_ids_distinct() {
        for system_id in [42, 250] {
            let endpoint = maggcs_core::mavlink::Endpoint::Serial {
                port: "/dev/ttyACM900123".into(),
                baudrate: 115_200,
            };
            let identity = maggcs_core::mavlink::serial::FcIdentity {
                system_id,
                component_id: 1,
                autopilot: maggcs_core::telemetry::Autopilot::Px4,
                vehicle: maggcs_core::telemetry::VehicleType::Quadrotor,
            };
            let config = super::motor_serial_target_config(
                maggcs_core::mavlink::ConnectionConfig {
                    target_system_id: 1,
                    ..Default::default()
                },
                &identity,
                endpoint.clone(),
            )
            .expect("detected target configuration");
            assert_eq!(config.target_system_id, system_id);
            assert_eq!(config.target_component_id, 1);
            assert_eq!(config.endpoint, endpoint);
            assert_ne!(config.system_id, system_id);
            assert_ne!(config.system_id, 255, "QGC's ID is not claimed");
            assert!(!config.auto_discover_target);
        }
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

    async fn install_idle_scope_session(
        state: &crate::state::AppState,
        handle: &ConnectionHandle,
    ) -> PreparedStart {
        let target = crate::motor_test_state::MotorTestTargetRef {
            connection_id: handle.handle_id(),
            target_system_id: 1,
            target_component_id: 1,
        };
        let limits = crate::motor_test_state::motor_test_limits(1);
        let models = vec![
            super::PwmModel::new(1000, 2000, false, 0.0)
                .expect("model")
                .with_protocol(super::OutputProtocol::Pwm { rate_hz: Some(400) });
            2
        ];
        let outputs: Vec<super::ScopeOutputMeta> = (1..=2)
            .map(|motor| super::ScopeOutputMeta {
                connection_id: handle.handle_id(),
                revision: 1,
                motor,
                channel: maggcs_core::motor_test::mapping::ServoChannel::new(
                    super::ServoPort::Main,
                    motor,
                )
                .expect("channel"),
                unit: maggcs_core::motor_test::protocol::OutputUnits::Microseconds,
            })
            .collect();
        let (service, mut events, output) =
            super::MotorTestService::spawn(handle.clone(), limits.clone());
        service
            .send(super::MotorTestCommand::SetMotorCount { motors: 2 })
            .await
            .expect("motor count");
        service
            .send(super::MotorTestCommand::SetPwmModels {
                models: models.clone(),
            })
            .await
            .expect("models");
        let forwarder =
            tauri::async_runtime::spawn(async move { while events.recv().await.is_some() {} });
        *state.motor_test.running.lock() = Some(crate::motor_test_state::RunningMotorTest {
            operation: None,
            svc: service,
            forwarder,
            output,
            target,
            scope_outputs: outputs.clone(),
            tone_generation: 0,
        });
        PreparedStart {
            handle: handle.clone(),
            target,
            limits,
            pwm_models: models,
            scope_outputs: outputs,
            active_motors: 2,
        }
    }

    #[tokio::test]
    async fn an_idle_session_is_not_reused_after_its_output_revision_changes() {
        let state = crate::state::AppState::default();
        let (handle, _peer) = loopback_link().await;
        let mut prepared = install_idle_scope_session(&state, &handle).await;
        assert!(reusable_session(&state, &prepared)
            .await
            .expect("same context")
            .is_some());
        for output in &mut prepared.scope_outputs {
            output.revision = 2;
        }
        assert!(reusable_session(&state, &prepared)
            .await
            .expect("released old context")
            .is_none());
        assert!(state.motor_test.running.lock().is_none());
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn a_running_session_refuses_changed_scope_configuration() {
        let state = crate::state::AppState::default();
        let (handle, _peer) = loopback_link().await;
        let mut prepared = install_idle_scope_session(&state, &handle).await;
        state
            .motor_test
            .note_state(maggcs_core::motor_test::SessionState::ManualRunning);
        for output in &mut prepared.scope_outputs {
            output.revision = 2;
        }
        let error = reusable_session(&state, &prepared)
            .await
            .err()
            .expect("busy session refused");
        assert!(error.contains("output configuration"));
        {
            let guard = state.motor_test.running.lock();
            let running = guard.as_ref().expect("old session retained");
            assert!(running
                .scope_outputs
                .iter()
                .all(|output| output.revision == 1));
        }
        handle.shutdown().await;
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

    use super::prepare_start;

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
            id: LinkId::next().expect("link ID"),
            connection: handle,
            hub,
            mission,
            command,
            logs: None,
            params: None,
            rtk: None,
        });

        // The primary link is up, but it must NOT satisfy the motor test.
        assert!(
            crate::main_link_service::candidate(&state).is_none(),
            "UDP primary is never offered as a shared serial target"
        );
        assert_eq!(
            prepare_start(&state).map(|_| ()),
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

    use ::mavlink::common::ESC_STATUS_DATA;
    use maggcs_core::logs::capture::CapturePhase;
    use maggcs_core::mavlink::{ConnectionConfig, ConnectionHandle, Endpoint};
    use maggcs_core::motor_test::mag_alarm::{MagAlarm, MagAlarmConfig};
    use maggcs_core::motor_test::mapping::OutputMap;
    use maggcs_core::motor_test::protocol::{OutputProtocol, OutputProtocols};
    use maggcs_core::motor_test::pwm::PwmModel;
    use maggcs_core::motor_test::scope::MAX_ESC;
    use maggcs_core::motor_test::SessionState;
    use maggcs_core::params::ParamService;
    use maggcs_core::signals::derived::EscAggregator;
    use maggcs_core::telemetry::{Autopilot, BaseMode, FlightState, Heartbeat, VehicleType};

    use crate::motor_test_state::{
        AuxiliaryLinkRuntime, MotorTelemetry, MotorTestTarget, MotorTestTargetRef,
        OutputConfigSnapshot, TargetLinkRuntime, TARGET_COMPONENT_ID,
    };
    use crate::state::AppState;

    use super::{
        ensure_idle_session, prepare_start, store_esc_status, telemetry_payload, FunctionRead,
        TARGET_HEARTBEAT_STALE,
    };

    fn mark_capture_recording(state: &AppState) {
        state.motor_test.capture.lock().status.lock().phase = CapturePhase::Recording;
    }

    fn refusal(state: &AppState) -> String {
        prepare_start(state).err().expect("start must be refused")
    }

    /// Models that look like they were read from the FC (the start gate passes).
    fn fc_models() -> Vec<PwmModel> {
        vec![
            PwmModel::new(1100, 1900, false, 0.0)
                .expect("model")
                .with_protocol(OutputProtocol::Pwm { rate_hz: Some(400) });
            4
        ]
    }

    fn checked_map(main: [Option<i64>; 8]) -> OutputMap {
        let main = main.map(|value| match value {
            Some(value) => FunctionRead::Value(value),
            None => FunctionRead::Absent,
        });
        let aux = [FunctionRead::Absent; 8];
        maggcs_core::motor_test::mapping::build_output_map_checked(4, &main, &aux)
    }

    fn fc_map() -> OutputMap {
        let mut main = [None; 8];
        for (index, slot) in main.iter_mut().take(4).enumerate() {
            *slot = Some(maggcs_core::motor_test::MOTOR_FUNCTION_BASE + index as i64);
        }
        checked_map(main)
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
                target_system_id: system_id,
                auto_discover_target: false,
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
                config: std::sync::Arc::new(parking_lot::Mutex::new(Some(OutputConfigSnapshot {
                    revision: 1,
                    target: MotorTestTargetRef {
                        connection_id: handle.handle_id(),
                        target_system_id: system_id,
                        target_component_id: TARGET_COMPONENT_ID,
                    },
                    map: fc_map(),
                    models: fc_models(),
                    protocols: OutputProtocols {
                        main: OutputProtocol::Pwm { rate_hz: Some(400) },
                        aux: OutputProtocol::Pwm { rate_hz: Some(400) },
                    },
                    protocols_known: true,
                    dshot_3d_enable: Some(0),
                    dshot_bidir_en: Some(0),
                }))),
                config_stale: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                param_names: Default::default(),
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

    async fn send_target_heartbeat(handle: &ConnectionHandle, system_id: u8, armed: bool) {
        let Endpoint::UdpListener { addr } = handle.config().endpoint else {
            panic!("loopback target");
        };
        let mut events = handle.subscribe();
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("peer");
        let message =
            maggcs_core::mavlink::MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
                custom_mode: 2 << 16,
                mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
                autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
                base_mode: if armed {
                    ::mavlink::common::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED
                } else {
                    ::mavlink::common::MavModeFlag::empty()
                },
                system_status: ::mavlink::common::MavState::MAV_STATE_STANDBY,
                mavlink_version: 3,
            });
        let mut bytes = Vec::new();
        ::mavlink::write_v2_msg(
            &mut bytes,
            maggcs_core::mavlink::MavHeader {
                system_id,
                component_id: TARGET_COMPONENT_ID,
                sequence: 7,
            },
            &message,
        )
        .expect("wire heartbeat");
        peer.send_to(&bytes, addr).await.expect("send heartbeat");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(maggcs_core::mavlink::ConnectionEvent::Message(env)) = events.recv().await
                {
                    if env.header.system_id == system_id {
                        break;
                    }
                }
            }
        })
        .await
        .expect("heartbeat observed by the connection");
    }

    async fn seed_target_heartbeat(state: &AppState, armed: bool) {
        let (handle, system_id) = {
            let selected = state.motor_test.target.lock();
            let target = &selected.as_ref().expect("target").target;
            (target.handle.clone(), target.target_system_id)
        };
        send_target_heartbeat(&handle, system_id, armed).await;
    }

    fn install_telemetry(state: &AppState, hb: Option<Heartbeat>, age: Option<Duration>) {
        *state.motor_test.telemetry.lock() = Some(MotorTelemetry {
            esc: EscAggregator::new(MAX_ESC),
            alarm: MagAlarm::new(MagAlarmConfig::default()),
            calib: None,
            last_mag_total_nt: None,
            hb,
            fc_outputs: [None, None],
            last_heartbeat_at: age.map(|age| Instant::now() - age),
            esc_seen_at: [None; MAX_ESC],
        });
    }

    #[tokio::test]
    async fn selected_target_supplies_fc_status_and_readiness_without_a_primary_link() {
        let state = AppState::default();
        let handle = install_target(&state, 42).await;
        seed_target_heartbeat(&state, false).await;

        assert!(state.connection().is_none());
        assert!(state.primary_params().is_none());
        assert!(
            state.motor_test.telemetry.lock().is_none(),
            "FC state does not depend on the window observer"
        );
        let status = state.actuator_status();
        assert!(status.connected && status.target_connected);
        assert!(status.fc_alive && status.target_fc_alive);
        assert!(!status.armed);
        assert!(status.autopilot_px4);
        assert_eq!(status.mode_label, "Altitude");
        assert_eq!(status.endpoint, status.target_endpoint);
        assert_eq!(status.target_system_id, Some(42));
        assert_eq!(
            prepare_start(&state)
                .expect("selected FC is ready")
                .handle
                .handle_id(),
            handle.handle_id()
        );

        let target = state.motor_test.target.lock().take().expect("target");
        target.stop().await;
    }

    #[tokio::test]
    async fn fc_status_rejects_unselected_foreign_and_stale_heartbeats() {
        let state = AppState::default();
        install_telemetry(&state, Some(heartbeat(42, true)), Some(Duration::ZERO));
        let unselected = state.actuator_status();
        assert!(!unselected.fc_alive && !unselected.target_fc_alive);
        assert!(!unselected.armed && !unselected.autopilot_px4);

        let handle = install_target(&state, 42).await;
        assert!(
            !state.actuator_status().fc_alive,
            "an observer's synthetic state cannot supply the FC heartbeat"
        );
        seed_target_heartbeat(&state, false).await;
        send_target_heartbeat(&handle, 99, true).await;
        let selected = state.actuator_status();
        assert!(
            selected.fc_alive && !selected.armed,
            "foreign traffic cannot replace the selected serial FC"
        );
        *state.motor_test.telemetry.lock() = None;
        assert!(
            state.actuator_status().fc_alive,
            "resetting the window observer retains FC state"
        );
        tokio::time::sleep(TARGET_HEARTBEAT_STALE + Duration::from_millis(20)).await;
        assert!(
            !state.actuator_status().fc_alive,
            "received_at controls freshness"
        );
        seed_target_heartbeat(&state, false).await;
        assert!(state.actuator_status().fc_alive);

        let target = state.motor_test.target.lock().take().expect("target");
        target.stop().await;
    }

    fn esc_frame(index: u8) -> ESC_STATUS_DATA {
        ESC_STATUS_DATA {
            index,
            rpm: [1000, 2000, 0, 4000],
            voltage: [12.0; 4],
            current: [1.0, 2.0, 3.0, 4.0],
            ..Default::default()
        }
    }

    /// MR-09: fresh, stale and unseen samples are distinct, including true zero.
    #[test]
    fn status_distinguishes_fresh_stale_and_missing_esc_rpm() {
        let state = AppState::default();
        install_telemetry(&state, None, None);
        {
            let mut guard = state.motor_test.telemetry.lock();
            let telemetry = guard.as_mut().expect("telemetry");
            store_esc_status(telemetry, &esc_frame(0), Instant::now());
        }
        let status = state.actuator_status();
        assert_eq!(status.esc_rpm[0].rpm, Some(1000.0));
        assert!(!status.esc_rpm[0].stale);
        assert_eq!(
            status.esc_rpm[2].rpm,
            Some(0.0),
            "reported zero is not missing"
        );
        assert!(!status.esc_rpm[2].stale);
        assert_eq!(status.esc_rpm[4].rpm, None, "bank 2 never reported");
        assert_eq!(status.esc_rpm[4].age_s, None);
        assert!(!status.esc_rpm[4].stale);
        assert_eq!(status.esc_rpm.len(), 8);

        {
            let mut guard = state.motor_test.telemetry.lock();
            store_esc_status(
                guard.as_mut().expect("telemetry"),
                &esc_frame(0),
                Instant::now() - Duration::from_secs(1),
            );
        }
        let stale = state.actuator_status();
        assert_eq!(stale.esc_rpm[0].rpm, Some(1000.0), "last value is kept");
        assert!(stale.esc_rpm[0].stale);
        assert!(stale.esc_rpm[0].age_s.is_some_and(|age| age >= 1.0));
        *state.motor_test.telemetry.lock() = None;
    }

    #[test]
    fn esc_index_four_has_independent_values_and_freshness() {
        let state = AppState::default();
        install_telemetry(&state, None, None);
        let first = esc_frame(0);
        let second = ESC_STATUS_DATA {
            rpm: [5000, 6000, 0, 8000],
            ..esc_frame(4)
        };
        let now = Instant::now();
        {
            let mut guard = state.motor_test.telemetry.lock();
            let telemetry = guard.as_mut().expect("telemetry");
            store_esc_status(telemetry, &first, now - Duration::from_secs(1));
            store_esc_status(telemetry, &second, now);
            let payload = telemetry_payload(telemetry);
            assert_eq!(payload.esc.len(), MAX_ESC * 3);
            assert!(payload
                .esc
                .iter()
                .any(|(id, value)| id == "esc5.rpm" && *value == 5000.0));
            assert!(payload
                .esc
                .iter()
                .any(|(id, value)| id == "esc1.rpm" && *value == 1000.0));
        }
        let status = state.actuator_status();
        for (row, rpm) in status.esc_rpm[..4].iter().zip(first.rpm) {
            assert_eq!(row.rpm, Some(f64::from(rpm)));
            assert!(row.stale);
        }
        for (row, rpm) in status.esc_rpm[4..].iter().zip(second.rpm) {
            assert_eq!(row.rpm, Some(f64::from(rpm)));
            assert!(!row.stale);
        }
        {
            let mut guard = state.motor_test.telemetry.lock();
            let telemetry = guard.as_mut().expect("telemetry");
            store_esc_status(telemetry, &second, now - Duration::from_secs(1));
            store_esc_status(telemetry, &first, Instant::now());
        }
        let inverted = state.actuator_status();
        assert!(inverted.esc_rpm[..4].iter().all(|row| !row.stale));
        assert!(inverted.esc_rpm[4..].iter().all(|row| row.stale));
        assert_eq!(inverted.esc_rpm[4].rpm, Some(5000.0));
    }

    #[test]
    fn the_second_esc_bank_can_report_before_the_first() {
        let state = AppState::default();
        install_telemetry(&state, None, None);
        {
            let mut guard = state.motor_test.telemetry.lock();
            let telemetry = guard.as_mut().expect("telemetry");
            store_esc_status(telemetry, &esc_frame(4), Instant::now());
            let payload = telemetry_payload(telemetry);
            assert_eq!(payload.esc.len(), 12, "only the observed bank is emitted");
            assert!(payload.esc.iter().any(|(id, _)| id == "esc5.rpm"));
            assert!(!payload.esc.iter().any(|(id, _)| id == "esc1.rpm"));
        }
        let status = state.actuator_status();
        assert!(status.esc_rpm[..4]
            .iter()
            .all(|row| row.rpm.is_none() && row.age_s.is_none() && !row.stale));
        for (row, rpm) in status.esc_rpm[4..].iter().zip(esc_frame(4).rpm) {
            assert_eq!(row.rpm, Some(f64::from(rpm)));
            assert!(row.age_s.is_some());
            assert!(!row.stale);
        }
    }

    #[test]
    fn out_of_range_esc_frames_do_not_refresh_existing_samples() {
        let state = AppState::default();
        install_telemetry(&state, None, None);
        {
            let mut guard = state.motor_test.telemetry.lock();
            let telemetry = guard.as_mut().expect("telemetry");
            store_esc_status(
                telemetry,
                &esc_frame(0),
                Instant::now() - Duration::from_secs(1),
            );
            let seen_at = telemetry.esc_seen_at;
            let signals = telemetry.esc.signals();
            for index in [8, u8::MAX] {
                store_esc_status(telemetry, &esc_frame(index), Instant::now());
                assert_eq!(telemetry.esc_seen_at, seen_at);
                assert_eq!(telemetry.esc.signals(), signals);
            }
        }
        let status = state.actuator_status();
        assert!(status.esc_rpm[..4].iter().all(|row| row.stale));
        assert!(status.esc_rpm[4..].iter().all(|row| row.rpm.is_none()));
    }

    #[test]
    fn overlapping_esc_frames_refresh_only_the_reported_slots() {
        let state = AppState::default();
        install_telemetry(&state, None, None);
        {
            let mut guard = state.motor_test.telemetry.lock();
            let telemetry = guard.as_mut().expect("telemetry");
            store_esc_status(
                telemetry,
                &esc_frame(0),
                Instant::now() - Duration::from_secs(1),
            );
            store_esc_status(
                telemetry,
                &ESC_STATUS_DATA {
                    rpm: [10, 20, 30, 40],
                    ..esc_frame(1)
                },
                Instant::now(),
            );
        }
        let status = state.actuator_status();
        assert_eq!(status.esc_rpm[0].rpm, Some(1000.0));
        assert!(status.esc_rpm[0].stale, "ESC1 was not in the newer frame");
        for (row, rpm) in status.esc_rpm[1..5].iter().zip([10, 20, 30, 40]) {
            assert_eq!(row.rpm, Some(f64::from(rpm)));
            assert!(!row.stale);
        }
        assert!(status.esc_rpm[5..].iter().all(|row| row.rpm.is_none()));
    }

    /// Task 3 acceptance: every start is bound to the current target and only
    /// passes with a fresh, matching, disarmed PX4 heartbeat.
    #[tokio::test(flavor = "multi_thread")]
    async fn prepare_start_requires_a_fresh_matching_disarmed_px4() {
        let state = AppState::default();
        assert_eq!(refusal(&state), "no serial PX4 target — connect it first");

        let handle = install_target(&state, 1).await;
        assert_eq!(
            refusal(&state),
            "waiting for the selected serial PX4 heartbeat"
        );
        seed_target_heartbeat(&state, true).await;
        assert_eq!(
            refusal(&state),
            "target is armed — disarm it before starting"
        );

        seed_target_heartbeat(&state, false).await;
        let custom_model = PwmModel::new(1100, 1900, true, 0.5)
            .expect("model")
            .with_disabled(true);
        if let Some(runtime) = state.motor_test.target.lock().as_mut() {
            runtime
                .target
                .config
                .lock()
                .as_mut()
                .expect("config")
                .models = vec![custom_model; 4];
        }
        mark_capture_recording(&state);
        let prepared = prepare_start(&state).expect("ready target");
        assert_eq!(prepared.pwm_models, vec![custom_model; 4]);
        assert_eq!(prepared.target.connection_id, handle.handle_id());
        assert_eq!(prepared.target.target_system_id, 1);
        assert_eq!(prepared.target.target_component_id, 1);
        assert_eq!(prepared.limits.max_value, 1.0);
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

    #[test]
    fn target_heartbeat_validation_requires_matching_disarmed_px4_and_fresh_receive_time() {
        let target = MotorTestTargetRef {
            connection_id: 1,
            target_system_id: 42,
            target_component_id: 1,
        };
        let now = Instant::now();
        assert!(super::validate_target_heartbeat(target, &heartbeat(42, false), now, now).is_ok());
        assert_eq!(
            super::validate_target_heartbeat(target, &heartbeat(42, true), now, now),
            Err("target is armed — disarm it before starting".into())
        );
        assert_eq!(
            super::validate_target_heartbeat(
                target,
                &heartbeat(42, false),
                now - TARGET_HEARTBEAT_STALE - Duration::from_millis(1),
                now
            ),
            Err("target heartbeat is stale — check the serial PX4 link".into())
        );
        let mut wrong_component = heartbeat(42, false);
        wrong_component.component_id = 2;
        for hb in [heartbeat(99, false), wrong_component] {
            assert_eq!(
                super::validate_target_heartbeat(target, &hb, now, now),
                Err("target heartbeat identity changed — reconnect the serial PX4".into())
            );
        }
        let mut other_autopilot = heartbeat(42, false);
        other_autopilot.autopilot = Autopilot::ArduPilot;
        assert_eq!(
            super::validate_target_heartbeat(target, &other_autopilot, now, now),
            Err("target is not a PX4 autopilot".into())
        );
    }

    /// A start uses exactly one published revision; a snapshot read from a
    /// different link or no snapshot at all is refused before telemetry.
    #[tokio::test(flavor = "multi_thread")]
    async fn prepare_start_binds_one_published_configuration_revision() {
        let state = AppState::default();
        install_target(&state, 1).await;
        seed_target_heartbeat(&state, false).await;
        let prepared = prepare_start(&state).expect("start prepared");
        let published = {
            let guard = state.motor_test.target.lock();
            let published = guard
                .as_ref()
                .expect("target")
                .target
                .config
                .lock()
                .clone()
                .expect("config");
            published
        };
        assert_eq!(prepared.pwm_models, published.models);
        assert_eq!(published.revision, 1);
        assert_eq!(
            prepared.scope_outputs,
            super::output_scope_metadata(&published).expect("scope metadata")
        );
        assert_eq!(
            state.actuator_status().scope_outputs,
            prepared.scope_outputs
        );

        {
            let guard = state.motor_test.target.lock();
            guard
                .as_ref()
                .expect("target")
                .target
                .config_stale
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        assert!(refusal(&state).contains("stale"));
        assert!(state.actuator_status().scope_outputs.is_empty());
        assert!(super::scope_outputs_for_target(&state, published.target).is_empty());
        {
            let guard = state.motor_test.target.lock();
            guard
                .as_ref()
                .expect("target")
                .target
                .config_stale
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }

        {
            let guard = state.motor_test.target.lock();
            guard
                .as_ref()
                .expect("target")
                .target
                .config
                .lock()
                .as_mut()
                .expect("config")
                .target
                .connection_id += 1;
        }
        assert!(refusal(&state).contains("different target"));
        assert!(state.actuator_status().scope_outputs.is_empty());
        assert!(super::scope_outputs_for_target(&state, published.target).is_empty());

        {
            let guard = state.motor_test.target.lock();
            *guard.as_ref().expect("target").target.config.lock() = None;
        }
        assert!(refusal(&state).contains("has not been read"));

        let target = state.motor_test.target.lock().take();
        if let Some(target) = target {
            target.stop().await;
        }
        *state.motor_test.telemetry.lock() = None;
    }

    /// Status must not combine a map from one refresh with models from
    /// another: both come from the same snapshot clone.
    #[tokio::test(flavor = "multi_thread")]
    async fn status_reads_map_and_models_from_the_same_snapshot() {
        let state = AppState::default();
        install_target(&state, 1).await;
        let custom = vec![
            PwmModel::new(1200, 1800, true, 0.2)
                .expect("model")
                .with_protocol(OutputProtocol::Pwm { rate_hz: Some(400) });
            4
        ];
        {
            let guard = state.motor_test.target.lock();
            guard
                .as_ref()
                .expect("target")
                .target
                .config
                .lock()
                .as_mut()
                .expect("config")
                .models = custom.clone();
        }
        let status = state.actuator_status();
        assert_eq!(status.output_map, fc_map());
        assert_eq!(status.pwm_models, custom);

        let target = state.motor_test.target.lock().take();
        if let Some(target) = target {
            target.stop().await;
        }
    }

    /// Manual SD capture is optional, even after a failed or existing capture.
    #[tokio::test(flavor = "multi_thread")]
    async fn prepare_start_does_not_require_an_sd_capture() {
        let state = AppState::default();
        install_target(&state, 1).await;
        seed_target_heartbeat(&state, false).await;
        for phase in [
            CapturePhase::Idle,
            CapturePhase::Ready,
            CapturePhase::Recording,
            CapturePhase::ExistingRecording,
            CapturePhase::Failed,
            CapturePhase::Complete,
        ] {
            state.motor_test.capture.lock().status.lock().phase = phase;
            assert!(prepare_start(&state).is_ok(), "{phase:?}");
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
        seed_target_heartbeat(&state, false).await;
        {
            let runtime = state.motor_test.target.lock();
            runtime
                .as_ref()
                .expect("target")
                .target
                .config
                .lock()
                .as_mut()
                .expect("config")
                .models = vec![PwmModel::default(); 4];
        }
        let error = refusal(&state);
        assert!(
            (1..=4).all(|motor| error.contains(&format!("M{motor}: output range not read")))
                && error.contains("refresh the mapping"),
            "the error lists the unread motors: {error}"
        );
        {
            let runtime = state.motor_test.target.lock();
            runtime
                .as_ref()
                .expect("target")
                .target
                .config
                .lock()
                .as_mut()
                .expect("config")
                .models = fc_models();
        }
        assert!(prepare_start(&state).is_ok(), "FC-read ranges pass again");
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
        seed_target_heartbeat(&state, false).await;
        mark_capture_recording(&state);
        let first = prepare_start(&state).expect("first target");

        let second_handle = install_target(&state, 1).await;
        assert_eq!(
            refusal(&state),
            "waiting for the selected serial PX4 heartbeat",
            "the old connection cannot authorize its replacement"
        );
        seed_target_heartbeat(&state, false).await;
        let second = prepare_start(&state).expect("replacement target");
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

    use maggcs_core::motor_test::mapping::{FunctionRead, OutputMap};
    use maggcs_core::motor_test::protocol::{OutputProtocol, OutputProtocols};
    use maggcs_core::motor_test::pwm::PwmModel;

    use super::OutputConfigRead;
    use crate::state::AppState;

    fn checked_map(main: [Option<i64>; 8]) -> OutputMap {
        let main = main.map(|value| match value {
            Some(value) => FunctionRead::Value(value),
            None => FunctionRead::Absent,
        });
        let aux = [FunctionRead::Absent; 8];
        maggcs_core::motor_test::mapping::build_output_map_checked(4, &main, &aux)
    }

    /// Regression (user report 2026-10-22: the motor-test window froze on
    /// open): `actuator_status` took the non-reentrant target mutex twice in
    /// one expression, so the second `.lock()` never returned. The status
    /// poll must complete promptly, even with no target connected.
    #[test]
    fn pwm_model_from_raw_reads_endpoints_and_flags() {
        let model = super::pwm_model_from_raw(
            Some(1100),
            Some(1900),
            true,
            true,
            0.5,
            true,
            OutputProtocol::Pwm { rate_hz: Some(400) },
        );
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
        let expected =
            PwmModel::default().with_protocol(OutputProtocol::Pwm { rate_hz: Some(400) });
        let model = super::pwm_model_from_raw(
            Some(2000),
            Some(1000),
            false,
            false,
            0.0,
            true,
            OutputProtocol::Pwm { rate_hz: Some(400) },
        );
        assert_eq!(model, expected);
        let model = super::pwm_model_from_raw(
            None,
            Some(1900),
            false,
            false,
            0.0,
            true,
            OutputProtocol::Pwm { rate_hz: Some(400) },
        );
        assert_eq!(model, expected);
    }

    #[test]
    fn an_unmapped_motor_gets_the_explicit_fallback_model() {
        let model = super::pwm_model_from_raw(
            None,
            None,
            false,
            false,
            0.0,
            false,
            OutputProtocol::Unknown,
        );
        assert_eq!(model, PwmModel::default());
        assert_eq!(
            model.range_source,
            maggcs_core::motor_test::pwm::PwmRangeSource::Fallback
        );
        assert_eq!(model.min_us, 1100);
        assert_eq!(model.max_us, 1900);
    }

    /// A failed read must keep the previous snapshot; a mapping the FC really
    /// reports as gapped is a new (partial) configuration, not a transient
    /// failure.
    #[test]
    fn a_read_failure_is_distinct_from_a_reported_mapping_gap() {
        let model = PwmModel::new(1100, 1900, false, 0.0)
            .expect("model")
            .with_protocol(OutputProtocol::Pwm { rate_hz: Some(400) });
        let gap_map = checked_map([Some(101), None, Some(103), None, None, None, None, None]);
        let gap = OutputConfigRead {
            map: gap_map,
            models: vec![model; 4],
            protocols: OutputProtocols::default(),
            protocols_known: true,
            dshot_3d_enable: Some(0),
            dshot_bidir_en: Some(0),
        };
        assert!(!gap.read_failure(), "a gap is reported by the FC");
        assert!(!gap.is_startable());

        let mut main = [FunctionRead::Absent; 8];
        main[0] = FunctionRead::Value(101);
        main[1] = FunctionRead::Unreadable;
        let unreadable = OutputConfigRead {
            map: maggcs_core::motor_test::mapping::build_output_map_checked(
                4,
                &main,
                &[FunctionRead::Absent; 8],
            ),
            models: vec![model; 4],
            protocols: OutputProtocols::default(),
            protocols_known: true,
            dshot_3d_enable: Some(0),
            dshot_bidir_en: Some(0),
        };
        assert!(unreadable.read_failure());
        assert!(!unreadable.is_startable());

        let mut protocols = unreadable;
        protocols.map.state = maggcs_core::motor_test::mapping::OutputMapState::Complete;
        protocols.protocols_known = false;
        assert!(protocols.read_failure());
    }

    fn list_motor_function(index: usize) -> i64 {
        maggcs_core::motor_test::MOTOR_FUNCTION_BASE + index as i64
    }

    fn map_with_motors(count: i64) -> maggcs_core::motor_test::mapping::OutputMap {
        let mut main = [None; 8];
        for (index, slot) in main
            .iter_mut()
            .take(usize::try_from(count).expect("motor count"))
            .enumerate()
        {
            *slot = Some(list_motor_function(index));
        }
        checked_map(main)
    }

    #[test]
    fn the_start_gate_checks_only_the_active_motor_prefix() {
        assert!(super::refuse_unread_pwm_ranges(
            &maggcs_core::motor_test::mapping::OutputMap::unknown(4),
            &[],
            false,
            Some(0)
        )
        .is_err());
        let model = PwmModel::new(1100, 1900, false, 0.0)
            .expect("model")
            .with_protocol(OutputProtocol::Pwm { rate_hz: Some(400) });
        for count in 1_i64..=4 {
            let map = map_with_motors(count);
            let models = vec![model; usize::try_from(count).expect("count")];
            let result = super::refuse_unread_pwm_ranges(&map, &models, true, Some(0));
            assert!(result.is_ok(), "{count} motors should start: {result:?}");
        }
        assert!(
            super::refuse_unread_pwm_ranges(&map_with_motors(3), &[model; 2], true, Some(0))
                .is_err()
        );
    }

    #[test]
    fn a_mapping_gap_blocks_the_start_gate() {
        let mut main = [None; 8];
        main[0] = Some(1);
        main[2] = Some(3);
        let map = checked_map(main);
        let model = PwmModel::new(1100, 1900, false, 0.0)
            .expect("model")
            .with_protocol(OutputProtocol::Pwm { rate_hz: Some(400) });
        assert!(super::refuse_unread_pwm_ranges(&map, &[model; 4], true, Some(0)).is_err());
    }

    #[test]
    fn dshot_requires_a_known_non_3d_configuration() {
        let dshot = PwmModel::dshot(
            0.055,
            false,
            0.0,
            OutputProtocol::DShot {
                rate: maggcs_core::motor_test::DShotRate::R600,
            },
        )
        .expect("dshot model");
        let map = map_with_motors(1);
        assert!(super::refuse_unread_pwm_ranges(&map, &[dshot], true, Some(1)).is_err());
        assert!(super::refuse_unread_pwm_ranges(&map, &[dshot], true, None).is_err());
        let result = super::refuse_unread_pwm_ranges(&map, &[dshot], true, Some(0));
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn an_incomplete_parameter_inventory_blocks_the_start_gate() {
        let map = map_with_motors(1);
        let model = PwmModel::new(1100, 1900, false, 0.0)
            .expect("model")
            .with_protocol(OutputProtocol::Pwm { rate_hz: Some(400) });
        assert!(super::refuse_unread_pwm_ranges(&map, &[model], false, Some(0)).is_err());
    }

    #[test]
    fn dshot_models_use_mixer_units_and_block_unread_dshot_min() {
        let protocol = OutputProtocol::DShot {
            rate: maggcs_core::motor_test::DShotRate::R600,
        };
        let model = super::dshot_model_from_raw(Some(0.055), false, false, 0.0, protocol);
        assert_eq!(model.min_us, 110);
        assert_eq!(model.max_us, maggcs_core::motor_test::DSHOT_MAX_THROTTLE);
        assert_eq!(
            model.range_source,
            maggcs_core::motor_test::pwm::PwmRangeSource::Fc
        );
        assert!(model.protocol.is_dshot());

        let unread = super::dshot_model_from_raw(None, false, false, 0.0, protocol);
        assert_eq!(
            unread.range_source,
            maggcs_core::motor_test::pwm::PwmRangeSource::Fallback
        );
        assert_eq!(unread.max_us, maggcs_core::motor_test::DSHOT_MAX_THROTTLE);
        assert!(unread.protocol.is_dshot());
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
mod main_sitl_feed_tests {
    //! The SITL feed can borrow the primary link (operator report: a second
    //! `udpout` to PX4's listening port receives nothing — PX4 v1.17 pins one
    //! GCS partner per MAVLink instance).

    use std::time::Duration;

    use maggcs_core::commands::CommandService;
    use maggcs_core::mavlink::connection::LinkStatus;
    use maggcs_core::mavlink::{
        ConnectionConfig, ConnectionHandle, Endpoint, LinkState, MavHeader, MavMessage,
    };
    use maggcs_core::mission::service::{MissionIds, MissionService};
    use maggcs_core::motor_test::mapping::ServoPort;
    use maggcs_core::telemetry::hub::{TelemetryHub, DEFAULT_PUSH_HZ};
    use maggcs_core::telemetry::VehicleId;

    use crate::motor_test_state::{SitlSourceConfig, SitlSourceOrigin};
    use crate::state::{ActiveLink, AppState, LinkId};

    use super::{forward_slot, main_sitl_candidate, sitl_feed_runtime};

    async fn send_heartbeat(peer: &tokio::net::UdpSocket, port: u16) {
        let message = MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
            autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: ::mavlink::common::MavModeFlag::empty(),
            system_status: ::mavlink::common::MavState::MAV_STATE_STANDBY,
            mavlink_version: 3,
        });
        let mut bytes = Vec::new();
        ::mavlink::write_v2_msg(
            &mut bytes,
            MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            &message,
        )
        .expect("wire heartbeat");
        peer.send_to(&bytes, ("127.0.0.1", port))
            .await
            .expect("send heartbeat");
    }

    async fn send_servo(peer: &tokio::net::UdpSocket, port: u16, bank: u8, pwm: [u16; 4]) {
        let message = MavMessage::SERVO_OUTPUT_RAW(::mavlink::common::SERVO_OUTPUT_RAW_DATA {
            time_usec: 0,
            servo1_raw: pwm[0],
            servo2_raw: pwm[1],
            servo3_raw: pwm[2],
            servo4_raw: pwm[3],
            port: bank,
            ..Default::default()
        });
        let mut bytes = Vec::new();
        ::mavlink::write_v2_msg(
            &mut bytes,
            MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 1,
            },
            &message,
        )
        .expect("wire servo output");
        peer.send_to(&bytes, ("127.0.0.1", port))
            .await
            .expect("send servo output");
    }

    async fn wait_for(timeout: Duration, mut check: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + timeout;
        while !check() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "condition not reached within {timeout:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A loopback primary connection plus a fake PX4 that teaches it its
    /// identity. The returned socket is the fake FC.
    async fn install_primary(state: &AppState) -> (ConnectionHandle, tokio::net::UdpSocket, u16) {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").expect("reserve UDP port");
        let port = probe.local_addr().expect("probe address").port();
        drop(probe);
        let endpoint = format!("udpin:127.0.0.1:{port}");
        let config = ConnectionConfig {
            endpoint: Endpoint::try_from(endpoint.as_str()).expect("endpoint"),
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
        let hub = TelemetryHub::spawn(events, DEFAULT_PUSH_HZ, VehicleId::new(1, 1));
        let (mission, _) = MissionService::spawn(
            handle.clone(),
            MissionIds {
                self_system: 250,
                self_component: 250,
                target_system: 1,
                target_component: 1,
            },
        );
        let (command, _) = CommandService::spawn(handle.clone(), 1, 1);
        state.set_primary_link(ActiveLink {
            id: LinkId::next().expect("link ID"),
            connection: handle.clone(),
            hub,
            mission,
            command,
            logs: None,
            params: None,
            rtk: None,
        });

        let fc = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("fake FC socket");
        send_heartbeat(&fc, port).await;
        wait_for(Duration::from_secs(2), || {
            handle.target_identity().is_some()
        })
        .await;
        state.publish_link(LinkStatus::alive(endpoint));
        (handle, fc, port)
    }

    fn attach_main_feed(state: &AppState, handle: &ConnectionHandle) {
        let offer = main_sitl_candidate(state).expect("main-link offer");
        let runtime = sitl_feed_runtime(
            state,
            handle.clone(),
            SitlSourceConfig {
                endpoint: offer.endpoint,
                system_id: offer.system_id,
                component_id: offer.component_id,
                port: ServoPort::Main,
            },
            SitlSourceOrigin::MainLink,
        )
        .expect("feed runtime");
        *state.motor_test.sitl.lock() = Some(runtime);
    }

    #[test]
    fn no_offer_without_a_primary_link() {
        assert!(main_sitl_candidate(&AppState::default()).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn main_link_feed_updates_the_forward_slot_and_keeps_the_primary_open() {
        let state = AppState::default();
        let (handle, fc, port) = install_primary(&state).await;

        let offer = main_sitl_candidate(&state).expect("main-link offer");
        assert_eq!(offer.system_id, 1);
        assert_eq!(offer.component_id, 1);
        assert_eq!(offer.transport_generation, handle.generation());

        attach_main_feed(&state, &handle);
        send_servo(&fc, port, 0, [1100, 1200, 1300, 1400]).await;
        wait_for(Duration::from_secs(2), || {
            state.motor_test.sitl.lock().as_ref().is_some_and(|link| {
                link.slot
                    .lock()
                    .is_ok_and(|slot| slot.last_update_s.is_some())
            })
        })
        .await;

        let status = state.actuator_status();
        assert!(status.sitl_connected);
        assert_eq!(status.sitl_origin, Some(SitlSourceOrigin::MainLink));
        assert_eq!(status.input_pwm, vec![1100, 1200, 1300, 1400]);
        assert!(status.input_values.iter().all(|value| *value > 0.0));
        assert!(status.sitl_feed_age_s.is_some());
        assert!(status.main_sitl_source.is_some());
        assert!(forward_slot(&state).is_ok());

        // Detaching the borrowed feed only stops the tap: the main transport
        // (and its main-window subscribers) stay connected.
        let runtime = state.motor_test.sitl.lock().take().expect("feed");
        runtime.stop().await;
        assert_eq!(handle.state().await, LinkState::Connected);
        assert!(!state.actuator_status().sitl_connected);

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_primary_reconnect_expires_the_borrowed_feed() {
        let state = AppState::default();
        let (handle, fc, port) = install_primary(&state).await;
        attach_main_feed(&state, &handle);
        send_servo(&fc, port, 0, [1200, 1300, 1400, 1500]).await;
        wait_for(Duration::from_secs(2), || {
            state.actuator_status().sitl_feed_age_s.is_some()
        })
        .await;

        let before = handle.generation();
        handle.reconnect().await.expect("reconnect primary");
        wait_for(Duration::from_secs(2), || handle.generation() > before).await;
        assert!(
            !state.actuator_status().sitl_connected,
            "borrowed generation expired"
        );
        send_heartbeat(&fc, port).await;
        wait_for(Duration::from_secs(2), || {
            handle.target_identity().is_some()
        })
        .await;
        state.publish_link(LinkStatus::alive(
            handle.config().endpoint.to_address_string(),
        ));

        let status = state.actuator_status();
        assert!(!status.sitl_connected, "borrowed generation expired");
        assert!(status.input_values.is_empty());
        assert_eq!(
            forward_slot(&state).map(|_| ()),
            Err("SITL source link changed; reconnect it".to_string())
        );
        let offer = main_sitl_candidate(&state).expect("new main-link offer");
        assert_eq!(offer.transport_generation, handle.generation());

        let stale = state.motor_test.sitl.lock().take().expect("stale feed");
        stale.stop().await;
        handle.shutdown().await;
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
    //! demand stays inside `motor_test_limits()`; every run ends with a
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

    use crate::motor_test_state::motor_test_limits;

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
        let protocols = super::read_output_protocols(params).await;
        println!("output protocols: {protocols:?}");
        let models = super::read_pwm_models(params, &map, protocols).await;
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
        let dshot_3d = super::read_int_param(&params, "DSHOT_3D_ENABLE").await;
        refuse_unread_pwm_ranges(&map, &models, true, dshot_3d)
            .expect("every launched motor needs an FC-read range");

        let limits = motor_test_limits(BENCH_SYSTEM_ID);
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
        let dshot_3d = super::read_int_param(&params, "DSHOT_3D_ENABLE").await;
        refuse_unread_pwm_ranges(&map, &models, true, dshot_3d)
            .expect("every launched motor needs an FC-read range");
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

        let limits = motor_test_limits(BENCH_SYSTEM_ID);
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
        // The requested raised cosine is capped by the fixed bench envelope.
        let peak_demand = (2.0 * BENCH_AMPLITUDE).min(motor_test_limits(BENCH_SYSTEM_ID).max_value);
        for motor in [1u8, 3u8] {
            let ch = usize::from(map.channel_for(motor).expect("mapped").channel - 1);
            let model = analysis_models[usize::from(motor - 1)];
            let [lo, hi] = spans[ch];
            let range_hi = f32::from(model.min_us) + peak_demand * model.span_us();
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
