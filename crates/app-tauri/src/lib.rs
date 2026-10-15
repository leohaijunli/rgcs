//! MagGCS desktop entry (Tauri 2).
//!
//! Embeds `maggcs-core` directly (ADR-001): MAVLink connections run in the
//! Tauri process, telemetry/link events are pushed to the React frontend via
//! Tauri events, and commands let the UI control the link.

mod actuator_service;
mod alert_service;
mod command_service;
mod commands;
mod console_service;
mod inspector_service;
mod logs_service;
mod mission_service;
mod motor_test_state;
mod params_service;
mod rtk_service;
mod state;
mod telemetry_pump;
mod terrain_server;
mod terrain_service;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(AppState::default())
        .manage(std::sync::Arc::new(console_service::ConsoleState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::disconnect,
            commands::shutdown_app,
            commands::link_status,
            alert_service::alert_snapshot,
            alert_service::alert_raise,
            alert_service::alert_ack,
            alert_service::alert_ack_all,
            alert_service::alert_clear_acknowledged,
            commands::get_snapshot,
            commands::enumerate_devices,
            commands::set_sitl_home,
            commands::install_udev_rules,
            commands::mission_upload,
            commands::mission_download,
            commands::mission_clear,
            commands::mission_set_current,
            commands::survey_generate_sweep,
            commands::survey_generate_cloverleaf,
            commands::survey_compile_pattern,
            terrain_service::terrain_load,
            terrain_service::terrain_discover,
            terrain_service::terrain3d_start,
            terrain_service::terrain3d_stop,
            terrain_service::terrain3d_status,
            terrain_service::terrain3d_download,
            terrain_service::terrain3d_build,
            terrain_service::terrain3d_dem_source,
            terrain_service::terrain3d_build_from_dem,
            console_service::console_send,
            console_service::console_close,
            logs_service::logs_request_list,
            logs_service::logs_download,
            logs_service::logs_cancel,
            commands::terrain_drape,
            terrain_service::terrain_sample,
            terrain_service::terrain_process_raw,
            terrain_service::geoid_sample,
            rtk_service::rtk_start_serial,
            rtk_service::rtk_stop,
            rtk_service::rtk_last_stats,
            rtk_service::rtk_foreign_state,
            commands::send_command,
            commands::send_goto,
            commands::send_set_home,
            commands::flight_mode_table,
            commands::send_set_mode,
            commands::send_arm_disarm,
            commands::set_message_interval,
            actuator_service::actuator_open,
            actuator_service::actuator_close,
            actuator_service::actuator_status,
            actuator_service::actuator_start_manual,
            actuator_service::actuator_start_preset,
            actuator_service::actuator_connect_sitl,
            actuator_service::actuator_disconnect_sitl,
            actuator_service::actuator_connect_target,
            actuator_service::actuator_disconnect_target,
            actuator_service::actuator_start_forward,
            actuator_service::actuator_set_alarm_config,
            actuator_service::actuator_acknowledge_alarm,
            actuator_service::actuator_set_values,
            actuator_service::actuator_stop,
            actuator_service::actuator_estop,
            actuator_service::actuator_reset_emergency,
            params_service::params_open,
            params_service::params_pull,
            params_service::params_cancel,
            params_service::params_set,
            params_service::params_fc_version,
            params_service::params_reboot,
            params_service::params_read,
            params_service::params_cache_get,
            params_service::params_cache_len,
            inspector_service::inspector_open,
            inspector_service::inspector_close,
            inspector_service::inspector_connect,
            inspector_service::inspector_disconnect,
            inspector_service::inspector_status,
            inspector_service::inspector_set_traces,
            inspector_service::set_mag_calibration,
            inspector_service::inspector_catalog,
            inspector_service::inspector_list_algorithms,
            inspector_service::inspector_open_ulog,
            inspector_service::inspector_replay,
            inspector_service::inspector_replay_status,
            inspector_service::inspector_use_live,
            inspector_service::inspector_source
        ])
        .setup(|app| {
            // One alert pump for the process: every transition the core
            // alert center records is forwarded to all webviews (A3).
            {
                use tauri::Manager;
                let alert_app = app.handle().clone();
                let alerts = app.state::<AppState>().alerts();
                tauri::async_runtime::spawn(alert_service::pump(alert_app, alerts));
            }

            // Serial hotplug watcher (issues.md #40): pushes a fresh device
            // list to the frontend whenever a port appears or disappears, so
            // the connection dropdown stays live without a manual refresh.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use maggcs_core::devices::{DeviceDatabase, DeviceManager};
                use tauri::Emitter;
                let watcher = DeviceManager::new(DeviceDatabase::builtin()).spawn_watcher();
                let mut events = watcher.subscribe();
                loop {
                    match events.recv().await {
                        Ok(_) => {
                            let manager = DeviceManager::new(DeviceDatabase::builtin());
                            match manager.enumerate() {
                                Ok(devices) => {
                                    let _ = app_handle.emit("devices_changed", &devices);
                                }
                                Err(_) => continue,
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
