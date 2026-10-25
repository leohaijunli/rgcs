//! Derived signals assembled from several MAVLink messages (plan A4).
//!
//! [`EscAggregator`] is the normalization layer for `ESC_STATUS` (id 291):
//! each frame carries four ESCs and an `index` identifying the first ESC,
//! so wire index 4 covers ESCs 5–8. The aggregator keeps the latest observed
//! `esc{k}.rpm/voltage/current` values; an unseen ESC has no sample.

use std::ops::Range;

/// How many ESCs one `ESC_STATUS` frame covers.
pub const ESCS_PER_FRAME: usize = 4;

/// Supported zero-based ESC slots covered by an `ESC_STATUS` frame.
/// The wire index is the first ESC, not a batch number. A frame at or beyond
/// `esc_count` yields an empty range; a partial final frame is clipped.
pub fn esc_frame_range(first_esc_index: u8, esc_count: usize) -> Range<usize> {
    let first = usize::from(first_esc_index);
    first.min(esc_count)..(first + ESCS_PER_FRAME).min(esc_count)
}

/// Latest normalized values for one ESC.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EscSample {
    pub rpm: f64,
    pub voltage: f64,
    pub current: f64,
}

/// Aggregates `ESC_STATUS` frames into per-ESC latest values (T4: `index`
/// differs across frames; every frame maps onto its own four ESCs).
#[derive(Debug, Default)]
pub struct EscAggregator {
    /// Slot `k` = ESC `k` (0-based here; the UI numbers them 1-based).
    escs: Vec<Option<EscSample>>,
}

impl EscAggregator {
    pub fn new(esc_count: usize) -> Self {
        Self {
            escs: vec![None; esc_count],
        }
    }

    pub fn esc_count(&self) -> usize {
        self.escs.len()
    }

    /// Feed one `ESC_STATUS` frame: `(index, rpm[4], voltage[4], current[4])`.
    /// `index` is the first zero-based ESC (0 = ESCs 1–4, 4 = ESCs 5–8).
    pub fn feed(
        &mut self,
        index: u8,
        rpm: &[i32; ESCS_PER_FRAME],
        voltage: &[f32; ESCS_PER_FRAME],
        current: &[f32; ESCS_PER_FRAME],
    ) {
        for (offset, slot) in esc_frame_range(index, self.escs.len()).enumerate() {
            self.escs[slot] = Some(EscSample {
                rpm: f64::from(rpm[offset]),
                voltage: f64::from(voltage[offset]),
                current: f64::from(current[offset]),
            });
        }
    }

    /// Latest sample for ESC `k` (0-based), or `None` if unseen/out of range.
    pub fn sample(&self, k: usize) -> Option<EscSample> {
        self.escs.get(k).copied().flatten()
    }

