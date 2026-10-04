//! Thin adapter: forwards `core::telemetry::hub::TelemetryHub` watch channels
//! onto the Tauri event bus for the React UI.
//!
//! - `"telemetry"` — throttled `TelemetrySnapshot` (20 Hz, from the hub).
//! - `"link"` — `LinkStatus` on change.
//! - `"link_error"` — transient failure message.

use maggcs_core::telemetry::hub::TelemetryHub;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// Run the hub-forwarding loop until the hub shuts down.
pub async fn run(app: AppHandle, hub: TelemetryHub) {
    let mut snap_rx = hub.subscribe_snapshot();
    let mut link_rx = hub.subscribe_link();
    let mut err_rx = hub.subscribe_error();

    loop {
        tokio::select! {
            changed = snap_rx.changed() => {
                if changed.is_ok() {
                    let _ = app.emit("telemetry", snap_rx.borrow().clone());
                } else {
                    return;
                }
            }
            changed = link_rx.changed() => {
                if changed.is_ok() {
                    let status = link_rx.borrow().clone();
                    app.state::<AppState>().publish_link(status.clone());
                    let _ = app.emit("link", status);
                } else {
                    return;
                }
            }
            changed = err_rx.changed() => {
                if changed.is_ok() {
                    if let Some(msg) = err_rx.borrow().clone() {
                        let _ = app.emit("link_error", msg);
                    }
                } else {
                    return;
                }
            }
        }
    }
}
