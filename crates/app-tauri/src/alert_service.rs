//! Alert center adapter (plan §3 A3): the process owns one
//! `core::alerts::AlertCenter`, every transition is fanned out on a broadcast
//! channel, and one pump forwards it to every webview as the single `"alert"`
//! Tauri event.
//!
//! Rust-side sources (link errors today; STATUSTEXT, battery, GPS/RTK, DEM,
//! VIC in L1/L3/L7/M2/M7) call [`AlertState::raise`] — L7 adds the
//! `AlertCenter::level` wrapper for threshold alerts. The UI uses the
//! `alert_raise` command for failures only the frontend can see, and
//! `alert_ack` / `alert_ack_all` for the operator's acknowledgement.
//! Everything funnels through the same center, so dedup, latching,
//! sound-worthy transitions and the badge count have exactly one
//! implementation.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use maggcs_core::alerts::{Alert, AlertCenter, AlertEvent, AlertInput};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::broadcast;

use crate::state::AppState;

/// Bounded fan-out: a stalled webview must never block a source thread. The
/// receiver skips missed events and re-reads `alert_snapshot` if it lagged.
const ALERT_CHANNEL_CAPACITY: usize = 256;

/// Epoch milliseconds. The core center never reads a clock — every transition
/// is stamped here, at the process boundary that owns wall time.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// The process-wide alert center plus its event fan-out.
pub struct AlertState {
    center: Mutex<AlertCenter>,
    events: broadcast::Sender<AlertEvent>,
}

impl Default for AlertState {
    fn default() -> Self {
        let (events, _) = broadcast::channel(ALERT_CHANNEL_CAPACITY);
        Self {
            center: Mutex::new(AlertCenter::default()),
            events,
        }
    }
}

impl AlertState {
    /// Subscribe to alert transitions (the pump and the tests).
    pub fn subscribe(&self) -> broadcast::Receiver<AlertEvent> {
        self.events.subscribe()
    }

    /// Publish a transition to every subscriber.
    fn publish(&self, event: AlertEvent) -> AlertEvent {
        // No subscribers yet (no window open) is not an error: the center
        // keeps the state, and a late webview reads `alert_snapshot`.
        let _ = self.events.send(event.clone());
        event
    }

    fn publish_all(&self, events: Vec<AlertEvent>) -> Vec<AlertEvent> {
        for event in &events {
            let _ = self.events.send(event.clone());
        }
        events
    }

    /// Raise (or repeat) a one-shot alert.
    pub fn raise(&self, input: AlertInput) -> AlertEvent {
        let event = self.center.lock().raise(input, now_ms());
        self.publish(event)
    }

    /// Acknowledge one alert (the operator saw it).
    pub fn acknowledge(&self, id: &str) -> Option<AlertEvent> {
        let event = self.center.lock().acknowledge(id)?;
        Some(self.publish(event))
    }

    /// Acknowledge everything visible.
    pub fn acknowledge_all(&self) -> Vec<AlertEvent> {
        let events = self.center.lock().acknowledge_all();
        self.publish_all(events)
    }

    /// Drop acknowledged alerts from the history; returns how many went.
    pub fn clear_acknowledged(&self) -> u32 {
        self.center.lock().clear_acknowledged() as u32
    }

    /// Every retained alert, newest first.
    pub fn snapshot(&self) -> Vec<Alert> {
        self.center.lock().snapshot()
    }
}

/// Forward every alert transition to all webviews (`"alert"`).
pub async fn pump(app: AppHandle, alerts: Arc<AlertState>) {
    let mut rx = alerts.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                let _ = app.emit("alert", event);
            }
            // A window that fell behind re-reads `alert_snapshot` on its next
            // boot; keep draining rather than tearing the pump down.
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Everything the UI shows on boot (badge, banner, drawer).
#[tauri::command]
pub fn alert_snapshot(state: State<'_, AppState>) -> Vec<Alert> {
    state.alerts().snapshot()
}

/// Raise an alert from the frontend — for failures only the UI can see
/// (a `.plan` that will not parse, a drape that failed, a window that will
/// not open). Rust-side sources must call `AlertState::raise` directly.
#[tauri::command]
pub fn alert_raise(state: State<'_, AppState>, input: AlertInput) -> Alert {
    state.alerts().raise(input).alert
}

/// The operator acknowledged one alert.
#[tauri::command]
pub fn alert_ack(state: State<'_, AppState>, id: String) {
    state.alerts().acknowledge(&id);
}

/// The operator acknowledged everything visible.
#[tauri::command]
pub fn alert_ack_all(state: State<'_, AppState>) {
    state.alerts().acknowledge_all();
}

/// Drop acknowledged alerts from the history (drawer "clear").
#[tauri::command]
pub fn alert_clear_acknowledged(state: State<'_, AppState>) -> u32 {
    state.alerts().clear_acknowledged()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maggcs_core::alerts::{AlertEventKind, AlertSource, Severity};

    fn warn(text: &str) -> AlertInput {
        AlertInput::new(AlertSource::Link, Severity::Warn, text)
    }

    #[test]
    fn raise_fans_out_and_dedups_in_one_place() {
        let state = AlertState::default();
        let mut rx = state.subscribe();

        let first = state.raise(warn("Serial write failed"));
        let second = state.raise(warn("Serial write failed"));

        assert_eq!(first.kind, AlertEventKind::Raised);
        assert_eq!(second.kind, AlertEventKind::Repeated);
        assert_eq!(second.alert.count, 2);

        let a = rx.try_recv().expect("first transition");
        let b = rx.try_recv().expect("second transition");
        assert_eq!(a.kind, AlertEventKind::Raised);
        assert_eq!(b.kind, AlertEventKind::Repeated);
        assert_eq!(state.snapshot().len(), 1);
    }

    #[test]
    fn acknowledge_publishes_the_transition_and_clears_the_badge() {
        let state = AlertState::default();
        let mut rx = state.subscribe();
        let raised = state.raise(AlertInput::new(
            AlertSource::Gps,
            Severity::Critical,
            "GPS lost",
        ));
        let _ = rx.try_recv();

        let acked = state
            .acknowledge(&raised.alert.id)
            .expect("the alert is known");
        assert_eq!(acked.kind, AlertEventKind::Acknowledged);
        assert!(acked.alert.acked);

        let seen = rx.try_recv().expect("ack transition");
        assert_eq!(seen.kind, AlertEventKind::Acknowledged);
        assert!(state.snapshot()[0].acked);
        assert!(state.acknowledge_all().is_empty(), "nothing left to ack");
    }

    #[test]
    fn timestamps_come_from_the_process_clock() {
        let state = AlertState::default();
        let before = now_ms();
        let raised = state.raise(warn("late alert"));
        let after = now_ms();

        assert!(raised.alert.first_ts_ms >= before);
        assert!(raised.alert.last_ts_ms <= after);
    }

    #[test]
    fn clear_acknowledged_drops_history_but_keeps_open_alerts() {
        let state = AlertState::default();
        let a = state.raise(warn("ack me"));
        state.raise(warn("keep me"));
        state.acknowledge(&a.alert.id);

        assert_eq!(state.clear_acknowledged(), 1);
        let remaining = state.snapshot();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].text, "keep me");
    }
}
