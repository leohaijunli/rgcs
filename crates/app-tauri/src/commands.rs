//! Tauri commands exposed to the React UI.

use std::time::Duration;

use maggcs_core::devices::{DeviceDatabase, DeviceManager, SerialDeviceInfo};
use maggcs_core::mavlink::connection::{spawn_connection, LinkStatus};
use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
use maggcs_core::mission::types::MissionItem;
use maggcs_core::telemetry::hub::{TelemetryHub, DEFAULT_PUSH_HZ};
use maggcs_core::telemetry::TelemetrySnapshot;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::mission_service::MissionService;
use crate::state::AppState;
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
    let mut config = ConnectionConfig {
        endpoint: Endpoint::try_from(endpoint.as_str()).map_err(|e| e.to_string())?,
        ..Default::default()
    };
    if let Some(ms) = heartbeat_timeout_ms {
        config.heartbeat_timeout = Duration::from_millis(ms);
    }

    state.take_hub();
    state.take_mission();
    if let Some(old) = state.take_connection() {
        old.shutdown().await;
    }

    let (handle, _rx) = spawn_connection(config.clone())
        .await
        .map_err(|e| e.to_string())?;
    let hub = TelemetryHub::spawn(handle.clone(), DEFAULT_PUSH_HZ);
    let mission = MissionService::spawn(
        app.clone(),
        handle.clone(),
        config.system_id,
        config.component_id,
        config.target_system_id,
        config.target_component_id,
    );
    state.set_connection(handle);
    state.set_hub(hub.clone());
    state.set_mission(mission);
    tauri::async_runtime::spawn(telemetry_pump::run(app.clone(), hub));

    Ok(ConnectResponse {
        ok: true,
        endpoint: config.endpoint.to_address_string(),
    })
}

/// Stop the active connection and hub.
#[tauri::command]
pub async fn disconnect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.take_hub();
    state.take_mission();
    if let Some(handle) = state.take_connection() {
        handle.shutdown().await;
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
    let mission = state
        .mission()
        .ok_or_else(|| "not connected".to_string())?;
    mission.upload(items).await
}

/// Download the FC mission.
#[tauri::command]
pub async fn mission_download(state: State<'_, AppState>) -> Result<(), String> {
    let mission = state
        .mission()
        .ok_or_else(|| "not connected".to_string())?;
    mission.download().await
}

/// Clear the FC mission.
#[tauri::command]
pub async fn mission_clear(state: State<'_, AppState>) -> Result<(), String> {
    let mission = state
        .mission()
        .ok_or_else(|| "not connected".to_string())?;
    mission.clear().await
}

/// Set the active waypoint.
#[tauri::command]
pub async fn mission_set_current(
    state: State<'_, AppState>,
    seq: u16,
) -> Result<(), String> {
    let mission = state
        .mission()
        .ok_or_else(|| "not connected".to_string())?;
    mission.set_current(seq).await
}
