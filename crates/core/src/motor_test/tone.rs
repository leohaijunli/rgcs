//! AM32 beacon choices and PX4 DShot CLI addressing (MR-21).
//! Durations are upstream AM32 nominal playback times, not acoustic receipts.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::mapping::{OutputMap, OutputMapState, ServoChannel, ServoPort, MOTOR_FUNCTION_BASE};
use super::pwm::PwmModel;

const TWO_NOTE_MS: u64 = 300;
const SWEEP_MS: u64 = 600;
const TRIPLE_MS: u64 = 225;
const MAX_TONE_MOTORS: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorTone {
    Off,
    Rising,
    Falling,
    Sweep,
    Triple,
}

impl MotorTone {
    pub fn beacon(self) -> Option<u8> {
        match self {
            Self::Off => None,
            Self::Rising => Some(1),
            Self::Falling => Some(2),
            Self::Sweep => Some(3),
            Self::Triple => Some(4),
        }
    }

    pub fn duration(self) -> Duration {
        Duration::from_millis(match self {
            Self::Off => 0,
            Self::Rising | Self::Falling => TWO_NOTE_MS,
            Self::Sweep => SWEEP_MS,
            Self::Triple => TRIPLE_MS,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorToneSettings {
    pub start: MotorTone,
    pub stop: MotorTone,
}

impl Default for MotorToneSettings {
    fn default() -> Self {
        Self {
            start: MotorTone::Rising,
            stop: MotorTone::Falling,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorToneTrigger {
    Starting,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MotorToneError {
    #[error("M{motor}: a DShot output is required for ESC tones")]
    UnsupportedOutput { motor: u8 },
    #[error("M{motor}: DShot driver configuration does not match the motor mapping")]
    InvalidMapping { motor: u8 },
    #[error("tone target or output configuration changed")]
    TargetChanged,
    #[error("motor session changed before tone completion")]
    SessionChanged,
    #[error("target shell or motor output is busy")]
    Busy,
    #[error("motor start/tone cancelled")]
    Cancelled,
    #[error("ESC tone command timed out")]
    Timeout,
    #[error("ESC tone: {message}")]
    Shell { message: String },
}

/// Select only configured DShot motors. Mixed PWM/DShot benches never address
/// their pulse outputs with a DShot command.
pub fn tone_outputs(
    map: &OutputMap,
    models: &[PwmModel],
) -> Result<Vec<(u8, ServoChannel)>, MotorToneError> {
    if map.state != OutputMapState::Complete
        || map.active_count() == 0
        || map.active_count() > MAX_TONE_MOTORS
        || map.has_gap()
        || !map.issues.is_empty()
    {
        return Err(MotorToneError::InvalidMapping { motor: 1 });
    }
    let mut outputs = Vec::new();
    for motor in 1..=map.active_count() {
        let channel = map
            .channel_for(motor)
            .ok_or(MotorToneError::InvalidMapping { motor })?;
        let model = models
            .get(usize::from(motor - 1))
            .ok_or(MotorToneError::InvalidMapping { motor })?;
        if channel.channel == 0 || channel.channel > super::SERVO_CHANNELS_PER_PORT {
            return Err(MotorToneError::InvalidMapping { motor });
        }
        if model.protocol.is_dshot() {
            outputs.push((motor, channel));
        }
    }
    if outputs.is_empty() {
        return Err(MotorToneError::UnsupportedOutput { motor: 1 });
    }
    Ok(outputs)
}

/// Cross-check the current driver, not just cached parameter reads. PX4 v1.17
/// prints a zero-based Channel index; the CLI's `-m` is one-based on that same
/// physical FMU bank. Function IDs are PX4's 101=M1, 102=M2, ... .
pub fn commands_for_status(
    outputs: &[(u8, ServoChannel)],
    tone: MotorTone,
    status: &str,
) -> Result<Vec<String>, MotorToneError> {
    let Some(beacon) = tone.beacon() else {
        return Ok(Vec::new());
    };
    let invalid = || MotorToneError::InvalidMapping {
        motor: outputs.first().map_or(1, |(motor, _)| *motor),
    };
    let mut prefix = None;
    let mut mask = None;
    let mut initialized = false;
    let mut on = false;
    let mut channels = BTreeMap::new();
    for line in status.lines().map(str::trim) {
        if line == "Channel Configuration:" {
            continue;
        }
        if let Some((_, value)) = line.split_once("Param prefix: ") {
            if prefix.is_some() {
                return Err(invalid());
            }
            prefix = Some(match value.trim() {
                "PWM_MAIN" => ServoPort::Main,
                "PWM_AUX" => ServoPort::Aux,
                _ => return Err(invalid()),
            });
        } else if let Some((_, value)) = line.split_once("Outputs used: 0x") {
            if mask.is_some() {
                return Err(invalid());
            }
            mask = Some(u32::from_str_radix(value.trim(), 16).map_err(|_| invalid())?);
        } else if let Some((_, value)) = line.split_once("Outputs initialized: ") {
            initialized = value == "yes";
        } else if let Some((_, value)) = line.split_once("Outputs on: ") {
            on = value == "yes";
        } else if let Some(rest) = line.strip_prefix("Channel ") {
            let (index, fields) = rest.split_once(':').ok_or_else(invalid)?;
            let index: u8 = index.trim().parse().map_err(|_| invalid())?;
            let field = |name: &str| -> Result<i64, MotorToneError> {
                let value = fields
                    .split(',')
                    .find_map(|item| item.trim().strip_prefix(name))
                    .ok_or_else(invalid)?;
                value.trim().parse().map_err(|_| invalid())
            };
            let values = (field("func:")?, field("value:")?, field("disarmed:")?);
            if channels.insert(index, values).is_some() {
                return Err(invalid());
            }
        }
    }
    if !initialized || !on {
        return Err(MotorToneError::Busy);
    }
    let prefix = prefix.ok_or_else(invalid)?;
    let mask = mask.ok_or_else(invalid)?;
    let mut addressed = BTreeSet::new();
    let mut commands = Vec::new();
    for &(motor, channel) in outputs {
        let bad_motor = || MotorToneError::InvalidMapping { motor };
        let index = channel.channel.checked_sub(1).ok_or_else(bad_motor)?;
        let bit = 1u32.checked_shl(u32::from(index)).ok_or_else(bad_motor)?;
        let function = MOTOR_FUNCTION_BASE + i64::from(motor) - 1;
        let &(actual, value, disarmed) = channels.get(&index).ok_or_else(bad_motor)?;
        if channel.port != prefix
            || mask & bit == 0
            || actual != function
            || channels.values().filter(|(f, _, _)| *f == function).count() != 1
            || !addressed.insert(index)
        {
            return Err(bad_motor());
        }
        if value != 0 || disarmed != 0 {
            return Err(MotorToneError::Busy);
        }
        commands.push(format!("dshot beep{beacon} -m {}", channel.channel));
    }
    if commands.is_empty() {
        return Err(invalid());
    }
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "INFO [dshot] Outputs initialized: yes\nINFO [dshot] Outputs used: 0x5\nINFO [dshot] Outputs on: yes\nINFO [dshot] Param prefix: PWM_AUX\nChannel Configuration:\nChannel 0: func: 102, value: 0, failsafe: 0, disarmed: 0, min: 110, max: 1999, center: 0\nChannel 2: func: 101, value: 0, failsafe: 0, disarmed: 0, min: 110, max: 1999, center: 0";

    fn outputs() -> Vec<(u8, ServoChannel)> {
        vec![
            (
                1,
                ServoChannel {
                    port: ServoPort::Aux,
                    channel: 3,
                },
            ),
            (
                2,
                ServoChannel {
                    port: ServoPort::Aux,
                    channel: 1,
                },
            ),
        ]
    }

    #[test]
    fn patterns_use_only_transient_beacons_and_preserve_independent_settings() {
        for (tone, number, ms) in [
            (MotorTone::Rising, 1, 300),
            (MotorTone::Falling, 2, 300),
            (MotorTone::Sweep, 3, 600),
            (MotorTone::Triple, 4, 225),
        ] {
            assert_eq!(
                commands_for_status(&outputs(), tone, STATUS).expect("tone"),
                vec![
                    format!("dshot beep{number} -m 3"),
                    format!("dshot beep{number} -m 1")
                ]
            );
            assert_eq!(tone.duration(), Duration::from_millis(ms));
        }
        assert!(commands_for_status(&[], MotorTone::Off, "")
            .expect("off")
            .is_empty());
        let settings = MotorToneSettings {
            start: MotorTone::Off,
            stop: MotorTone::Sweep,
        };
        assert_eq!(
            serde_json::from_str::<MotorToneSettings>(
                &serde_json::to_string(&settings).expect("encode")
            )
            .expect("decode"),
            settings
        );
        assert!(
            serde_json::from_str::<MotorToneSettings>(r#"{"start":"save","stop":"falling"}"#)
                .is_err()
        );
    }

    #[test]
    fn current_driver_must_match_the_physical_bank_mask_and_function() {
        for wrong in [
            STATUS.replace("PWM_AUX", "PWM_MAIN"),
            STATUS.replace("0x5", "0x1"),
            STATUS.replace("func: 101", "func: 103"),
            STATUS.replace("Channel 2:", "Channel 3:"),
        ] {
            assert!(matches!(
                commands_for_status(&outputs(), MotorTone::Rising, &wrong),
                Err(MotorToneError::InvalidMapping { .. })
            ));
        }
        let duplicate = format!("{STATUS}\nChannel 4: func: 101, value: 0, disarmed: 0");
        assert!(commands_for_status(&outputs(), MotorTone::Rising, &duplicate).is_err());
        assert_eq!(
            commands_for_status(
                &outputs(),
                MotorTone::Rising,
                &STATUS.replace("value: 0", "value: 110")
            ),
            Err(MotorToneError::Busy)
        );
    }

    #[test]
    fn pulse_outputs_are_never_sent_dshot_commands() {
        use super::super::mapping::{build_output_map_checked, FunctionRead};
        use super::super::protocol::{DShotRate, OutputProtocol};
        let mut main = [FunctionRead::Absent; 8];
        main[0] = FunctionRead::Value(101);
        let mut aux = [FunctionRead::Absent; 8];
        aux[2] = FunctionRead::Value(102);
        let map = build_output_map_checked(4, &main, &aux);
        let pulse = PwmModel::new(1000, 2000, false, 0.0).expect("model");
        let dshot = pulse.with_protocol(OutputProtocol::DShot {
            rate: DShotRate::R600,
        });
        assert_eq!(
            tone_outputs(&map, &[pulse, dshot]).expect("mixed"),
            vec![(
                2,
                ServoChannel {
                    port: ServoPort::Aux,
                    channel: 3
                }
            )]
        );
        assert!(matches!(
            tone_outputs(&map, &[pulse, pulse]),
            Err(MotorToneError::UnsupportedOutput { .. })
        ));
    }
}
