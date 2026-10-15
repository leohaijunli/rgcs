//! Stream health for the walk instruments (W1).
//!
//! Watches the mag and GNSS sample streams for **staleness**, **rate out of
//! range**, **GNSS fix changes** and **clock jumps**, emitting edge-triggered
//! [`StreamEvent`]s. Events map onto the alert center (A3) via
//! [`StreamEvent::to_alert`].
//!
//! Thresholds are *operational* stream-health values (not survey/QC
//! measurement thresholds): provided by [`MonitorConfig`], sourced from the app
//! config (A6), with conservative defaults here.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::alerts::{AlertInput, AlertSource, Severity};
use crate::telemetry::GpsFixType;

use super::sample::{InstrumentKind, InstrumentSample};

/// Operational stream-health thresholds (A6-overridable).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MonitorConfig {
    /// Below this mag rate the stream is considered degraded.
    pub mag_min_rate_hz: f64,
    /// Above this mag rate the stream is considered degraded.
    pub mag_max_rate_hz: f64,
    /// No mag sample for this long ⇒ stale.
    pub mag_stale_after_ms: f64,
    /// No GNSS sample for this long ⇒ stale.
    pub gnss_stale_after_ms: f64,
    /// Sliding window for rate estimation.
    pub rate_window_ms: f64,
    /// Sample-time rollback/jump larger than this ⇒ [`StreamEvent::TimeJump`].
    pub time_jump_threshold_ms: f64,
}

/// Expected `HIGHRES_IMU` magnetometer rate. Operator-confirmed ≈100 Hz
/// (ADR-020); the default health band brackets it at half/double.
pub const EXPECTED_MAG_RATE_HZ: f64 = 100.0;

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            mag_min_rate_hz: EXPECTED_MAG_RATE_HZ * 0.5,
            mag_max_rate_hz: EXPECTED_MAG_RATE_HZ * 2.0,
            mag_stale_after_ms: 500.0,
            gnss_stale_after_ms: 2_000.0,
            rate_window_ms: 2_000.0,
            time_jump_threshold_ms: 2_000.0,
        }
    }
}

/// Coarse health for a status badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StreamHealth {
    /// Nothing observed yet.
    Unknown,
    /// Both streams healthy.
    Ok,
    /// Magnetometer stream stale.
    MagStale,
    /// Magnetometer rate outside the configured band.
    MagRate,
    /// GNSS stream stale.
    GnssStale,
    /// GNSS fix degraded below its best recent value.
    GnssDegraded,
    /// An instrument clock jump was detected.
    ClockJump,
}

/// A stream-health transition (edge-triggered).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub enum StreamEvent {
    MagStale { age_ms: f64 },
    MagRecovered,
    MagRateOutOfRange { rate_hz: f64 },
    GnssStale { age_ms: f64 },
    GnssRecovered,
    GnssFixChanged { from: GpsFixType, to: GpsFixType },
    TimeJump { delta_ms: f64 },
}

impl StreamEvent {
    /// Severity this event should raise if forwarded to the alert center.
    pub fn severity(&self) -> Severity {
        match self {
            Self::MagStale { .. } | Self::MagRateOutOfRange { .. } | Self::GnssStale { .. } => {
                Severity::Warn
            }
            Self::TimeJump { .. } => Severity::Warn,
            Self::GnssFixChanged { from, to } => {
                if fix_rank(*to) < fix_rank(*from) {
                    Severity::Warn
                } else {
                    Severity::Info
                }
            }
            Self::MagRecovered | Self::GnssRecovered => Severity::Info,
        }
    }

