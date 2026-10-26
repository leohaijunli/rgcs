//! Target PWM model and ACTUATOR_TEST compensation (plan §2, task 5).
//!
//! `ACTUATOR_TEST` takes a normalized function value. PX4 then applies the
//! target channel's PWM range, optional reversal and thrust model before the
//! physical pulse is produced. Sending `(pulse - 1000) / 1000` therefore only
//! matches a channel with 1000–2000 µs, no reversal and `THR_MDL_FAC == 0`.
//!
//! This module inverts that chain: the requested fraction of the target's
//! PWM range is converted with `a·x² + (1−a)·x` (and `x = 1−fraction` for a
//! reversed channel), matching PX4's forward model.

use super::protocol::{OutputProtocol, OutputUnits};

/// PWM endpoints accepted from `PWM_*_MINn` / `PWM_*_MAXn`.
pub const PWM_US_MAX: u16 = 3000;

/// PX4 `DSHOT_MAX_THROTTLE`: DShot mixer outputs are reported in 0…1999
/// (`SERVO_OUTPUT_RAW`), not microseconds.
pub const DSHOT_MAX_THROTTLE: u16 = 1999;

/// Fallback PWM range used when the mapped channel's `PWM_*_MINn`/`MAXn`
/// could not be read (or the motor is unmapped). This must never be applied
/// silently: the model carries [`PwmRangeSource::Fallback`] and the start
/// path refuses to launch on it.
pub const FALLBACK_MIN_US: u16 = 1100;
pub const FALLBACK_MAX_US: u16 = 1900;

/// Where a model's PWM range came from: read from the PX4, or the operator
/// fallback (which disallows starting until the mapping is refreshed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PwmRangeSource {
    /// `PWM_*_MINn`/`MAXn` were read from the FC this session targets.
    Fc,
    /// Not read (unreadable parameters or an unmapped motor); the fallback
    /// range applies and starting is blocked at the app boundary. Legacy
    /// recordings with no provenance also use this marker.
    #[default]
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum PwmModelError {
    #[error("PWM range must be 0…3000 µs with min < max")]
    InvalidRange,
    #[error("THR_MDL_FAC must be finite and in 0…1")]
    InvalidThrustFactor,
    #[error("DSHOT_MIN must be finite and in 0…1")]
    InvalidDShotMin,
    #[error("requested pulse is outside the mapped output range")]
    UnreachablePulse,
    #[error("requested throttle is outside the mapped output range")]
    UnreachableThrottle,
}

/// What one mapped output's numbers mean. `Pulse` is microseconds (PWM /
/// OneShot); `Throttle` is PX4's DShot mixer range (0…1).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputRange {
    Pulse { min_us: u16, max_us: u16 },
    Throttle { min: f32, max: f32 },
}

/// One mapped physical output's configuration.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PwmModel {
    /// PWM/OneShot: microseconds. DShot/BDShot: mixer units, 0…1999.
    pub min_us: u16,
    pub max_us: u16,
    pub reversed: bool,
    pub thrust_factor: f32,
    /// Legacy flag recording a nonzero `PWM_*_DISn` disarmed pulse. This is
    /// not evidence that the active motor function is disabled.
    pub disabled: bool,
    /// Whether the range was read from the FC or is the fallback default.
    /// Legacy recordings without provenance remain explicitly unverified.
    #[serde(default)]
    pub range_source: PwmRangeSource,
    /// Protocol detected (or overridden) for the bank carrying this output.
    #[serde(default)]
    pub protocol: OutputProtocol,
    /// PX4 `DSHOT_MIN` (0…1); the idle throttle fraction for DShot. Unused
    /// for PWM/OneShot.
    #[serde(default)]
    pub dshot_min: f32,
}

impl Default for PwmModel {
    fn default() -> Self {
        Self {
            min_us: FALLBACK_MIN_US,
            max_us: FALLBACK_MAX_US,
            reversed: false,
            thrust_factor: 0.0,
            disabled: false,
            range_source: PwmRangeSource::Fallback,
            protocol: OutputProtocol::Unknown,
            dshot_min: 0.0,
        }
    }
}

