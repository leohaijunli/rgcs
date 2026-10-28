//! Scope samples for the motor-test window's signal list + plot area.
//!
//! Every producer (SITL servo tap, target-FC telemetry tap, the sender's
//! command readback) turns what it sees into [`ScopeSample`]s — one
//! `(id, host time, value)` triple each — and pushes them into a shared
//! [`ScopeBus`]. A single flusher in the shell drains the bus at ~20 Hz and
//! emits the batch to the window, which keeps one ring buffer per id.
//!
//! A signal exists in the window only once its first sample arrives, so
//! "is there an RPM / current signal at all" is answered by the signal
//! list itself: ESC ids are never emitted for a frame that was not received.
//!
//! Signal ids (the window groups and labels them):
//!
//! | id                              | meaning                               | unit |
//! |---------------------------------|---------------------------------------|------|
//! | `sitl.servo1..4`                | SITL `SERVO_OUTPUT_RAW` main outputs  | µs   |
//! | `cmd.pwm1..4`                   | converted ACTUATOR_TEST command      | µs / DShot counts |
//! | `fc.motor1..4`                  | mapped target-FC output per motor    | µs / DShot counts |
//! | `mag.<source>.raw_total`         | source-specific `|B|` before calibration | nT |
//! | `mag.<source>.cal_total`         | source-specific `|A·(m−b)|`          | nT   |
//! | `bat{k}.current`                | `BATTERY_STATUS` current, k = 1-based | A    |
//! | `esc{k}.rpm/current/voltage`    | `ESC_STATUS`, k = 1-based             | rpm / A / V |

use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::calib::mag::MagCalibration;
use crate::motor_test::mapping::{
    OutputMap, OutputMapState, ServoChannel, ServoPort, SERVO_CHANNELS_PER_PORT,
};
use crate::motor_test::protocol::{OutputProtocol, OutputUnits};
use crate::motor_test::pwm::PwmModel;
use crate::signals::derived::esc_frame_range;
pub use crate::signals::derived::ESCS_PER_FRAME;

/// Servo / motor outputs shown per source.
pub const SERVO_COUNT: usize = 4;
/// Highest ESC number (1-based) the scope will emit.
pub const MAX_ESC: usize = 8;
/// Pending samples kept when nobody drains the bus (window closed): the
/// oldest are dropped first so memory stays bounded.
pub const MAX_PENDING: usize = 20_000;

/// A complete MAVLink magnetometer triple shown in the Motor Test scope.
///
/// PX4 1.17 maps `SCALED_IMU`, `SCALED_IMU2` and `SCALED_IMU3` to distinct
/// `sensor_mag` instances. Keeping that identity in every scope id prevents a
/// value or calibration result from one instance entering another ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MagScopeSource {
    HighresImu,
    ScaledImu1,
    ScaledImu2,
    ScaledImu3,
}

impl MagScopeSource {
    /// Stable part of the frontend signal id.
    pub const fn id(self) -> &'static str {
        match self {
            Self::HighresImu => "highres_imu",
            Self::ScaledImu1 => "scaled_imu1",
            Self::ScaledImu2 => "scaled_imu2",
            Self::ScaledImu3 => "scaled_imu3",
        }
    }

    fn raw_total_id(self) -> String {
        format!("mag.{}.raw_total", self.id())
    }

    fn cal_total_id(self) -> String {
        format!("mag.{}.cal_total", self.id())
    }
}

/// Why a plotted series has an intentional break instead of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeGapReason {
    Released,
    Stale,
    RunBoundary,
}

/// Local IPC identity and units for one configured motor output.
/// The same metadata accompanies predicted commands and mapped FC feedback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeOutputMeta {
    pub connection_id: usize,
    pub revision: u64,
    pub motor: u8,
    pub channel: ServoChannel,
    pub unit: OutputUnits,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutputScopeError {
    #[error("output configuration is incomplete")]
    IncompleteConfiguration,
    #[error("M{motor} has no unique valid output mapping")]
    InvalidMapping { motor: u8 },
    #[error("M{motor} has no output model")]
    MissingModel { motor: u8 },
    #[error("M{motor} has an unknown output protocol")]
    UnknownProtocol { motor: u8 },
}

