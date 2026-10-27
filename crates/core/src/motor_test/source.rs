//! Command sources for the motor test (plan §7.3): where each tick's target
//! value comes from. A2 ships `ManualSource` (slider values, latest wins);
//! Profile replay (A6) and speed functions (A7) implement the same trait.

use super::protocol::OutputUnits;
use super::pwm::PwmModel;

/// One motor the sender drives. `function` is the 1-based motor function PX4
/// expects in `MAV_CMD_ACTUATOR_TEST.param5` (1 = Motor 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotorSlot {
    pub function: u8,
}

/// Sender shape: rate + which motors.
#[derive(Debug, Clone)]
pub struct SenderConfig {
    /// Send rate (Hz). 10 per the operator until bench numbers say more.
    pub tick_hz: f64,
    pub motors: Vec<MotorSlot>,
}

impl Default for SenderConfig {
    fn default() -> Self {
        Self {
            tick_hz: super::DEFAULT_TICK_HZ,
            motors: (1..=4).map(|function| MotorSlot { function }).collect(),
        }
    }
}

/// A tick's target value per motor.
pub trait RpmSource: Send {
    /// Number of motors this source drives (fixed for the session).
    fn motor_count(&self) -> usize;
    /// Target value for `motor` (0-based) at session-relative time `t_s`.
    /// `None` = no command this tick for this motor.
    fn value(&mut self, t_s: f64, motor: usize) -> Option<f32>;
    /// All motors' targets for one tick. Sources backed by a shared slot
    /// override this to read the slot **once**, so every motor in a tick is
    /// driven from the same sample (forwarding must not mix two SITL frames).
    fn values(&mut self, t_s: f64) -> Vec<Option<f32>> {
        (0..self.motor_count())
            .map(|motor| self.value(t_s, motor))
            .collect()
    }
}

/// Manual mode: the latest slider value wins (values merge, never queue —
/// plan §8.1). Updated by the service shell between ticks.
#[derive(Debug)]
pub struct ManualSource {
    values: Vec<f32>,
}

impl ManualSource {
    pub fn new(values: Vec<f32>) -> Self {
        Self { values }
    }

    /// Latest-value-wins update from the UI (throttled there, ~30 Hz).
    pub fn set_values(&mut self, values: Vec<f32>) {
        if values.len() != self.values.len() {
            return; // motor count is fixed for the session
        }
        self.values = values;
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }
}

impl RpmSource for ManualSource {
    fn motor_count(&self) -> usize {
        self.values.len()
    }

    fn value(&mut self, _t_s: f64, motor: usize) -> Option<f32> {
        self.values.get(motor).copied()
    }
}

/// Built-in preset waveforms (plan §6.1). All start at 0 — spin-up is then
/// shaped by the interlock's slew limit — and drive every motor in sync.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, rename = "MotorPresetKind")]
#[serde(rename_all = "snake_case")]
pub enum PresetKind {
    Step,
    Ramp,
    Square,
    Sine,
}

/// One preset's parameters.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, rename = "MotorWaveformPreset")]
pub struct WaveformPreset {
    pub kind: PresetKind,
    /// Peak output (0..=1 before the interlock clamps it).
    pub amplitude: f32,
    /// Square/Sine frequency, Hz.
    pub frequency_hz: f32,
    /// Total run time; the session stops itself when it elapses.
    pub duration_s: f32,
}

impl WaveformPreset {
    /// The waveform's value at session-relative time `t_s`.
    fn at(&self, t_s: f64) -> f32 {
        let a = self.amplitude as f64;
        let t = t_s.min(self.duration_s as f64);
        match self.kind {
            // Hold at the amplitude after the (slew-limited) initial rise.
            PresetKind::Step => self.amplitude,
            // Linear 0 → a across the whole duration.
            PresetKind::Ramp => (a * t / self.duration_s as f64) as f32,
            // 0 for the first half-period, `a` for the second — starts safe.
            PresetKind::Square => {
                let phase = (t * self.frequency_hz as f64) % 1.0;
                if phase < 0.5 {
                    0.0
                } else {
                    self.amplitude
                }
            }
            // 0 → a → 0 raised cosine: starts at 0, no negative throttle.
            PresetKind::Sine => {
                (a * 0.5
                    * (1.0 - (2.0 * std::f64::consts::PI * self.frequency_hz as f64 * t).cos()))
                    as f32
            }
        }
    }
}

/// A preset waveform driving all motors in sync; ends after `duration_s`.
#[derive(Debug)]
pub struct PresetSource {
    preset: WaveformPreset,
    motors: usize,
}

