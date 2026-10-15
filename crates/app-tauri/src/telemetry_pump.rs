//! Thin adapter: forwards `core::telemetry::hub::TelemetryHub` watch channels
//! onto the Tauri event bus for the React UI.
//!
//! - `"telemetry"` — throttled `TelemetrySnapshot` (20 Hz, from the hub).
//! - `"link"` — `LinkStatus` on change.
//! - `"link_error"` — `TelemetryError` (kind + message + timestamp), lossless.
//! - `"telemetry_dropped"` — cumulative inbound frames dropped by the hub.
//! - `"statustext"` — every assembled `STATUSTEXT` (L1): the FC's own words,
//!   with the command it explains when it answers a rejection.
//!
//! Link failures and alarming status texts also go into the alert center
//! (A3/L1), so the badge and the banner stack read the same state.

use maggcs_core::alerts::{AlertInput, AlertSource, Severity};
use maggcs_core::telemetry::hub::TelemetryHub;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// Run the hub-forwarding loop until the hub shuts down.
pub async fn run(app: AppHandle, hub: TelemetryHub) {
    let mut snap_rx = hub.subscribe_snapshot();
    let mut link_rx = hub.subscribe_link();
    let mut err_rx = hub.subscribe_error();
    let mut status_rx = hub.subscribe_status_text();
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
            text = status_rx.recv() => {
                match text {
                    Ok(text) => {
                        // L1: the FC's own words. Anything it calls a problem
                        // (Emergency…Warning) becomes an alert, so dedup,
                        // latching and the badge stay in one place; the whole
                        // stream lands in the event log via the same event.
                        // A rejected command's reason is never silent — that
                        // is the one case the operator must not miss.
                        if text.severity.is_alarming() || text.command.is_some() {
                            let body = match &text.command {
                                Some(command) => format!("{command} rejected: {}", text.text),
                                None => text.text.clone(),
                            };
                            app.state::<AppState>().alerts().raise(AlertInput::new(
                                AlertSource::Statustext,
                                text.severity.alert_severity(),
                                body,
                            ));
                        }
                        let _ = app.emit("statustext", &text);
                    }
                    // A webview that fell behind re-reads what it needs; the
                    // FC's log is not worth stalling the pump for.
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