/// Build one complete set of output descriptors from a configuration snapshot.
/// Unknown protocols do not inherit the protocol helper's default µs unit.
pub fn output_metadata(
    connection_id: usize,
    revision: u64,
    map: &OutputMap,
    models: &[PwmModel],
) -> Result<Vec<ScopeOutputMeta>, OutputScopeError> {
    let motors = usize::from(map.active_count());
    if revision == 0 || map.state != OutputMapState::Complete || motors == 0 || motors > SERVO_COUNT
    {
        return Err(OutputScopeError::IncompleteConfiguration);
    }
    let mut outputs: Vec<ScopeOutputMeta> = Vec::with_capacity(motors);
    for assignment in map.active_motors() {
        let motor = assignment.motor;
        let channel = assignment
            .channel
            .and_then(|channel| ServoChannel::new(channel.port, channel.channel))
            .ok_or(OutputScopeError::InvalidMapping { motor })?;
        if outputs.iter().any(|output| output.channel == channel) {
            return Err(OutputScopeError::InvalidMapping { motor });
        }
        let model = models
            .get(usize::from(motor - 1))
            .ok_or(OutputScopeError::MissingModel { motor })?;
        if model.protocol == OutputProtocol::Unknown {
            return Err(OutputScopeError::UnknownProtocol { motor });
        }
        outputs.push(ScopeOutputMeta {
            connection_id,
            revision,
            motor,
            channel,
            unit: model.protocol.units(),
        });
    }
    Ok(outputs)
}

/// One scope sample. `t` is host epoch seconds at receive time — the only
/// clock the SITL link, the serial link and the sender share.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeSample {
    pub id: String,
    pub t: f64,
    pub v: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<ScopeGapReason>,
    /// Absent for fixed-unit signals (SITL, magnetic, ESC and battery).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<ScopeOutputMeta>,
}

impl ScopeSample {
    pub fn value(id: String, t: f64, v: f64) -> Self {
        Self {
            id,
            t,
            v: Some(v),
            gap: None,
            output: None,
        }
    }

    pub fn gap(
        id: String,
        t: f64,
        reason: ScopeGapReason,
        output: Option<ScopeOutputMeta>,
    ) -> Self {
        Self {
            id,
            t,
            v: None,
            gap: Some(reason),
            output,
        }
    }

    fn with_output(mut self, output: &ScopeOutputMeta) -> Self {
        self.output = Some(output.clone());
        self
    }
}

/// `servo{n}` samples from the SITL's selected `SERVO_OUTPUT_RAW` bank.
/// Raw microseconds, untouched (0 = output not driven).
pub fn servo_samples(prefix: &str, t: f64, pwm: [u16; SERVO_COUNT]) -> Vec<ScopeSample> {
    pwm.iter()
        .enumerate()
        .map(|(i, &us)| ScopeSample::value(format!("{prefix}.servo{}", i + 1), t, f64::from(us)))
        .collect()
}

/// `cmd.pwm{n}` samples: converted output values from the sender's models.
/// Metadata identifies their units and configuration. None breaks the plot on
/// release; it must not be invented as a zero or minimum physical pulse.
pub fn command_samples(
    t: f64,
    values: &[Option<f32>],
    outputs: &[ScopeOutputMeta],
) -> Vec<ScopeSample> {
    outputs
        .iter()
        .filter(|output| (1..=SERVO_COUNT as u8).contains(&output.motor))
        .filter_map(|output| {
            let value = values.get(usize::from(output.motor - 1))?;
            Some(match value {
                Some(value) => {
                    ScopeSample::value(format!("cmd.pwm{}", output.motor), t, f64::from(*value))
                        .with_output(output)
                }
                None => ScopeSample::gap(
                    format!("cmd.pwm{}", output.motor),
                    t,
                    ScopeGapReason::Released,
                    Some(output.clone()),
                ),
            })
        })
        .collect()
}

/// Emit only the motor functions mapped to the received MAIN/AUX frame.
/// Physical channels retain their bank identity; no servo-to-motor guess is made.
pub fn mapped_feedback_samples(
    t: f64,
    port: ServoPort,
    values: &[u16; SERVO_CHANNELS_PER_PORT as usize],
    outputs: &[ScopeOutputMeta],
) -> Vec<ScopeSample> {
    outputs
        .iter()
        .filter(|output| {
            output.channel.port == port && (1..=SERVO_COUNT as u8).contains(&output.motor)
        })
        .filter_map(|output| {
            let slot = usize::from(output.channel.channel).checked_sub(1)?;
            let value = values.get(slot)?;
            Some(
                ScopeSample::value(format!("fc.motor{}", output.motor), t, f64::from(*value))
                    .with_output(output),
            )
        })
        .collect()
}

