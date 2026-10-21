//! Tauri commands exposed to the React UI.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ::mavlink::common::MavCmd;
use maggcs_core::devices::{DeviceDatabase, DeviceManager, SerialDeviceInfo};
use maggcs_core::lifecycle::{ShutdownError, StopReason, TeardownGuard};
use maggcs_core::mavlink::connection::{
    spawn_connection, ConnectionEvent, ConnectionHandle, LinkStatus,
};
use maggcs_core::mavlink::message::MavMessage;
use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
use maggcs_core::mission::service::MissionIds;
use maggcs_core::mission::types::MissionItem;
use maggcs_core::safety::{
    arm_gate_with_ack, evaluate, evaluate_force_arm, ArmGate, Checklist, ChecklistItemId,
    ForceArmOutcome, ForceArmRequest, PlanState, PreflightRefusal, RefusalKind, SafetyConfig,
};
use maggcs_core::survey::{CloverleafPattern, PatternPlan, SurveyPattern};
use maggcs_core::telemetry::hub::{TelemetryHub, DEFAULT_PUSH_HZ};
use maggcs_core::telemetry::{TelemetrySnapshot, VehicleId};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::command_service;
use crate::lifecycle_service;
use crate::mission_service;
use crate::state::{ActiveLink, AppState, LinkId};
use crate::telemetry_pump;

/// Response of the `connect` command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ConnectResponse {
    pub ok: bool,
    pub endpoint: String,
    /// Flight controller system id the link settled on (auto-discovered or
    /// the configured expectation).
    pub target_system_id: u8,
    /// Flight controller component id the link settled on.
    pub target_component_id: u8,
    /// True when the ids above were learned from the FC's own heartbeat
    /// rather than taken from configuration.
    pub target_discovered: bool,
}

/// Open a MAVLink connection and start the telemetry hub.
///
/// Idempotent: any previous connection/hub is shut down first.
#[tauri::command]
pub async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    endpoint: String,
    heartbeat_timeout_ms: Option<u64>,
) -> Result<ConnectResponse, String> {
    // Serialize connect/disconnect so two concurrent calls cannot race the
    // link swap (issues.md #21).
    let _guard = state.ops().lock().await;

    let mut config = ConnectionConfig {
        endpoint: Endpoint::try_from(endpoint.as_str()).map_err(|e| e.to_string())?,
        ..Default::default()
    };
    if let Some(ms) = heartbeat_timeout_ms {
        config.heartbeat_timeout = Duration::from_millis(ms);
    }

    let (handle, events_rx, first_result) = spawn_connection(config.clone())
        .await
        .map_err(|e| e.to_string())?;

    // Wait for the first connect/bind attempt so failures surface here instead
    // of "ok: true" followed by a background error (issue #4).
    let outcome = match tokio::time::timeout(FIRST_CONNECT_TIMEOUT, first_result).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(failure))) => {
            handle.shutdown().await;
            Err(failure.message.to_string())
        }
        Ok(Err(_)) => {
            // Worker exited without reporting (shutdown raced the connect).
            handle.shutdown().await;
            Err("connection worker stopped unexpectedly".to_string())
        }
        Err(_) => {
            handle.shutdown().await;
            Err(format!(
                "timed out after {} s waiting for the link to come up",
                FIRST_CONNECT_TIMEOUT.as_secs()
            ))
        }
    };
    outcome?;

    // The FC ids are settled either by auto-discovery (first autopilot
    // heartbeat on the link, e.g. a non-default PX4 sysid) or by the
    // configured expectation when the link stays quiet.
    let (target_system_id, target_component_id, target_discovered) = if config.auto_discover_target
    {
        resolve_target(&handle, &config).await
    } else {
        (config.target_system_id, config.target_component_id, false)
    };

    // The hub consumes the initial event stream, so the `Connected` emitted by
    // the worker cannot be missed (issue #2).
    let target = VehicleId::new(target_system_id, target_component_id);
    let hub = TelemetryHub::spawn(events_rx, DEFAULT_PUSH_HZ, target);
    let mission = mission_service::spawn(
        app.clone(),
        handle.clone(),
        MissionIds {
            self_system: config.system_id,
            self_component: config.component_id,
            target_system: target_system_id,
            target_component: target_component_id,
        },
    );
    let command = command_service::spawn(
        app.clone(),
        handle.clone(),
        target_system_id,
        target_component_id,
    );
    tauri::async_runtime::spawn(telemetry_pump::run(app.clone(), hub.clone()));

    // Swap in the new link atomically and tear down the previous one only
    // after the new link is up (a failed reconnect no longer kills a working
    // link).
    let log_connection = handle.clone();
    let previous = state.set_primary_link(ActiveLink {
        id: LinkId::next(),
        connection: handle,
        hub,
        mission,
        command,
        logs: None,
        params: None,
        rtk: None,
    });
    if let Some(mut old) = previous {
        if let Some(rtk) = old.rtk.take() {
            let _ = rtk
                .stop(maggcs_core::lifecycle::DEFAULT_STAGE_TIMEOUT)
                .await;
        }
        if let Some(params) = old.params.take() {
            let _ = params
                .stop(maggcs_core::lifecycle::DEFAULT_STAGE_TIMEOUT)
                .await;
        }
        if let Some(logs) = old.logs.take() {
            let _ = logs
                .stop(maggcs_core::lifecycle::DEFAULT_STAGE_TIMEOUT)
                .await;
        }
        old.hub.shutdown();
        old.connection.shutdown().await;
    }
    let logs = state.logs.spawn(app.clone(), log_connection);
    state.set_primary_logs(logs);
    let params_connection = state.connection();
    if let Some(handle) = params_connection {
        let params = state.params.spawn(&app, handle.clone());
        state.set_primary_params(params);
        let rtk =
            crate::rtk_service::RtkRuntime::spawn(Arc::clone(&state.rtk), app.clone(), handle);
        state.set_primary_rtk(rtk);
    }
    // Supplementary telemetry (L6): ask the FC for explicit stream rates so
    // landed state, EKF health, vibration and link quality reach the preflight
    // checklist and the link alerts instead of relying on PX4 defaults.
    if let Some(h) = state.connection() {
        request_supplementary_streams(&h, target_system_id, target_component_id).await;
    }
    // A (re)connected link re-attaches an already-open inspector: otherwise a
    // link swap would leave the inspector window open but dry (plan P0-6).
    // `attach_inspector` stops the old tap (draining the old stream) and starts
    // a fresh one for this link.
    if state.inspector().subs.is_active() {
        if let Some(h) = state.connection() {
            state.attach_inspector(h);
        }
    }
    // The motor-test window's mag/ESC/servo tap rides its **own serial target
    // link** (`actuator_connect_target`), not the main GCS link: the scope's
    // "FC" curves and the mag readout must show the hardware under test, and
    // never silently mirror whatever the main link happens to be connected to
    // (a SITL sim or another FC). START gating still falls back to the main
    // link heartbeat in `actuator_status`.
    // Walk-instrument runner (W1) rides the same MAVLink link/sensors as the
    // drone; re-attach only while a window has registered its channel.
    if state.instruments().has_window() {
        if let Some(h) = state.connection() {
            state.instruments().attach(h);
        }
    }
    // Flight recorder (A4): a fresh session per link, so a swap closes the old
    // tlog/event log and opens a new one. Errors are non-fatal (recording is
    // best effort); they surface as a warning alert.
    if let Some(h) = state.connection() {
        if let Err(e) = state.recorder().start(&app, &h, state.alerts()) {
            state.alerts().raise(maggcs_core::alerts::AlertInput::new(
                maggcs_core::alerts::AlertSource::System,
                maggcs_core::alerts::Severity::Warn,
                format!("flight recorder unavailable: {e}"),
            ));
        }
    }

    Ok(ConnectResponse {
        ok: true,
        endpoint: config.endpoint.to_address_string(),
        target_system_id,
        target_component_id,
        target_discovered,
    })
}

