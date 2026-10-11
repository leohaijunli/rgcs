//! Derived signals assembled from several MAVLink messages (plan A4).
//!
//! [`EscAggregator`] is the normalization layer for `ESC_STATUS` (id 291):
//! each frame carries four ESCs and an `index` saying *which* four, so the
//! aggregator maps `index*4 + slot` → `esc{k}` and keeps the latest value per
//! signal. The Signal Inspector's tap feeds it and pulls a snapshot of
//! `esc{k}.rpm/voltage/current` samples after every frame.

/// How many ESCs one `ESC_STATUS` frame covers.
pub const ESCS_PER_FRAME: usize = 4;

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
    escs: Vec<EscSample>,
}

impl EscAggregator {
    pub fn new(esc_count: usize) -> Self {
        Self {
            escs: vec![EscSample::default(); esc_count],
        }
    }

    pub fn esc_count(&self) -> usize {
        self.escs.len()
    }

    /// Feed one `ESC_STATUS` frame: `(index, rpm[4], voltage[4], current[4])`.
    /// `index` is 0-based in MAVLink (0 = ESCs 1–4).
    pub fn feed(&mut self, index: u8, rpm: &[i16; 4], voltage: &[f32; 4], current: &[f32; 4]) {
        let base = index as usize * ESCS_PER_FRAME;
        for (offset, slot) in (base..base + ESCS_PER_FRAME).enumerate() {
            if slot < self.escs.len() {
                self.escs[slot] = EscSample {
                    rpm: rpm[offset] as f64,
                    voltage: voltage[offset] as f64,
                    current: current[offset] as f64,
                };
            }
        }
    }

    /// Latest sample for ESC `k` (0-based).
    pub fn sample(&self, k: usize) -> Option<EscSample> {
        self.escs.get(k).copied()
    }

    /// All ESCs as `(name, value)` pairs: `esc1.rpm`, `esc2.current`, …
    pub fn signals(&self) -> Vec<(String, f64)> {
        let mut out = Vec::with_capacity(self.escs.len() * 3);
        for (k, esc) in self.escs.iter().enumerate() {
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

    const RPM: [i16; 4] = [1000, 1100, 1200, 1300];
    const VOLTS: [f32; 4] = [12.0, 12.1, 12.2, 12.3];
    const AMPS: [f32; 4] = [1.0, 2.0, 3.0, 4.0];

    #[test]
    fn first_frame_maps_to_escs_one_to_four() {
        let mut agg = EscAggregator::new(8);
        agg.feed(0, &RPM, &VOLTS, &AMPS);
        let s = agg.signals();
        assert_eq!(s.len(), 24, "8 ESCs × 3 signals");
        assert_eq!(value_of(&s, "esc1.rpm"), 1000.0);
        assert_eq!(value_of(&s, "esc4.current"), 4.0);
        // ESCs 5–8 (frame index 1) have not been seen yet: still zero.
        assert_eq!(value_of(&s, "esc5.rpm"), 0.0);
    }

    #[test]
    fn second_index_maps_to_escs_five_to_eight() {
        let mut agg = EscAggregator::new(8);
        agg.feed(1, &RPM, &VOLTS, &AMPS);
        assert_eq!(value_of(&agg.signals(), "esc5.rpm"), 1000.0);
        assert_eq!(value_of(&agg.signals(), "esc8.current"), 4.0);
        assert_eq!(
            value_of(&agg.signals(), "esc1.rpm"),
            0.0,
            "frame 1 is not ESCs 1–4"
        );
    }

    #[test]
    fn latest_frame_wins_per_esc() {
        // A frame carries all four ESCs of its index as one unit: the newest
        // frame's values replace the previous ones wholesale.
        let mut agg = EscAggregator::new(4);
        agg.feed(0, &RPM, &VOLTS, &AMPS);
        assert_eq!(value_of(&agg.signals(), "esc1.rpm"), 1000.0);
        let newer = [i16::MAX - 1, 0, 0, 0];
        agg.feed(0, &newer, &VOLTS, &AMPS);
        assert_eq!(value_of(&agg.signals(), "esc1.rpm"), (i16::MAX - 1) as f64);
        assert_eq!(
            value_of(&agg.signals(), "esc2.rpm"),
            0.0,
            "newest frame wins"
        );
    }

    #[test]
    fn out_of_range_index_is_ignored() {
        let mut agg = EscAggregator::new(4);
        agg.feed(3, &RPM, &VOLTS, &AMPS); // would map to ESCs 13–16: none exist
        assert!(agg.signals().iter().all(|(_, v)| *v == 0.0));
    }

    fn value_of(signals: &[(String, f64)], name: &str) -> f64 {
        signals
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
            .unwrap_or(f64::NAN)
    }
}
