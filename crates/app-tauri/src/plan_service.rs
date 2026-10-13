//! Flight-plan design window (ADR-016 pattern, bidirectional): a separate
//! webview mirroring the main window's mission store. The store stays
//! resident in the main window — this module only opens the window and
//! shuttles snapshots/intents (`plan_state` out, `plan_edit` in); the
//! mirroring logic itself lives in `frontend/src/plan/planBridge.ts`.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Open the flight-plan design window, or focus it if it already exists.
///
/// # Errors
///
/// A stringified Tauri error when the window cannot be created.
#[tauri::command]
pub fn plan_window_open(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("plan") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, "plan", WebviewUrl::App("plan.html".into()))
        .devtools(true)
        .title("Flight Plan Design")
        .inner_size(680.0, 900.0)
        .min_inner_size(480.0, 600.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Broadcast a mission snapshot to the plan window. Called by the main
/// window's bridge (see `frontend/src/desktop/planHost.ts`) whenever the
/// mission store changes; the debounce lives on the JS side.
///
/// # Errors
///
/// Never; the emit result is ignored (a closed plan window just drops it).
#[tauri::command]
pub fn plan_window_state(
    app: tauri::AppHandle,
    snapshot: serde_json::Value,
) -> Result<(), String> {
    use tauri::Emitter;
    let _ = app.emit_to("plan", "plan_state", snapshot);
    Ok(())
}
