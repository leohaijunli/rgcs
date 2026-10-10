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
}