impl PwmModel {
    /// Validate the target channel's PWM configuration.
    pub fn new(
        min_us: u16,
        max_us: u16,
        reversed: bool,
        thrust_factor: f32,
    ) -> Result<Self, PwmModelError> {
        if min_us >= max_us || max_us > PWM_US_MAX {
            return Err(PwmModelError::InvalidRange);
        }
        if !thrust_factor.is_finite() || !(0.0..=1.0).contains(&thrust_factor) {
            return Err(PwmModelError::InvalidThrustFactor);
        }
        Ok(Self {
            min_us,
            max_us,
            reversed,
            thrust_factor,
            disabled: false,
            range_source: PwmRangeSource::Fc,
            protocol: OutputProtocol::Pwm { rate_hz: None },
            dshot_min: 0.0,
        })
    }

    /// DShot/BDShot channel. PX4 ignores `PWM_*_MINn/MAXn` for these outputs
    /// and maps the normalized value onto `DSHOT_MIN × 1999 … 1999`
    /// (`DShot::apply_min_values`), so the range is derived from `DSHOT_MIN`.
    pub fn dshot(
        dshot_min: f32,
        reversed: bool,
        thrust_factor: f32,
        protocol: OutputProtocol,
    ) -> Result<Self, PwmModelError> {
        if !dshot_min.is_finite() || !(0.0..1.0).contains(&dshot_min) {
            return Err(PwmModelError::InvalidDShotMin);
        }
        if !thrust_factor.is_finite() || !(0.0..=1.0).contains(&thrust_factor) {
            return Err(PwmModelError::InvalidThrustFactor);
        }
        let min_us = (dshot_min * f32::from(DSHOT_MAX_THROTTLE)).round() as u16;
        Ok(Self {
            min_us,
            max_us: DSHOT_MAX_THROTTLE,
            reversed,
            thrust_factor,
            disabled: false,
            range_source: PwmRangeSource::Fc,
            protocol,
            dshot_min,
        })
    }

    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn with_protocol(mut self, protocol: OutputProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Unit of `min_us`/`max_us` for this output.
    pub fn units(self) -> OutputUnits {
        self.protocol.units()
    }

    /// Typed output range: microseconds for PWM/OneShot, 0…1 throttle for
    /// DShot/BDShot.
    pub fn output_range(self) -> OutputRange {
        match self.units() {
            OutputUnits::Microseconds => OutputRange::Pulse {
                min_us: self.min_us,
                max_us: self.max_us,
            },
            OutputUnits::DShotThrottle => OutputRange::Throttle {
                min: f32::from(self.min_us) / f32::from(DSHOT_MAX_THROTTLE),
                max: f32::from(self.max_us) / f32::from(DSHOT_MAX_THROTTLE),
            },
        }
    }

    /// Width of the output range in `min_us`/`max_us` units (µs for
    /// PWM/OneShot, 0…1999 mixer units for DShot/BDShot).
    pub fn span_us(self) -> f32 {
        f32::from(self.max_us) - f32::from(self.min_us)
    }

    /// Fraction of this channel's output range for a requested pulse. This is
    /// the value the safety limits and slew limiter condition, *before* the
    /// thrust model compensation.
    pub fn fraction_for_pulse(self, pulse_us: f32) -> Result<f32, PwmModelError> {
        if !pulse_us.is_finite()
            || pulse_us < f32::from(self.min_us)
            || pulse_us > f32::from(self.max_us)
        {
            return Err(PwmModelError::UnreachablePulse);
        }
        Ok((pulse_us - f32::from(self.min_us)) / self.span_us())
    }

    /// Fraction of this channel's output range for a requested output.
    ///
    /// PWM/OneShot take microseconds; DShot/BDShot take the throttle in
    /// 0…1 (`SERVO_OUTPUT_RAW` divided by [`DSHOT_MAX_THROTTLE`]), which PX4
    /// clamps to `DSHOT_MIN … 1.0`.
    pub fn fraction_for_output(self, value: f32) -> Result<f32, PwmModelError> {
        match self.units() {
            OutputUnits::Microseconds => self.fraction_for_pulse(value),
            OutputUnits::DShotThrottle => {
                let min = f32::from(self.min_us) / f32::from(DSHOT_MAX_THROTTLE);
                let max = f32::from(self.max_us) / f32::from(DSHOT_MAX_THROTTLE);
                if !value.is_finite() || value < min || value > max {
                    return Err(PwmModelError::UnreachableThrottle);
                }
                Ok((value - min) / (max - min))
            }
        }
    }

    /// The normalized `ACTUATOR_TEST.param1` value that PX4 turns back into
    /// the requested fraction of the physical PWM range.
    pub fn command_for_fraction(self, fraction: f32) -> f32 {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let x = if self.reversed {
            1.0 - fraction
        } else {
            fraction
        };
        let command = self.thrust_factor * x * x + (1.0 - self.thrust_factor) * x;
        command.clamp(0.0, 1.0)
    }

    pub fn command_for_pulse(self, pulse_us: f32) -> Result<f32, PwmModelError> {
        Ok(self.command_for_fraction(self.fraction_for_pulse(pulse_us)?))
    }

    /// Protocol-aware variant of [`Self::command_for_pulse`]: microseconds for
    /// PWM/OneShot, throttle 0…1 for DShot/BDShot.
    pub fn command_for_output(self, value: f32) -> Result<f32, PwmModelError> {
        Ok(self.command_for_fraction(self.fraction_for_output(value)?))
    }

    /// Inverse of [`Self::command_for_fraction`] in physical output units.
    /// Exposed for offline round-trip verification.
    pub fn pulse_for_command(self, command: f32) -> Result<f32, PwmModelError> {
        if !command.is_finite() || !(0.0..=1.0).contains(&command) {
            return Err(PwmModelError::UnreachablePulse);
        }
        let a = f64::from(self.thrust_factor);
        let c = f64::from(command);
        let x = if a == 0.0 {
            c
        } else {
            // a·x² + (1−a)·x − c = 0; take the root in 0…1.
            let linear = 1.0 - a;
            (-linear + (linear * linear + 4.0 * a * c).sqrt()) / (2.0 * a)
        };
        let fraction = if self.reversed { 1.0 - x } else { x };
        Ok(f32::from(self.min_us) + fraction as f32 * self.span_us())
    }

    /// Protocol-aware variant of [`Self::pulse_for_command`]: microseconds for
    /// PWM/OneShot, throttle 0…1 for DShot/BDShot.
    pub fn output_for_command(self, command: f32) -> Result<f32, PwmModelError> {
        let output = self.pulse_for_command(command)?;
        match self.units() {
            OutputUnits::Microseconds => Ok(output),
            OutputUnits::DShotThrottle => Ok(output / f32::from(DSHOT_MAX_THROTTLE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &[(u16, u16, bool, f32)] = &[
        (1000, 2000, false, 0.0),
        (1100, 1900, false, 0.0),
        (1100, 1900, false, 0.5),
        (1000, 2000, true, 0.0),
        (1100, 1900, true, 0.5),
    ];

    #[test]
    fn the_documented_examples_match() {
        let plain = PwmModel::new(1000, 2000, false, 0.0).expect("model");
        assert!((plain.command_for_pulse(1400.0).expect("pulse") - 0.4).abs() < 1e-6);

        let narrow = PwmModel::new(1100, 1900, false, 0.0).expect("model");
        assert!((narrow.command_for_pulse(1400.0).expect("pulse") - 0.375).abs() < 1e-6);

        let modeled = PwmModel::new(1100, 1900, false, 0.5).expect("model");
        assert!((modeled.command_for_pulse(1400.0).expect("pulse") - 0.257_812_5).abs() < 1e-6);
    }

    #[test]
    fn round_trip_recovers_the_requested_pulse_within_one_microsecond() {
        let pulses = [
            1000.0, 1100.0, 1200.0, 1300.0, 1400.0, 1500.0, 1600.0, 1700.0, 1800.0, 1900.0, 2000.0,
        ];
        for &(min, max, reversed, factor) in CASES {
            let model = PwmModel::new(min, max, reversed, factor).expect("model");
            for pulse in pulses {
                if pulse < f32::from(min) || pulse > f32::from(max) {
                    continue;
                }
                let command = model.command_for_pulse(pulse).expect("command");
                let recovered = model.pulse_for_command(command).expect("inverse");
                assert!(
                    (recovered - pulse).abs() <= 1.0,
                    "{min}–{max} rev={reversed} a={factor}: {pulse} -> {command} -> {recovered}"
                );
            }
        }
    }

    #[test]
    fn reversal_inverts_the_fraction_before_the_thrust_model() {
        let model = PwmModel::new(1000, 2000, true, 0.0).expect("model");
        assert!((model.command_for_fraction(0.25) - 0.75).abs() < 1e-6);
        assert!((model.command_for_fraction(0.75) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn invalid_ranges_and_thrust_factors_are_rejected() {
        assert_eq!(
            PwmModel::new(2000, 1000, false, 0.0),
            Err(PwmModelError::InvalidRange)
        );
        assert_eq!(
            PwmModel::new(1000, 3001, false, 0.0),
            Err(PwmModelError::InvalidRange)
        );
        assert_eq!(
            PwmModel::new(1000, 2000, false, 1.5),
            Err(PwmModelError::InvalidThrustFactor)
        );
        assert_eq!(
            PwmModel::new(1000, 2000, false, f32::NAN),
            Err(PwmModelError::InvalidThrustFactor)
        );
    }

    #[test]
    fn non_finite_or_out_of_range_pulses_are_rejected() {
        let model = PwmModel::default();
        assert_eq!(
            model.command_for_pulse(f32::NAN),
            Err(PwmModelError::UnreachablePulse)
        );
        assert_eq!(
            model.command_for_pulse(999.0),
            Err(PwmModelError::UnreachablePulse)
        );
        assert_eq!(
            model.command_for_pulse(2001.0),
            Err(PwmModelError::UnreachablePulse)
        );
    }

    #[test]
    fn fallback_default_is_1100_1900_and_explicitly_marked() {
        let model = PwmModel::default();
        assert_eq!(model.min_us, FALLBACK_MIN_US);
        assert_eq!(model.max_us, FALLBACK_MAX_US);
        assert_eq!(model.range_source, PwmRangeSource::Fallback);
        // A fallback model still encodes its range fraction exactly.
        let mid = model.command_for_pulse(1500.0).expect("mid pulse");
        assert!(
            (model.pulse_for_command(mid).expect("mid round trip") - 1500.0).abs() <= 1.0,
            "fallback round trip: {mid} -> {}",
            model.pulse_for_command(mid).expect("mid round trip")
        );
    }

    #[test]
    fn a_constructor_model_comes_from_the_fc() {
        let model = PwmModel::new(1150, 1850, false, 0.0).expect("model");
        assert_eq!(model.range_source, PwmRangeSource::Fc);
        // with_disabled only flips the flag; the source marker is kept.
        assert_eq!(model.with_disabled(true).range_source, PwmRangeSource::Fc);
    }

    fn legacy_snapshot() -> serde_json::Value {
        serde_json::json!({
            "min_us": 1000,
            "max_us": 2000,
            "reversed": true,
            "thrust_factor": 0.5,
            "disabled": true
        })
    }

    #[test]
    fn legacy_pwm_snapshots_load_without_inventing_fc_provenance() {
        let model: PwmModel = serde_json::from_value(legacy_snapshot()).expect("legacy snapshot");
        assert_eq!((model.min_us, model.max_us), (1000, 2000));
        assert!(model.reversed && model.disabled);
        assert_eq!(model.thrust_factor, 0.5);
        assert_eq!(model.range_source, PwmRangeSource::Fallback);
        assert_eq!(model.protocol, OutputProtocol::Unknown);
        assert_eq!(model.dshot_min, 0.0);
    }

    #[test]
    fn current_snapshots_preserve_explicit_provenance_protocol_and_dshot_min() {
        let models = [
            PwmModel::new(1150, 1850, true, 0.5).expect("PWM model"),
            PwmModel::dshot(
                0.055,
                false,
                0.0,
                OutputProtocol::BDShot {
                    rate: super::super::protocol::DShotRate::R600,
                },
            )
            .expect("BDShot model"),
            PwmModel::default(),
        ];
        for model in models {
            let json = serde_json::to_string(&model).expect("serialize model");
            let loaded: PwmModel = serde_json::from_str(&json).expect("current snapshot");
            assert_eq!(loaded, model);
        }
    }

    #[test]
    fn legacy_defaults_do_not_accept_malformed_or_missing_required_fields() {
        for (field, invalid) in [
            ("range_source", serde_json::json!("unverified")),
            ("range_source", serde_json::Value::Null),
            ("protocol", serde_json::json!({ "kind": "unsupported" })),
            ("protocol", serde_json::Value::Null),
            ("dshot_min", serde_json::json!("0.055")),
            ("dshot_min", serde_json::Value::Null),
        ] {
            let mut json = legacy_snapshot();
            json[field] = invalid;
            assert!(serde_json::from_value::<PwmModel>(json).is_err(), "{field}");
        }
        let mut missing_range = legacy_snapshot();
        missing_range
            .as_object_mut()
            .expect("snapshot object")
            .remove("min_us");
        assert!(serde_json::from_value::<PwmModel>(missing_range).is_err());
    }

    #[test]
    fn a_non_finite_fraction_commands_zero_not_nan() {
        let model = PwmModel::default();
        assert_eq!(model.command_for_fraction(f32::NAN), 0.0);
        assert_eq!(model.command_for_fraction(f32::INFINITY), 0.0);
    }

    #[test]
    fn dshot_range_comes_from_dshot_min_not_the_pwm_parameters() {
        let model = PwmModel::dshot(
            0.055,
            false,
            0.0,
            OutputProtocol::DShot {
                rate: super::super::protocol::DShotRate::R600,
            },
        )
        .expect("dshot model");
        assert_eq!(model.min_us, 110);
        assert_eq!(model.max_us, DSHOT_MAX_THROTTLE);
        assert_eq!(model.units(), OutputUnits::DShotThrottle);
        assert_eq!(
            model.output_range(),
            OutputRange::Throttle {
                min: 110.0 / 1999.0,
                max: 1.0
            }
        );
        assert!(model.protocol.is_dshot());
    }

    #[test]
    fn dshot_commands_round_trip_through_the_px4_forward_model() {
        let model = PwmModel::dshot(
            0.055,
            false,
            0.0,
            OutputProtocol::DShot {
                rate: super::super::protocol::DShotRate::R300,
            },
        )
        .expect("dshot model");
        let min = f32::from(model.min_us) / f32::from(DSHOT_MAX_THROTTLE);
        for throttle in [min, 0.1, 0.25, 0.5, 0.75, 1.0] {
            let command = model.command_for_output(throttle).expect("command");
            let recovered = model.output_for_command(command).expect("inverse");
            assert!(
                (recovered - throttle).abs() <= 1e-4,
                "{throttle} -> {command} -> {recovered}"
            );
        }
    }

    #[test]
    fn dshot_rejects_throttles_outside_the_dshot_range() {
        let model = PwmModel::dshot(
            0.055,
            false,
            0.0,
            OutputProtocol::DShot {
                rate: super::super::protocol::DShotRate::R150,
            },
        )
        .expect("dshot model");
        assert_eq!(
            model.fraction_for_output(0.0),
            Err(PwmModelError::UnreachableThrottle)
        );
        assert_eq!(
            model.fraction_for_output(f32::NAN),
            Err(PwmModelError::UnreachableThrottle)
        );
        assert_eq!(
            model.fraction_for_output(1.01),
            Err(PwmModelError::UnreachableThrottle)
        );
    }

    #[test]
    fn invalid_dshot_min_is_rejected() {
        let protocol = OutputProtocol::DShot {
            rate: super::super::protocol::DShotRate::R600,
        };
        assert_eq!(
            PwmModel::dshot(1.0, false, 0.0, protocol),
            Err(PwmModelError::InvalidDShotMin)
        );
        assert_eq!(
            PwmModel::dshot(-0.1, false, 0.0, protocol),
            Err(PwmModelError::InvalidDShotMin)
        );
        assert_eq!(
            PwmModel::dshot(f32::NAN, false, 0.0, protocol),
            Err(PwmModelError::InvalidDShotMin)
        );
    }
}