/// How long `connect` waits for the first bind/connect attempt to resolve.
const FIRST_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long `connect` waits for the FC's first heartbeat (auto-discovery)
/// before falling back to the configured target ids.
const TARGET_DISCOVER_TIMEOUT: Duration = Duration::from_secs(5);

/// Resolve the FC's system/component ids by watching for the first autopilot
/// heartbeat on a freshly connected link.
///
/// Returns the discovered ids and `true` when they came from the FC itself.
/// Falls back to the configured expectation on timeout or when the worker
/// already reports a heartbeat from the expected node.
async fn resolve_target(handle: &ConnectionHandle, config: &ConnectionConfig) -> (u8, u8, bool) {
    let mut events = handle.subscribe();
    let outcome = tokio::time::timeout(TARGET_DISCOVER_TIMEOUT, async {
        loop {
            match events.recv().await {
                Ok(ConnectionEvent::TargetDiscovered {
                    system_id,
                    component_id,
                }) => {
                    return (system_id, component_id);
                }
                Ok(ConnectionEvent::Message(env)) => {
                    // The configured target is already alive: keep it as-is.
                    if env.header.system_id == config.target_system_id
                        && env.header.component_id == config.target_component_id
                        && matches!(env.message, MavMessage::HEARTBEAT(_))
                    {
                        return (config.target_system_id, config.target_component_id);
                    }
                }
                Err(_) => return (config.target_system_id, config.target_component_id),
                _ => {}
            }
        }
    })
    .await;
    match outcome {
        Ok((sys, comp)) => {
            let discovered = sys != config.target_system_id || comp != config.target_component_id;
            (sys, comp, discovered)
        }
        Err(_) => (config.target_system_id, config.target_component_id, false),
    }
}

/// Ask the FC to stream the supplementary telemetry at useful rates (L6).
///
/// PX4 omits or slows several of these (`EXTENDED_SYS_STATE`, `VIBRATION`,
/// `RADIO_STATUS`) by default, so the preflight checklist and the link-health
/// alerts would otherwise read stale or absent data. Fire-and-forget on the
/// link: `MAV_CMD_SET_MESSAGE_INTERVAL` is accepted while disarmed, and a
/// message the FC cannot stream is simply never sent. Stream rates are
/// volatile — a reboot reverts them — so nothing persistent is written.
async fn request_supplementary_streams(
    handle: &ConnectionHandle,
    target_system: u8,
    target_component: u8,
) {
    for req in maggcs_core::telemetry::SUPPLEMENTARY_STREAM_RATES {
        let message = MavMessage::COMMAND_LONG(::mavlink::common::COMMAND_LONG_DATA {
            param1: req.message_id as f32,
            param2: req.interval_us as f32,
            param3: 0.0,
            param4: 0.0,
            param5: 0.0,
            param6: 0.0,
            param7: 0.0,
            command: MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL,
            target_system,
            target_component,
            confirmation: 0,
        });
        let _ = handle.send(message).await;
    }
}

/// Stop the active connection and hub.
///
/// Runs the link-scoped half of the teardown supervisor (A1): motor test →
/// RTK → logs → params → mission/command → link, in that order and each
/// inside its own budget. The app-scoped terrain server is left running so
/// the 3D view survives a reconnect.
///
/// Refused while the vehicle is armed or airborne (L4): the operator must
/// land and disarm, then retry. The main window also stays open on refusal.
/// A refusal raises a `warn` alert even if the caller ignores the error.
#[tauri::command]
pub async fn disconnect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.ops().lock().await;
    lifecycle_service::run(&app, StopReason::Disconnect)
        .await
        .map_err(|err| refusal_message("Disconnect", &err))?;
    crate::actuator_service::stop_auxiliary_links(&state).await;
    // Inspector: stop the live tap and clear the observed message table so
    // the signal tree shows only what the *next* link produces (real FC → SITL
    // must not leave the old FC's messages listed). Trace configs stay until
    // the window closes (ADR-016); only observed fields and counters drop.
    state.inspector_reset_source();
    // Walk-instrument runner: stop it with the link (the window may stay open).
    state.instruments().detach();
    state.publish_link(LinkStatus::disconnected());
    let _ = app.emit("link", LinkStatus::disconnected());
    Ok(())
}

/// One operator-facing sentence for a refused teardown: the reason plus the
/// way out (L4's guidance, shown before the typed confirmation exists).
fn refusal_message(action: &str, err: &ShutdownError) -> String {
    match err {
        ShutdownError::Refused { reason } => format!(
            "{action} refused: {reason}. Land and disarm first, then retry. The window remains open."
        ),
        other => format!("{action} failed: {other}"),
    }
}

/// Ask what a teardown would do *right now*, before the operator commits (L4).
///
/// Read-only: it answers from the same policy the supervisor enforces, so the
/// dialog can show "refused: the vehicle is armed" or ask for the typed
/// keyword while the command that follows re-checks it anyway.
///
/// `at_stake` is the UI's own state — local work that would be lost (an
/// unuploaded mission); core decides whether that warrants a keyword.
#[tauri::command]
pub async fn teardown_guard(
    state: State<'_, AppState>,
    reason: StopReason,
    at_stake: bool,
) -> Result<TeardownGuard, String> {
    Ok(TeardownGuard::of(
        lifecycle_service::policy(),
        reason,
        lifecycle_service::vehicle_state(&state),
        at_stake,
    ))
}