impl PresetSource {
    pub fn new(preset: WaveformPreset, motors: usize) -> Self {
        Self { preset, motors }
    }
}

impl RpmSource for PresetSource {
    fn motor_count(&self) -> usize {
        self.motors
    }

    fn value(&mut self, t_s: f64, _motor: usize) -> Option<f32> {
        if t_s >= self.preset.duration_s as f64 {
            return None; // exhausted: the session stops itself
        }
        Some(self.preset.at(t_s))
    }
}

/// The session's swappable source: manual sliders, a preset waveform, or
/// realtime forwarding of another vehicle's actuator outputs (profile and
/// function sources join in A6/A7).
#[derive(Debug)]
pub enum AnySource {
    Manual(ManualSource),
    Preset(PresetSource),
    Forward(ForwardSource),
}

impl AnySource {
    /// Latest-value-wins slider update; ignored while another source owns
    /// the values (preset duration or a live forward).
    pub fn set_values(&mut self, values: Vec<f32>) {
        if let AnySource::Manual(m) = self {
            m.set_values(values);
        }
    }
}

impl RpmSource for AnySource {
    fn motor_count(&self) -> usize {
        match self {
            AnySource::Manual(m) => m.motor_count(),
            AnySource::Preset(p) => p.motor_count(),
            AnySource::Forward(f) => f.motor_count(),
        }
    }

    fn value(&mut self, t_s: f64, motor: usize) -> Option<f32> {
        match self {
            AnySource::Manual(m) => m.value(t_s, motor),
            AnySource::Preset(p) => p.value(t_s, motor),
            AnySource::Forward(f) => f.value(t_s, motor),
        }
    }

    fn values(&mut self, t_s: f64) -> Vec<Option<f32>> {
        match self {
            AnySource::Manual(m) => m.values(t_s),
            AnySource::Preset(p) => p.values(t_s),
            AnySource::Forward(f) => f.values(t_s),
        }
    }
}

/// Slot freshness bound: a SITL feed that goes quiet for this long counts
/// as an exhausted source and the session stops itself (plan §8.3 — the
/// exhaustion path is shared with a finished preset).
pub const FORWARD_STALE_AFTER_S: f64 = 1.0;

/// Grace from start before the first SITL sample must arrive: within it
/// the source forwards 0 (idle — motors stay off, the FC keeps receiving
/// timeouts) instead of insta-stopping a just-started session.
pub const FORWARD_START_GRACE_S: f64 = 2.0;

/// Realtime forward (revised D3): the latest actuator outputs of another
/// vehicle (a PX4 SITL streaming `SERVO_OUTPUT_RAW`), normalized to 0..=1
/// by the tap that feeds the slot. The sender reads at its own tick —
/// latest value wins, nothing is queued, so a fast SITL cannot overrun the
/// link and a slow one cannot stack up.
///
/// The slot is shared with the (app-side) tap: `Arc<Mutex<ForwardState>>`.
pub type ForwardSlot = std::sync::Arc<std::sync::Mutex<ForwardState>>;

#[derive(Debug)]
pub struct ForwardState {
    /// Last value per motor, normalized 0..=1.
    pub values: Vec<f32>,
    /// Last raw `SERVO_OUTPUT_RAW` microseconds per motor, untouched, so the
    /// window can show the exact source value (not a re-derived estimate).
    pub raw_pwm: Vec<u16>,
    /// Seconds since epoch of the last tap update; `None` before the first.
    pub last_update_s: Option<f64>,
}

impl ForwardState {
    pub fn new(motors: usize) -> Self {
        Self {
            values: vec![0.0; motors],
            raw_pwm: vec![0; motors],
            last_update_s: None,
        }
    }
}

#[derive(Debug)]
pub struct ForwardSource {
    slot: ForwardSlot,
    motors: usize,
    /// Target PWM models, indexed by motor. Empty means "use the slot's
    /// pre-normalized values" (tests / pre-Task-5 fallback).
    models: Vec<PwmModel>,
    /// Idle-forward grace deadline (epoch seconds), set at construction.
    grace_until_s: f64,
}

impl ForwardSource {
    pub fn new(slot: ForwardSlot, motors: usize) -> Self {
        Self::with_models(slot, motors, Vec::new())
    }

    /// Forward the SITL's raw pulse through the same target PWM model used
    /// for manual/preset commands, so 1400 µs stays 1400 µs on a reversed or
    /// narrow-range mapped output.
    pub fn with_models(slot: ForwardSlot, motors: usize, models: Vec<PwmModel>) -> Self {
        Self {
            slot,
            motors,
            models,
            grace_until_s: now_epoch_s() + FORWARD_START_GRACE_S,
        }
    }

