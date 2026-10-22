//! Physical output mapping for the motor test (plan §2, task 4).
//!
//! `ACTUATOR_TEST` addresses a logical **motor function** (Motor 1..4). PX4
//! maps that function to a physical output through `PWM_MAIN_FUNCx` /
//! `PWM_AUX_FUNCx`; feedback must be read from that same port/channel. Treating
//! `SERVO_OUTPUT_RAW` servo1..4 as motors 1..4 is only correct for the default
//! wiring and is wrong as soon as MAIN/AUX or the channel assignment differs.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Physical PWM bank carrying a `SERVO_OUTPUT_RAW` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ServoPort {
    /// `SERVO_OUTPUT_RAW.port == 0`; PX4 `PWM_MAIN_FUNCx`.
    Main,
    /// `SERVO_OUTPUT_RAW.port == 1`; PX4 `PWM_AUX_FUNCx`.
    Aux,
}

impl ServoPort {
    pub const fn wire(self) -> u8 {
        match self {
            Self::Main => 0,
            Self::Aux => 1,
        }
    }

    pub const fn from_wire(port: u8) -> Option<Self> {
        match port {
            0 => Some(Self::Main),
            1 => Some(Self::Aux),
            _ => None,
        }
    }

    pub const fn param_prefix(self) -> &'static str {
        match self {
            Self::Main => "PWM_MAIN",
            Self::Aux => "PWM_AUX",
        }
    }
}

/// One output connector: bank + 1-based channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServoChannel {
    pub port: ServoPort,
    pub channel: u8,
}

impl ServoChannel {
    pub const fn new(port: ServoPort, channel: u8) -> Option<Self> {
        if channel != 0 && channel <= SERVO_CHANNELS_PER_PORT {
            Some(Self { port, channel })
        } else {
            None
        }
    }
}

