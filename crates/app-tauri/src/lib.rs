//! MagGCS desktop entry (Tauri 2).
//!
//! Embeds `maggcs-core` directly (ADR-001): MAVLink connections run in the
//! Tauri process, telemetry/link events are pushed to the React frontend via
//! Tauri events, and commands let the UI control the link.

mod actuator_service;
mod alert_service;
mod calibration_service;
mod command_service;
mod commands;
mod config_service;
mod console_service;
mod diagnostics_service;
mod inspector_service;
mod instruments_service;
mod lifecycle_service;
mod logs_service;
mod map_service;
mod mission_service;
mod motor_test_state;
mod params_service;
mod project_service;
mod recorder_service;
mod rtk_service;
mod shared_state_service;
mod state;
mod telemetry_pump;
mod terrain_server;
mod terrain_service;

use maggcs_core::lifecycle::StopReason;
use state::AppState;
use tauri::{Emitter, Manager};

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
            commands::geomag_declination,
            commands::shutdown_app,
            commands::teardown_guard,
            commands::link_status,
            commands::take_teardown_refusal,
            alert_service::alert_snapshot,
            alert_service::alert_raise,
            alert_service::alert_ack,
            alert_service::alert_ack_all,
            alert_service::alert_clear_acknowledged,
            commands::get_snapshot,
            commands::enumerate_devices,
            commands::set_sitl_home,
            config_service::config_load,
            config_service::config_save,
            project_service::project_create,
            project_service::project_open,
            project_service::project_close,
            project_service::project_status,
            project_service::project_save,
            project_service::project_list_surveys,
            project_service::project_load_survey,
            project_service::project_save_survey,
            project_service::project_list_sessions,
            project_service::project_save_session,
            project_service::project_export,
            project_service::project_import,
            commands::install_udev_rules,
            commands::mission_upload,
            commands::mission_download,
            commands::mission_clear,
            commands::mission_set_current,
            commands::survey_generate_sweep,
            commands::survey_generate_cloverleaf,
            commands::survey_compile_pattern,
            terrain_service::terrain_load,
            terrain_service::drape_preview,
            terrain_service::drape_monitor_start,
            terrain_service::drape_monitor_stop,
            terrain_service::drape_monitor_ack,
            terrain_service::drape_monitor_step,
            terrain_service::drape_report,
            terrain_service::drape_export,
            terrain_service::drape_bias,
            terrain_service::terrain_discover,
            terrain_service::terrain3d_start,
            terrain_service::terrain3d_stop,
            terrain_service::terrain3d_status,
            terrain_service::terrain3d_download,
            terrain_service::terrain3d_build,
            terrain_service::terrain3d_dem_source,
            terrain_service::terrain3d_build_from_dem,
            terrain_service::survey_prepare_status,
            terrain_service::survey_prepare,
            terrain_service::survey_prepare_cancel,
            terrain_service::terrain3d_build_from_survey,
            console_service::console_send,
            console_service::console_close,
            logs_service::logs_request_list,
            logs_service::logs_download,
            logs_service::logs_cancel,
            terrain_service::terrain_select,
            terrain_service::terrain3d_cancel,
            terrain_service::terrain_sample,
            terrain_service::terrain_process_raw,
            terrain_service::geoid_sample,
            terrain_service::vertical_datum_offset,
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
            commands::preflight_checklist,
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
            actuator_service::actuator_set_sdlog_mode,
            actuator_service::actuator_logs_request_list,
            actuator_service::actuator_logs_download,
            actuator_service::actuator_logs_cancel,
            actuator_service::actuator_logs_status,
            actuator_service::actuator_recorder_start,
            actuator_service::actuator_recorder_stop,
            actuator_service::actuator_recorder_status,
            actuator_service::actuator_set_sdlog_mode,
            actuator_service::actuator_logs_request_list,
            actuator_service::actuator_logs_download,
            actuator_service::actuator_logs_cancel,
            actuator_service::actuator_logs_status,
            actuator_service::actuator_recorder_start,
            actuator_service::actuator_recorder_stop,
            actuator_service::actuator_recorder_status,
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
            calibration_service::mag_calibration_status,
            commands::clock_info,
            commands::tools_status,
            map_service::map_open,
            map_service::map_close,
            map_service::map_status,
            shared_state_service::shared_state_get,
            shared_state_service::shared_state_snapshot,
            shared_state_service::shared_state_set,
            commands::udev_rules_present,
            diagnostics_service::diagnostics_export,
            inspector_service::inspector_catalog,
            inspector_service::inspector_list_algorithms,
            inspector_service::inspector_open_ulog,
            inspector_service::inspector_replay,
            inspector_service::inspector_replay_status,
            inspector_service::inspector_use_live,
            inspector_service::inspector_source,
            instruments_service::instruments_connect,
            instruments_service::instruments_disconnect,
            instruments_service::instruments_status,
            recorder_service::recorder_status,
            recorder_service::recorder_log_event
        ])
        .setup(|app| {
            // A6: the arm gate reads safety thresholds from the persisted
            // config, so load it before the first command can run.
            let config = config_service::load_at_startup(app.handle());
            app.state::<AppState>().set_config(config);
            // L8: the mag calibration survives restarts via its sidecar file.
            calibration_service::load_at_startup(app.handle(), &app.state::<AppState>());
            // A10: the rolling file log + panic hook; the guard lives forever.
            {
                use tauri::Manager;
                let log_dir = app
                    .path()
                    .app_log_dir()
                    .unwrap_or_else(|_| std::env::temp_dir());
                let path = diagnostics_service::init_file_logger(&log_dir);
                diagnostics_service::install_panic_hook(&path);
            }

            // One alert pump for the process: every transition the core
            // alert center records is forwarded to all webviews (A3).
            {
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
        // Closing the main window (X, Alt-F4, session logout) requests the
        // same guarded teardown as in-app Shut down. The close is initially
        // prevented; if armed/airborne, keep the window alive and notify the
        // frontend so it can show the refusal. No flight command is sent.
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                let state = app.state::<AppState>();
                if !state.begin_close() {
                    return; // a teardown is already in flight
                }
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<AppState>();
                    let _ops = state.ops().lock().await;
                    match lifecycle_service::run(&app, StopReason::WindowClose).await {
                        Ok(_) => {
                            actuator_service::stop_auxiliary_links(&state).await;
                            app.exit(0)
                        }
                        Err(err) => {
                            let state = app.state::<AppState>();
                            state.cancel_close();
                            let message = match err {
                                maggcs_core::lifecycle::ShutdownError::Refused { reason } => reason,
                                other => other.to_string(),
                            };
                            state.set_close_refusal(message.clone());
                            let _ = app.emit("teardown_refused", message);
                        }
                    }
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