/// Latest known link status (fallback when events were missed).
#[tauri::command]
pub async fn link_status(state: State<'_, AppState>) -> Result<Option<LinkStatus>, String> {
    Ok(state.link_status())
}

/// Take a native window-close refusal that may have arrived before the main
/// webview installed its event listener.
#[tauri::command]
pub async fn take_teardown_refusal(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.take_close_refusal())
}

/// Stop the link cleanly and exit the process.
///
/// Unlike `disconnect`, this quits MagGCS: useful when the window chrome is
/// not reachable (field tablet / kiosk) so the operator needs an in-app
/// shutdown control (handoff item 9).
#[tauri::command]
pub async fn shutdown_app(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.ops().lock().await;
    // The full teardown, every stage including the terrain server (A1).
    // Refused while armed or airborne (L4), including the window-close path.
    lifecycle_service::run(&app, StopReason::ShutdownApp)
        .await
        .map_err(|err| refusal_message("Shut down", &err))?;
    crate::actuator_service::stop_auxiliary_links(&state).await;
    let _ = app.emit("link", LinkStatus::disconnected());
    app.exit(0);
    Ok(())
}

/// Current telemetry snapshot (useful after a webview reload).
#[tauri::command]
pub async fn get_snapshot(state: State<'_, AppState>) -> Result<Option<TelemetrySnapshot>, String> {
    Ok(state.hub().map(|h| h.snapshot()))
}

/// Enumerate attached serial/USB devices (plan §9).
#[tauri::command]
pub fn enumerate_devices() -> Result<Vec<SerialDeviceInfo>, String> {
    let manager = DeviceManager::new(DeviceDatabase::builtin());
    manager.enumerate().map_err(|e| e.to_string())
}

/// One Tools ▾ entry's coordination state (plan §7 U11): whether the window
/// is open (the menu shows a dot) and whether its work is in flight (the
/// motor-test row shows "sending").
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct ToolStatus {
    /// `inspector` | `actuator` | `params` — matches the open commands.
    pub id: String,
    pub open: bool,
    /// The tool's work is in flight (motor test is sending ActuatorTest
    /// frames); the menu badges it so the operator sees it without switching.
    pub busy: bool,
}

/// The Tools ▾ coordination snapshot (U11). Clicking a menu entry already
/// focuses the existing window (the open commands `show` + `set_focus`),
/// so this only feeds the badges.
#[tauri::command]
pub fn tools_status(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Vec<ToolStatus> {
    use tauri::Manager;
    let open = |label: &str| app.get_webview_window(label).is_some();
    let motor_sending = state.motor_test.session().is_some_and(|s| s.is_sending());
    vec![
        ToolStatus {
            id: "inspector".into(),
            open: open("inspector"),
            busy: false,
        },
        ToolStatus {
            id: "actuator".into(),
            open: open("actuator"),
            busy: motor_sending,
        },
        ToolStatus {
            id: "params".into(),
            open: open("params"),
            busy: false,
        },
    ]
}

/// The startup card's clock row (L8): epoch milliseconds plus the IANA
/// timezone name when the OS exposes one. std has no portable timezone API,
/// so Linux reads `/etc/timezone` (or the `/etc/localtime` symlink) and
/// other platforms report `None` — the row then says "unknown", never a
/// guessed name.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct ClockInfo {
    /// Over the IPC a JSON number, not a bigint (see `PlannedWaypoint` note).
    #[ts(type = "number")]
    pub epoch_ms: u64,
    pub timezone: Option<String>,
}

#[tauri::command]
pub fn clock_info() -> ClockInfo {
    ClockInfo {
        epoch_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        timezone: read_timezone(),
    }
}

/// Best-effort IANA timezone name; `None` beats a wrong name.
fn read_timezone() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(tz) = std::fs::read_to_string("/etc/timezone") {
            let tz = tz.trim();
            if !tz.is_empty() {
                return Some(tz.to_string());
            }
        }
        if let Ok(target) = std::fs::read_link("/etc/localtime") {
            let s = target.display().to_string();
            if let Some(i) = s.find("zoneinfo/") {
                return Some(s[i + "zoneinfo/".len()..].to_string());
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::var("TZ").ok().filter(|tz| !tz.is_empty())
    }
}

/// Whether the udev permission rules are installed (L8 startup card). `None`
/// on non-Linux (the row says "n/a"); the helper writes the fixed
/// `99-maggcs.rules` file, so existence is the whole check.
#[tauri::command]
pub fn udev_rules_present() -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    Some(std::path::Path::new("/etc/udev/rules.d/99-maggcs.rules").is_file())
}

/// SITL home file name inside the app config dir.
const SITL_HOME_FILE: &str = "sitl-home.json";

/// Write `{"lat":..,"lon":..}` into `dir/sitl-home.json`, creating `dir` first.
///
/// Split out of the command so the file format is unit-testable without a
/// Tauri runtime. The SITL helper (`scripts/sitl/sitl-home.sh`) reads this file
/// so PX4 spawns at the same location the operator set in Settings -> Vehicle.
fn write_sitl_home_file(dir: &Path, lat: f64, lon: f64) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(
        dir.join(SITL_HOME_FILE),
        format!("{{\"lat\":{lat},\"lon\":{lon}}}\n"),
    )
}

/// Persist the Settings -> Vehicle initial position for the SITL helper.
///
/// Mirrors the browser-side `localStorage` setting into the app config dir so a
/// headless PX4 SITL run can spawn at the operator's chosen location. Non-fatal
/// on the UI side: the map settings still persist regardless.
#[tauri::command]
pub async fn set_sitl_home(app: AppHandle, lat: f64, lon: f64) -> Result<(), String> {
    if !lat.is_finite() || !lon.is_finite() {
        return Err("initial position must be finite numbers".to_string());
    }
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("cannot resolve app config dir: {e}"))?;
    write_sitl_home_file(&dir, lat, lon).map_err(|e| e.to_string())
}

/// Start uploading a mission to the FC.
#[tauri::command]
pub async fn mission_upload(
    state: State<'_, AppState>,
    items: Vec<MissionItem>,
) -> Result<(), String> {
    let mission = state.mission().ok_or_else(|| "not connected".to_string())?;
    mission.upload(items).await.map_err(|e| e.to_string())
}

/// Download the FC mission.
#[tauri::command]
pub async fn mission_download(state: State<'_, AppState>) -> Result<(), String> {
    let mission = state.mission().ok_or_else(|| "not connected".to_string())?;
    mission.download().await.map_err(|e| e.to_string())
}

/// Clear the FC mission.
#[tauri::command]
pub async fn mission_clear(state: State<'_, AppState>) -> Result<(), String> {
    let mission = state.mission().ok_or_else(|| "not connected".to_string())?;
    mission.clear().await.map_err(|e| e.to_string())
}

