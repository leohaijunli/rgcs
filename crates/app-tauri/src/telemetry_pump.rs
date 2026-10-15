//! Thin adapter: forwards `core::telemetry::hub::TelemetryHub` watch channels
//! onto the Tauri event bus for the React UI.
//!
//! - `"telemetry"` — throttled `TelemetrySnapshot` (20 Hz, from the hub).
//! - `"link"` — `LinkStatus` on change.
//! - `"link_error"` — `TelemetryError` (kind + message + timestamp), lossless.
//! - `"telemetry_dropped"` — cumulative inbound frames dropped by the hub.
//!
//! Link failures also go into the alert center (A3), so the badge and the
//! banner stack read the same state.

use maggcs_core::alerts::{AlertInput, AlertSource, Severity};
use maggcs_core::telemetry::hub::TelemetryHub;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// Run the hub-forwarding loop until the hub shuts down.
pub async fn run(app: AppHandle, hub: TelemetryHub) {
    let mut snap_rx = hub.subscribe_snapshot();
    let mut link_rx = hub.subscribe_link();
    let mut err_rx = hub.subscribe_error();
    let mut dropped_rx = hub.subscribe_dropped_frames();

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
            error = err_rx.recv() => {
                match error {
                    Ok(err) => {
                        // Same source of truth as the banner: the alert
                        // center owns dedup and the badge (A3). L3 promotes a
                        // full heartbeat loss to a latched Critical banner on
                        // top of this.
                        app.state::<AppState>().alerts().raise(
                            AlertInput::new(AlertSource::Link, Severity::Warn, err.message.clone()),
                        );
                        let _ = app.emit("link_error", err);
                    }
                    // The pump fell behind the error channel; the hub already
                    // counts such gaps separately, so keep draining.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
            changed = dropped_rx.changed() => {
                if changed.is_ok() {
                    let _ = app.emit("telemetry_dropped", *dropped_rx.borrow());
                } else {
                    return;
                }
            }
        }
    }
}
