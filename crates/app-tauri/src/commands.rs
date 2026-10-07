//! Tauri commands exposed to the React UI.

use std::time::Duration;

use ::mavlink::common::MavCmd;
use maggcs_core::devices::{DeviceDatabase, DeviceManager, SerialDeviceInfo};
use maggcs_core::mavlink::connection::{spawn_connection, LinkStatus};
use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
use maggcs_core::mission::service::MissionIds;
use maggcs_core::mission::types::MissionItem;
use maggcs_core::telemetry::hub::{TelemetryHub, DEFAULT_PUSH_HZ};
use maggcs_core::telemetry::{TelemetrySnapshot, VehicleId};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

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

/// Named vehicle command.
///
/// `name` is one of `"rtl"`, `"pause"`, `"continue"`. The command is enqueued
/// to the command service, which sends a `COMMAND_LONG` and retransmits until
/// the FC acks; the outcome arrives asynchronously on the `"command"` event
/// (issue #5).
///
/// Pause/continue use `MAV_CMD_DO_PAUSE_CONTINUE` (param1: 0 = pause,
/// 1 = continue, per the MAVLink spec). PX4's exact handling is still to be
/// confirmed on SITL v1.17 — a rejection comes back as a NACK and is shown in
/// the UI, so the failure mode is safe.
#[tauri::command]
pub async fn send_command(name: String, state: State<'_, AppState>) -> Result<(), String> {
    let service = state.command().ok_or_else(|| "not connected".to_string())?;
    let (command, params) = named_command(&name)?;
    service
        .send(command, params)
        .await
        .map_err(|e| e.to_string())
}

/// `MAV_CMD_DO_PAUSE_CONTINUE` param1: pause.
const PAUSE_CONTINUE_PAUSE: f32 = 0.0;
/// `MAV_CMD_DO_PAUSE_CONTINUE` param1: continue.
const PAUSE_CONTINUE_RESUME: f32 = 1.0;

/// Map a UI command name to its MAVLink command and parameters.
fn named_command(name: &str) -> Result<(MavCmd, [f32; 7]), String> {
    let mut params = [0.0f32; 7];
    let command = match name {
        "rtl" => MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
        "pause" => {
            params[0] = PAUSE_CONTINUE_PAUSE;
            MavCmd::MAV_CMD_DO_PAUSE_CONTINUE
        }
        "continue" => {
            params[0] = PAUSE_CONTINUE_RESUME;
            MavCmd::MAV_CMD_DO_PAUSE_CONTINUE
        }
        other => return Err(format!("unknown command: {other}")),
    };
    Ok((command, params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtl_maps_to_return_to_launch() {
        let (command, params) = named_command("rtl").expect("rtl");
        assert_eq!(command, MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH);
        assert_eq!(params, [0.0; 7]);
    }

    #[test]
    fn pause_and_continue_share_do_pause_continue() {
        let (pause_cmd, pause) = named_command("pause").expect("pause");
        let (cont_cmd, cont) = named_command("continue").expect("continue");
        assert_eq!(pause_cmd, MavCmd::MAV_CMD_DO_PAUSE_CONTINUE);
        assert_eq!(cont_cmd, MavCmd::MAV_CMD_DO_PAUSE_CONTINUE);
        assert_eq!(pause[0], PAUSE_CONTINUE_PAUSE);
        assert_eq!(cont[0], PAUSE_CONTINUE_RESUME);
        assert_eq!(pause[1..], cont[1..], "only param1 differs");
    }

    #[test]
    fn unknown_name_is_rejected() {
        assert!(named_command("banana").is_err());
    }
}
