//! Multi-injector detection (ADR-008 forwarding rules, issue N4).
//!
//! PX4 relays inbound `GPS_RTCM_DATA` from one MAVLink channel to the others
//! (`MavlinkReceiver` → `Mavlink::handle_message` → `forward_message` when
//! forwarding is on), while our own injections leave on this channel and never
//! echo back to us. Any `GPS_RTCM_DATA` we therefore *receive* was sent by
//! somebody else — e.g. QGC on its own channel — meaning two injectors are
//! feeding the same FC.

use std::time::{Duration, Instant};

/// A foreign packet keeps the warning up this long; mirrors the injection
/// stall threshold (PX4 switches GPS links after 5 s without valid RTCM).
pub const FOREIGN_WINDOW: Duration = Duration::from_secs(5);

/// Rolling detector for RTCM injections that did not come from us.
#[derive(Debug, Default, Clone)]
pub struct ForeignInjector {
    /// Foreign packets since the link started.
    packets: u64,
    /// First foreign packet (diagnostic aid).
    first_seen: Option<Instant>,
    /// Most recent foreign packet.
    last_seen: Option<Instant>,
}

impl ForeignInjector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one received `GPS_RTCM_DATA` packet at `now`.
    pub fn observe(&mut self, now: Instant) {
        self.packets += 1;
        if self.first_seen.is_none() {
            self.first_seen = Some(now);
        }
        self.last_seen = Some(now);
    }

    /// True when a foreign packet arrived within `window` of `now`.
    pub fn active(&self, now: Instant, window: Duration) -> bool {
        self.last_seen
            .is_some_and(|t| now.duration_since(t) <= window)
    }

    /// Foreign packets received since the link started.
    pub fn packets(&self) -> u64 {
        self.packets
    }

    /// Seconds since the last foreign packet (`None` = none ever received).
    pub fn seconds_since_last(&self, now: Instant) -> Option<f64> {
        self.last_seen.map(|t| now.duration_since(t).as_secs_f64())
    }

    /// Forget everything (link swap / disconnect).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// What the UI sees: a snapshot of [`ForeignInjector`] emitted on change
/// (snake_case like [`super::service::RtkStats`]).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS,
)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct ForeignState {
    /// A foreign injector was seen within [`FOREIGN_WINDOW`].
    pub active: bool,
    /// Foreign packets since the link started.
    pub packets: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_counts_and_activates() {
        let base = Instant::now();
        let mut inj = ForeignInjector::new();
        assert!(!inj.active(base, FOREIGN_WINDOW));
        assert_eq!(inj.packets(), 0);
        inj.observe(base);
        inj.observe(base + Duration::from_secs(1));
        assert_eq!(inj.packets(), 2);
        assert!(inj.active(base + Duration::from_secs(1), FOREIGN_WINDOW));
    }

    #[test]
    fn warning_expires_after_window() {
        let base = Instant::now();
        let mut inj = ForeignInjector::new();
        inj.observe(base);
        // Inside the window (inclusive boundary): still warned.
        assert!(inj.active(base + FOREIGN_WINDOW, FOREIGN_WINDOW));
        // One second past it: quiet again, but the packet count survives.
        assert!(!inj.active(
            base + FOREIGN_WINDOW + Duration::from_secs(1),
            FOREIGN_WINDOW
        ));
        assert_eq!(inj.packets(), 1);
    }

    #[test]
    fn sustained_injection_keeps_warning_up() {
        let base = Instant::now();
        let mut inj = ForeignInjector::new();
        // A foreign injector trickling packets every 3 s never expires
        // inside a 5 s window.
        for step in 0..4 {
            inj.observe(base + Duration::from_secs(step * 3));
            assert!(inj.active(base + Duration::from_secs(step * 3), FOREIGN_WINDOW));
        }
        assert_eq!(inj.packets(), 4);
    }

    #[test]
    fn seconds_since_last_tracks_age() {
        let base = Instant::now();
        let mut inj = ForeignInjector::new();
        assert_eq!(inj.seconds_since_last(base), None);
        inj.observe(base);
        let age = inj
            .seconds_since_last(base + Duration::from_secs(2))
            .unwrap();
        assert!((age - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn reset_forgets_everything() {
        let base = Instant::now();
        let mut inj = ForeignInjector::new();
        inj.observe(base);
        inj.reset();
        assert_eq!(inj.packets(), 0);
        assert!(!inj.active(base, FOREIGN_WINDOW));
        assert_eq!(inj.seconds_since_last(base), None);
    }

    #[test]
    fn state_snapshot_roundtrips() {
        let state = ForeignState {
            active: true,
            packets: 7,
        };
        let json = serde_json::to_string(&state).expect("serialize");
        assert_eq!(json, r#"{"active":true,"packets":7}"#);
        let back: ForeignState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, state);
    }
}
