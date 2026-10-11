//! RTK injection bridge (R4): owns the core [`RtkServiceHandle`] for the
//! primary link, forwards the 1 Hz stats to the `"rtk_stats"` event, and
//! exposes the start/stop commands. The service is spawned per link (it
//! stops with the link, like the command service); the QGC-style drawer UI
//! lives in the main window frontend.

use std::sync::{Arc, Mutex as StdMutex};

use tauri::{AppHandle, Emitter, State};

use maggcs_core::rtk::service::{RtkServiceHandle, RtkStats};

use crate::state::AppState;

/// Cached latest stats, so a freshly opened drawer shows something
/// immediately instead of waiting for the next 1 Hz tick.
#[derive(Default)]
pub struct RtkState {
    /// Handle of the primary link's injection service.
    handle: StdMutex<Option<RtkServiceHandle>>,
    last: StdMutex<Option<RtkStats>>,
}

impl RtkState {
    /// Bind the service of a freshly connected link and forward its stats
    /// onto the `"rtk_stats"` event.
    pub fn attach(
        self: Arc<Self>,
        app: AppHandle,
        handle: RtkServiceHandle,
        mut stats_rx: tokio::sync::mpsc::Receiver<RtkStats>,
    ) {
        *self.handle.lock().expect("rtk handle lock poisoned") = Some(handle.clone());
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(stats) = stats_rx.recv().await {
                *state.last.lock().expect("rtk stats lock poisoned") = Some(stats.clone());
                if app.emit("rtk_stats", &stats).is_err() {
                    return; // UI gone
                }
            }
        });
    }

    /// Drop the previous link's service (link swap).
    pub fn detach(&self) {
        *self.handle.lock().expect("rtk handle lock poisoned") = None;
        *self.last.lock().expect("rtk stats lock poisoned") = None;
    }

    fn service(&self) -> Option<RtkServiceHandle> {
        self.handle
            .lock()
            .expect("rtk handle lock poisoned")
            .clone()
    }
}

/// Start injecting RTCM from the F9P serial port (single active source —
/// starting again stops the previous source first, in the core service).
#[tauri::command]
pub async fn rtk_start_serial(
    port_name: String,
    baud_rate: u32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let handle = state.rtk.service().ok_or("not connected")?;
    handle
        .start_serial(maggcs_core::rtk::source::SerialSourceConfig {
            port_name,
            baud_rate,
        })
        .await
        .map_err(|e| e.to_string())
}

/// Stop the active injection source (idempotent).
#[tauri::command]
pub async fn rtk_stop(state: State<'_, AppState>) -> Result<(), String> {
    let handle = state.rtk.service().ok_or("not connected")?;
    handle.stop().await;
    Ok(())
}

/// Last stats snapshot (a freshly opened drawer shows it immediately).
#[tauri::command]
pub fn rtk_last_stats(state: State<'_, AppState>) -> Option<RtkStats> {
    state
        .rtk
        .last
        .lock()
        .expect("rtk stats lock poisoned")
        .clone()
}