/// Set the active waypoint.
#[tauri::command]
pub async fn mission_set_current(state: State<'_, AppState>, seq: u16) -> Result<(), String> {
    let mission = state.mission().ok_or_else(|| "not connected".to_string())?;
    mission.set_current(seq).await.map_err(|e| e.to_string())
}

/// Generate a parameterised survey sweep (optionally with tie lines).
///
/// Pure geometry: no link is required. The result is appended to the plan as
/// editable waypoints.
#[tauri::command]
pub fn survey_generate_sweep(pattern: SurveyPattern) -> Result<PatternPlan, String> {
    pattern.generate().map_err(|e| e.to_string())
}

/// Generate a parameterised cloverleaf calibration manoeuvre.
#[tauri::command]
pub fn survey_generate_cloverleaf(pattern: CloverleafPattern) -> Result<PatternPlan, String> {
    pattern.generate().map_err(|e| e.to_string())
}

/// Compile a generated pattern into the mission-ready waypoint sequence and
/// quality stats (S1/S2). Runs in Rust (ADR-015 convention); the frontend
/// only renders.
#[tauri::command]
pub fn survey_compile_pattern(
    plan: PatternPlan,
    options: maggcs_core::survey::compile::CompileOptions,
) -> Result<maggcs_core::survey::compile::CompiledPattern, String> {
    maggcs_core::survey::compile::compile_pattern(&plan, &options).map_err(|e| e.to_string())
}