    fn target_fraction(&self, motor: usize, state: &ForwardState) -> Option<f32> {
        let Some(model) = self.models.get(motor) else {
            return state.values.get(motor).copied();
        };
        match model.units() {
            // A DShot target is addressed in normalized mixer demand, not
            // microseconds: feeding a PWM pulse into its 0…1999 range would
            // turn 1000 µs idle into a large nonzero throttle.
            OutputUnits::DShotThrottle => {
                state.values.get(motor).copied().map(|v| v.clamp(0.0, 1.0))
            }
            // PWM/OneShot targets keep the source's physical pulse: the
            // fraction is expressed in the *target* channel's range, so PX4
            // reproduces the same pulse after applying its own model.
            OutputUnits::Microseconds => {
                let raw = state.raw_pwm.get(motor).copied().unwrap_or(0);
                let pulse = f32::from(raw).clamp(f32::from(model.min_us), f32::from(model.max_us));
                Some(model.fraction_for_pulse(pulse).unwrap_or(0.0))
            }
        }
    }
}

impl RpmSource for ForwardSource {
    fn motor_count(&self) -> usize {
        self.motors
    }

    fn value(&mut self, t_s: f64, motor: usize) -> Option<f32> {
        self.values(t_s).into_iter().nth(motor).flatten()
    }

    /// One lock per tick: the four motors must all be forwarded from the
    /// same SITL `SERVO_OUTPUT_RAW` sample, never a mix of two frames.
    fn values(&mut self, _t_s: f64) -> Vec<Option<f32>> {
        let now = now_epoch_s();
        let Ok(guard) = self.slot.lock() else {
            return vec![Some(0.0); self.motors]; // poisoned lock: idle, never spin
        };
        let fresh = guard
            .last_update_s
            .is_some_and(|t| t + FORWARD_STALE_AFTER_S >= now);
        if fresh {
            return (0..self.motors)
                .map(|motor| self.target_fraction(motor, &guard))
                .collect();
        }
        // No sample yet (or the feed went stale) within the start grace:
        // forward idle so a just-started session survives the first taps.
        if now < self.grace_until_s {
            return vec![Some(0.0); self.motors];
        }
        vec![None; self.motors] // exhausted: the session stops itself
    }
}

