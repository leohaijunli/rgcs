//! PX4 output-protocol detection for a motor output bank.
//!
//! PX4 selects the physical protocol **per timer group** with the
//! `PWM_MAIN_TIMn` / `PWM_AUX_TIMn` parameters (`pwm_timer_param` in
//! `mixer_module/module.yaml`):
//!
//! | value | protocol |
//! | --- | --- |
//! | `-8` / `-7` / `-6` | BDShot150 / BDShot300 / BDShot600 |
//! | `-5` / `-4` / `-3` | DShot150 / DShot300 / DShot600 |
//! | `-1` | OneShot |
//! | `50` / `100` / `200` / `400` | PWM at that rate |
//!
//! MAVLink does not expose the channel → timer mapping, so detection is
//! **per bank**: every readable `TIMn` value on the mapped bank is folded into
//! one protocol. PX4 leaves unassigned timer groups at the 400 Hz default, so
//! a lone `400` never outvotes a DShot/OneShot group; two different non-PWM
//! protocols (or two different non-default PWM rates) mean the bank is mixed
//! and the result is [`OutputProtocol::Unknown`].

use serde::{Deserialize, Serialize};

/// DShot bit rate. The number is the bit rate in kbit/s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DShotRate {
    R150,
    R300,
    R600,
}

impl DShotRate {
    pub const fn kbit(self) -> u16 {
        match self {
            Self::R150 => 150,
            Self::R300 => 300,
            Self::R600 => 600,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::R150 => "DShot150",
            Self::R300 => "DShot300",
            Self::R600 => "DShot600",
        }
    }
}

/// Physical protocol PX4 applies to a bank of outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputProtocol {
    /// PWM; `rate_hz` is `None` when the bank mixes non-default rates.
    Pwm {
        rate_hz: Option<u16>,
    },
    OneShot,
    DShot {
        rate: DShotRate,
    },
    BDShot {
        rate: DShotRate,
    },
    /// No readable `TIMn` parameter, or conflicting groups on one bank.
    Unknown,
}

/// Unit of the numbers in [`crate::motor_test::PwmModel::min_us`] /
/// `max_us` and in `SERVO_OUTPUT_RAW` for this protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputUnits {
    /// PWM/OneShot: microseconds.
    Microseconds,
    /// DShot/BDShot: mixer units, 0…1999 (`DSHOT_MAX_THROTTLE`).
    DShotThrottle,
}

impl OutputProtocol {
    /// Decode one PX4 `PWM_*_TIMn` value. `None` for values PX4 does not
    /// document (including `0`), so they are treated as unreadable.
    pub const fn from_tim_value(value: i64) -> Option<Self> {
        match value {
            -8 => Some(Self::BDShot {
                rate: DShotRate::R150,
            }),
            -7 => Some(Self::BDShot {
                rate: DShotRate::R300,
            }),
            -6 => Some(Self::BDShot {
                rate: DShotRate::R600,
            }),
            -5 => Some(Self::DShot {
                rate: DShotRate::R150,
            }),
            -4 => Some(Self::DShot {
                rate: DShotRate::R300,
            }),
            -3 => Some(Self::DShot {
                rate: DShotRate::R600,
            }),
            -1 => Some(Self::OneShot),
            50 => Some(Self::Pwm { rate_hz: Some(50) }),
            100 => Some(Self::Pwm { rate_hz: Some(100) }),
            200 => Some(Self::Pwm { rate_hz: Some(200) }),
            400 => Some(Self::Pwm { rate_hz: Some(400) }),
            _ => None,
        }
    }

    pub const fn units(self) -> OutputUnits {
        match self {
            Self::DShot { .. } | Self::BDShot { .. } => OutputUnits::DShotThrottle,
            Self::Pwm { .. } | Self::OneShot | Self::Unknown => OutputUnits::Microseconds,
        }
    }

    pub const fn is_dshot(self) -> bool {
        matches!(self, Self::DShot { .. } | Self::BDShot { .. })
    }

    /// Short label for the bench panel.
    pub fn label(self) -> String {
        match self {
            Self::Pwm {
                rate_hz: Some(rate),
            } => format!("PWM {rate} Hz"),
            Self::Pwm { rate_hz: None } => "PWM".to_owned(),
            Self::OneShot => "OneShot".to_owned(),
            Self::DShot { rate } => rate.label().to_owned(),
            Self::BDShot { rate } => format!("B{}", rate.label()),
            Self::Unknown => "unknown".to_owned(),
        }
    }
}

