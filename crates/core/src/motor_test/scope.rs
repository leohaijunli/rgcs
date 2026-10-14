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
//! | `fwd.cmd1..4`                   | value sent to the FC as ACTUATOR_TEST | %    |
//! | `fc.servo1..4`                  | real FC `SERVO_OUTPUT_RAW` (serial)   | µs   |
//! | `mag.raw_total`, `mag.cal_total`| `|B|` before / after calibration      | nT   |
//! | `esc{k}.rpm/current/voltage`    | `ESC_STATUS`, k = 1-based             | rpm / A / V |

use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::calib::mag::MagCalibration;

/// Servo / motor outputs shown per source.
pub const SERVO_COUNT: usize = 4;
/// ESC slots one `ESC_STATUS` frame covers (`index * 4 + slot`).
pub const ESCS_PER_FRAME: usize = 4;
/// Highest ESC number (1-based) the scope will emit.
pub const MAX_ESC: usize = 8;
/// Pending samples kept when nobody drains the bus (window closed): the
/// oldest are dropped first so memory stays bounded.
pub const MAX_PENDING: usize = 20_000;

pub const MAG_RAW_TOTAL_ID: &str = "mag.raw_total";
pub const MAG_CAL_TOTAL_ID: &str = "mag.cal_total";

/// One scope sample. `t` is host epoch seconds at receive time — the only
/// clock the SITL link, the serial link and the sender share.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeSample {
    pub id: String,
    pub t: f64,
    pub v: f64,
}

impl ScopeSample {
    fn new(id: String, t: f64, v: f64) -> Self {
        Self { id, t, v }
    }
}

/// `servo{n}` samples from one `SERVO_OUTPUT_RAW`: `prefix` is `sitl` or
/// `fc`. Raw microseconds, untouched (0 = output not driven).
pub fn servo_samples(prefix: &str, t: f64, pwm: [u16; SERVO_COUNT]) -> Vec<ScopeSample> {
    pwm.iter()
        .enumerate()
        .map(|(i, &us)| ScopeSample::new(format!("{prefix}.servo{}", i + 1), t, f64::from(us)))
        .collect()
}

