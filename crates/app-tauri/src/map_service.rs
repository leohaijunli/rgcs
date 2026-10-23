//! Detachable map window (plan §7 U15): "Pop out map" moves MapView into its
//! own webview so the main window can run map-free (bigger HUD, command
//! strip, checklist). The map window is a *view only* — no command path
//! changes, and RTL/Hold stay in the main window's TopBar.
//!
//! Window semantics: closing the map window re-embeds (it does NOT quit);
//! the main window's close still goes through `Lifecycle::shutdown`, which
//! takes the map window down with it (`close_map_on_shutdown`). The map
//! window's own webview/WebGL crash is contained by construction (separate
//! webview), and the main window renders a "map unavailable / reopen"
//! placeholder from the `map_window` events.

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// The map webview's label (the frontend keys the detached flag on it).
pub const MAP_LABEL: &str = "map";

/// Open (or focus) the detachable map window. The window boots `map.html`,
/// which mounts the same MapView + alert layer as the embedded one.
///
/// # Errors
///
/// A stringified error when the webview cannot be created.
#[tauri::command]
pub fn map_open(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(MAP_LABEL) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, MAP_LABEL, WebviewUrl::App("map.html".into()))
        .title("Q-Mag — Map")
        .inner_size(1280.0, 860.0)
        .min_inner_size(640.0, 400.0)
        .build()
        .map_err(|e| e.to_string())?;
    // Closing the map window re-embeds: tell the main window (and any other)
    // that the map is back, and never touch the lifecycle (only the main
    // window owns shutdown).
    let app_for_events = app.clone();
    win.on_window_event(move |event| {
        // Only Destroyed: CloseRequested fires while the window still exists,
        // which would make the main window re-embed too early.
        if let tauri::WindowEvent::Destroyed = event {
            let _ = app_for_events.emit("map_window", "closed");
        }
    });
    let _ = app.emit("map_window", "opened");
    Ok(())
}

/// Close the map window (the "Bring map back" action).
///
/// # Errors
///
/// Never — a missing window is a no-op.
#[tauri::command]
pub fn map_close(app: AppHandle) -> Result<bool, String> {
    match app.get_webview_window(MAP_LABEL) {
        Some(win) => {
            win.close().map_err(|e| e.to_string())?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Whether the detachable map window is open (the main window's layout gate).
#[tauri::command]
pub fn map_status(app: AppHandle) -> bool {
    app.get_webview_window(MAP_LABEL).is_some()
}

/// Close the map window as part of the main window's teardown (U15 + L4):
/// called from the lifecycle shutdown path so a detached map never outlives
/// the app. Best-effort — the process exit takes it down anyway. Generic
/// over the runtime so `lifecycle_service::run<R>` can call it.
pub fn close_map_on_shutdown<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window(MAP_LABEL) {
        let _ = win.close();
    }
}
