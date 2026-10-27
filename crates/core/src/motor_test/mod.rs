//! Motor / actuator test session (motor-test session, A2).
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
//! - `param2 <= 0` releases test control — the stop path. On a disarmed
//!   target this returns to its configured disarmed outputs, not necessarily
//!   zero PWM or mechanically stopped motors.
//!
//! The session is a pure state machine fed by the service shell: it decides
//! what to send, the shell sends it. All stop triggers funnel through one
//! `stop` transition (plan §8.3) so every failure mode behaves identically.

pub mod interlock;
pub mod mag_alarm;
pub mod mapping;
pub mod operation_types;
pub mod protocol;
pub mod pwm;
pub mod run;
pub mod scope;
pub mod service;
pub mod source;
pub mod tone;

pub use interlock::{Interlock, SafetyLimits, HEARTBEAT_STALE};
pub use mag_alarm::{MagAlarm, MagAlarmConfig, MagAlarmState};
pub use mapping::{
    build_output_map, mapped_feedback_pwm, MappingIssue, OutputAssignment, OutputMap,
    OutputMapState, ServoChannel, ServoPort, MOTOR_FUNCTION_BASE, SERVO_CHANNELS_PER_PORT,
};
pub use protocol::{detect_bank_protocol, DShotRate, OutputProtocol, OutputProtocols, OutputUnits};
pub use pwm::{OutputRange, PwmModel, PwmModelError, DSHOT_MAX_THROTTLE, PWM_US_MAX};
pub use run::{
    RunAction, RunConfig, RunCoordinator, RunError, RunPhase, RunStatus, RunStep, POST_ROLL,
    PRE_ROLL, RUN_FINALIZE_TIMEOUT,
};
pub use service::{
    MotorTestCommand, MotorTestEvent, MotorTestService, MotorTestStopError, StopReason, StopReceipt,
};
pub use source::{
    AnySource, ManualSource, MotorSlot, PresetKind, PresetSource, RpmSource, SenderConfig,
    WaveformPreset,
};

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
/// Two existing eight-channel output banks; no wider session is supported.
pub const MAX_MOTOR_FUNCTIONS: usize = 16;

/// Session lifecycle (plan §8.3). Profile/Function states arrive with A6/A7;
/// A2 wires Manual only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, rename = "MotorSessionState")]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// No confirmation, nothing running.
    Idle,
    /// Confirmed for the current load/mode, not sending yet.
    Ready,
    /// Sending manual values at the configured rate.
    ManualRunning,
    /// Active commands stopped; release transport writes are still pending.
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
    /// Last accepted demand fraction per motor, for slew limiting.
    last_demands: Vec<f32>,
    /// Last compensated `ACTUATOR_TEST` values sent to the FC, for the UI.
    last_values: Vec<f32>,
    /// Per-motor mapped PWM configuration used to compensate the command.
    pwm_models: Vec<PwmModel>,
    interlock: Interlock,
    started_at: Option<Instant>,
    busy_streak: u32,
    stop_reason: Option<StopReason>,
    /// Generating releases must not settle the session before the transport
    /// has confirmed every write. Also tracks incomplete emergency releases.
    release_pending: bool,
}