/// `fwd.cmd{n}` samples: the values the sender actually put on the wire
/// (post clamp/slew), 0..1 → percent.
pub fn command_samples(t: f64, values: &[f32]) -> Vec<ScopeSample> {
    values
        .iter()
        .take(SERVO_COUNT)
        .enumerate()
        .map(|(i, &v)| ScopeSample::new(format!("fwd.cmd{}", i + 1), t, f64::from(v) * 100.0))
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

/// `mag.raw_total` always; `mag.cal_total` only while a calibration is loaded.
pub fn mag_samples(t: f64, totals: MagTotals) -> Vec<ScopeSample> {
    let mut out = vec![ScopeSample::new(MAG_RAW_TOTAL_ID.into(), t, totals.raw)];
    if let Some(cal) = totals.cal {
        out.push(ScopeSample::new(MAG_CAL_TOTAL_ID.into(), t, cal));
    }
    out
}

/// `esc{k}.rpm/current/voltage` for one `ESC_STATUS` frame. `index` is
/// 0-based (0 = ESCs 1–4). Non-finite values (a field the ESC does not
/// report) are skipped, so an ESC without current telemetry never gets a
/// `current` signal of garbage; a field reported as exact zero is still
/// emitted — the window tells "all zero" apart from "no data".
pub fn esc_samples(
    index: u8,
    t: f64,
    rpm: &[i32; ESCS_PER_FRAME],
    voltage: &[f32; ESCS_PER_FRAME],
    current: &[f32; ESCS_PER_FRAME],
) -> Vec<ScopeSample> {
    let base = usize::from(index) * ESCS_PER_FRAME;
    let mut out = Vec::with_capacity(ESCS_PER_FRAME * 3);
    for slot in 0..ESCS_PER_FRAME {
        let k = base + slot + 1;
        if k > MAX_ESC {
            break;
        }
        out.push(ScopeSample::new(format!("esc{k}.rpm"), t, f64::from(rpm[slot])));
        for (name, v) in [("voltage", voltage[slot]), ("current", current[slot])] {
            if v.is_finite() {
                out.push(ScopeSample::new(format!("esc{k}.{name}"), t, f64::from(v)));
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

    fn ids(s: &[ScopeSample]) -> Vec<&str> {
        s.iter().map(|x| x.id.as_str()).collect()
    }

    #[test]
    fn servo_samples_are_numbered_from_one() {
        let s = servo_samples("sitl", 1.5, [1000, 1100, 1200, 1300]);
        assert_eq!(
            ids(&s),
            vec!["sitl.servo1", "sitl.servo2", "sitl.servo3", "sitl.servo4"]
        );
        assert_eq!(s[2].v, 1200.0);
        assert!(s.iter().all(|x| x.t == 1.5));
    }

    #[test]
    fn command_samples_are_percent_and_capped_at_four() {
        let s = command_samples(0.0, &[0.5, 0.25, 0.0, 1.0, 0.9]);
        assert_eq!(s.len(), 4);
        assert!((s[0].v - 50.0).abs() < 1e-6);
        assert!((s[3].v - 100.0).abs() < 1e-6);
    }

    #[test]
    fn mag_totals_without_calibration_has_no_cal_trace() {
        let t = mag_totals([30_000.0, 40_000.0, 0.0], None);
        assert!((t.raw - 50_000.0).abs() < 1e-9);
        assert_eq!(t.cal, None);
        assert!((t.effective() - 50_000.0).abs() < 1e-9);
        assert_eq!(ids(&mag_samples(0.0, t)), vec!["mag.raw_total"]);
    }

    #[test]
    fn mag_totals_with_calibration_applies_a_times_raw_minus_b() {
        // A = I, b = [3000, 4000, 0]: the offset removed leaves 27k/36k.
        let cal = MagCalibration::from_json(
            r#"{"a": [[1,0,0],[0,1,0],[0,0,1]], "b": [3000, 4000, 0]}"#,
        )
        .expect("valid calibration");
        let t = mag_totals([30_000.0, 40_000.0, 0.0], Some(&cal));
        assert!((t.raw - 50_000.0).abs() < 1e-9);
        let c = t.cal.expect("calibrated total");
        assert!((c - 45_000.0).abs() < 1e-6, "3-4-5 triangle scaled by 9000: {c}");
        assert!((t.effective() - c).abs() < 1e-12);
        assert_eq!(
            ids(&mag_samples(0.0, t)),
            vec!["mag.raw_total", "mag.cal_total"]
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
        assert_eq!(rpm4.v, 40_000.0, "rpm above i16::MAX must not wrap");
        assert!(s.iter().any(|x| x.id == "esc3.current" && x.v == 3.0));
    }

    #[test]
    fn esc_frame_one_maps_to_five_to_eight_and_two_is_dropped() {
        let s = esc_samples(1, 0.0, &[1; 4], &[1.0; 4], &[1.0; 4]);
        assert!(s.iter().any(|x| x.id == "esc5.rpm"));
        assert!(s.iter().any(|x| x.id == "esc8.current"));
        assert!(esc_samples(2, 0.0, &[1; 4], &[1.0; 4], &[1.0; 4]).is_empty());
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
        bus.push_all(servo_samples("fc", 0.0, [1; 4]));
        assert_eq!(bus.drain().len(), 4);
        assert!(bus.drain().is_empty(), "drained exactly once");

        let big: Vec<ScopeSample> = (0..MAX_PENDING + 10)
            .map(|i| ScopeSample::new("x".into(), i as f64, 0.0))
            .collect();
        bus.push_all(big);
        let out = bus.drain();
        assert_eq!(out.len(), MAX_PENDING);
        assert_eq!(out[0].t, 10.0, "the oldest were dropped first");
    }
}
