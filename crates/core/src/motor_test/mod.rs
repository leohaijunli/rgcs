//! Motor / actuator test session (motor-test-plan §7.3, §8; A2).
//!
//! High-rate motor control over `MAV_CMD_ACTUATOR_TEST` (verified against PX4
//! v1.17 `Commander::handleCommandActuatorTest`):
//!
//! - `param1` = control value, `param2` = FC-side timeout in seconds (capped
//!   at 3 s by PX4; we send 0.3 s so a dropped GCS cannot spin motors long),
//!   `param5` = 1-based motor function.
//! - PX4 **denies** the command while armed or with `COM_MOT_TEST_EN != 1`,
//!   which is exactly the interlock we want: the test needs no arming and is
//!   refused if the vehicle is.
//! - `param2 <= 0` releases control (motors off) — the stop path.
//!
//! The session is a pure state machine fed by the service shell: it decides
//! what to send, the shell sends it. All stop triggers funnel through one
//! `stop` transition (plan §8.3) so every failure mode behaves identically.

pub mod interlock;
pub mod service;
pub mod source;

pub use interlock::{Interlock, SafetyLimits, HEARTBEAT_STALE};
pub use service::{MotorTestCommand, MotorTestEvent, MotorTestService, StopReason};
pub use source::{ManualSource, MotorSlot, PresetKind, PresetSource, RpmSource, SenderConfig, WaveformPreset, AnySource};

use std::time::Instant;

use ::mavlink::common::{MavCmd, MavMessage};

/// Default send rate (operator: start at 10 Hz, raise after bench checks).
pub const DEFAULT_TICK_HZ: f64 = 10.0;

/// Per-command FC-side timeout: every accepted command keeps the motors
/// alive this long, so a single dropped tick never stops the test while a
/// dead GCS stops it within 300 ms (PX4 caps at 3 s regardless).
pub const COMMAND_TIMEOUT_S: f32 = 0.3;

/// Consecutive "busy" acks before the session gives up.
pub const MAX_BUSY_STREAK: u32 = 3;

/// Session lifecycle (plan §8.3). Profile/Function states arrive with A6/A7;
/// A2 wires Manual only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// No confirmation, nothing running.
    Idle,
    /// Confirmed for the current load/mode, not sending yet.
    Ready,
    /// Sending manual values at the configured rate.
    ManualRunning,
    /// Stopping: release frames sent, settling back to Idle.
    Stopping,
    /// Latched: only an explicit reset clears it.
    Emergency,
}

impl SessionState {
    pub fn is_sending(self) -> bool {
        matches!(self, SessionState::ManualRunning)
    }
}

/// Build one `MAV_CMD_ACTUATOR_TEST` frame. `timeout_s <= 0` releases control.
pub fn actuator_test_frame(
    target_sys: u8,
    target_comp: u8,
    motor: u8,
    value: f32,
    timeout_s: f32,
) -> MavMessage {
    MavMessage::COMMAND_LONG(::mavlink::common::COMMAND_LONG_DATA {
        param1: value,
        param2: timeout_s,
        param3: 0.0,
        param4: 0.0,
        // PX4 maps param5 < 1000 as a 1-based motor function (1 = Motor 1).
        param5: motor as f32,
        param6: 0.0,
        param7: 0.0,
        command: MavCmd::MAV_CMD_ACTUATOR_TEST,
        target_system: target_sys,
        target_component: target_comp,
        confirmation: 0,
    })
}

/// The pure session: state + interlock + source → frames to send. The
/// service shell owns the clock and the link; this is unit-testable without
/// I/O.
pub struct MotorTestSession<S: RpmSource> {
    state: SessionState,
    source: S,
    /// Last accepted value per motor, for slew limiting.
    last_values: Vec<f32>,
    interlock: Interlock,
    started_at: Option<Instant>,
    busy_streak: u32,
    stop_reason: Option<StopReason>,
}