/// Seconds since the epoch (`SystemTime`), for tap-freshness stamps.
fn now_epoch_s() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_ten_hz_four_motors() {
        let c = SenderConfig::default();
        assert!((c.tick_hz - 10.0).abs() < 1e-9);
        assert_eq!(c.motors.len(), 4);
        assert_eq!(c.motors[0].function, 1);
    }

    #[test]
    fn manual_source_latest_value_wins() {
        let mut s = ManualSource::new(vec![0.1, 0.2]);
        s.set_values(vec![0.5, 0.6]);
        assert_eq!(s.value(0.0, 0), Some(0.5));
        assert_eq!(s.value(0.0, 1), Some(0.6));
        // A wrong-length update is ignored, never truncates the session.
        s.set_values(vec![0.9]);
        assert_eq!(s.motor_count(), 2);
    }

    fn preset(kind: PresetKind, duration_s: f32, frequency_hz: f32) -> WaveformPreset {
        WaveformPreset {
            kind,
            amplitude: 0.8,
            frequency_hz,
            duration_s,
        }
    }

    #[test]
    fn presets_start_safe_except_the_step() {
        // Ramp/square/sine all command 0 at t=0. The step is *defined* by
        // jumping straight to the amplitude — safety there comes from the
        // interlock's slew limit shaping the actual spin-up on the wire.
        for kind in [PresetKind::Ramp, PresetKind::Square] {
            let p = preset(kind, 5.0, 1.0);
            assert_eq!(p.at(0.0), 0.0, "{kind:?} at t=0");
        }
        let step = preset(PresetKind::Step, 5.0, 1.0);
        assert_eq!(step.at(0.0), 0.8);
        // The raised cosine is 0 at t=0 by construction …
        let sine = preset(PresetKind::Sine, 5.0, 1.0);
        assert!(sine.at(0.0).abs() < 1e-6);
        // …and peaks at the amplitude (sine at the half period).
        assert!((sine.at(0.5) - 0.8).abs() < 1e-6, "sine peak");
    }

    #[test]
    fn ramp_is_linear_and_capped_at_duration() {
        let p = preset(PresetKind::Ramp, 4.0, 1.0);
        assert!((p.at(1.0) - 0.2).abs() < 1e-6);
        assert!((p.at(3.0) - 0.6).abs() < 1e-6);
        assert!((p.at(4.0) - 0.8).abs() < 1e-6);
        // The square toggles half-periods: low first, then high.
        let sq = preset(PresetKind::Square, 10.0, 1.0);
        assert_eq!(sq.at(0.25), 0.0);
        assert_eq!(sq.at(0.75), 0.8);
    }

    #[test]
    fn preset_source_exhausts_after_duration() {
        let mut s = PresetSource::new(preset(PresetKind::Step, 1.0, 1.0), 2);
        assert!(s.value(0.5, 0).is_some());
        assert!(s.value(1.0, 0).is_none(), "exactly at duration: done");
        assert!(s.value(5.0, 1).is_none());
    }

    #[test]
    fn any_source_routes_manual_updates_to_the_manual_variant() {
        let mut s = AnySource::Manual(ManualSource::new(vec![0.0]));
        s.set_values(vec![0.5]);
        assert_eq!(s.value(0.0, 0), Some(0.5));
        // A preset owns the values: slider updates are ignored, not queued.
        let mut s = AnySource::Preset(PresetSource::new(preset(PresetKind::Ramp, 1.0, 1.0), 1));
        s.set_values(vec![0.9]);
        assert_eq!(s.value(0.0, 0), Some(0.0), "ramp at t=0, slider ignored");
        assert!((s.value(0.5, 0).unwrap() - 0.4).abs() < 1e-6, "ramp mid");
    }

    fn forward_slot(motors: usize) -> ForwardSlot {
        std::sync::Arc::new(std::sync::Mutex::new(ForwardState::new(motors)))
    }

    /// Epoch seconds, matching the source's own clock.
    fn epoch_now() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }

    #[test]
    fn forward_follows_the_latest_slot_values() {
        let slot = forward_slot(2);
        {
            let mut st = slot.lock().unwrap();
            st.values = vec![0.2, 0.7];
            st.last_update_s = Some(epoch_now());
        }
        let mut s = ForwardSource::new(slot, 2);
        assert_eq!(s.value(0.0, 0), Some(0.2));
        assert_eq!(s.value(0.0, 1), Some(0.7));
    }

    #[test]
    fn forward_is_idle_within_the_start_grace_then_exhausts() {
        // No sample at all: within the grace the source forwards idle (0),
        // after it the session's exhaustion path stops the test.
        let mut s = ForwardSource::new(forward_slot(1), 1);
        assert_eq!(s.value(0.0, 0), Some(0.0), "grace: idle, not exhausted");
        // Sleep past the grace; still no sample.
        std::thread::sleep(std::time::Duration::from_millis(
            (FORWARD_START_GRACE_S * 1000.0) as u64 + 100,
        ));
        assert_eq!(s.value(0.0, 0), None, "past grace with no feed: exhausted");
    }

    #[test]
    fn forward_exhausts_when_the_feed_goes_stale() {
        let slot = forward_slot(1);
        {
            let mut st = slot.lock().unwrap();
            st.values = vec![0.4];
            st.last_update_s = Some(epoch_now());
        }
        // Stale the slot beyond FORWARD_STALE_AFTER_S and the grace.
        let mut s = ForwardSource::new(slot, 1);
        assert_eq!(s.value(0.0, 0), Some(0.4), "fresh feed passes through");
        {
            let mut st = s_slot(&s);
            st.last_update_s = Some(epoch_now() - FORWARD_STALE_AFTER_S - 10.0);
        }
        // Within the start grace the stale feed still forwards idle …
        assert_eq!(s.value(0.0, 0), Some(0.0));
        // … and after the grace it counts as exhausted.
        std::thread::sleep(std::time::Duration::from_millis(
            (FORWARD_START_GRACE_S * 1000.0) as u64 + 100,
        ));
        assert_eq!(s.value(0.0, 0), None, "stale feed: exhausted");
    }

    #[test]
    fn forward_converts_raw_pulse_through_the_target_model() {
        let slot = forward_slot(2);
        {
            let mut st = slot.lock().unwrap();
            st.values = vec![0.4, 0.4];
            st.raw_pwm = vec![1400, 1400];
            st.last_update_s = Some(epoch_now());
        }
        let models = vec![
            PwmModel::new(1000, 2000, false, 0.0).expect("model"),
            PwmModel::new(1100, 1900, false, 0.5).expect("model"),
        ];
        let mut s = ForwardSource::with_models(slot, 2, models);
        let values = s.values(0.0);
        assert!((values[0].expect("identity") - 0.4).abs() < 1e-6);
        assert!((values[1].expect("narrow") - 0.375).abs() < 1e-6);
    }

    #[test]
    fn forward_clamps_pulses_outside_the_target_range() {
        let slot = forward_slot(1);
        {
            let mut st = slot.lock().unwrap();
            st.raw_pwm = vec![900];
            st.last_update_s = Some(epoch_now());
        }
        let models = vec![PwmModel::new(1100, 1900, false, 0.0).expect("model")];
        let mut s = ForwardSource::with_models(slot, 1, models);
        assert_eq!(s.value(0.0, 0), Some(0.0));
    }

    fn dshot_model(dshot_min: f32) -> PwmModel {
        PwmModel::dshot(
            dshot_min,
            false,
            0.0,
            super::super::protocol::OutputProtocol::DShot {
                rate: super::super::protocol::DShotRate::R600,
            },
        )
        .expect("dshot model")
    }

    #[test]
    fn dshot_forward_uses_the_normalized_demand_not_the_source_pulse() {
        // Regression (R1): 1000 µs is the source's idle pulse. Treating it
        // as a DShot mixer count produced 1000/1999 ≈ 0.5 throttle.
        for (raw, normalized) in [(1000, 0.0), (1500, 0.5), (2000, 1.0)] {
            let slot = forward_slot(1);
            {
                let mut st = slot.lock().unwrap();
                st.values = vec![normalized];
                st.raw_pwm = vec![raw];
                st.last_update_s = Some(epoch_now());
            }
            let mut s = ForwardSource::with_models(slot, 1, vec![dshot_model(0.055)]);
            assert_eq!(s.value(0.0, 0), Some(normalized), "raw {raw} µs");
        }
    }

    #[test]
    fn dshot_forward_is_monotonic_for_one_two_and_four_motors() {
        for motors in [1usize, 2, 4] {
            let slot = forward_slot(motors);
            let values: Vec<f32> = [0.0, 0.25, 0.5, 1.0][..motors].to_vec();
            {
                let mut st = slot.lock().unwrap();
                st.values = values.clone();
                st.raw_pwm = vec![1000; motors];
                st.last_update_s = Some(epoch_now());
            }
            let models = vec![dshot_model(0.055); motors];
            let mut s = ForwardSource::with_models(slot, motors, models);
            let forwards = s.values(0.0);
            for (motor, expected) in values.iter().enumerate() {
                let forwarded = forwards[motor].expect("demand");
                assert!(
                    (forwarded - expected).abs() < 1e-6,
                    "{motors} motors: M{} {forwarded} != {expected}",
                    motor + 1
                );
                if motor > 0 {
                    assert!(
                        forwarded >= forwards[motor - 1].expect("previous"),
                        "{motors} motors: not monotonic"
                    );
                }
            }
        }
    }

    #[test]
    fn pwm_and_oneshot_forward_preserve_the_source_pulse() {
        let slot = forward_slot(2);
        {
            let mut st = slot.lock().unwrap();
            st.values = vec![0.4, 0.4];
            st.raw_pwm = vec![1400, 1400];
            st.last_update_s = Some(epoch_now());
        }
        let models = vec![
            PwmModel::new(1000, 2000, false, 0.0).expect("PWM"),
            PwmModel::new(1100, 1900, false, 0.0)
                .expect("OneShot")
                .with_protocol(super::super::protocol::OutputProtocol::OneShot),
        ];
        let mut s = ForwardSource::with_models(slot, 2, models);
        let values = s.values(0.0);
        assert!((values[0].expect("PWM") - 0.4).abs() < 1e-6);
        assert!((values[1].expect("OneShot") - 0.375).abs() < 1e-6);
    }

    #[test]
    fn forward_values_come_from_one_snapshot_for_all_motors() {
        let slot = forward_slot(4);
        {
            let mut st = slot.lock().unwrap();
            st.values = vec![0.25, 0.5, 0.75, 1.0];
            st.last_update_s = Some(epoch_now());
        }
        let mut s = ForwardSource::new(slot, 4);
        // One call = one slot lock, so a SITL frame arriving mid-tick cannot
        // split the four motors across two samples.
        assert_eq!(
            s.values(0.0),
            vec![Some(0.25), Some(0.5), Some(0.75), Some(1.0)]
        );
    }

    /// Test-only: reach the slot behind a source.
    fn s_slot(s: &ForwardSource) -> std::sync::MutexGuard<'_, ForwardState> {
        s.slot.lock().unwrap()
    }
}