    /// Map to an alert-center input (A3) with a stable dedup key.
    pub fn to_alert(&self) -> AlertInput {
        let source = match self {
            Self::MagStale { .. } | Self::MagRecovered | Self::MagRateOutOfRange { .. } => {
                AlertSource::Mag
            }
            Self::GnssStale { .. } | Self::GnssRecovered | Self::GnssFixChanged { .. } => {
                AlertSource::Gps
            }
            Self::TimeJump { .. } => AlertSource::System,
        };
        let (text, key) = match self {
            Self::MagStale { age_ms } => (
                format!("Magnetometer stream stale ({:.1} s)", age_ms / 1000.0),
                "mag-stale",
            ),
            Self::MagRecovered => ("Magnetometer stream recovered".to_string(), "mag-stale"),
            Self::MagRateOutOfRange { rate_hz } => (
                format!("Magnetometer rate {rate_hz:.0} Hz out of range"),
                "mag-rate",
            ),
            Self::GnssStale { age_ms } => (
                format!("GNSS stream stale ({:.1} s)", age_ms / 1000.0),
                "gnss-stale",
            ),
            Self::GnssRecovered => ("GNSS stream recovered".to_string(), "gnss-stale"),
            Self::GnssFixChanged { from, to } => {
                (format!("GNSS fix {from:?} → {to:?}"), "gnss-fix")
            }
            Self::TimeJump { delta_ms } => (
                format!("Instrument clock jump {delta_ms:.0} ms"),
                "clock-jump",
            ),
        };
        AlertInput {
            source,
            severity: self.severity(),
            text,
            key: Some(key.to_string()),
        }
    }
}

/// Monitors instrument streams and emits edge-triggered health events.
#[derive(Debug, Clone)]
pub struct StreamMonitor {
    config: MonitorConfig,
    mag: StreamState,
    gnss: StreamState,
    clock_jump: bool,
}

#[derive(Debug, Clone, Default)]
struct StreamState {
    last_host_ms: Option<f64>,
    #[allow(dead_code)]
    last_t_ms: Option<f64>,
    arrivals: VecDeque<f64>,
    stale: bool,
    rate_bad: bool,
    peak_fix_rank: Option<i32>,
    last_fix: Option<GpsFixType>,
    degraded: bool,
}

impl StreamMonitor {
    /// Build a monitor with the given thresholds.
    pub fn new(config: MonitorConfig) -> Self {
        Self {
            config,
            mag: StreamState::default(),
            gnss: StreamState::default(),
            clock_jump: false,
        }
    }

    /// The configured thresholds.
    pub fn config(&self) -> MonitorConfig {
        self.config
    }

    /// Coarse health for a UI badge.
    pub fn health(&self) -> StreamHealth {
        if self.clock_jump {
            return StreamHealth::ClockJump;
        }
        if self.mag.stale {
            return StreamHealth::MagStale;
        }
        if self.mag.rate_bad {
            return StreamHealth::MagRate;
        }
        if self.gnss.stale {
            return StreamHealth::GnssStale;
        }
        if self.gnss.degraded {
            return StreamHealth::GnssDegraded;
        }
        if self.mag.last_host_ms.is_some() || self.gnss.last_host_ms.is_some() {
            StreamHealth::Ok
        } else {
            StreamHealth::Unknown
        }
    }

