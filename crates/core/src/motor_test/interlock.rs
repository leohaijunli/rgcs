//! Interlocks and output limits for the motor test (plan §8.2).
//!
//! Every source value passes through [`Interlock::clamp`] on its way to the
//! wire, and every tick passes [`Interlock::check`]: the two together mean no
//! input (slider, seek, speedup, script) can exceed the configured envelope,
//! and the session never outlives its safety inputs.

use std::time::{Duration, Instant};

use super::service::StopReason;

/// No heartbeat for this long while sending = the FC view is stale → stop.
pub const HEARTBEAT_STALE: Duration = Duration::from_secs(2);

/// Output envelope + link identity for the session.
#[derive(Debug, Clone)]
pub struct SafetyLimits {
    /// Maximum absolute output value (load-dependent; 0..=1 for a normal
    /// multirotor motor, which cannot reverse).
    pub max_value: f32,
    /// Maximum output change per second — the spin-up/spin-down ramp.
    pub max_slew_per_s: f32,
    /// Age at which the FC view counts as stale while sending.
    pub heartbeat_timeout: Duration,
    /// Target the frames are addressed to.
    pub target_sys: u8,
    pub target_comp: u8,
}

/// Armed/heartbeat watchdog + value conditioning.
pub struct Interlock {
    limits: SafetyLimits,
    armed: Option<bool>,
    last_heartbeat: Option<Instant>,
}

impl Interlock {
    pub fn new(limits: SafetyLimits) -> Self {
        Self {
            limits,
            armed: None,
            last_heartbeat: None,
        }
    }

    pub fn limits(&self) -> &SafetyLimits {
        &self.limits
    }

    /// A HEARTBEAT arrived: record the armed state and freshness.
    pub fn observe_heartbeat(&mut self, armed: bool, now: Instant) {
        self.armed = Some(armed);
        self.last_heartbeat = Some(now);
    }

    /// The link died without a goodbye: the view is immediately stale.
    pub fn link_lost(&mut self) {
        self.last_heartbeat = None;
        self.armed = None;
    }

    /// Pre-send check. Fails while armed, before the first heartbeat, or
    /// when the heartbeat went stale — all stop the session.
    pub fn check(&self, now: Instant) -> Result<(), StopReason> {
        match self.armed {
            Some(true) => return Err(StopReason::Armed),
            None => return Err(StopReason::HeartbeatStale),
            Some(false) => {}
        }
        match self.last_heartbeat {
            Some(at) if now.duration_since(at) <= self.limits.heartbeat_timeout => Ok(()),
            _ => Err(StopReason::HeartbeatStale),
        }
    }

    /// Condition one source value: finite → clamp to ±max → slew-limit
    /// against the previous accepted value over one tick. A non-finite
    /// target (slider bug, script error) **holds** the previous value —
    /// neither a jump nor a sudden drop, and never NaN on the wire.
    pub fn clamp(&self, prev: f32, target: f32) -> f32 {
        if !target.is_finite() {
            return prev;
        }
        let capped = target.clamp(-self.limits.max_value, self.limits.max_value);
        let step = self.limits.max_slew_per_s / DEFAULT_TICK_HZ as f32;
        (capped - prev).clamp(-step, step) + prev
    }
}

use super::DEFAULT_TICK_HZ;

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> SafetyLimits {
        SafetyLimits {
            max_value: 0.8,
            max_slew_per_s: 4.0, // 0.4 per 100 ms tick
            heartbeat_timeout: Duration::from_secs(2),
            target_sys: 1,
            target_comp: 1,
        }
    }

    #[test]
    fn clamp_caps_the_envelope() {
        let i = Interlock::new(limits());
        assert!(
            (i.clamp(0.0, 2.0) - 0.4).abs() < 1e-6,
            "slew applies before cap"
        );
        // Walking to the cap takes a few ticks.
        let mut v = 0.0;
        for _ in 0..10 {
            v = i.clamp(v, 2.0);
        }
        assert!((v - 0.8).abs() < 1e-6, "settles at max_value, got {v}");
    }

    #[test]
    fn non_finite_holds_the_previous_value() {
        let i = Interlock::new(limits());
        assert!((i.clamp(0.5, f32::NAN) - 0.5).abs() < 1e-6);
        assert!((i.clamp(0.5, f32::INFINITY) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn check_fails_when_armed_or_stale() {
        let mut i = Interlock::new(limits());
        let now = Instant::now();
        assert_eq!(i.check(now), Err(StopReason::HeartbeatStale));
        i.observe_heartbeat(false, now);
        assert!(i.check(now).is_ok());
        i.observe_heartbeat(true, now);
        assert_eq!(i.check(now), Err(StopReason::Armed));
        i.observe_heartbeat(false, now);
        assert_eq!(
            i.check(now + Duration::from_secs(3)),
            Err(StopReason::HeartbeatStale)
        );
    }
}