pub const SERVO_CHANNELS_PER_PORT: u8 = 8;
/// PX4 `output_functions.yaml`: Motor1 = 101, Motor2 = 102, ...
pub const MOTOR_FUNCTION_BASE: i64 = 101;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OutputMapState {
    /// Every requested motor function is assigned to exactly one channel.
    Complete,
    /// Some functions are unassigned or their parameters could not be read.
    Partial,
    /// A function is assigned more than once, so its feedback is ambiguous.
    Invalid,
    /// No assignment parameter could be read.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum MappingIssue {
    /// The function parameter for this output could not be read.
    Unreadable { port: ServoPort, channel: u8 },
    /// Two physical outputs claim the same motor function.
    Duplicate {
        motor: u8,
        first: ServoChannel,
        second: ServoChannel,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OutputAssignment {
    /// Logical Motor function (1-based).
    pub motor: u8,
    /// Physical output, or `None` when unmapped/ambiguous/unreadable.
    pub channel: Option<ServoChannel>,
}

/// Function-to-output mapping for the motor functions this test can drive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OutputMap {
    pub motors: Vec<OutputAssignment>,
    pub state: OutputMapState,
    pub issues: Vec<MappingIssue>,
}

impl OutputMap {
    /// No mapping is known yet (before the target parameters are read).
    pub fn unknown(motors: u8) -> Self {
        Self {
            motors: (1..=motors)
                .map(|motor| OutputAssignment {
                    motor,
                    channel: None,
                })
                .collect(),
            state: OutputMapState::Unknown,
            issues: Vec::new(),
        }
    }

    pub fn channel_for(&self, motor: u8) -> Option<ServoChannel> {
        self.motors
            .iter()
            .find(|assignment| assignment.motor == motor)
            .and_then(|assignment| assignment.channel)
    }
}

/// Build the mapping from raw `PWM_MAIN_FUNC1..8` / `PWM_AUX_FUNC1..8`
/// values. `None` marks a parameter that could not be read; `Some(0)` is a
/// valid "output disabled" value, not a read failure.
pub fn build_output_map(
    motors: u8,
    main_functions: &[Option<i64>; SERVO_CHANNELS_PER_PORT as usize],
    aux_functions: &[Option<i64>; SERVO_CHANNELS_PER_PORT as usize],
) -> OutputMap {
    let mut issues = Vec::new();
    let mut readable = 0usize;
    let mut assignments = Vec::with_capacity(motors as usize);
    let mut invalid = false;

    for (port, values) in [
        (ServoPort::Main, main_functions),
        (ServoPort::Aux, aux_functions),
    ] {
        for (index, value) in values.iter().enumerate() {
            if value.is_some() {
                readable += 1;
            } else {
                issues.push(MappingIssue::Unreadable {
                    port,
                    channel: index as u8 + 1,
                });
            }
        }
    }

    for motor in 1..=motors {
        let function = MOTOR_FUNCTION_BASE + i64::from(motor) - 1;
        let mut channels = Vec::new();
        for (port, values) in [
            (ServoPort::Main, main_functions),
            (ServoPort::Aux, aux_functions),
        ] {
            for (index, value) in values.iter().enumerate() {
                if *value == Some(function) {
                    if let Some(channel) = ServoChannel::new(port, index as u8 + 1) {
                        channels.push(channel);
                    }
                }
            }
        }

        let channel = match channels.as_slice() {
            [only] => Some(*only),
            [first, second, ..] => {
                invalid = true;
                issues.push(MappingIssue::Duplicate {
                    motor,
                    first: *first,
                    second: *second,
                });
                None
            }
            [] => None,
        };
        assignments.push(OutputAssignment { motor, channel });
    }

    let assigned = assignments
        .iter()
        .filter(|assignment| assignment.channel.is_some())
        .count();
    let state = if invalid {
        OutputMapState::Invalid
    } else if readable == 0 {
        OutputMapState::Unknown
    } else if assigned == usize::from(motors) {
        OutputMapState::Complete
    } else {
        OutputMapState::Partial
    };

    OutputMap {
        motors: assignments,
        state,
        issues,
    }
}

/// The feedback PWM for one motor, taken from its mapped physical output.
/// Returns `None` when the motor is unmapped or that bank has not arrived.
pub fn mapped_feedback_pwm(
    map: &OutputMap,
    main: Option<&[u16; SERVO_CHANNELS_PER_PORT as usize]>,
    aux: Option<&[u16; SERVO_CHANNELS_PER_PORT as usize]>,
    motor: u8,
) -> Option<u16> {
    let channel = map.channel_for(motor)?;
    let pwm = match channel.port {
        ServoPort::Main => main?,
        ServoPort::Aux => aux?,
    };
    pwm.get(usize::from(channel.channel.checked_sub(1)?))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> [Option<i64>; 8] {
        [None; 8]
    }

    #[test]
    fn permuted_main_and_aux_assignments_map_to_the_physical_channel() {
        let mut main = [Some(0); 8];
        let mut aux = [Some(0); 8];
        main[0] = Some(101); // MAIN1 = Motor1
        main[4] = Some(102); // MAIN5 = Motor2
        aux[1] = Some(103); // AUX2 = Motor3
        aux[2] = Some(104); // AUX3 = Motor4

        let map = build_output_map(4, &main, &aux);
        assert_eq!(map.state, OutputMapState::Complete);
        assert_eq!(
            map.channel_for(1),
            Some(ServoChannel {
                port: ServoPort::Main,
                channel: 1
            })
        );
        assert_eq!(
            map.channel_for(2),
            Some(ServoChannel {
                port: ServoPort::Main,
                channel: 5
            })
        );
        assert_eq!(
            map.channel_for(3),
            Some(ServoChannel {
                port: ServoPort::Aux,
                channel: 2
            })
        );
        assert_eq!(
            map.channel_for(4),
            Some(ServoChannel {
                port: ServoPort::Aux,
                channel: 3
            })
        );
    }

    #[test]
    fn feedback_comes_from_the_mapped_bank_and_channel() {
        let mut main = [Some(0); 8];
        main[4] = Some(102); // MAIN5 = Motor2
        let map = build_output_map(2, &main, &[Some(0); 8]);
        let main_pwm = [1000, 1010, 1020, 1030, 1450, 1050, 1060, 1070];
        let aux_pwm = [9000; 8];

        assert_eq!(
            mapped_feedback_pwm(&map, Some(&main_pwm), Some(&aux_pwm), 2),
            Some(1450)
        );
        assert_eq!(mapped_feedback_pwm(&map, None, Some(&aux_pwm), 2), None);
    }

    #[test]
    fn duplicate_assignment_is_invalid_and_has_no_feedback() {
        let mut main = [Some(0); 8];
        main[0] = Some(102);
        main[1] = Some(102);
        let map = build_output_map(2, &main, &[Some(0); 8]);

        assert_eq!(map.state, OutputMapState::Invalid);
        assert_eq!(map.channel_for(2), None);
        assert!(map
            .issues
            .iter()
            .any(|issue| matches!(issue, MappingIssue::Duplicate { motor: 2, .. })));
    }

    #[test]
    fn unreadable_and_disabled_outputs_are_distinguished() {
        let mut main = empty();
        main[0] = Some(0); // readable, output disabled
        let map = build_output_map(1, &main, &empty());
        assert_eq!(map.state, OutputMapState::Partial);
        assert_eq!(map.channel_for(1), None);
        assert!(map.issues.iter().any(|issue| matches!(
            issue,
            MappingIssue::Unreadable {
                port: ServoPort::Main,
                channel: 2
            }
        )));

        let unknown = build_output_map(1, &empty(), &empty());
        assert_eq!(unknown.state, OutputMapState::Unknown);
    }

    #[test]
    fn servo_channel_rejects_out_of_range_channels() {
        assert!(ServoChannel::new(ServoPort::Main, 0).is_none());
        assert!(ServoChannel::new(ServoPort::Main, 9).is_none());
        assert!(ServoChannel::new(ServoPort::Main, 8).is_some());
    }
}