    /// Feed a decoded sample at host time `now_ms`.
    pub fn observe(&mut self, sample: &InstrumentSample, now_ms: f64) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        match sample {
            InstrumentSample::Mag(m) => {
                let state = &mut self.mag;
                if state.stale {
                    state.stale = false;
                    events.push(StreamEvent::MagRecovered);
                }
                if let Some(delta) = state.time_jump(m.t_ms, self.config.time_jump_threshold_ms) {
                    self.clock_jump = true;
                    events.push(StreamEvent::TimeJump { delta_ms: delta });
                }
                state.record(now_ms, m.t_ms, self.config.rate_window_ms);
                if let Some(rate) = state.rate(self.config.rate_window_ms) {
                    let bad =
                        rate < self.config.mag_min_rate_hz || rate > self.config.mag_max_rate_hz;
                    if bad && !state.rate_bad {
                        state.rate_bad = true;
                        events.push(StreamEvent::MagRateOutOfRange { rate_hz: rate });
                    } else if !bad && state.rate_bad {
                        state.rate_bad = false;
                    }
                }
            }
            InstrumentSample::Gnss(g) => {
                let state = &mut self.gnss;
                if state.stale {
                    state.stale = false;
                    events.push(StreamEvent::GnssRecovered);
                }
                if let Some(delta) = state.time_jump(g.t_ms, self.config.time_jump_threshold_ms) {
                    self.clock_jump = true;
                    events.push(StreamEvent::TimeJump { delta_ms: delta });
                }
                state.record(now_ms, g.t_ms, self.config.rate_window_ms);
                let rank = fix_rank(g.fix);
                let peak = state.peak_fix_rank.map_or(rank, |p| p.max(rank));
                state.peak_fix_rank = Some(peak);
                state.degraded = rank < peak;
                if let Some(from) = state.last_fix {
                    if from != g.fix {
                        events.push(StreamEvent::GnssFixChanged { from, to: g.fix });
                    }
                }
                state.last_fix = Some(g.fix);
            }
        }
        events
    }

    /// Call periodically to detect staleness without new samples.
    pub fn poll(&mut self, now_ms: f64) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if let Some(last) = self.mag.last_host_ms {
            let age = now_ms - last;
            if age > self.config.mag_stale_after_ms && !self.mag.stale {
                self.mag.stale = true;
                events.push(StreamEvent::MagStale { age_ms: age });
            }
        }
        if let Some(last) = self.gnss.last_host_ms {
            let age = now_ms - last;
            if age > self.config.gnss_stale_after_ms && !self.gnss.stale {
                self.gnss.stale = true;
                events.push(StreamEvent::GnssStale { age_ms: age });
            }
        }
        events
    }
}

impl StreamState {
    fn record(&mut self, host_ms: f64, t_ms: f64, window_ms: f64) {
        self.last_host_ms = Some(host_ms);
        self.last_t_ms = Some(t_ms);
        self.arrivals.push_back(host_ms);
        let cutoff = host_ms - window_ms;
        while self.arrivals.front().is_some_and(|&f| f < cutoff) {
            self.arrivals.pop_front();
        }
    }

    fn rate(&self, window_ms: f64) -> Option<f64> {
        let first = *self.arrivals.front()?;
        let last = *self.arrivals.back()?;
        let span_ms = last - first;
        if span_ms < window_ms * 0.5 {
            return None;
        }
        Some(self.arrivals.len() as f64 / (span_ms / 1000.0))
    }

    fn time_jump(&self, t_ms: f64, threshold_ms: f64) -> Option<f64> {
        let previous = self.last_t_ms?;
        let delta = t_ms - previous;
        if delta.abs() > threshold_ms {
            Some(delta)
        } else {
            None
        }
    }
}

/// Rank a fix for "better/worse" comparisons (higher = better).
fn fix_rank(fix: GpsFixType) -> i32 {
    match fix {
        GpsFixType::NoGps => 0,
        GpsFixType::NoFix => 1,
        GpsFixType::Fix2d => 2,
        GpsFixType::Fix3d => 3,
        GpsFixType::Dgps => 4,
        GpsFixType::RtkFloat => 5,
        GpsFixType::RtkFixed => 6,
        GpsFixType::Static => 3,
        GpsFixType::Ppp => 5,
    }
}

