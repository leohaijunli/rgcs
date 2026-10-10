//! Command sources for the motor test (plan §7.3): where each tick's target
//! value comes from. A2 ships `ManualSource` (slider values, latest wins);
//! Profile replay (A6) and speed functions (A7) implement the same trait.

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
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetKind {
    Step,
    Ramp,
    Square,
    Sine,
}

/// One preset's parameters.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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

/// The session's swappable source: manual sliders or a preset waveform
/// (profile/function sources join in A6/A7).
#[derive(Debug)]
pub enum AnySource {
    Manual(ManualSource),
    Preset(PresetSource),
}

impl AnySource {
    /// Latest-value-wins slider update; ignored while a preset runs (the
    /// waveform owns the values for its duration).
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
        }
    }

    fn value(&mut self, t_s: f64, motor: usize) -> Option<f32> {
        match self {
            AnySource::Manual(m) => m.value(t_s, motor),
            AnySource::Preset(p) => p.value(t_s, motor),
        }
    }
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
}
