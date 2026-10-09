//! Causal moving average via a ring buffer and a running sum, O(1)/sample.
//!
//! The running sum is periodically recomputed from the buffer so floating-point
//! drift never accumulates (the plan's §5 "定期重算和以避免浮点漂移").

use std::collections::VecDeque;

use super::param::{ParamSpec, ParamValues};
use super::{DspError, Processor};

/// Window length parameter (samples).
pub const PARAM_WINDOW: &str = "window";

/// Recomputed running sum every this many samples.
const RECOMPUTE_EVERY: usize = 1 << 16;

pub struct MovingAverage {
    window: usize,
    buf: VecDeque<f64>,
    sum: f64,
    since_recompute: usize,
}

impl MovingAverage {
    pub fn new(window: usize) -> Result<Self, DspError> {
        if window == 0 {
            return Err(DspError::InvalidParam(format!(
                "window must be >= 1, got {window}"
            )));
        }
        Ok(Self {
            window,
            buf: VecDeque::with_capacity(window),
            sum: 0.0,
            since_recompute: 0,
        })
    }

    fn recompute(&mut self) {
        self.sum = self.buf.iter().sum();
        self.since_recompute = 0;
    }
}

impl Processor for MovingAverage {
    fn configure(&mut self, params: &ParamValues, _fs_hz: f64) -> Result<(), DspError> {
        let window = params.finite(PARAM_WINDOW)? as usize;
        if window == 0 {
            return Err(DspError::InvalidParam(format!(
                "window must be >= 1, got {window}"
            )));
        }
        if window != self.window {
            self.window = window;
            self.buf.clear();
            self.sum = 0.0;
            self.since_recompute = 0;
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.sum = 0.0;
        self.since_recompute = 0;
    }

    fn process(&mut self, x: f64) -> f64 {
        if self.buf.len() == self.window {
            if let Some(old) = self.buf.pop_front() {
                self.sum -= old;
            }
        }
        self.buf.push_back(x);
        self.sum += x;
        self.since_recompute += 1;
        if self.since_recompute >= RECOMPUTE_EVERY {
            self.recompute();
        }
        self.sum / self.buf.len() as f64
    }
}

/// Parameter spec for the moving average.
pub fn moving_average_params() -> Vec<ParamSpec> {
    vec![ParamSpec::int(
        PARAM_WINDOW,
        "Window",
        "samples",
        1,
        100_000,
        10,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic LCG so the reference test needs no external RNG crate.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    #[test]
    fn matches_the_o_n_reference_on_random_input() {
        let window = 32;
        let mut ma = MovingAverage::new(window).unwrap();
        let mut ref_buf: VecDeque<f64> = VecDeque::with_capacity(window);
        let mut rng = Lcg(42);
        for _ in 0..100_000 {
            let x = rng.next();
            if ref_buf.len() == window {
                ref_buf.pop_front();
            }
            ref_buf.push_back(x);
            let naive: f64 = ref_buf.iter().sum::<f64>() / ref_buf.len() as f64;
            let fast = ma.process(x);
            assert!((fast - naive).abs() < 1e-9, "fast {fast} vs naive {naive}");
        }
    }

    #[test]
    fn constant_input_reaches_the_constant() {
        let mut ma = MovingAverage::new(16).unwrap();
        let mut out = 0.0;
        for _ in 0..100 {
            out = ma.process(2.0);
        }
        assert!((out - 2.0).abs() < 1e-12, "steady = {out}");
    }

    #[test]
    fn rejects_zero_window() {
        assert!(matches!(
            MovingAverage::new(0),
            Err(DspError::InvalidParam(_))
        ));
    }
}