    /// Observed ESCs as `(name, value)` pairs: `esc1.rpm`, `esc2.current`, …
    pub fn signals(&self) -> Vec<(String, f64)> {
        let mut out = Vec::with_capacity(self.escs.len() * 3);
        for (k, esc) in self.escs.iter().enumerate() {
            let Some(esc) = esc else {
                continue;
            };
            out.push((format!("esc{}.rpm", k + 1), esc.rpm));
            out.push((format!("esc{}.voltage", k + 1), esc.voltage));
            out.push((format!("esc{}.current", k + 1), esc.current));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RPM: [i32; 4] = [1000, 1100, 1200, 1300];
    const VOLTS: [f32; 4] = [12.0, 12.1, 12.2, 12.3];
    const AMPS: [f32; 4] = [1.0, 2.0, 3.0, 4.0];

    #[test]
    fn first_frame_maps_to_escs_one_to_four() {
        let mut agg = EscAggregator::new(8);
        agg.feed(0, &RPM, &VOLTS, &AMPS);
        let s = agg.signals();
        assert_eq!(s.len(), 12, "only the four observed ESCs emit signals");
        assert_eq!(value_of(&s, "esc1.rpm"), 1000.0);
        assert_eq!(value_of(&s, "esc4.current"), 4.0);
        assert_eq!(agg.sample(4), None, "ESC5 has not been seen");
        assert!(!s.iter().any(|(name, _)| name == "esc5.rpm"));
    }

    #[test]
    fn second_index_maps_to_escs_five_to_eight() {
        let mut agg = EscAggregator::new(8);
        agg.feed(4, &RPM, &VOLTS, &AMPS);
        assert_eq!(value_of(&agg.signals(), "esc5.rpm"), 1000.0);
        assert_eq!(value_of(&agg.signals(), "esc8.current"), 4.0);
        assert_eq!(agg.sample(0), None, "index 4 does not update ESCs 1–4");
        assert_eq!(agg.signals().len(), 12);
    }

    #[test]
    fn latest_frame_wins_per_esc() {
        // A frame carries all four ESCs of its index as one unit: the newest
        // frame's values replace the previous ones wholesale.
        let mut agg = EscAggregator::new(4);
        agg.feed(0, &RPM, &VOLTS, &AMPS);
        assert_eq!(value_of(&agg.signals(), "esc1.rpm"), 1000.0);
        // MAVLink `ESC_STATUS.rpm` is int32: values above i16::MAX must not wrap.
        let newer = [40_000, 0, 0, 0];
        agg.feed(0, &newer, &VOLTS, &AMPS);
        assert_eq!(value_of(&agg.signals(), "esc1.rpm"), 40_000.0);
        assert_eq!(
            value_of(&agg.signals(), "esc2.rpm"),
            0.0,
            "newest frame wins"
        );
    }

    #[test]
    fn out_of_range_index_is_ignored() {
        let mut agg = EscAggregator::new(4);
        agg.feed(0, &RPM, &VOLTS, &AMPS);
        let before = agg.signals();
        for index in [4, u8::MAX] {
            agg.feed(index, &[0; ESCS_PER_FRAME], &VOLTS, &AMPS);
            assert_eq!(agg.signals(), before);
        }
    }

    #[test]
    fn unseen_samples_are_absent_but_reported_zero_is_present() {
        let mut agg = EscAggregator::new(8);
        assert!(agg.signals().is_empty());
        assert_eq!(agg.sample(0), None);
        assert_eq!(agg.sample(8), None);
        agg.feed(4, &[0; ESCS_PER_FRAME], &VOLTS, &AMPS);
        assert_eq!(agg.sample(4).expect("reported ESC5").rpm, 0.0);
        assert_eq!(value_of(&agg.signals(), "esc5.rpm"), 0.0);
        assert_eq!(agg.sample(0), None);
    }

    #[test]
    fn banks_remain_independent_in_either_arrival_order() {
        for indices in [[0, 4], [4, 0]] {
            let mut agg = EscAggregator::new(8);
            for index in indices {
                let rpm = if index == 0 {
                    RPM
                } else {
                    [5000, 6000, 7000, 8000]
                };
                agg.feed(index, &rpm, &VOLTS, &AMPS);
            }
            assert_eq!(agg.sample(0).expect("ESC1").rpm, 1000.0);
            assert_eq!(agg.sample(7).expect("ESC8").rpm, 8000.0);
            assert_eq!(agg.signals().len(), 24);
            agg.feed(4, &[0; ESCS_PER_FRAME], &VOLTS, &AMPS);
            assert_eq!(agg.sample(0).expect("unchanged ESC1").rpm, 1000.0);
            assert_eq!(agg.sample(4).expect("updated ESC5").rpm, 0.0);
        }
    }

    #[test]
    fn unaligned_index_identifies_the_first_esc() {
        let mut agg = EscAggregator::new(8);
        agg.feed(1, &RPM, &VOLTS, &AMPS);
        assert_eq!(agg.sample(0), None);
        assert_eq!(agg.sample(1).expect("ESC2").rpm, 1000.0);
        assert_eq!(agg.sample(4).expect("ESC5").rpm, 1300.0);
        assert_eq!(agg.sample(5), None);
    }

    #[test]
    fn partial_final_frame_is_clipped_to_the_supported_slots() {
        let mut agg = EscAggregator::new(8);
        agg.feed(6, &RPM, &VOLTS, &AMPS);
        assert_eq!(agg.sample(6).expect("ESC7").rpm, 1000.0);
        assert_eq!(agg.sample(7).expect("ESC8").rpm, 1100.0);
        assert_eq!(agg.sample(8), None);
        assert_eq!(agg.signals().len(), 6);
        let mut empty = EscAggregator::new(0);
        empty.feed(0, &RPM, &VOLTS, &AMPS);
        assert!(empty.signals().is_empty());
    }

    fn value_of(signals: &[(String, f64)], name: &str) -> f64 {
        signals
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
            .unwrap_or(f64::NAN)
    }
}
