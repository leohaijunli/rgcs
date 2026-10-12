//! RTK injection bridge (R4): owns the core [`RtkServiceHandle`] for the
//! primary link, forwards the 1 Hz stats to the `"rtk_stats"` event, and
//! exposes the start/stop commands. The service is spawned per link (it
//! stops with the link, like the command service); the QGC-style drawer UI
//! lives in the main window frontend.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, State};

use maggcs_core::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::rtk::multi::{ForeignInjector, ForeignState, FOREIGN_WINDOW};
use maggcs_core::rtk::service::{RtkServiceHandle, RtkStats};

use crate::state::AppState;

/// Cached latest stats, so a freshly opened drawer shows something
/// immediately instead of waiting for the next 1 Hz tick.
#[derive(Default)]
pub struct RtkState {
    /// Handle of the primary link's injection service.
    handle: StdMutex<Option<RtkServiceHandle>>,
    last: StdMutex<Option<RtkStats>>,
    /// Foreign-injector snapshot (N4), updated on change.
    foreign: StdMutex<Option<ForeignState>>,
    /// Shutdown switch of the foreign observer task: dropping it (on
    /// `detach`) ends the task, which otherwise holds a `ConnectionHandle`
    /// and would keep the event bus — and itself — alive forever (M1).
    foreign_stop: StdMutex<Option<tokio::sync::watch::Sender<bool>>>,
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

    /// Drop the previous link's service (link swap or disconnect): ends the
    /// stats task (channel close), the injection service (last command
    /// handle dropped) and the foreign observer (`foreign_stop` dropped).
    pub fn detach(&self) {
        *self.handle.lock().expect("rtk handle lock poisoned") = None;
        *self.last.lock().expect("rtk stats lock poisoned") = None;
        *self.foreign.lock().expect("rtk foreign lock poisoned") = None;
        *self
            .foreign_stop
            .lock()
            .expect("rtk foreign stop lock poisoned") = None;
    }

    /// Observe inbound `GPS_RTCM_DATA` for the multi-injector warning
    /// (N4 / ADR-008): any packet on the bus came from another injector,
    /// since our own injections never echo back. Emits `rtk_foreign` on
    /// every state change and caches it for the drawer's first paint.
    ///
    /// The task outlives the event bus it subscribes to (it holds a
    /// `ConnectionHandle`, which keeps the broadcast channel open), so it
    /// exits through the `foreign_stop` switch installed here and dropped
    /// by [`RtkState::detach`] (M1).
    pub fn attach_foreign(self: Arc<Self>, app: AppHandle, conn: ConnectionHandle) {
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        *self
            .foreign_stop
            .lock()
            .expect("rtk foreign stop lock poisoned") = Some(stop_tx);
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            use ::mavlink::MessageData;
            let mut events = conn.subscribe_route(MessageRoute::messages(&[
                ::mavlink::common::GPS_RTCM_DATA_DATA::ID,
            ]));
            let mut injector = ForeignInjector::new();
            let mut last = ForeignState::default();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    // detach(): sender dropped or link swapped (M1).
                    _ = stop_rx.changed() => return,
                    ev = events.recv() => match ev {
                        Ok(ConnectionEvent::Message(_)) => injector.observe(Instant::now()),
                        Ok(_) => {} // lifecycle events pass the route unchanged
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    },
                    _ = tick.tick() => {} // age out FOREIGN_WINDOW without new packets
                }
                let now = Instant::now();
                let current = ForeignState {
                    active: injector.active(now, FOREIGN_WINDOW),
                    packets: injector.packets(),
                };
                if current == last {
                    continue;
                }
                last = current;
                *state.foreign.lock().expect("rtk foreign lock poisoned") = Some(current);
                if app.emit("rtk_foreign", current).is_err() {
                    return; // UI gone
                }
            }
        });
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

/// Last foreign-injector snapshot (N4): seeds the drawer before the first
/// `rtk_foreign` event.
#[tauri::command]
pub fn rtk_foreign_state(state: State<'_, AppState>) -> Option<ForeignState> {
    *state.rtk.foreign.lock().expect("rtk foreign lock poisoned")
}
