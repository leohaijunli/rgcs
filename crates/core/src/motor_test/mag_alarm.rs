//! Magnetic-field threshold alarm (plan §9.1, A4b).
//!
//! A pure state machine over the calibrated total field (`mag_total`, nT):
//!
//! ```text
//! Armed ──(smoothed > threshold for ≥ debounce)──▶ Alarm
//! Alarm ──(smoothed ≤ clear for any sample AND cooldown elapsed)──▶ Armed
//! ```
//!
//! False-trigger defenses (operator requirements 2026-10-10):
//! - the threshold is judged on an **EMA-smoothed** value, not the raw one
//! - a **debounce duration** must elapse above the threshold before firing
//! - re-arming needs the value back under the **hysteresis** clear level
//! - a **cooldown** after an alarm suppresses re-triggering even if the
//!   value crosses again immediately
//!
//! Pure logic: the caller feeds samples and pulls the state; no clock and no
//! I/O, so every branch is unit-testable.

/// Tunables (defaults are pre-M0 starting points, plan §9.1).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MagAlarmConfig {
    /// Threshold the smoothed total field must exceed, nT.
    pub threshold_nt: f64,
    /// The value must stay above the threshold this long before firing.
    pub debounce_s: f64,
    /// Re-arming clear level = threshold − hysteresis_nt.
    pub hysteresis_nt: f64,
    /// No new alarm within this long after the previous one.
    pub cooldown_s: f64,
    /// EMA smoothing factor for the raw total field (0 < α ≤ 1; 1 = no smoothing).
    pub ema_alpha: f64,
}

impl Default for MagAlarmConfig {
    fn default() -> Self {
        Self {
            threshold_nt: 55_000.0,
            debounce_s: 1.0,
            hysteresis_nt: 50.0,
            cooldown_s: 30.0,
            ema_alpha: 0.2,
        }
    }
}

/// Alarm lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagAlarmState {
    /// Watching: the smoothed value is at or under the threshold (or the
    /// cooldown after the last alarm has not fully elapsed).
    Armed,
    /// Fired: the audio + screen-flash alarm is active until the value
    /// clears (operator acknowledge also clears the audible part; the state
    /// machine only tracks the level).
    Alarm,
}

/// The watchdog. Feed one sample at a time; timestamps are seconds (monotonic
/// or epoch — only differences matter).
#[derive(Debug)]
pub struct MagAlarm {
    config: MagAlarmConfig,
    state: MagAlarmState,
    /// EMA of the raw total field.
    smoothed: Option<f64>,
    /// When the current above-threshold run started; `None` while at/below.
    above_since: Option<f64>,
    /// Epoch-style seconds of the last alarm *firing* (for cooldown).
    last_alarm_at: Option<f64>,
}

impl MagAlarm {
    pub fn new(config: MagAlarmConfig) -> Self {
        Self {
            config,
            state: MagAlarmState::Armed,
            smoothed: None,
            above_since: None,
            last_alarm_at: None,
        }
    }

    pub fn state(&self) -> MagAlarmState {
        self.state
    }

    /// The smoothed value driving the decision (for the UI readout).
    pub fn smoothed(&self) -> Option<f64> {
        self.smoothed
    }

    pub fn config(&self) -> &MagAlarmConfig {
        &self.config
    }

    /// Replace the tunables live (threshold changes re-arm the watchdog: the
    /// debounce run restarts against the new level).
    pub fn set_config(&mut self, config: MagAlarmConfig) {
        self.config = config;
        self.above_since = None;
    }