/// Which instrument a stale/rate event belongs to (for callers).
pub fn event_kind(event: &StreamEvent) -> InstrumentKind {
    match event {
        StreamEvent::MagStale { .. }
        | StreamEvent::MagRecovered
        | StreamEvent::MagRateOutOfRange { .. } => InstrumentKind::Magnetometer,
        StreamEvent::GnssStale { .. }
        | StreamEvent::GnssRecovered
        | StreamEvent::GnssFixChanged { .. } => InstrumentKind::Gnss,
        StreamEvent::TimeJump { .. } => InstrumentKind::Gnss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::{Height, HeightDatum};
    use crate::instruments::sample::{GnssSample, MagSample};

    fn mag(t_ms: f64) -> InstrumentSample {
        InstrumentSample::Mag(MagSample {
            t_ms,
            instance: Default::default(),
            x_nt: 0.0,
            y_nt: 0.0,
            z_nt: 0.0,
            total_nt: 0.0,
        })
    }

    fn gnss(t_ms: f64, fix: GpsFixType) -> InstrumentSample {
        InstrumentSample::Gnss(GnssSample {
            t_ms,
            fix,
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            altitude: Height::new(HeightDatum::AmslEgm96, 0.0),
            hdop: 0.0,
            satellites_visible: 0,
            velocity_m_s: 0.0,
            course_over_ground_deg: 0.0,
        })
    }

    #[test]
    fn mag_staleness_is_edge_triggered() {
        let mut m = StreamMonitor::new(MonitorConfig {
            mag_stale_after_ms: 500.0,
            ..MonitorConfig::default()
        });
        m.observe(&mag(0.0), 0.0);
        assert!(m.poll(300.0).is_empty(), "not stale yet");
        let events = m.poll(700.0);
        assert_eq!(events, vec![StreamEvent::MagStale { age_ms: 700.0 }]);
        assert_eq!(m.health(), StreamHealth::MagStale);
        // Still stale: no repeat.
        assert!(m.poll(900.0).is_empty());
        // A new sample recovers.
        let events = m.observe(&mag(700.0), 700.0);
        assert_eq!(events, vec![StreamEvent::MagRecovered]);
    }

    #[test]
    fn mag_rate_out_of_range_is_detected() {
        let mut m = StreamMonitor::new(MonitorConfig {
            mag_min_rate_hz: 50.0,
            mag_max_rate_hz: 200.0,
            rate_window_ms: 1_000.0,
            ..MonitorConfig::default()
        });
        // 5 Hz mag for 1.5 s → below the 50 Hz floor.
        let mut events = Vec::new();
        for i in 0..8 {
            let t = (i as f64) * 200.0;
            events.extend(m.observe(&mag(t), t));
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::MagRateOutOfRange { .. })),
            "expected a rate event, got {events:?}"
        );
        assert_eq!(m.health(), StreamHealth::MagRate);
    }

    #[test]
    fn gnss_fix_downgrade_emits_event_and_health() {
        let mut m = StreamMonitor::new(MonitorConfig::default());
        m.observe(&gnss(0.0, GpsFixType::RtkFixed), 0.0);
        let events = m.observe(&gnss(200.0, GpsFixType::RtkFloat), 200.0);
        assert_eq!(
            events,
            vec![StreamEvent::GnssFixChanged {
                from: GpsFixType::RtkFixed,
                to: GpsFixType::RtkFloat
            }]
        );
        assert_eq!(events[0].severity(), Severity::Warn);
        assert_eq!(m.health(), StreamHealth::GnssDegraded);
    }

    #[test]
    fn clock_jump_is_detected() {
        let mut m = StreamMonitor::new(MonitorConfig {
            time_jump_threshold_ms: 2_000.0,
            ..MonitorConfig::default()
        });
        m.observe(&mag(10_000.0), 0.0);
        let events = m.observe(&mag(500.0), 20.0); // FC reboot: time went backwards
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::TimeJump { .. })),
            "expected a jump, got {events:?}"
        );
        assert_eq!(m.health(), StreamHealth::ClockJump);
    }

    #[test]
    fn healthy_stream_reports_ok() {
        let mut m = StreamMonitor::new(MonitorConfig::default());
        for i in 0..30 {
            let t = (i as f64) * 10.0;
            m.observe(&mag(t), t);
        }
        m.observe(&gnss(0.0, GpsFixType::RtkFixed), 0.0);
        assert_eq!(m.health(), StreamHealth::Ok);
    }

    #[test]
    fn alert_mapping_uses_stable_keys() {
        let e = StreamEvent::MagStale { age_ms: 1234.0 };
        let a = e.to_alert();
        assert_eq!(a.source, AlertSource::Mag);
        assert_eq!(a.severity, Severity::Warn);
        assert_eq!(a.key.as_deref(), Some("mag-stale"));
    }
}