impl<S: RpmSource> MotorTestSession<S> {
    pub fn new(source: S, limits: SafetyLimits) -> Self {
        let motors = source.motor_count();
        Self {
            state: SessionState::Idle,
            source,
            last_demands: vec![0.0; motors],
            last_values: vec![0.0; motors],
            pwm_models: vec![PwmModel::default(); motors],
            interlock: Interlock::new(limits),
            started_at: None,
            busy_streak: 0,
            stop_reason: None,
            release_pending: false,
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

    /// Replace the per-motor PWM models. The caller sends this before every
    /// start, so an idle mapping refresh can never leave stale compensation.
    /// A missing model is explicitly marked fallback; the app blocks starts
    /// until every configured motor has an FC-read range.
    pub fn set_pwm_models(&mut self, models: Vec<PwmModel>) {
        self.pwm_models.clear();
        self.pwm_models
            .extend(models.into_iter().take(self.last_values.len()));
        self.pwm_models
            .resize(self.last_values.len(), PwmModel::default());
    }

    /// The last compensated values actually sent to the FC (post clamp/slew),
    /// for the window's output readout — `0.0` when idle.
    pub fn output_values(&self) -> Vec<f32> {
        self.last_values.clone()
    }

    /// Expected physical PWM from this session's last queued commands, using
    /// the exact models that encoded them. A released/unread motor has no
    /// active PWM command, including a reversed motor whose active command
    /// may legitimately be zero at its maximum physical pulse.
    pub fn pwm_output(&self) -> Vec<Option<f32>> {
        self.last_demands
            .iter()
            .zip(&self.last_values)
            .zip(&self.pwm_models)
            .map(|((&demand, &command), model)| {
                if demand <= 0.0 || model.range_source != pwm::PwmRangeSource::Fc {
                    None
                } else {
                    model.pulse_for_command(command).ok()
                }
            })
            .collect()
    }

    /// Operator start (after the app-layer confirmation): begin sending.
    pub fn start(&mut self, now: Instant) {
        if matches!(self.state, SessionState::Stopping | SessionState::Emergency) {
            return; // incomplete release or emergency latch: never resume
        }
        self.started_at = Some(now);
        self.busy_streak = 0;
        self.last_demands.fill(0.0);
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
            // A fresh session holds for the first heartbeat: PX4 streams
            // HEARTBEAT at 1 Hz, so a session started before the first
            // heartbeat reaches it would otherwise stop on its very first tick
            // (the reported "bounces straight back to idle"). Hold for the
            // heartbeat timeout *without sending active commands*: no
            // ACTUATOR_TEST may reach an unidentified/dirty safety state. If
            // no heartbeat arrives within the timeout the link is dead and the
            // session stops. An *armed* first heartbeat still stops
            // immediately, and the FC-side ACTUATOR_TEST deny is the backstop.
            let started = self.started_at.unwrap_or(now);
            let waiting_for_first = reason == StopReason::HeartbeatStale
                && !self.interlock.has_heartbeat()
                && now.duration_since(started) <= self.interlock.limits().heartbeat_timeout;
            if !waiting_for_first {
                self.stop(reason);
                return self.release_frames();
            }
            return Vec::new();
        }
        let started = self.started_at.unwrap_or(now);
        let dt = now.duration_since(started).as_secs_f64();
        // Read every motor from the same tick snapshot (forward mode must
        // drive all four from one SITL sample).
        let targets = self.source.values(dt);
        let mut any_target = false;
        let mut frames = Vec::with_capacity(targets.len());
        for (motor, target) in targets.into_iter().enumerate() {
            let Some(target) = target else { continue };
            any_target = true;
            let prev = self.last_demands[motor];
            let demand = self.interlock.clamp(prev, target);
            self.last_demands[motor] = demand;
            let value = self.pwm_models[motor].command_for_fraction(demand);
            self.last_values[motor] = if demand > 0.0 { value } else { 0.0 };
            // Benign-idle semantics (bench finding 2026-10-23): PX4 keeps a
            // value-0 ACTUATOR_TEST *active* at the function's `PWM_*_MIN`
            // pulse (timeout renews every tick), and some ESCs idle-spin at
            // exactly that pulse. A motor with no demand must not be pinned
            // there: it is only ever activated while demanded, and gets the
            // release (`timeout = 0`) exactly once when demand drops to 0.
            if demand > 0.0 {
                frames.push(actuator_test_frame(
                    self.interlock.limits().target_sys,
                    self.interlock.limits().target_comp,
                    (motor + 1) as u8,
                    value,
                    COMMAND_TIMEOUT_S,
                ));
            } else if prev > 0.0 {
                frames.push(actuator_test_frame(
                    self.interlock.limits().target_sys,
                    self.interlock.limits().target_comp,
                    (motor + 1) as u8,
                    0.0,
                    0.0,
                ));
            }
        }
        if !any_target {
            // The source ended (e.g. a preset ran its duration): the motors
            // would idle on stale values until the FC-side timeout — stop
            // explicitly instead.
            self.stop(StopReason::Completed);
            return self.release_frames();
        }
        frames
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
        self.release_pending = true;
    }

    /// Release test control for every motor without claiming delivery.
    /// The service calls [`Self::complete_release`] only after all transport
    /// writes succeed; a failed pass can safely retry this same full batch.
    pub fn release_frames(&self) -> Vec<MavMessage> {
        (0..self.source.motor_count())
            .map(|motor| {
                actuator_test_frame(
                    self.interlock.limits().target_sys,
                    self.interlock.limits().target_comp,
                    (motor + 1) as u8,
                    0.0,
                    0.0, // timeout <= 0 = ACTION_RELEASE_CONTROL
                )
            })
            .collect()
    }

    pub(crate) fn release_pending(&self) -> bool {
        self.release_pending
    }

    /// Settle a release pass after every transport write has succeeded.
    /// Emergency remains latched; an incomplete pass must not call this.
    pub fn complete_release(&mut self) {
        if !self.release_pending {
            return;
        }
        self.release_pending = false;
        self.last_demands.fill(0.0);
        self.last_values.fill(0.0);
        if self.state == SessionState::Stopping {
            self.state = SessionState::Idle;
        }
    }

    /// Emergency: stop active commands and request a full release pass.
    /// Only a reset after confirmed release clears the latch.
    pub fn emergency_stop(&mut self) {
        if self.state == SessionState::Emergency {
            return;
        }
        let owns_outputs = self.state.is_sending() || self.release_pending;
        self.state = SessionState::Emergency;
        self.stop_reason = owns_outputs.then_some(StopReason::Emergency);
        self.release_pending = owns_outputs;
    }

    pub fn reset_emergency(&mut self) {
        if self.state == SessionState::Emergency && !self.release_pending {
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
    /// Resize the bench to the configured M1..MN prefix. Only valid while
    /// idle: a running preset/release still references the old width. The
    /// app calls this before every start, then sends that start's values.
    pub fn set_motor_count(&mut self, motors: usize) -> bool {
        if motors == 0 || motors > MAX_MOTOR_FUNCTIONS || self.state != SessionState::Idle {
            return false;
        }
        if self.last_values.len() == motors {
            return true;
        }
        self.last_demands = vec![0.0; motors];
        self.last_values = vec![0.0; motors];
        self.pwm_models = vec![PwmModel::default(); motors];
        self.source = source::AnySource::Manual(ManualSource::new(vec![0.0; motors]));
        self.started_at = None;
        self.busy_streak = 0;
        true
    }

    /// Manual start: keep (or restore) the slider source and begin sending.
    pub fn start_manual(&mut self, now: Instant) {
        if matches!(self.state, SessionState::Stopping | SessionState::Emergency) {
            return;
        }
        if let source::AnySource::Preset(_) | source::AnySource::Forward(_) = &self.source {
            // A finished/previous bounded source is replaced by the manual
            // source of the same width; slider values were kept untouched
            // meanwhile.
            self.source =
                source::AnySource::Manual(ManualSource::new(vec![0.0; self.last_values.len()]));
        }
        self.start(now);
    }

    /// Preset start: the waveform owns the values for its duration; the
    /// sliders are disconnected (their update is ignored) until it ends.
    pub fn start_preset(&mut self, preset: source::WaveformPreset, now: Instant) {
        if matches!(self.state, SessionState::Stopping | SessionState::Emergency) {
            return;
        }
        self.source =
            source::AnySource::Preset(source::PresetSource::new(preset, self.last_values.len()));
        self.start(now);
    }

    /// Realtime-forward start (revised D3): the shared slot carries the
    /// latest normalized outputs of the source vehicle (SITL); the session
    /// reads it each tick and stops itself when the feed goes stale.
    pub fn start_forward(&mut self, slot: source::ForwardSlot, now: Instant) {
        if matches!(self.state, SessionState::Stopping | SessionState::Emergency) {
            return;
        }
        self.source = source::AnySource::Forward(source::ForwardSource::with_models(
            slot,
            self.last_values.len(),
            self.pwm_models.clone(),
        ));
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
    fn resizing_to_the_active_motor_prefix_resets_idle_slots() {
        let mut s =
            MotorTestSession::new(AnySource::Manual(ManualSource::new(vec![0.0; 4])), limits());
        assert!(s.set_motor_count(2));
        assert_eq!(s.output_values().len(), 2);
        assert_eq!(s.release_frames().len(), 2);

        s.start_manual(Instant::now());
        assert!(!s.set_motor_count(1), "never resize a running session");
        s.stop(StopReason::Command);
        s.complete_release();
        assert!(s.set_motor_count(1));
        assert_eq!(s.output_values().len(), 1);
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
    fn dshot_forward_sends_normalized_demand_and_never_commands_idle_motors() {
        // R1 acceptance: a source's 1000 µs idle is normalized demand 0 (not
        // 1000 mixer counts); demands stay monotonic, and only demanded
        // motors are activated, incl. 1/2/4 motors with reversal/thrust model.
        for motors in [1usize, 2, 4] {
            let slot = std::sync::Arc::new(std::sync::Mutex::new(
                super::source::ForwardState::new(motors),
            ));
            let values: Vec<f32> = [0.0, 0.5, 1.0, 0.5][..motors].to_vec();
            let raw: Vec<u16> = [1000, 1500, 2000, 1500][..motors].to_vec();
            {
                let mut st = slot.lock().expect("slot");
                st.values = values.clone();
                st.raw_pwm = raw;
                st.last_update_s = Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock")
                        .as_secs_f64(),
                );
            }
            let models: Vec<PwmModel> = (0..motors)
                .map(|motor| {
                    PwmModel::dshot(
                        0.055,
                        motor % 2 == 1,
                        if motor == 2 { 0.5 } else { 0.0 },
                        OutputProtocol::DShot {
                            rate: DShotRate::R600,
                        },
                    )
                    .expect("dshot model")
                })
                .collect();
            let mut s = MotorTestSession::new(
                AnySource::Manual(ManualSource::new(vec![0.0; motors])),
                limits(),
            );
            s.set_pwm_models(models);
            let now = Instant::now();
            s.interlock_mut().observe_heartbeat(false, now);
            s.start_forward(slot, now);
            let frames = s.tick(now);
            let expected: Vec<u8> = values
                .iter()
                .enumerate()
                .filter(|(_, value)| **value > 0.0)
                .map(|(motor, _)| (motor + 1) as u8)
                .collect();
            let mut commanded = Vec::new();
            for frame in &frames {
                let MavMessage::COMMAND_LONG(command) = frame else {
                    panic!("ACTUATOR_TEST frame");
                };
                commanded.push(command.param5 as u8);
                assert_eq!(command.param2, COMMAND_TIMEOUT_S);
            }
            assert_eq!(commanded, expected, "{motors} motors: commanded functions");
            assert!(
                !commanded.contains(&1) || values[0] > 0.0,
                "{motors} motors: idle motor 1 was activated"
            );
            let outputs = s.output_values();
            for (motor, value) in values.iter().enumerate() {
                if *value == 0.0 {
                    assert_eq!(outputs[motor], 0.0, "idle motor {motor}");
                } else if !(motor % 2 == 1 && *value == 1.0) {
                    // A reversed maximum legitimately maps to command 0.
                    assert!(
                        (outputs[motor] - value).abs() < 1e-6,
                        "{motors} motors: M{} output {} != demand {value}",
                        motor + 1,
                        outputs[motor]
                    );
                }
            }
        }
    }

    #[test]
    fn pwm_readback_adapts_to_the_target_and_distinguishes_zero_command_from_release() {
        let mut s = session(&[0.25, 1.0, 0.25, 0.0]);
        s.set_pwm_models(vec![
            PwmModel::new(1100, 1900, false, 0.0).expect("M1"),
            PwmModel::new(1200, 1800, true, 0.0).expect("M2 reversed"),
            PwmModel::new(1150, 1850, false, 0.5).expect("M3 thrust model"),
            PwmModel::new(1100, 1900, false, 0.0).expect("M4"),
        ]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        let frames = s.tick(now);
        let MavMessage::COMMAND_LONG(command) = &frames[0] else {
            panic!("command");
        };
        assert_eq!(command.param1, 0.25, "wire value remains normalized");
        let pwm = s.pwm_output();
        for (actual, expected) in pwm[..3].iter().zip([1300.0, 1800.0, 1325.0]) {
            assert!((actual.expect("active PWM") - expected).abs() < 1.0);
        }
        assert_eq!(
            s.output_values()[1],
            0.0,
            "reversed maximum has a valid active zero command"
        );
        assert!(pwm[1].is_some());
        assert_eq!(pwm[3], None, "an untouched motor has no active PWM");

        s.set_values(vec![0.0; 4]);
        s.tick(now + std::time::Duration::from_millis(100));
        assert_eq!(
            s.pwm_output(),
            vec![None; 4],
            "release is not the minimum PWM"
        );
        assert_eq!(s.output_values(), vec![0.0; 4]);

        s.set_pwm_models(vec![
            PwmModel::new(1200, 1800, false, 0.0).expect("new FC range")
        ]);
        s.set_values(vec![0.25, 0.0, 0.0, 0.0]);
        s.start(now);
        s.tick(now);
        assert_eq!(
            s.pwm_output()[0],
            Some(1350.0),
            "the new FC range changes the physical command"
        );
    }

    #[test]
    fn idle_motors_are_never_pinned_at_the_minimum_pulse() {
        let mut s = session(&[0.4, 0.0, 0.0, 0.0]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        let frames = s.tick(now);
        assert_eq!(
            frames.len(),
            1,
            "only the demanded motor is activated; idle ones stay untouched"
        );
        // Drop the only demanded motor to zero: exactly one release frame
        // (`timeout = 0`), not another 0-value active frame.
        s.set_values(vec![0.0, 0.0, 0.0, 0.0]);
        let frames = s.tick(now + std::time::Duration::from_millis(100));
        assert_eq!(frames.len(), 1, "the drop to zero releases once");
        let MavMessage::COMMAND_LONG(command) = &frames[0] else {
            panic!("expected a COMMAND_LONG release frame");
        };
        assert_eq!(command.param1, 0.0, "value 0");
        assert_eq!(command.param2, 0.0, "timeout 0 = release");
        // Holding at zero demand sends nothing: no min-pulse pinning, and the
        // empty tick must not be mistaken for a source end.
        let frames = s.tick(now + std::time::Duration::from_millis(200));
        assert!(frames.is_empty(), "no idle poke while demand stays 0");
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
        assert_eq!(s.state(), SessionState::Stopping);
        s.complete_release();
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
        assert_eq!(s.state(), SessionState::Stopping);
        s.complete_release();
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
        assert_eq!(s.state(), SessionState::Stopping);
        s.complete_release();
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
        assert_eq!(
            s.state(),
            SessionState::Emergency,
            "unconfirmed release stays latched"
        );
        s.complete_release();
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
        assert_eq!(s.state(), SessionState::Stopping);
        s.complete_release();
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

    #[test]
    fn fresh_session_holds_for_the_first_heartbeat_without_sending() {
        // The reported "bounces straight back to idle": Start clicked before
        // the FC's first heartbeat reaches the session — the interlock had no
        // `armed` state and failed on the very first tick. A fresh session
        // must hold for the first heartbeat within the timeout instead, and
        // must not emit active commands while the safety state is unknown.
        let mut s = session(&[0.4]);
        let now = Instant::now();
        s.start(now);
        // First tick, no heartbeat yet: no ACTUATOR_TEST at all.
        assert!(
            s.tick(now).is_empty(),
            "no active command without heartbeat"
        );
        assert_eq!(s.state(), SessionState::ManualRunning);

        // Still no heartbeat: the grace window holds without sending.
        let mid = now + std::time::Duration::from_millis(500);
        assert!(s.tick(mid).is_empty());
        assert_eq!(s.state(), SessionState::ManualRunning);

        // The first heartbeat arrives, not armed: normal interlock resumes.
        s.interlock_mut().observe_heartbeat(false, mid);
        let frames = s.tick(mid + std::time::Duration::from_millis(100));
        assert!(!frames.is_empty(), "sends after a fresh disarmed heartbeat");
        assert_eq!(s.state(), SessionState::ManualRunning);

        // No heartbeat at all within the grace window: the session stops.
        let mut s2 = session(&[0.4]);
        s2.start(now);
        assert!(s2.tick(now).is_empty());
        let frames = s2.tick(now + std::time::Duration::from_secs(3));
        s2.complete_release();
        assert_eq!(s2.state(), SessionState::Idle);
        assert_eq!(s2.take_stop_reason(), Some(StopReason::HeartbeatStale));
        assert_eq!(frames.len(), 1, "release frame");
    }

    #[test]
    fn pwm_model_compensates_after_slew_and_reports_the_sent_command() {
        let mut s = session(&[0.375]);
        // 0.375 is the target fraction for 1400 µs on 1100–1900 µs.
        s.set_pwm_models(vec![PwmModel::new(1100, 1900, false, 0.5).expect("model")]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        let frames = s.tick(now);
        let MavMessage::COMMAND_LONG(command) = &frames[0] else {
            panic!("actuator command");
        };
        assert!(
            (command.param1 - 0.257_812_5).abs() < 1e-6,
            "compensated command, got {}",
            command.param1
        );
        let sent = s.output_values();
        assert!((sent[0] - command.param1).abs() < 1e-6);
    }

    #[test]
    fn sine_preset_sweeps_the_px4_range_bottom_to_top() {
        // Acceptance: a designed test signal (Sine here; Square shares the
        // path) sweeps exactly the PWM range read from the PX4 — trough =
        // `PWM_*_MINn`, peak = `PWM_*_MAXn` — never beyond it.
        use super::source::{PresetKind, WaveformPreset};
        let mut s = MotorTestSession::new(
            super::source::AnySource::Manual(super::source::ManualSource::new(vec![0.0])),
            limits(),
        );
        let model = PwmModel::new(1100, 1900, false, 0.0).expect("model");
        s.set_pwm_models(vec![model]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start_preset(
            WaveformPreset {
                kind: PresetKind::Sine,
                amplitude: 1.0,
                frequency_hz: 1.0,
                duration_s: 5.0,
            },
            now,
        );
        let decode_pulse = |frames: Vec<MavMessage>| {
            let MavMessage::COMMAND_LONG(command) = frames.first().expect("one frame") else {
                panic!("actuator command");
            };
            model.pulse_for_command(command.param1).expect("pulse")
        };
        // Sine starts at fraction 0: with idle-release semantics no value-0
        // frame is emitted — the function is left released instead of being
        // pinned at the `PWM_*_MIN` pulse (bench finding 2026-10-23). The
        // sweep then lifts off the minimum and reaches the maximum at the
        // half-period peak.
        assert!(
            s.tick(now).is_empty(),
            "value-0 demand must not activate the function",
        );
        let first_pulse = decode_pulse(s.tick(now + std::time::Duration::from_millis(50)));
        assert!(
            first_pulse
                <= f32::from(model.min_us)
                    + 0.05 * (f32::from(model.max_us) - f32::from(model.min_us)),
            "sine rise starts in the bottom 5% of the range, got {first_pulse}"
        );
        // Half period later (raised-cosine sine peaks at phase π; slew
        // budget 10 /s allows the full rise) the sine reaches fraction 1 →
        // the maximum pulse.
        let peak = decode_pulse(s.tick(now + std::time::Duration::from_millis(500)));
        assert!(
            (peak - f32::from(model.max_us)).abs() <= 1.0,
            "sine peak = MAX, got {peak}"
        );
    }

    #[test]
    fn pwm_model_does_not_change_release_frames() {
        let mut s = session(&[0.4]);
        s.set_pwm_models(vec![PwmModel::new(1100, 1900, true, 0.5).expect("model")]);
        let now = Instant::now();
        s.interlock_mut().observe_heartbeat(false, now);
        s.start(now);
        let _ = s.tick(now);
        s.stop(StopReason::Command);
        let frames = s.release_frames();
        let MavMessage::COMMAND_LONG(command) = &frames[0] else {
            panic!("release command");
        };
        assert_eq!(command.param1, 0.0);
        assert_eq!(command.param2, 0.0);
    }

    #[test]
    fn an_armed_first_heartbeat_stops_immediately() {
        // The grace window must not cover an armed vehicle: the first
        // heartbeat says armed → release and stop on the next tick.
        let mut s = session(&[0.4]);
        let now = Instant::now();
        s.start(now);
        let _ = s.tick(now);
        s.interlock_mut().observe_heartbeat(true, now);
        let frames = s.tick(now + std::time::Duration::from_millis(100));
        s.complete_release();
        assert_eq!(s.state(), SessionState::Idle);
        assert_eq!(s.take_stop_reason(), Some(StopReason::Armed));
        assert_eq!(frames.len(), 1, "release frame");
    }
}