/// Total field before and after calibration, nanotesla.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagTotals {
    /// `|m_raw|`, as the FC reports it.
    pub raw: f64,
    /// `|A·(m_raw − b)|`; `None` when no calibration is loaded.
    pub cal: Option<f64>,
}

impl MagTotals {
    /// The value the alarm and the status readout watch: calibrated when a
    /// calibration is loaded, else raw pass-through (unchanged behaviour).
    pub fn effective(&self) -> f64 {
        self.cal.unwrap_or(self.raw)
    }
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `raw_nt` is one magnetometer triple already converted to nanotesla.
pub fn mag_totals(raw_nt: [f64; 3], cal: Option<&MagCalibration>) -> MagTotals {
    MagTotals {
        raw: norm(raw_nt),
        // Current compensation (K·I) stays off here exactly as in the
        // alarm path: no single "motor current" is defined yet.
        cal: cal.map(|c| norm(c.apply(raw_nt, None))),
    }
}

/// One source's raw total always; its calibrated total only while a
/// calibration is loaded. Source identity is part of both ids.
pub fn mag_samples(source: MagScopeSource, t: f64, totals: MagTotals) -> Vec<ScopeSample> {
    let mut out = vec![ScopeSample::value(source.raw_total_id(), t, totals.raw)];
    if let Some(cal) = totals.cal {
        out.push(ScopeSample::value(source.cal_total_id(), t, cal));
    }
    out
}

/// `bat{k}.current` — one battery's instantaneous current in amperes; `k` is
/// 1-based from `BATTERY_STATUS.id` (PX4's single battery is `bat1`, and is the
/// vehicle's total current). `current_ca` is the wire value in centiamps; a
/// negative value (`-1` = "autopilot does not measure the current") emits no
/// sample, so the list shows the group as having no data rather than a bogus
/// trace at `-0.01 A`.
pub fn battery_samples(id: u8, t: f64, current_ca: i16) -> Vec<ScopeSample> {
    if current_ca < 0 {
        return Vec::new();
    }
    vec![ScopeSample::value(
        format!("bat{}.current", u16::from(id) + 1),
        t,
        f64::from(current_ca) / 100.0,
    )]
}

/// `esc{k}.rpm/current/voltage` for one `ESC_STATUS` frame. `index` is
/// the first zero-based ESC (0 = ESCs 1–4, 4 = ESCs 5–8).
/// Non-finite values (fields the ESC does not report) are skipped, so an ESC
/// without current telemetry never gets a
/// `current` signal of garbage; a field reported as exact zero is still
/// emitted — the window tells "all zero" apart from "no data".
pub fn esc_samples(
    index: u8,
    t: f64,
    rpm: &[i32; ESCS_PER_FRAME],
    voltage: &[f32; ESCS_PER_FRAME],
    current: &[f32; ESCS_PER_FRAME],
) -> Vec<ScopeSample> {
    let mut out = Vec::with_capacity(ESCS_PER_FRAME * 3);
    for (slot, esc) in esc_frame_range(index, MAX_ESC).enumerate() {
        let k = esc + 1;
        out.push(ScopeSample::value(
            format!("esc{k}.rpm"),
            t,
            f64::from(rpm[slot]),
        ));
        for (name, v) in [("voltage", voltage[slot]), ("current", current[slot])] {
            if v.is_finite() {
                out.push(ScopeSample::value(
                    format!("esc{k}.{name}"),
                    t,
                    f64::from(v),
                ));
            }
        }
    }
    out
}

/// Shared producer → flusher queue. Cloning shares the queue.
#[derive(Debug, Clone, Default)]
pub struct ScopeBus(Arc<Mutex<Vec<ScopeSample>>>);

impl ScopeBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append samples; drops the oldest beyond [`MAX_PENDING`].
    pub fn push_all(&self, samples: Vec<ScopeSample>) {
        if samples.is_empty() {
            return;
        }
        if let Ok(mut q) = self.0.lock() {
            q.extend(samples);
            if q.len() > MAX_PENDING {
                let excess = q.len() - MAX_PENDING;
                q.drain(..excess);
            }
        }
    }

