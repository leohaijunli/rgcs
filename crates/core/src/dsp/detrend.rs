//! Sliding-window detrend: remove a constant offset or a linear fit from the
//! most recent `window` samples, O(1)/sample via running sums.
//!
//! For a linear fit over the window, the least-squares slope and intercept are
//! computed from running sums of x, t, t² and t·x, so the output is the input
//! minus the fitted value at the sample's own time.

use std::collections::VecDeque;

use super::param::{ParamSpec, ParamValues};
use super::{DspError, Processor};

/// Window length parameter (samples).
pub const PARAM_WINDOW: &str = "window";
/// Fit mode: 0 = constant (subtract the mean), 1 = linear (subtract a line).
pub const PARAM_MODE: &str = "mode";

const EPS: f64 = 1e-15;

pub struct Detrend {
    window: usize,
    linear: bool,
    ts: VecDeque<f64>,
    xs: VecDeque<f64>,
    sum_x: f64,
    sum_t: f64,
    sum_t2: f64,
    sum_tx: f64,
    /// Monotonically increasing sample index (the fit's time axis).
    n: u64,
}

impl Detrend {
    pub fn new(window: usize, linear: bool) -> Result<Self, DspError> {
        if window < 2 {
            return Err(DspError::InvalidParam(format!(
                "window must be >= 2, got {window}"
            )));
        }
        Ok(Self {
            window,
            linear,
            ts: VecDeque::with_capacity(window),
            xs: VecDeque::with_capacity(window),
            sum_x: 0.0,
            sum_t: 0.0,
            sum_t2: 0.0,
            sum_tx: 0.0,
            n: 0,
        })
    }

    fn push(&mut self, t: f64, x: f64) {
        if self.ts.len() == self.window {
            let old_t = self.ts.pop_front().unwrap();
            let old_x = self.xs.pop_front().unwrap();
            self.sum_x -= old_x;
            self.sum_t -= old_t;
            self.sum_t2 -= old_t * old_t;
            self.sum_tx -= old_t * old_x;
        }
        self.ts.push_back(t);
        self.xs.push_back(x);
        self.sum_x += x;
        self.sum_t += t;
        self.sum_t2 += t * t;
        self.sum_tx += t * x;
    }

    /// Fitted value at `t` over the current window.
    fn fit(&self, t: f64) -> f64 {
        let n = self.ts.len() as f64;
        if n < 2.0 {
            return self.sum_x / n.max(1.0);
        }
        if !self.linear {
            return self.sum_x / n;
        }
        let denom = n * self.sum_t2 - self.sum_t * self.sum_t;
        let slope = if denom.abs() > EPS {
            (n * self.sum_tx - self.sum_t * self.sum_x) / denom
        } else {
            0.0
        };
        let intercept = (self.sum_x - slope * self.sum_t) / n;
        slope * t + intercept
    }
}

impl Processor for Detrend {
    fn configure(&mut self, params: &ParamValues, _fs_hz: f64) -> Result<(), DspError> {
        let window = params.finite(PARAM_WINDOW)? as usize;
        if window < 2 {
            return Err(DspError::InvalidParam(format!(
                "window must be >= 2, got {window}"
            )));
        }
        let linear = params.boolean(PARAM_MODE);
        self.window = window;
        self.linear = linear;
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        self.ts.clear();
        self.xs.clear();
        self.sum_x = 0.0;
        self.sum_t = 0.0;
        self.sum_t2 = 0.0;
        self.sum_tx = 0.0;
        self.n = 0;
    }

    fn process(&mut self, x: f64) -> f64 {
        let t = self.n as f64;
        self.n += 1;
        let fitted = self.fit(t);
        self.push(t, x);
        x - fitted
    }
}

/// Parameter spec for the detrend.
pub fn detrend_params() -> Vec<ParamSpec> {
    vec![
        ParamSpec::int(PARAM_WINDOW, "Window", "samples", 2, 100_000, 100),
        ParamSpec::boolean(PARAM_MODE, "Linear fit (off = mean)", true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_a_linear_ramp_in_linear_mode() {
        let mut d = Detrend::new(64, true).unwrap();
        // Warm the window, then check the residual on a pure ramp.
        for i in 0..128 {
            d.process(3.0 * i as f64 + 10.0);
        }
        let residual = d.process(3.0 * 128.0 + 10.0);
        assert!(residual.abs() < 1e-9, "residual = {residual}");
    }

    #[test]
    fn removes_a_constant_offset_in_mean_mode() {
        let mut d = Detrend::new(32, false).unwrap();
        for _ in 0..64 {
            d.process(7.0);
        }
        let residual = d.process(7.0);
        assert!(residual.abs() < 1e-12, "residual = {residual}");
    }

    #[test]
    fn a_sine_is_not_destroyed_by_detrend() {
        let mut d = Detrend::new(256, true).unwrap();
        // After warm-up, the output should look like the (AC) input, i.e. not
        // collapse to zero.
        let mut peak = 0.0f64;
        for i in 0..1024 {
            let out = d.process((i as f64 * 0.2).sin());
            peak = peak.max(out.abs());
        }
        assert!(peak > 0.5, "sine peak preserved = {peak}");
    }

    #[test]
    fn rejects_window_below_two() {
        assert!(matches!(
            Detrend::new(1, true),
            Err(DspError::InvalidParam(_))
        ));
    }
}
