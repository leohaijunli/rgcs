//! MagGCS desktop entry (Tauri 2).
//!
//! Embeds `maggcs-core` directly (ADR-001): MAVLink connections run in the
//! Tauri process, telemetry/link events are pushed to the React frontend via
//! Tauri events, and commands let the UI control the link.

mod command_service;
mod commands;
mod inspector_service;
mod mission_service;
mod state;
mod telemetry_pump;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::disconnect,
            commands::shutdown_app,
            commands::link_status,
            commands::get_snapshot,
            commands::enumerate_devices,
            commands::set_sitl_home,
            commands::mission_upload,
            commands::mission_download,
            commands::mission_clear,
            commands::mission_set_current,
            commands::survey_generate_sweep,
            commands::survey_generate_cloverleaf,
            commands::send_command,
            inspector_service::inspector_open,
            inspector_service::inspector_close,
            inspector_service::inspector_connect,
            inspector_service::inspector_disconnect,
            inspector_service::inspector_subscribe,
            inspector_service::inspector_catalog,
            inspector_service::inspector_list_algorithms
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