    /// Feed one total-field sample (nT) at time `t_s`. Returns `true` on the
    /// transition into `Alarm` (fire the audio + flash once).
    pub fn feed(&mut self, total_nt: f64, t_s: f64) -> bool {
        // EMA smooth: seed with the first sample.
        self.smoothed = Some(match self.smoothed {
            None => total_nt,
            Some(prev) => {
                let a = self.config.ema_alpha.clamp(0.0, 1.0);
                a * total_nt + (1.0 - a) * prev
            }
        });
        let value = self.smoothed.unwrap_or(total_nt);
        let clear_level = self.config.threshold_nt - self.config.hysteresis_nt;

        match self.state {
            MagAlarmState::Armed => {
                if value > self.config.threshold_nt {
                    let since = *self.above_since.get_or_insert(t_s);
                    if t_s - since >= self.config.debounce_s {
                        self.state = MagAlarmState::Alarm;
                        self.above_since = None;
                        self.last_alarm_at = Some(t_s);
                        return true; // fire once
                    }
                } else {
                    // Below the threshold: the debounce run restarts.
                    self.above_since = None;
                }
                false
            }
            MagAlarmState::Alarm => {
                // Clear conditions: back under the hysteresis level AND the
                // cooldown elapsed since the last firing.
                let cooled = self
                    .last_alarm_at
                    .is_none_or(|fired| t_s - fired >= self.config.cooldown_s);
                if value <= clear_level && cooled {
                    self.state = MagAlarmState::Armed;
                    self.above_since = None;
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> MagAlarmConfig {
        MagAlarmConfig {
            threshold_nt: 50_000.0,
            debounce_s: 1.0,
            hysteresis_nt: 50.0,
            cooldown_s: 30.0,
            ema_alpha: 1.0, // no smoothing in most tests: exact values
        }
    }

    #[test]
    fn a_single_spike_does_not_fire() {
        // One sample above the threshold, then back under: the debounce run
        // restarts and no alarm ever fires (T: anti-false-trigger).
        let mut a = MagAlarm::new(config());
        assert!(!a.feed(60_000.0, 0.0));
        assert!(!a.feed(40_000.0, 0.1));
        assert!(!a.feed(60_000.0, 0.2));
        assert!(!a.feed(40_000.0, 0.3));
        assert_eq!(a.state(), MagAlarmState::Armed);
    }

    #[test]
    fn sustained_excess_fires_after_the_debounce() {
        let mut a = MagAlarm::new(config());
        assert!(!a.feed(60_000.0, 0.0));
        assert!(!a.feed(60_000.0, 0.5), "still within the debounce window");
        assert!(a.feed(60_000.0, 1.0), "fires at ≥ 1 s above threshold");
        assert_eq!(a.state(), MagAlarmState::Alarm);
    }

    #[test]
    fn alarm_holds_until_the_hysteresis_clear_level() {
        let mut a = MagAlarm::new(config());
        a.feed(60_000.0, 0.0);
        a.feed(60_000.0, 1.0); // fires
                               // Between threshold (50 000) and clear (49 950): still above the
                               // clear level → the alarm holds even though the cooldown elapses.
        a.feed(49_980.0, 100.0);
        assert_eq!(a.state(), MagAlarmState::Alarm);
        // Under the clear level → re-armed.
        a.feed(49_000.0, 101.0);
        assert_eq!(a.state(), MagAlarmState::Armed);
    }

    #[test]
    fn cooldown_suppresses_immediate_refire() {
        let mut a = MagAlarm::new(config());
        a.feed(60_000.0, 0.0);
        a.feed(60_000.0, 1.0); // fires at t=1
                               // Drops under the clear level, but the cooldown (30 s) keeps the
                               // alarm latched until it elapses.
        a.feed(40_000.0, 2.0);
        assert_eq!(a.state(), MagAlarmState::Alarm, "cooling: still latched");
        a.feed(40_000.0, 20.0);
        assert_eq!(a.state(), MagAlarmState::Alarm, "still within cooldown");
        // Cooldown elapsed (34 s since the firing) AND under the clear
        // level: re-armed.
        a.feed(40_000.0, 35.0);
        assert_eq!(a.state(), MagAlarmState::Armed);
        // A fresh sustained excess fires again.
        a.feed(60_000.0, 36.0);
        assert!(!a.feed(60_000.0, 36.5), "still within the debounce");
        assert!(a.feed(60_000.0, 37.0), "second alarm after the cooldown");
    }

    #[test]
    fn ema_smoothing_rejects_sample_noise() {
        // α = 0.2: one huge spike moves the smoothed value only partly, and
        // the debounce requires the excess to *persist* — a single noise
        // burst cannot fire the alarm.
        let cfg = MagAlarmConfig {
            ema_alpha: 0.2,
            threshold_nt: 50_000.0,
            ..config()
        };
        let mut a = MagAlarm::new(cfg);
        // Steady 40 000 baseline for a while.
        for i in 0..20 {
            a.feed(40_000.0, i as f64 * 0.1);
        }
        // A single 100 000 spike: the smoothed value rises (0.2·100k +
        // 0.8·40k = 52 k) but only for an instant — the debounce holds.
        a.feed(100_000.0, 2.0);
        assert_eq!(a.state(), MagAlarmState::Armed, "a spike does not fire");
        // Back to the baseline: the debounce run resets, nothing fires.
        a.feed(40_000.0, 2.1);
        a.feed(40_000.0, 5.0);
        assert_eq!(a.state(), MagAlarmState::Armed);
    }

    #[test]
    fn threshold_change_restarts_the_debounce() {
        let mut a = MagAlarm::new(config());
        a.feed(60_000.0, 0.0);
        // The operator raises the threshold above the current value: the
        // run above the old level must not fire against the new one.
        a.set_config(MagAlarmConfig {
            threshold_nt: 70_000.0,
            ..config()
        });
        assert!(!a.feed(60_000.0, 0.5));
        assert_eq!(a.state(), MagAlarmState::Armed);
    }
}