/// Fold every readable `PWM_{MAIN,AUX}_TIMn` value of one bank into a single
/// protocol. `None` marks a parameter that could not be read.
///
/// PX4's 400 Hz default is only used when no other protocol and no other PWM
/// rate is present, so a DShot motor group is not hidden by untouched timer
/// groups. Conflicting non-PWM protocols yield [`OutputProtocol::Unknown`].
pub fn detect_bank_protocol(tim_values: &[Option<i64>]) -> OutputProtocol {
    let mut non_pwm: Option<OutputProtocol> = None;
    let mut pwm_rates: Vec<u16> = Vec::new();
    let mut mixed_non_pwm = false;

    for value in tim_values.iter().flatten() {
        match OutputProtocol::from_tim_value(*value) {
            Some(OutputProtocol::Pwm {
                rate_hz: Some(rate),
            }) => {
                if !pwm_rates.contains(&rate) {
                    pwm_rates.push(rate);
                }
            }
            Some(OutputProtocol::Pwm { rate_hz: None }) => {}
            Some(protocol) => match non_pwm {
                None => non_pwm = Some(protocol),
                Some(known) if known == protocol => {}
                Some(_) => mixed_non_pwm = true,
            },
            None => {}
        }
    }

    if mixed_non_pwm {
        return OutputProtocol::Unknown;
    }
    if let Some(protocol) = non_pwm {
        return protocol;
    }

    // Ignore the 400 Hz default when a configured rate exists.
    let configured: Vec<u16> = pwm_rates
        .iter()
        .copied()
        .filter(|rate| *rate != 400)
        .collect();
    match configured.as_slice() {
        [rate] => OutputProtocol::Pwm {
            rate_hz: Some(*rate),
        },
        [] if pwm_rates.contains(&400) => OutputProtocol::Pwm { rate_hz: Some(400) },
        [] => OutputProtocol::Unknown,
        _ => OutputProtocol::Pwm { rate_hz: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_tim_values_decode_to_protocols() {
        let cases = [
            (
                -8,
                OutputProtocol::BDShot {
                    rate: DShotRate::R150,
                },
            ),
            (
                -7,
                OutputProtocol::BDShot {
                    rate: DShotRate::R300,
                },
            ),
            (
                -6,
                OutputProtocol::BDShot {
                    rate: DShotRate::R600,
                },
            ),
            (
                -5,
                OutputProtocol::DShot {
                    rate: DShotRate::R150,
                },
            ),
            (
                -4,
                OutputProtocol::DShot {
                    rate: DShotRate::R300,
                },
            ),
            (
                -3,
                OutputProtocol::DShot {
                    rate: DShotRate::R600,
                },
            ),
            (-1, OutputProtocol::OneShot),
            (50, OutputProtocol::Pwm { rate_hz: Some(50) }),
            (100, OutputProtocol::Pwm { rate_hz: Some(100) }),
            (200, OutputProtocol::Pwm { rate_hz: Some(200) }),
            (400, OutputProtocol::Pwm { rate_hz: Some(400) }),
        ];
        for (value, expected) in cases {
            assert_eq!(
                OutputProtocol::from_tim_value(value),
                Some(expected),
                "{value}"
            );
        }
        for undocumented in [0, 1, 49, -2, -9, 1000] {
            assert_eq!(
                OutputProtocol::from_tim_value(undocumented),
                None,
                "{undocumented}"
            );
        }
    }

    #[test]
    fn dshot_banks_ignore_px4s_400hz_default() {
        // Motors on a DShot timer, other groups still at PX4's 400 Hz default.
        let tim = [Some(-3), Some(-3), Some(400), Some(400)];
        assert_eq!(
            detect_bank_protocol(&tim),
            OutputProtocol::DShot {
                rate: DShotRate::R600
            }
        );
    }

    #[test]
    fn a_uniform_pwm_bank_keeps_its_configured_rate() {
        let tim = [Some(50), Some(50), Some(400)];
        assert_eq!(
            detect_bank_protocol(&tim),
            OutputProtocol::Pwm { rate_hz: Some(50) }
        );

        let all_default = [Some(400), Some(400)];
        assert_eq!(
            detect_bank_protocol(&all_default),
            OutputProtocol::Pwm { rate_hz: Some(400) }
        );
    }

    #[test]
    fn conflicting_groups_are_reported_as_unknown() {
        let mixed_protocols = [Some(-3), Some(-1)];
        assert_eq!(
            detect_bank_protocol(&mixed_protocols),
            OutputProtocol::Unknown
        );

        let mixed_dshot_rates = [Some(-5), Some(-3)];
        assert_eq!(
            detect_bank_protocol(&mixed_dshot_rates),
            OutputProtocol::Unknown
        );

        let mixed_pwm_rates = [Some(50), Some(200)];
        assert_eq!(
            detect_bank_protocol(&mixed_pwm_rates),
            OutputProtocol::Pwm { rate_hz: None }
        );
    }

    #[test]
    fn unreadable_or_undocumented_values_do_not_claim_a_protocol() {
        assert_eq!(detect_bank_protocol(&[]), OutputProtocol::Unknown);
        assert_eq!(detect_bank_protocol(&[None, None]), OutputProtocol::Unknown);
        assert_eq!(
            detect_bank_protocol(&[None, Some(0)]),
            OutputProtocol::Unknown
        );
        assert_eq!(
            detect_bank_protocol(&[None, Some(-3)]),
            OutputProtocol::DShot {
                rate: DShotRate::R600
            }
        );
    }

    #[test]
    fn units_follow_the_protocol() {
        assert_eq!(
            OutputProtocol::DShot {
                rate: DShotRate::R300
            }
            .units(),
            OutputUnits::DShotThrottle
        );
        assert_eq!(OutputProtocol::OneShot.units(), OutputUnits::Microseconds);
        assert_eq!(
            OutputProtocol::Pwm { rate_hz: Some(50) }.units(),
            OutputUnits::Microseconds
        );
        assert_eq!(OutputProtocol::Unknown.units(), OutputUnits::Microseconds);
    }
}
