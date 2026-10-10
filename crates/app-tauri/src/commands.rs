//! Tauri commands exposed to the React UI.

use std::fs;
use std::path::Path;
use std::time::Duration;

use ::mavlink::common::MavCmd;
use maggcs_core::devices::{DeviceDatabase, DeviceManager, SerialDeviceInfo};
use maggcs_core::mavlink::connection::{spawn_connection, LinkStatus};
use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
use maggcs_core::mission::service::MissionIds;
use maggcs_core::mission::types::MissionItem;
use maggcs_core::survey::{CloverleafPattern, PatternPlan, SurveyPattern};
use maggcs_core::telemetry::hub::{TelemetryHub, DEFAULT_PUSH_HZ};
use maggcs_core::telemetry::{TelemetrySnapshot, VehicleId};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::command_service;
use crate::mission_service;
use crate::state::{ActiveLink, AppState, LinkId};
use crate::telemetry_pump;

/// Response of the `connect` command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ConnectResponse {
    pub ok: bool,
    pub endpoint: String,
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

    // The hub consumes the initial event stream, so the `Connected` emitted by
    // the worker cannot be missed (issue #2).
    let target = VehicleId::new(config.target_system_id, config.target_component_id);
    let hub = TelemetryHub::spawn(events_rx, DEFAULT_PUSH_HZ, target);
    let mission = mission_service::spawn(
        app.clone(),
        handle.clone(),
        MissionIds {
            self_system: config.system_id,
            self_component: config.component_id,
            target_system: config.target_system_id,
            target_component: config.target_component_id,
        },
    );
    let command = command_service::spawn(
        app.clone(),
        handle.clone(),
        config.target_system_id,
        config.target_component_id,
    );
    tauri::async_runtime::spawn(telemetry_pump::run(app.clone(), hub.clone()));

    // Swap in the new link atomically and tear down the previous one only
    // after the new link is up (a failed reconnect no longer kills a working
    // link).
    let previous = state.set_primary_link(ActiveLink {
        id: LinkId::next(),
        connection: handle,
        hub,
        mission,
        command,
    });
    if let Some(old) = previous {
        old.hub.shutdown();
        old.connection.shutdown().await;
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

    Ok(ConnectResponse {
        ok: true,
        endpoint: config.endpoint.to_address_string(),
    })
}

/// How long `connect` waits for the first bind/connect attempt to resolve.
const FIRST_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Stop the active connection and hub.
#[tauri::command]
pub async fn disconnect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.ops().lock().await;
    if let Some(link) = state.take_primary_link() {
        link.hub.shutdown();
        link.connection.shutdown().await;
        // Dropping the mission/command handles stops their service tasks.
    }
    state.publish_link(LinkStatus::disconnected());
    let _ = app.emit("link", LinkStatus::disconnected());
    Ok(())
}

/// Latest known link status (fallback when events were missed).
#[tauri::command]
pub async fn link_status(state: State<'_, AppState>) -> Result<Option<LinkStatus>, String> {
    Ok(state.link_status())
}

/// Stop the link cleanly and exit the process.
///
/// Unlike `disconnect`, this quits MagGCS: useful when the window chrome is
/// not reachable (field tablet / kiosk) so the operator needs an in-app
/// shutdown control (handoff item 9).
#[tauri::command]
pub async fn shutdown_app(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.ops().lock().await;
    if let Some(link) = state.take_primary_link() {
        link.hub.shutdown();
        link.connection.shutdown().await;
    }
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
pub async fn send_command(name: String, state: State<'_, AppState>) -> Result<(), String> {
    let service = state.command().ok_or_else(|| "not connected".to_string())?;
    let (command, params) = named_command(&name)?;
    service
        .send(command, params)
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

/// Map a UI command name to its MAVLink command and parameters.
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
    fn takeoff_and_land_map_to_nav_commands() {
        let (takeoff_cmd, takeoff) = named_command("takeoff").expect("takeoff");
        let (land_cmd, land) = named_command("land").expect("land");
        assert_eq!(takeoff_cmd, MavCmd::MAV_CMD_NAV_TAKEOFF);
        assert_eq!(land_cmd, MavCmd::MAV_CMD_NAV_LAND);
        assert_eq!(takeoff, [0.0; 7]);
        assert_eq!(land, [0.0; 7]);
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