/// Named vehicle command.
///
/// `name` is one of `"takeoff"`, `"land"`, `"rtl"`. The command is enqueued
/// to the command service, which sends a `COMMAND_LONG` and retransmits until
/// the FC acks; the outcome arrives asynchronously on the `"command"` event
/// (issue #5).
///
/// `MAV_CMD_DO_PAUSE_CONTINUE` was removed: PX4 v1.17 has no handler for it
/// (SITL-confirmed NACK, operator report 2026-10-10); takeoff/land are the
/// flight-phase commands PX4 v1.17 actually supports.
#[tauri::command]
pub async fn send_command(
    name: String,
    ground_amsl_m: Option<f64>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let service = state.command().ok_or_else(|| "not connected".to_string())?;
    let (command, params) = named_command(&name)?;

    // Takeoff (MS3, ADR-017): PX4's own `commander takeoff` flow is
    // NAV_TAKEOFF -> await ACK -> ARM (Commander.cpp:336-345). The mode
    // intent is accepted while disarmed; nobody would arm afterwards, so we
    // replicate the sequence here: param7 = current AMSL + the relative
    // 10 m climb, then arm once the FC accepted the mode.
    if name == "takeoff" {
        let fix = state
            .hub()
            .and_then(|h| h.snapshot().global_position)
            .ok_or_else(|| "no position fix yet — cannot compute the takeoff target".to_string())?;
        maggcs_core::CommandService::send_await(
            &service,
            MavCmd::MAV_CMD_NAV_TAKEOFF,
            takeoff_params(fix.latitude_deg, fix.longitude_deg, fix.altitude.meters()),
            TAKEOFF_ACK_TIMEOUT,
        )
        .await
        .map_err(|e| e.to_string())?;
        let armed = state
            .hub()
            .and_then(|h| h.snapshot().heartbeat)
            .is_some_and(|hb| hb.base_mode.safety_armed);
        if !armed {
            service
                .send(
                    MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
                    arm_disarm_params(true),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    // Landing (operator report, SITL): PX4 reads `NAV_LAND`'s `param7` as an
    // **AMSL** altitude exactly like takeoff, so a bare `[0.0; 7]` asked the
    // vehicle to land at AMSL 0 — a descending-to-nowhere target that ended
    // with the aircraft holding altitude over lower ground. Send the ground
    // under the vehicle instead (the frontend samples the terrain, mesh
    // first); without a terrain sample the current fix stands in, since a
    // vehicle on the ground knows its own height.
    if name == "land" {
        let fix = state
            .hub()
            .and_then(|h| h.snapshot().global_position)
            .ok_or_else(|| "no position fix yet — cannot compute the landing target".to_string())?;
        let ground = ground_amsl_m
            .filter(|m| m.is_finite())
            .unwrap_or_else(|| fix.altitude.meters());
        return service
            .send(
                MavCmd::MAV_CMD_NAV_LAND,
                land_params(fix.latitude_deg, fix.longitude_deg, ground),
            )
            .await
            .map_err(|e| e.to_string());
    }

    service
        .send(command, params)
        .await
        .map_err(|e| e.to_string())
}

/// Resolve the switchable mode table against the given vehicle state
/// (ADR-017): which entries are enabled and why. Pure core logic — the Fly
/// view only renders the result.
#[tauri::command]
pub fn flight_mode_table(
    context: maggcs_core::telemetry::px4_mode::ModeContext,
) -> Vec<maggcs_core::telemetry::px4_mode::ModeAvailability> {
    maggcs_core::telemetry::px4_mode::available_modes(&context)
}

/// Relative takeoff altitude (operator setting, 2026-10-11).
const TAKEOFF_ALTITUDE_M: f32 = 10.0;
/// How long takeoff waits for the NAV_TAKEOFF ack before giving up on the
/// sequence (the session's own retry budget is ~4 s).
const TAKEOFF_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// `MAV_CMD_NAV_TAKEOFF` parameters. PX4 reads `param7` as an **AMSL**
/// altitude (`navigator_main.cpp`: `rep->current.alt = cmd.param7`), so the
/// GCS must add the relative 10 m to the vehicle's current AMSL (telemetry).
/// `param5/6` must be the **current lat/lon**: `PX4_ISFINITE(0.0)` is true,
/// so zeros would aim the takeoff loiter at 0°N/0°E — the equator.
fn takeoff_params(lat_deg: f64, lon_deg: f64, current_amsl_m: f64) -> [f32; 7] {
    let mut params = [0.0; 7];
    params[4] = lat_deg as f32;
    params[5] = lon_deg as f32;
    params[6] = (current_amsl_m as f32) + TAKEOFF_ALTITUDE_M;
    params
}

/// `MAV_CMD_NAV_LAND` parameters.
///
/// Same convention as takeoff: PX4's navigator treats `param7` as an **AMSL**
/// altitude, so the GCS sends the ground under the vehicle. `param5/6` carry
/// the current lat/lon because `PX4_ISFINITE(0.0)` is true — zeros would aim
/// the landing at 0°N/0°E (the equator).
fn land_params(lat_deg: f64, lon_deg: f64, ground_amsl_m: f64) -> [f32; 7] {
    let mut params = [0.0; 7];
    params[4] = lat_deg as f32;
    params[5] = lon_deg as f32;
    params[6] = ground_amsl_m as f32;
    params
}

/// `MAV_CMD_DO_SET_MODE` parameters (ADR-017): `param1` must set
/// `MAV_MODE_FLAG_CUSTOM_MODE_ENABLED`, then PX4 main/sub mode.
fn set_mode_params(main_mode: u8, sub_mode: u8) -> [f32; 7] {
    [1.0, main_mode as f32, sub_mode as f32, 0.0, 0.0, 0.0, 0.0]
}

/// `MAV_CMD_COMPONENT_ARM_DISARM` parameters (ADR-017): `param1` 1 = arm /
/// 0 = disarm; we never send the 21196 kill code from the UI.
fn arm_disarm_params(arm: bool) -> [f32; 7] {
    [if arm { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// `MAV_CMD_DO_REPOSITION` bit 0: switch the vehicle to position control.
/// PX4 v1.14+ rejects the command outright with `UNSUPPORTED` unless this bit
/// is set — the flag tells commander to move into loiter (reposition) mode
/// and let the navigator fly the coordinates.
const MAV_DO_REPOSITION_FLAG_CHANGE_MODE: f32 = 1.0;

/// `MAV_CMD_DO_REPOSITION` parameters for a click-to-fly: `param1` = -1 uses
/// the default ground speed, `param2` carries the change-mode flag (PX4
/// requires it), `param4` = NaN keeps the current heading, `param7` is the
/// altitude expressed as AMSL (no `MAV_DO_REPOSITION_FLAGS` AGL bit).
fn reposition_params(lat_deg: f64, lon_deg: f64, amsl_m: f64) -> [f32; 7] {
    let mut params = [0.0f32; 7];
    params[0] = -1.0; // default ground speed
    params[1] = MAV_DO_REPOSITION_FLAG_CHANGE_MODE;
    params[3] = f32::NAN; // keep the current heading
    params[4] = lat_deg as f32;
    params[5] = lon_deg as f32;
    params[6] = amsl_m as f32;
    params
}

/// `MAV_CMD_DO_SET_HOME` parameters: `param1` = 0 selects the specified
/// location (`param5/6/7`); 1 would mean "use the vehicle's current position",
/// which PX4's commander reads as `cmd.param1 > 0.5f` and would ignore the
/// clicked coordinates entirely.
fn set_home_params(lat_deg: f64, lon_deg: f64, amsl_m: f64) -> [f32; 7] {
    let mut params = [0.0f32; 7];
    params[0] = 0.0; // use the specified location, not the current position
    params[4] = lat_deg as f32;
    params[5] = lon_deg as f32;
    params[6] = amsl_m as f32;
    params
}

/// Switch the PX4 flight mode (`MAV_CMD_DO_SET_MODE`, ADR-017). Fire and
/// forget through the command service like the other flight commands: the
/// ack verdict arrives on the `"command"` event (`set_mode`), the UI
/// additionally confirms via the heartbeat readback (MS3).
#[tauri::command]
pub async fn send_set_mode(
    main_mode: u8,
    sub_mode: u8,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let service = state.command().ok_or_else(|| "not connected".to_string())?;
    service
        .send(
            MavCmd::MAV_CMD_DO_SET_MODE,
            set_mode_params(main_mode, sub_mode),
        )
        .await
        .map_err(|e| e.to_string())
}

/// Arm or disarm the vehicle (`MAV_CMD_COMPONENT_ARM_DISARM`, ADR-017).
///
/// Arming runs the L5 preflight gate: every checklist item must pass, `Warn`
/// items need per-item confirmation (`force: None`), and `Fail`/`Unknown`
/// items are refused unless the operator supplies a [`ForceArmRequest`] — the
/// forced path, which only bypasses the **GCS** checklist. PX4's own prearm
/// checks always apply (the GCS never sends the 21196 forced-arming code) and
/// its verdict arrives on the `"command"` event (`arm_disarm`). Disarming is
/// never gated. A forced arm is recorded into the flight log.
#[tauri::command]
pub async fn send_arm_disarm(
    arm: bool,
    force: Option<ForceArmRequest>,
    plan_state: Option<PlanState>,
    acked_warnings: Option<Vec<ChecklistItemId>>,
    state: State<'_, AppState>,
) -> Result<(), PreflightRefusal> {
    if !arm {
        return send_arm_command(false, &state).await;
    }
    let Some(snapshot) = state.hub().map(|h| h.snapshot()) else {
        return Err(refusal(RefusalKind::NoLink, "no link — cannot arm"));
    };
    let checklist = evaluate(
        &snapshot,
        &app_plan_state(&state, plan_state),
        &safety_config(&state),
        epoch_ms(),
    );
    let Some(request) = force else {
        // The operator may have acknowledged specific `Warn` items in the
        // panel (U5); those count as Pass here. `Fail`/`Unknown` items and a
        // stale link still refuse — an acknowledgement never talks past those.
        return match arm_gate_with_ack(&checklist, &acked_warnings.unwrap_or_default()) {
            ArmGate::Allowed => send_arm_command(true, &state).await,
            ArmGate::RequiresAck { items } => Err(PreflightRefusal::new(
                RefusalKind::RequiresAck,
                &checklist,
                &items,
                "confirm the listed warnings before arming",
            )),
            ArmGate::Blocked { items } => Err(PreflightRefusal::new(
                RefusalKind::Blocked,
                &checklist,
                &items,
                "preflight blocks arming",
            )),
            ArmGate::NoLink => Err(refusal(RefusalKind::NoLink, "no link — cannot arm")),
        };
    };
    match evaluate_force_arm(&snapshot, &checklist) {
        ForceArmOutcome::Allowed => {
            let items = checklist
                .items
                .iter()
                .filter(|item| item.status != maggcs_core::safety::ChecklistStatus::Pass)
                .map(|item| item.id)
                .collect();
            // Audit persistence is a prerequisite to sending a forced arm.
            // Never fall through to an unrecorded override.
            state
                .recorder()
                .record_force_arm(items, request.reason, request.note)
                .await
                .map_err(|message| {
                    refusal(
                        RefusalKind::Blocked,
                        format!("forced arm not sent: {message}"),
                    )
                })?;
            send_arm_command(true, &state).await
        }
        ForceArmOutcome::NoLink => Err(refusal(
            RefusalKind::NoLink,
            "no link — a forced arm is not possible",
        )),
        ForceArmOutcome::AlreadyArmed => Err(refusal(
            RefusalKind::AlreadyArmed,
            "the vehicle is already armed",
        )),
        ForceArmOutcome::NotNeeded => Err(refusal(
            RefusalKind::NotNeeded,
            "the checklist is already clear — no force needed",
        )),
    }
}

/// The current evaluated preflight checklist (L5/U5), so the UI can render the
/// panel and gate Takeoff client-side without re-implementing the rules.
#[tauri::command]
pub async fn preflight_checklist(
    plan_state: Option<PlanState>,
    state: State<'_, AppState>,
) -> Result<Checklist, String> {
    let snapshot = state
        .hub()
        .map(|h| h.snapshot())
        .ok_or_else(|| "not connected".to_string())?;
    Ok(evaluate(
        &snapshot,
        &app_plan_state(&state, plan_state),
        &safety_config(&state),
        epoch_ms(),
    ))
}

/// Send the arm/disarm command itself, past the gate.
async fn send_arm_command(arm: bool, state: &AppState) -> Result<(), PreflightRefusal> {
    let service = state
        .command()
        .ok_or_else(|| refusal(RefusalKind::NoLink, "not connected"))?;
    service
        .send(MavCmd::MAV_CMD_COMPONENT_ARM_DISARM, arm_disarm_params(arm))
        .await
        .map_err(|e| refusal(RefusalKind::Blocked, e.to_string()))
}

/// App-side plan facts: the frontend supplies the mission-sync state, the
/// backend overrides the motor-test bit (it is authoritative there).
fn app_plan_state(state: &AppState, from_ui: Option<PlanState>) -> PlanState {
    let mut plan = from_ui.unwrap_or_default();
    plan.motor_test_active = state.motor_test.running.lock().is_some();
    plan
}

/// The thresholds the gate reads. A5 defaults for now; A6 persists them and
/// L7/M2 feed alert and mag thresholds from the same source later.
fn safety_config(state: &AppState) -> SafetyConfig {
    state.config().safety
}

/// A refusal with no item details.
fn refusal(kind: RefusalKind, message: impl Into<String>) -> PreflightRefusal {
    PreflightRefusal {
        kind,
        items: Vec::new(),
        message: message.into(),
    }
}

/// Epoch-ms now, matching `TelemetryFieldAges`.
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// L9: a click-to-fly target farther than this is refused — the map is not a
/// long-range navigator and a fat-fingered click must not launch a
/// cross-country flight. Configured via `SafetyConfig` (`goto_max_distance_m`).
const DEFAULT_GOTO_MAX_DISTANCE_M: f64 = 1_000.0;

/// Fly the vehicle to a spot clicked on the map (`MAV_CMD_DO_REPOSITION`).
/// The altitude is the vehicle's current AMSL, so the drone holds its height
/// and flies horizontally to `(lat, lon)` — the click-to-fly behaviour QGC's
/// "Fly to here" exposes. `param2` carries `MAV_DO_REPOSITION_FLAG_CHANGE_MODE`
/// so PX4 moves itself into loiter and accepts the target regardless of the
/// current mode.
///
/// **L9 guards**: the horizontal distance to the target must stay within
/// `safety.goto_max_distance_m` (default 1 km), and a target whose terrain
/// AMSL (when the DEM knows it) puts the vehicle's held altitude closer than
/// `safety.goto_min_clearance_m` above the surface is refused — the click
/// cannot command the drone into a hillside at its current height. Both
/// refusals carry the numbers so the operator sees why.
///
/// The ack verdict arrives on the `"command"` event (`goto`).
#[tauri::command]
pub async fn send_goto(
    lat_deg: f64,
    lon_deg: f64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let service = state.command().ok_or_else(|| "not connected".to_string())?;
    let fix = state
        .hub()
        .and_then(|h| h.snapshot().global_position)
        .ok_or_else(|| "no position fix yet — cannot command a move".to_string())?;

    // L9 guards: distance cap + terrain clearance under the held altitude
    // (the terrain verdict needs a DEM; without one it is skipped, the same
    // fail-open rule as the preflight `Unknown`).
    let safety = state.config().safety;
    let max_distance = safety
        .goto_max_distance_m
        .unwrap_or(DEFAULT_GOTO_MAX_DISTANCE_M);
    let terrain_m = if let (Some(_), Some(dem)) = (safety.goto_min_clearance_m, loaded_dem(&state))
    {
        use maggcs_core::terrain::ElevationSource;
        dem.as_ref()
            .sample_pair(maggcs_core::plan::GeoPoint::new(lat_deg, lon_deg))
            .ok()
            .map(|(dtm, _)| dtm.meters())
    } else {
        None
    };
    goto_guard(
        (fix.latitude_deg, fix.longitude_deg),
        (lat_deg, lon_deg),
        max_distance,
        terrain_m,
        fix.altitude.meters(),
        safety.goto_min_clearance_m,
    )?;

    service
        .send(
            MavCmd::MAV_CMD_DO_REPOSITION,
            reposition_params(lat_deg, lon_deg, fix.altitude.meters()),
        )
        .await
        .map_err(|e| e.to_string())
}

/// L9 guard decision (pure, for tests): `Err(reason)` refuses the click.
fn goto_guard(
    from: (f64, f64),
    to: (f64, f64),
    max_distance_m: f64,
    terrain_m: Option<f64>,
    held_amsl_m: f64,
    min_clearance_m: Option<f64>,
) -> Result<(), String> {
    let distance = haversine_m(from.0, from.1, to.0, to.1);
    if distance > max_distance_m {
        return Err(format!(
            "click-to-fly target is {distance:.0} m away — over the {max_distance_m:.0} m limit"
        ));
    }
    if let (Some(min_clearance), Some(terrain)) = (min_clearance_m, terrain_m) {
        let clearance = held_amsl_m - terrain;
        if clearance < min_clearance {
            return Err(format!(
                "the target's terrain is {terrain:.1} m — holding {clearance:.1} m above it is under the {min_clearance:.0} m minimum"
            ));
        }
    }
    Ok(())
}

/// Great-circle distance, metres.
fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0;
    let (dlat, dlon) = ((lat2 - lat1).to_radians(), (lon2 - lon1).to_radians());
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

/// The DEM the terrain store has open, when one is loaded.
fn loaded_dem(
    state: &State<'_, AppState>,
) -> Option<std::sync::Arc<maggcs_core::terrain::cog::CogDem>> {
    let guard = state.dem.lock();
    guard.as_ref().map(|(_, dem)| dem.clone())
}

/// Set the vehicle's home to a spot clicked on the map (`MAV_CMD_DO_SET_HOME`).
/// `param1` = 0 carries the clicked coordinates (`param5/6/7`), so PX4 sets
/// home there rather than at the vehicle's current position.
///
/// `amsl_m` is the **terrain height under the clicked point**, sampled by the
/// frontend (mesh first, then the DEM tile). It must not default to the
/// vehicle's own altitude: home is usually re-set while *airborne* over the
/// next site, and an airborne altitude bakes a home plane far above the
/// ground — every HOME-referenced height (relative legs, the landing target)
/// then sits too high. Without a terrain sample the current fix stands in,
/// which is correct for the landed case. The verdict arrives on the
/// `"command"` event (`set_home`).
#[tauri::command]
pub async fn send_set_home(
    lat_deg: f64,
    lon_deg: f64,
    amsl_m: Option<f64>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let service = state.command().ok_or_else(|| "not connected".to_string())?;
    let fix = state
        .hub()
        .and_then(|h| h.snapshot().global_position)
        .ok_or_else(|| "no position fix yet — cannot set home".to_string())?;
    let alt = amsl_m
        .filter(|m| m.is_finite())
        .unwrap_or_else(|| fix.altitude.meters());
    service
        .send(
            MavCmd::MAV_CMD_DO_SET_HOME,
            set_home_params(lat_deg, lon_deg, alt),
        )
        .await
        .map_err(|e| e.to_string())
}

/// Map a stream-rate request to `MAV_CMD_SET_MESSAGE_INTERVAL` parameters.
///
/// `interval_us` is already in microseconds; the special values 0 (default)
/// and -1 (disable) pass through unchanged.
fn set_message_interval_params(message_id: u32, interval_us: i32) -> [f32; 7] {
    let mut params = [0.0f32; 7];
    params[0] = message_id as f32;
    params[1] = interval_us as f32;
    params
}

/// Request the FC to stream a message at a fixed rate (Signal Inspector P7).
///
/// `interval_us` is the MAVLink interval in microseconds: 0 restores the
/// default rate, -1 disables the message, and any positive value sets it
/// (e.g. 1_000_000 / 50 = 20_000 for 50 Hz). The command goes through the
/// command service, so the ack/rejection arrives on the `"command"` event.
/// The local geomagnetic declination from the embedded WMM2025 (U8: survey
/// lines can be aligned with the local field, e.g. for magnetometry).
/// Computed at sea level for the current date; the altitude dependence of D
/// is far below the model's own uncertainty.
///
/// # Errors
///
/// A stringified [`maggcs_core::geomag::GeomagError`] outside the model's
/// validity window (2025–2030) or at degenerate positions.
#[tauri::command]
pub fn geomag_declination(lat: f64, lon: f64) -> Result<f64, String> {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs_f64();
    // Mean tropical year; the sub-milliyear error is irrelevant to D.
    let year = 1970.0 + unix / 31_556_925.0;
    let field = maggcs_core::geomag::field(lat, lon, 0.0, year).map_err(|e| e.to_string())?;
    Ok(field.declination_deg)
}

#[tauri::command]
pub async fn set_message_interval(
    message_id: u32,
    interval_us: i32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let service = state.command().ok_or_else(|| "not connected".to_string())?;
    service
        .send(
            MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL,
            set_message_interval_params(message_id, interval_us),
        )
        .await
        .map_err(|e| e.to_string())
}

/// Generate the udev rule text for a list of `(vendor_id, product_id)` pairs
/// (issues.md #42, ADR-009). One line per pair, bound to the tty subsystem,
/// world-readable. Pure function — unit-tested, previewed in the UI before
/// the pkexec prompt.
pub fn udev_rule_text(vid_pids: &[(String, String)]) -> String {
    let mut out = String::new();
    for (vid, pid) in vid_pids {
        out.push_str(&format!(
            "SUBSYSTEM==\"tty\", ATTRS{{idVendor}}==\"{vid}\", ATTRS{{idProduct}}==\"{pid}\", MODE=\"0666\"\n"
        ));
    }
    out
}

/// Install the udev rules for the given devices through polkit (ADR-009):
/// the helper script validates the rules again (defense in depth) before
/// writing the fixed rules file. Linux only.
#[tauri::command]
pub async fn install_udev_rules(vid_pids: Vec<(String, String)>) -> Result<String, String> {
    if !cfg!(target_os = "linux") {
        return Err("udev rules only apply on Linux".into());
    }
    if vid_pids.is_empty() {
        return Err("no devices selected".into());
    }
    let rules = udev_rule_text(&vid_pids);
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../helpers/udev-installer/install.sh");
    let script = script
        .canonicalize()
        .map_err(|e| format!("helper script not found: {e}"))?;
    use std::process::Stdio;
    let mut child = tokio::process::Command::new("pkexec")
        .args(["bash", &script.to_string_lossy(), "install"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("pkexec failed (is polkit installed?): {e}"))?;
    // Write the rule text to the helper's stdin (ADR-009: never on argv).
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(rules.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.shutdown().await.map_err(|e| e.to_string())?;
        drop(stdin); // EOF so the helper's `cat` returns
    }
    let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!(
            "helper rejected the rules: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Map a UI command name to its MAVLink command and parameters.
///
/// The returned parameters are the *default* payload; `takeoff` and `land`
/// are overridden in [`send_command`] with the position/altitude targets
/// computed from telemetry (see [`takeoff_params`] / [`land_params`]).
fn named_command(name: &str) -> Result<(MavCmd, [f32; 7]), String> {
    let command = match name {
        "takeoff" => MavCmd::MAV_CMD_NAV_TAKEOFF,
        "land" => MavCmd::MAV_CMD_NAV_LAND,
        "rtl" => MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
        other => return Err(format!("unknown command: {other}")),
    };
    Ok((command, [0.0; 7]))
}

#[cfg(test)]
mod tests {

    /// L9: the click-to-fly guards (pure decision function).
    #[test]
    fn a_goto_beyond_the_distance_limit_is_refused_with_the_numbers() {
        // 0.02 deg of longitude at 48.5 N ≈ 1.48 km — over the 1 km limit.
        let from = (48.5, -123.4);
        let to = (48.5, -123.4 + 0.02);
        let err = goto_guard(from, to, 1_000.0, None, 57.0, None).expect_err("must refuse");
        assert!(err.contains("m away"), "got {err}");
        // 0.005 deg ≈ 370 m — inside.
        assert!(goto_guard(from, (48.5, -123.4 + 0.005), 1_000.0, None, 57.0, None).is_ok());
    }

    #[test]
    fn a_goto_into_terrain_below_the_held_altitude_is_refused() {
        let from = (48.5, -123.4);
        let to = (48.5, -123.4 + 0.001);
        // The vehicle holds 57 m AMSL; the clicked point's terrain is 80 m —
        // flying there level would dig into the hillside.
        let err =
            goto_guard(from, to, 1_000.0, Some(80.0), 57.0, Some(30.0)).expect_err("must refuse");
        assert!(err.contains("terrain"), "got {err}");
        // Enough clearance passes.
        assert!(goto_guard(from, to, 1_000.0, Some(20.0), 57.0, Some(30.0)).is_ok());
        // No terrain known → no clearance verdict (fail-open, like Unknown).
        assert!(goto_guard(from, to, 1_000.0, None, 57.0, Some(30.0)).is_ok());
        // Guard disabled → nothing to say.
        assert!(goto_guard(from, to, 1_000.0, Some(80.0), 57.0, None).is_ok());
    }
    use super::*;

    /// `MAV_CMD_SET_MESSAGE_INTERVAL` param2: the requested interval in
    /// microseconds (0 = the message's default stream rate, -1 = disabled).
    const SET_MESSAGE_INTERVAL_DEFAULT_US: i32 = 0;
    const SET_MESSAGE_INTERVAL_DISABLED_US: i32 = -1;

    #[test]
    fn rtl_maps_to_return_to_launch() {
        let (command, params) = named_command("rtl").expect("rtl");
        assert_eq!(command, MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH);
        assert_eq!(params, [0.0; 7]);
    }

    #[test]
    fn set_mode_params_set_custom_mode_flag() {
        let p = set_mode_params(4, 3); // Auto / Loiter = Hold
        assert_eq!(p, [1.0, 4.0, 3.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn arm_disarm_params_never_carry_the_kill_code() {
        assert_eq!(arm_disarm_params(true), [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            arm_disarm_params(false),
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn preflight_thresholds_come_from_the_persisted_config() {
        let state = AppState::default();
        let mut config = maggcs_core::config::AppConfig::default();
        config.safety.min_satellites = 12;
        config.safety.require_rtk = true;
        state.set_config(config);

        let safety = safety_config(&state);
        assert_eq!(safety.min_satellites, 12);
        assert!(safety.require_rtk);
    }

    #[test]
    fn refused_teardown_message_gives_the_way_out() {
        let msg = refusal_message(
            "Disconnect",
            &ShutdownError::Refused {
                reason: "the vehicle is armed".to_string(),
            },
        );
        assert!(msg.contains("Disconnect refused"), "{msg}");
        assert!(msg.contains("armed"), "reason survives: {msg}");
        assert!(msg.contains("window remains open"), "{msg}");
        assert!(msg.contains("retry"), "{msg}");

        let other = refusal_message("Shut down", &ShutdownError::AlreadyRunning);
        assert!(other.contains("Shut down failed"), "{other}");
    }

    #[test]
    fn reposition_params_carry_position_and_keep_heading() {
        let p = reposition_params(48.6493, -123.3982, 90.5);
        assert_eq!(p[0], -1.0, "default ground speed");
        assert_eq!(
            p[1], MAV_DO_REPOSITION_FLAG_CHANGE_MODE,
            "param2 requests the mode change PX4 requires"
        );
        assert_eq!(p[2], 0.0, "reserved");
        assert!(p[3].is_nan(), "param4 = NaN keeps the current heading");
        assert_eq!(p[4], 48.6493);
        assert_eq!(p[5], -123.3982);
        assert_eq!(p[6], 90.5);
    }

    #[test]
    fn set_home_params_carry_the_specified_location() {
        let p = set_home_params(48.6, -123.4, 45.0);
        assert_eq!(
            p[0], 0.0,
            "param1 = 0 selects the specified location, not the current position"
        );
        assert_eq!(p[4], 48.6);
        assert_eq!(p[5], -123.4);
        assert_eq!(p[6], 45.0);
    }

    #[test]
    fn takeoff_params_carry_position_and_relative_altitude() {
        // Current fix (583.25 m AMSL) + 10 m relative = 593.25 m AMSL in
        // param7; lat/lon go into param5/6 so the takeoff target is the
        // vehicle's own position, not the equator.
        let params = takeoff_params(48.6493, -123.3982, 583.25);
        assert_eq!(params[4], 48.6493);
        assert_eq!(params[5], -123.3982);
        assert_eq!(params[6], 593.25);
        assert_eq!(&params[..4], &[0.0; 4]);
    }

    #[test]
    fn takeoff_and_land_map_to_nav_commands() {
        let (takeoff_cmd, takeoff) = named_command("takeoff").expect("takeoff");
        let (land_cmd, land) = named_command("land").expect("land");
        assert_eq!(takeoff_cmd, MavCmd::MAV_CMD_NAV_TAKEOFF);
        assert_eq!(land_cmd, MavCmd::MAV_CMD_NAV_LAND);
        // Defaults only: `send_command` replaces them with the computed
        // position/altitude targets (takeoff_params / land_params).
        assert_eq!(takeoff, [0.0; 7]);
        assert_eq!(land, [0.0; 7]);
    }

    #[test]
    fn land_params_carry_the_ground_target() {
        // param7 must be the terrain AMSL under the vehicle — a bare 0.0
        // asked PX4 to land at sea level, which left the aircraft hovering
        // over lower ground (operator report, SITL).
        let p = land_params(48.6493, -123.3982, 12.5);
        assert_eq!(p[4], 48.6493);
        assert_eq!(p[5], -123.3982);
        assert_eq!(p[6], 12.5);
        assert_eq!(p[0], 0.0, "param1: land at the position given below");
    }

    #[test]
    fn unknown_name_is_rejected() {
        assert!(named_command("banana").is_err());
    }

    #[test]
    fn set_message_interval_maps_id_and_interval() {
        let p = set_message_interval_params(30, 20_000);
        assert_eq!(p[0], 30.0, "param1 is the message id");
        assert_eq!(p[1], 20_000.0, "param2 is the interval in microseconds");
        assert_eq!(p[2..], [0.0; 5], "no other parameters");

        let on = set_message_interval_params(30, 1_000_000 / 50);
        assert_eq!(on[1], 20_000.0, "50 Hz = 20 ms = 20 000 us");

        assert_eq!(
            set_message_interval_params(30, SET_MESSAGE_INTERVAL_DEFAULT_US)[1],
            0.0,
            "0 restores the default rate"
        );
        assert_eq!(
            set_message_interval_params(30, SET_MESSAGE_INTERVAL_DISABLED_US)[1],
            -1.0,
            "-1 disables the message"
        );
    }

    #[test]
    fn sitl_home_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("maggcs-sitl-home-{}", std::process::id()));
        write_sitl_home_file(&dir, 48.6493, -123.3982).expect("write");
        let raw = std::fs::read_to_string(dir.join(SITL_HOME_FILE)).expect("read");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(value["lat"], 48.6493);
        assert_eq!(value["lon"], -123.3982);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
