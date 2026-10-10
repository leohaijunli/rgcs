//! Motor / mag interference test window service (motor-test-plan §5, A1).
//!
//! A thin adapter like `inspector_service` (ADR-001): the window lifecycle
//! lives here, the safety state machine and the high-rate sender will live in
//! `core::motor_test` once the M0 bench run pins the command semantics. A1
//! ships only the window, the emergency-stop hook point, and a real status
//! snapshot for the right-hand panel.

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;
use tauri::State;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tauri::WindowEvent;

use crate::state::AppState;

/// Status snapshot for the window's header and right panel (polled ~1 Hz by
/// the UI; safety-relevant signals are low-rate).
#[derive(Debug, Clone, Serialize)]
pub struct ActuatorStatus {
    pub connected: bool,
    pub fc_alive: bool,
    /// The one hard interlock: sending is only ever allowed while disarmed.
    pub armed: bool,
    /// PX4 nav state name, best-effort from the heartbeat's custom mode.
    pub mode: String,
    pub endpoint: Option<String>,
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
        .title("Motor / Mag Interference Test")
        .maximized(true)
        .min_inner_size(1024.0, 640.0)
        .build()
        .map_err(|e| e.to_string())?;
    // Closing the window must stop the motors (plan §8.3): the hook is
    // wired now even though the sender does not exist yet, so the path is
    // proven before A2 puts live hardware behind it.
    let app_for_cleanup = app.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            app_for_cleanup
                .state::<AppState>()
                .actuator_window_destroyed();
        }
    });
    Ok(())
}

/// Close the motor-test window and run the stop path.
#[tauri::command]
pub fn actuator_close(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.actuator_window_destroyed();
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

impl AppState {
    /// Snapshot the link + FC state for the actuator window.
    pub fn actuator_status(&self) -> ActuatorStatus {
        let link = self.link_status();
        let hub = self.hub();
        let (armed, mode) = match hub {
            Some(hub) => {
                let snap = hub.snapshot();
                match snap.heartbeat {
                    Some(hb) => (hb.base_mode.safety_armed, format!("{}", hb.custom_mode)),
                    None => (false, "—".to_string()),
                }
            }
            None => (false, "—".to_string()),
        };
        ActuatorStatus {
            connected: link
                .as_ref()
                .map(|l| l.link_state == maggcs_core::mavlink::LinkState::Connected)
                .unwrap_or(false),
            fc_alive: link.as_ref().map(|l| l.fc_alive).unwrap_or(false),
            armed,
            mode,
            endpoint: link.map(|l| l.endpoint),
        }
    }

    /// The motor-test window closed (or the app is shutting down): the one
    /// stop funnel. The A2 sender will hang off this hook.
    pub fn actuator_window_destroyed(&self) {
        // A1: nothing to stop yet — the sender does not exist. When it does,
        // this becomes `motor_test::stop_all()`.
    }
}