    /// Take everything queued so far (empty when nothing arrived).
    pub fn drain(&self) -> Vec<ScopeSample> {
        self.0
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    /// Discard everything queued (a window re-open starts clean).
    pub fn clear(&self) {
        if let Ok(mut q) = self.0.lock() {
            q.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motor_test::mapping::OutputAssignment;
    use crate::motor_test::protocol::DShotRate;

    fn ids(s: &[ScopeSample]) -> Vec<&str> {
        s.iter().map(|x| x.id.as_str()).collect()
    }

    fn output_fixture() -> (OutputMap, Vec<PwmModel>) {
        let channels = [
            ServoChannel::new(ServoPort::Main, 5).expect("MAIN5"),
            ServoChannel::new(ServoPort::Aux, 1).expect("AUX1"),
            ServoChannel::new(ServoPort::Main, 1).expect("MAIN1"),
            ServoChannel::new(ServoPort::Aux, 8).expect("AUX8"),
        ];
        let map = OutputMap {
            motors: channels
                .into_iter()
                .enumerate()
                .map(|(index, channel)| OutputAssignment {
                    motor: index as u8 + 1,
                    channel: Some(channel),
                })
                .collect(),
            state: OutputMapState::Complete,
            issues: Vec::new(),
        };
        let models = [
            OutputProtocol::Pwm { rate_hz: Some(400) },
            OutputProtocol::DShot {
                rate: DShotRate::R600,
            },
            OutputProtocol::OneShot,
            OutputProtocol::BDShot {
                rate: DShotRate::R300,
            },
        ]
        .into_iter()
        .map(|protocol| {
            PwmModel::new(1000, 2000, false, 0.0)
                .expect("model")
                .with_protocol(protocol)
        })
        .collect();
        (map, models)
    }

    #[test]
    fn servo_samples_are_numbered_from_one() {
        let s = servo_samples("sitl", 1.5, [1000, 1100, 1200, 1300]);
        assert_eq!(
            ids(&s),
            vec!["sitl.servo1", "sitl.servo2", "sitl.servo3", "sitl.servo4"]
        );
        assert_eq!(s[2].v, Some(1200.0));
        assert!(s.iter().all(|x| x.t == 1.5));
    }

    #[test]
    fn command_samples_bind_units_and_keep_release_gaps() {
        let (map, models) = output_fixture();
        let outputs = output_metadata(17, 3, &map, &models).expect("metadata");
        let s = command_samples(
            0.0,
            &[Some(1500.0), Some(1300.0), None, Some(1900.0), Some(1600.0)],
            &outputs,
        );
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].id, "cmd.pwm1");
        assert_eq!(s[0].v, Some(1500.0));
        assert_eq!(s[3].v, Some(1900.0));
        assert_eq!(s[2].v, None);
        assert_eq!(s[2].gap, Some(ScopeGapReason::Released));
        let json = serde_json::to_value(&s).expect("scope JSON");
        assert!(json[2]["v"].is_null(), "release is a gap on the wire");
        assert_eq!(json[2]["gap"], "released");
        assert_eq!(json[0]["output"]["unit"], "microseconds");
        assert_eq!(json[3]["output"]["unit"], "d_shot_throttle");
        assert_eq!(json[3]["output"]["revision"], 3);
    }

    #[test]
    fn output_metadata_binds_protocol_units_and_physical_channels() {
        let (map, models) = output_fixture();
        let outputs = output_metadata(17, 3, &map, &models).expect("metadata");
        assert_eq!(outputs.len(), SERVO_COUNT);
        assert!(outputs
            .iter()
            .all(|output| output.connection_id == 17 && output.revision == 3));
        assert_eq!(
            outputs[0].channel,
            ServoChannel::new(ServoPort::Main, 5).expect("MAIN5")
        );
        assert_eq!(
            outputs[1].channel,
            ServoChannel::new(ServoPort::Aux, 1).expect("AUX1")
        );
        assert_eq!(
            outputs.iter().map(|output| output.unit).collect::<Vec<_>>(),
            vec![
                OutputUnits::Microseconds,
                OutputUnits::DShotThrottle,
                OutputUnits::Microseconds,
                OutputUnits::DShotThrottle,
            ]
        );
    }

    #[test]
    fn output_metadata_refuses_unknown_or_ambiguous_configuration() {
        let (mut map, mut models) = output_fixture();
        assert_eq!(
            output_metadata(17, 0, &map, &models),
            Err(OutputScopeError::IncompleteConfiguration)
        );
        assert_eq!(
            output_metadata(17, 3, &map, &models[..2]),
            Err(OutputScopeError::MissingModel { motor: 3 })
        );
        models[1].protocol = OutputProtocol::Unknown;
        assert_eq!(
            output_metadata(17, 3, &map, &models),
            Err(OutputScopeError::UnknownProtocol { motor: 2 })
        );
        models[1].protocol = OutputProtocol::DShot {
            rate: DShotRate::R600,
        };
        map.motors[1].channel = map.motors[0].channel;
        assert_eq!(
            output_metadata(17, 3, &map, &models),
            Err(OutputScopeError::InvalidMapping { motor: 2 })
        );
        map.state = OutputMapState::Partial;
        assert_eq!(
            output_metadata(17, 3, &map, &models),
            Err(OutputScopeError::IncompleteConfiguration)
        );
    }

    #[test]
    fn mapped_feedback_uses_the_correct_bank_channel_and_units() {
        let (map, models) = output_fixture();
        let outputs = output_metadata(17, 3, &map, &models).expect("metadata");
        let main = mapped_feedback_samples(
            1.0,
            ServoPort::Main,
            &[1300, 0, 0, 0, 1500, 0, 0, 0],
            &outputs,
        );
        let aux =
            mapped_feedback_samples(1.0, ServoPort::Aux, &[750, 0, 0, 0, 0, 0, 0, 900], &outputs);
        assert_eq!(ids(&main), vec!["fc.motor1", "fc.motor3"]);
        assert_eq!(ids(&aux), vec!["fc.motor2", "fc.motor4"]);
        assert_eq!((main[0].v, main[1].v), (Some(1500.0), Some(1300.0)));
        assert_eq!((aux[0].v, aux[1].v), (Some(750.0), Some(900.0)));
        assert_eq!(main[0].output, Some(outputs[0].clone()));
        assert_eq!(aux[0].output, Some(outputs[1].clone()));
        let json = serde_json::to_value(&aux).expect("scope JSON");
        assert_eq!(json[0]["output"]["unit"], "d_shot_throttle");
        assert_eq!(json[0]["output"]["channel"]["port"], "aux");
    }

    #[test]
    fn outputs_without_metadata_or_values_do_not_invent_samples() {
        let (map, models) = output_fixture();
        let outputs = output_metadata(17, 3, &map, &models).expect("metadata");
        assert!(command_samples(0.0, &[Some(1500.0)], &[]).is_empty());
        assert!(mapped_feedback_samples(0.0, ServoPort::Main, &[1500; 8], &[]).is_empty());
        let short = command_samples(0.0, &[Some(1500.0)], &outputs);
        assert_eq!(ids(&short), vec!["cmd.pwm1"]);
    }

    #[test]
    fn battery_current_is_amps_and_numbered_from_one() {
        let s = battery_samples(0, 1.0, 1234);
        assert_eq!(ids(&s), vec!["bat1.current"]);
        assert!(s[0].v.is_some_and(|value| (value - 12.34).abs() < 1e-9));
        assert_eq!(ids(&battery_samples(1, 1.0, 50)), vec!["bat2.current"]);
    }

    #[test]
    fn battery_without_current_measurement_emits_no_signal() {
        assert!(battery_samples(0, 1.0, -1).is_empty());
    }

    #[test]
    fn mag_totals_without_calibration_has_no_cal_trace() {
        let t = mag_totals([30_000.0, 40_000.0, 0.0], None);
        assert!((t.raw - 50_000.0).abs() < 1e-9);
        assert_eq!(t.cal, None);
        assert!((t.effective() - 50_000.0).abs() < 1e-9);
        assert_eq!(
            ids(&mag_samples(MagScopeSource::HighresImu, 0.0, t)),
            vec!["mag.highres_imu.raw_total"]
        );
    }

    #[test]
    fn mag_totals_with_calibration_applies_a_times_raw_minus_b() {
        // A = I, b = [3000, 4000, 0]: the offset removed leaves 27k/36k.
        let cal =
            MagCalibration::from_json(r#"{"a": [[1,0,0],[0,1,0],[0,0,1]], "b": [3000, 4000, 0]}"#)
                .expect("valid calibration");
        let t = mag_totals([30_000.0, 40_000.0, 0.0], Some(&cal));
        assert!((t.raw - 50_000.0).abs() < 1e-9);
        let c = t.cal.expect("calibrated total");
        assert!(
            (c - 45_000.0).abs() < 1e-6,
            "3-4-5 triangle scaled by 9000: {c}"
        );
        assert!((t.effective() - c).abs() < 1e-12);
        assert_eq!(
            ids(&mag_samples(MagScopeSource::HighresImu, 0.0, t)),
            vec!["mag.highres_imu.raw_total", "mag.highres_imu.cal_total"]
        );
    }

    #[test]
    fn magnetometer_sources_have_disjoint_scope_ids_and_values() {
        let fixtures = [
            (MagScopeSource::HighresImu, 50_000.0, "highres_imu"),
            (MagScopeSource::ScaledImu1, 10_000.0, "scaled_imu1"),
            (MagScopeSource::ScaledImu2, 20_000.0, "scaled_imu2"),
            (MagScopeSource::ScaledImu3, 30_000.0, "scaled_imu3"),
        ];
        let mut samples = Vec::new();
        for (source, raw, id) in fixtures {
            let source_samples = mag_samples(
                source,
                1.0,
                MagTotals {
                    raw,
                    cal: Some(raw - 100.0),
                },
            );
            assert_eq!(source_samples[0].id, format!("mag.{id}.raw_total"));
            assert_eq!(source_samples[0].v, Some(raw));
            assert_eq!(source_samples[1].id, format!("mag.{id}.cal_total"));
            assert_eq!(source_samples[1].v, Some(raw - 100.0));
            samples.extend(source_samples);
        }
        let unique: std::collections::HashSet<_> = samples.iter().map(|s| &s.id).collect();
        assert_eq!(
            unique.len(),
            8,
            "every source and calibration state has its own ring id"
        );
    }

    #[test]
    fn esc_frame_zero_maps_to_escs_one_to_four() {
        let s = esc_samples(
            0,
            2.0,
            &[1000, 2000, 3000, 40_000],
            &[12.0; 4],
            &[1.0, 2.0, 3.0, 4.0],
        );
        assert_eq!(s.len(), 12);
        let rpm4 = s.iter().find(|x| x.id == "esc4.rpm").expect("esc4.rpm");
        assert_eq!(rpm4.v, Some(40_000.0), "rpm above i16::MAX must not wrap");
        assert!(s.iter().any(|x| x.id == "esc3.current" && x.v == Some(3.0)));
    }

    #[test]
    fn esc_frame_four_maps_to_five_to_eight_and_out_of_range_is_dropped() {
        let s = esc_samples(
            4,
            0.0,
            &[1000, 2000, 3000, 4000],
            &[12.0; 4],
            &[1.0, 2.0, 3.0, 4.0],
        );
        assert_eq!(s.len(), 12);
        assert!(s.iter().any(|x| x.id == "esc5.rpm" && x.v == Some(1000.0)));
        assert!(s.iter().any(|x| x.id == "esc8.current" && x.v == Some(4.0)));
        assert!(!s.iter().any(|x| x.id == "esc1.rpm"));
        for index in [8, u8::MAX] {
            assert!(esc_samples(index, 0.0, &[1; 4], &[1.0; 4], &[1.0; 4]).is_empty());
        }
    }

    #[test]
    fn esc_frames_use_the_first_index_and_clip_the_last_slots() {
        let s = esc_samples(1, 0.0, &[1000, 2000, 3000, 4000], &[12.0; 4], &[1.0; 4]);
        assert_eq!(s[0].id, "esc2.rpm");
        assert!(s.iter().any(|x| x.id == "esc5.rpm" && x.v == Some(4000.0)));
        let partial = esc_samples(6, 0.0, &[1000, 2000, 3000, 4000], &[12.0; 4], &[1.0; 4]);
        assert_eq!(partial.len(), 6);
        assert_eq!(partial[0].id, "esc7.rpm");
        assert!(partial
            .iter()
            .any(|x| x.id == "esc8.rpm" && x.v == Some(2000.0)));
    }

    #[test]
    fn esc_without_current_telemetry_emits_no_current_signal() {
        let s = esc_samples(0, 0.0, &[100; 4], &[f32::NAN; 4], &[f32::NAN; 4]);
        assert!(s.iter().all(|x| x.id.ends_with(".rpm")));
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn bus_drains_once_and_caps_pending() {
        let bus = ScopeBus::new();
        bus.push_all(servo_samples("sitl", 0.0, [1; 4]));
        assert_eq!(bus.drain().len(), 4);
        assert!(bus.drain().is_empty(), "drained exactly once");

        let big: Vec<ScopeSample> = (0..MAX_PENDING + 10)
            .map(|i| ScopeSample::value("x".into(), i as f64, 0.0))
            .collect();
        bus.push_all(big);
        let out = bus.drain();
        assert_eq!(out.len(), MAX_PENDING);
        assert_eq!(out[0].t, 10.0, "the oldest were dropped first");
    }
}