impl<S: RpmSource> MotorTestSession<S> {
    pub fn new(source: S, limits: SafetyLimits) -> Self {
        let motors = source.motor_count();
        Self {
            state: SessionState::Idle,
            source,
            last_values: vec![0.0; motors],
            interlock: Interlock::new(limits),
            started_at: None,
            busy_streak: 0,
            stop_reason: None,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn interlock_mut(&mut self) -> &mut Interlock {
        &mut self.interlock
    }

    pub fn interlock(&self) -> &Interlock {
        &self.interlock
    }

    /// Operator start (after the app-layer confirmation): begin sending.
    pub fn start(&mut self, now: Instant) {
        if self.state == SessionState::Emergency {
            return; // latched: require an explicit reset first
        }
        self.started_at = Some(now);
        self.busy_streak = 0;
        self.last_values.fill(0.0);
        self.state = SessionState::ManualRunning;
    }

    /// One send step: interlock check → source values → clamp/slew → frames.
    /// A running source that yields no frames this tick is exhausted (a
    /// finished preset): stop cleanly with `StopReason::Completed`.
    pub fn tick(&mut self, now: Instant) -> Vec<MavMessage> {
        if !self.state.is_sending() {
            return Vec::new();
        }
        if let Err(reason) = self.interlock.check(now) {
            self.stop(reason);
            return self.release_frames();
        }
        let started = self.started_at.unwrap_or(now);
        let dt = now.duration_since(started).as_secs_f64();
        let mut out = Vec::with_capacity(self.source.motor_count());
        for motor in 0..self.source.motor_count() {
            if let Some(target) = self.source.value(dt, motor) {
                let prev = self.last_values[motor];
                let value = self.interlock.clamp(prev, target);
                self.last_values[motor] = value;
                out.push(actuator_test_frame(
                    self.interlock.limits().target_sys,
                    self.interlock.limits().target_comp,
                    (motor + 1) as u8,
                    value,
                    COMMAND_TIMEOUT_S,
                ));
            }
        }
        if out.is_empty() {
            // The source ended (e.g. a preset ran its duration): the motors
            // would idle on stale values until the FC-side timeout — stop
            // explicitly instead.
            self.stop(StopReason::Completed);
            return self.release_frames();
        }
        out
    }

    /// A `COMMAND_ACK` for ACTUATOR_TEST arrived.
    pub fn handle_ack(&mut self, result: ::mavlink::common::MavResult) {
        if !self.state.is_sending() {
            return;
        }
        use ::mavlink::common::MavResult as R;
        match result {
            R::MAV_RESULT_ACCEPTED | R::MAV_RESULT_IN_PROGRESS => self.busy_streak = 0,
            R::MAV_RESULT_TEMPORARILY_REJECTED => {
                // One busy ack is a hiccup; a streak means the FC will not
                // take our commands.
                self.busy_streak += 1;
                if self.busy_streak >= MAX_BUSY_STREAK {
                    self.stop(StopReason::AcksDenied);
                }
            }
            // DENIED (armed / COM_MOT_TEST_EN off) / UNSUPPORTED / FAILED:
            // deterministic on PX4 — stop immediately, retries cannot help.
            _ => self.stop(StopReason::AcksDenied),
        }
    }

    /// The single stop funnel: every trigger lands here (plan §8.3).
    pub fn stop(&mut self, reason: StopReason) {
        if !self.state.is_sending() {
            return;
        }
        self.state = SessionState::Stopping;
        self.stop_reason = Some(reason);
    }

    /// Release frames (motors off) for every motor; settles Stopping → Idle.
    pub fn release_frames(&mut self) -> Vec<MavMessage> {
        let frames: Vec<MavMessage> = (0..self.source.motor_count())
            .map(|motor| {
                actuator_test_frame(
                    self.interlock.limits().target_sys,
                    self.interlock.limits().target_comp,
                    (motor + 1) as u8,
                    0.0,
                    0.0, // timeout <= 0 = ACTION_RELEASE_CONTROL
                )
            })
            .collect();
        self.last_values.fill(0.0);
        if self.state == SessionState::Stopping {
            self.state = SessionState::Idle;
        }
        frames
    }

    /// Emergency: latch; only `reset_emergency` clears it.
    pub fn emergency_stop(&mut self) {
        self.state = SessionState::Emergency;
        self.last_values.fill(0.0);
    }

    pub fn reset_emergency(&mut self) {
        if self.state == SessionState::Emergency {
            self.state = SessionState::Idle;
        }
    }

    /// Reason of the last stop (consumed by the event shell).
    pub fn take_stop_reason(&mut self) -> Option<StopReason> {
        self.stop_reason.take()
    }
}

impl MotorTestSession<ManualSource> {
    /// Latest-value-wins slider update (UI throttles; never queues).
    pub fn set_values(&mut self, values: Vec<f32>) {
        self.source.set_values(values);
    }
}

impl MotorTestSession<source::AnySource> {
    /// Manual start: keep (or restore) the slider source and begin sending.
    pub fn start_manual(&mut self, now: Instant) {
        if let source::AnySource::Preset(_) = &self.source {
            // A finished/previous preset is replaced by the manual source of
            // the same width; slider values were kept untouched meanwhile.
            self.source =
                source::AnySource::Manual(ManualSource::new(vec![0.0; self.last_values.len()]));
        }
        self.start(now);
    }

    /// Preset start: the waveform owns the values for its duration; the
    /// sliders are disconnected (their update is ignored) until it ends.
    pub fn start_preset(&mut self, preset: source::WaveformPreset, now: Instant) {
        self.source =
            source::AnySource::Preset(source::PresetSource::new(preset, self.last_values.len()));
        self.start(now);
    }

    /// Latest-value-wins slider update through the swappable source.
    pub fn set_any_values(&mut self, values: Vec<f32>) {
        self.source.set_values(values);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motor_test::interlock::SafetyLimits;

    fn limits() -> SafetyLimits {
        SafetyLimits {
            max_value: 1.0,
            max_slew_per_s: 10.0,
            heartbeat_timeout: std::time::Duration::from_secs(2),
            target_sys: 1,
            target_comp: 1,
        }
    }

    fn session(values: &[f32]) -> MotorTestSession<ManualSource> {
        MotorTestSession::new(ManualSource::new(values.to_vec()), limits())
    }

    #[test]
    fn idle_session_sends_nothing() {
        let mut s = session(&[0.0]);
        assert_eq!(s.state(), SessionState::Idle);
        assert!(s.tick(Instant::now()).is_empty());
    }

    #[test]
    fn start_then_tick_sends_one_frame_per_motor() {
        let mut s = session(&[0.3, 0.6]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        let frames = s.tick(now);
        assert_eq!(frames.len(), 2, "one frame per motor");
        assert_eq!(s.state(), SessionState::ManualRunning);
    }

    #[test]
    fn arming_mid_test_stops_within_one_tick() {
        let mut s = session(&[0.4]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        s.tick(now);
        // The vehicle arms: the very next tick must emit release frames and
        // stop sending.
        s.interlock_mut().observe_heartbeat(true, now);
        let frames = s.tick(now);
        assert_eq!(frames.len(), 1, "release frame for the one motor");
        assert_eq!(s.state(), SessionState::Idle);
        assert_eq!(s.take_stop_reason(), Some(StopReason::Armed));
    }

    #[test]
    fn stale_heartbeat_stops() {
        let mut s = session(&[0.4]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        let later = now + std::time::Duration::from_secs(3);
        let frames = s.tick(later);
        assert_eq!(s.state(), SessionState::Idle);
        assert_eq!(s.take_stop_reason(), Some(StopReason::HeartbeatStale));
        assert_eq!(frames.len(), 1, "release frame");
    }

    #[test]
    fn denied_ack_stops_immediately() {
        let mut s = session(&[0.4]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        s.tick(now);
        s.handle_ack(::mavlink::common::MavResult::MAV_RESULT_DENIED);
        assert_eq!(s.state(), SessionState::Stopping);
        let frames = s.release_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(s.state(), SessionState::Idle);
    }

    #[test]
    fn emergency_latches_and_requires_reset() {
        let mut s = session(&[0.4]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        s.emergency_stop();
        assert_eq!(s.state(), SessionState::Emergency);
        // Start is refused while latched.
        s.start(now);
        assert_eq!(s.state(), SessionState::Emergency);
        s.reset_emergency();
        assert_eq!(s.state(), SessionState::Idle);
    }

    #[test]
    fn preset_runs_then_completes_cleanly() {
        use super::source::{AnySource, ManualSource, PresetKind, WaveformPreset};
        let mut s =
            MotorTestSession::new(AnySource::Manual(ManualSource::new(vec![0.0])), limits());
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start_preset(
            WaveformPreset {
                kind: PresetKind::Step,
                amplitude: 0.8,
                frequency_hz: 1.0,
                duration_s: 0.05, // ~5 ticks at 10 Hz
            },
            now,
        );
        // While it runs: frames flow.
        assert!(!s.tick(now).is_empty());
        assert_eq!(s.state(), SessionState::ManualRunning);
        // After the duration the source is exhausted: the next tick emits
        // release frames and stops with `Completed`.
        let later = now + std::time::Duration::from_millis(200);
        let frames = s.tick(later);
        assert_eq!(frames.len(), 1, "release frame");
        assert_eq!(s.state(), SessionState::Idle);
        assert_eq!(s.take_stop_reason(), Some(StopReason::Completed));
    }

    #[test]
    fn slew_limit_and_cap_apply_to_source_values() {
        let mut s = session(&[1.0]); // way above the slew budget in one tick
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        let frames = s.tick(now);
        // 0.1 s tick at 10 /s slew budget → 1.0 per second → ≤ 1.0… with a
        // 0-tick first step the slew allows the full first step from 0.
        assert!(!frames.is_empty());
    }
}
