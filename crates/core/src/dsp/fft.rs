//! Real-signal spectrum analysis: window functions, a radix-2 FFT, and the
//! [`FftAnalyzer`] (an [`Analyzer`] implementation).
//!
//! The radix-2 Cooley-Tukey FFT is written in place so the crate stays
//! dependency-free; lengths must be powers of two (16..=8192 per
//! [`MIN_N`]/[`MAX_N`]). A pure sine that completes an integer number of periods
//! in the window lands exactly on one bin with magnitude ≈ A·N/2 (rectangular
//! window), which the tests assert to < 1 %.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::param::ParamSpec;
use super::{Analyzer, DspError, SpectrumFrame};

/// Smallest FFT length.
pub const MIN_N: usize = 16;
/// Largest FFT length.
pub const MAX_N: usize = 8192;

/// Cutoff… FFT length parameter.
pub const PARAM_N: &str = "n";
/// Window function parameter (index into [`WINDOW_NAMES`]).
pub const PARAM_WINDOW: &str = "window";
/// Output scale parameter.
pub const PARAM_SCALE: &str = "scale";
/// Detrend the window before the transform.
pub const PARAM_DETREND: &str = "detrend";

/// Windowing functions available to the analyzer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Rectangular,
    Hann,
    Hamming,
    Blackman,
    FlatTop,
}

/// Names in registry order (the `window` parameter is an index into this).
pub const WINDOW_NAMES: &[&str] = &["rectangular", "hann", "hamming", "blackman", "flat_top"];

/// Output scale for the spectrum bins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SpectrumScale {
    Magnitude,
    Psd,
    Decibels,
}

/// In-place iterative radix-2 FFT on real (`re`) and imaginary (`im`) arrays.
pub fn fft_inplace(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert_eq!(n, im.len());
    debug_assert!(n.is_power_of_two());
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    // Butterflies.
    let mut len = 2usize;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        let (tw_r, tw_i) = angle.sin_cos();
        let half = len >> 1;
        let mut start = 0usize;
        while start < n {
            let (mut cr, mut ci) = (1.0, 0.0);
            let (mut a, mut b) = (start, start + half);
            for _ in 0..half {
                let xr = re[b] * cr - im[b] * ci;
                let xi = im[b] * cr + re[b] * ci;
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
                a += 1;
                b += 1;
                // Rotate the twiddle.
                let tr = cr * tw_r - ci * tw_i;
                ci = ci * tw_r + cr * tw_i;
                cr = tr;
            }
            start += len;
        }
        len <<= 1;
    }
}

fn window_values(kind: WindowKind, n: usize) -> Vec<f64> {
    let two_pi = 2.0 * std::f64::consts::PI;
    (0..n)
        .map(|i| {
            let w = two_pi * i as f64 / (n - 1) as f64;
            match kind {
                WindowKind::Rectangular => 1.0,
                WindowKind::Hann => 0.5 - 0.5 * w.cos(),
                WindowKind::Hamming => 0.54 - 0.46 * w.cos(),
                WindowKind::Blackman => 0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos(),
                WindowKind::FlatTop => {
                    0.21557895 - 0.41663158 * w.cos() + 0.277263158 * (2.0 * w).cos()
                        - 0.083578947 * (3.0 * w).cos()
                        + 0.006947368 * (4.0 * w).cos()
                }
            }
        })
        .collect()
}

/// The windowed FFT analyzer: buffers `n` samples, transforms on the fly.
pub struct FftAnalyzer {
    n: usize,
    fs: f64,
    window: WindowKind,
    scale: SpectrumScale,
    detrend: bool,
    buf: VecDeque<f64>,
    latest: Option<SpectrumFrame>,
}

impl FftAnalyzer {
    pub fn new(
        n: usize,
        fs: f64,
        window: WindowKind,
        scale: SpectrumScale,
        detrend: bool,
    ) -> Result<Self, DspError> {
        if !(MIN_N..=MAX_N).contains(&n) || !n.is_power_of_two() {
            return Err(DspError::InvalidFftLength(n));
        }
        if !fs.is_finite() || fs <= 0.0 {
            return Err(DspError::InvalidParam(format!(
                "fs must be positive, got {fs}"
            )));
        }
        Ok(Self {
            n,
            fs,
            window,
            scale,
            detrend,
            buf: VecDeque::with_capacity(n),
            latest: None,
        })
    }

    fn compute(&mut self) {
        let mut re: Vec<f64> = self.buf.iter().copied().collect();
        let window = window_values(self.window, self.n);
        let window_sum: f64 = window.iter().sum();
        let wss: f64 = window.iter().map(|w| w * w).sum();
        if self.detrend {
            let mean = re.iter().sum::<f64>() / re.len() as f64;
            for v in re.iter_mut() {
                *v -= mean;
            }
        }
        let mut im = vec![0.0; self.n];
        for (v, w) in re.iter_mut().zip(window.iter()) {
            *v *= w;
        }
        fft_inplace(&mut re, &mut im);
        let coherent_gain = if self.window == WindowKind::Rectangular {
            self.n as f64 / 2.0
        } else {
            window_sum / 2.0
        };
        let mut bins = Vec::with_capacity(self.n);
        let mut peak_bin = 0usize;
        let mut peak_value = f64::NEG_INFINITY;
        for (i, (r, im)) in re.iter().zip(&im).enumerate() {
            let mag = (r * r + im * im).sqrt() / coherent_gain;
            let value = match self.scale {
                SpectrumScale::Magnitude => mag,
                SpectrumScale::Psd => 2.0 * mag * mag / (self.fs * wss),
                SpectrumScale::Decibels => 20.0 * (mag.max(1e-12)).log10(),
            };
            if i > 0 && value > peak_value {
                peak_value = value;
                peak_bin = i;
            }
            bins.push(value);
        }
        self.latest = Some(SpectrumFrame {
            fs: self.fs,
            n: self.n,
            delta_f: self.fs / self.n as f64,
            nyquist: self.fs / 2.0,
            bins,
            peak_bin,
            peak_freq_hz: peak_bin as f64 * self.fs / self.n as f64,
            peak_value,
        });
    }
}

impl Analyzer for FftAnalyzer {
    fn push(&mut self, _t: f64, x: f64) {
        if self.buf.len() == self.n {
            self.buf.pop_front();
        }
        self.buf.push_back(x);
        if self.buf.len() == self.n {
            self.compute();
        }
    }

    fn poll(&mut self) -> Option<SpectrumFrame> {
        self.latest.clone()
    }
}

/// Parameter spec for the FFT analyzer.
pub fn fft_params() -> Vec<ParamSpec> {
    vec![
        ParamSpec::int(
            PARAM_N,
            "FFT length",
            "samples",
            MIN_N as i64,
            MAX_N as i64,
            1024,
        ),
        ParamSpec::float(
            PARAM_WINDOW,
            "Window",
            "",
            0.0,
            WINDOW_NAMES.len() as f64 - 1.0,
            1.0,
        ),
        ParamSpec::float(PARAM_SCALE, "Scale", "", 0.0, 2.0, 0.0),
        ParamSpec::boolean(PARAM_DETREND, "Remove the mean", true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peak_lands_on_its_bin_with_the_right_magnitude() {
        let n = 1024;
        let fs = 1024.0;
        let f = 100.0; // exactly 100 periods in the window
        let a = 0.5;
        let mut an = FftAnalyzer::new(
            n,
            fs,
            WindowKind::Rectangular,
            SpectrumScale::Magnitude,
            false,
        )
        .unwrap();
        for i in 0..n {
            an.push(
                i as f64,
                a * (2.0 * std::f64::consts::PI * f * i as f64 / fs).sin(),
            );
        }
        let frame = an.poll().expect("a frame after a full window");
        assert_eq!(frame.peak_bin, 100, "peak bin");
        // Normalised so the peak reads the sine's amplitude A.
        let want = a;
        assert!(
            (frame.peak_value - want).abs() < want * 0.01,
            "peak {} vs {}",
            frame.peak_value,
            want
        );
    }

    #[test]
    fn fft_length_must_be_a_power_of_two() {
        assert!(matches!(
            FftAnalyzer::new(
                1000,
                1000.0,
                WindowKind::Hann,
                SpectrumScale::Magnitude,
                false
            ),
            Err(DspError::InvalidFftLength(_))
        ));
        assert!(matches!(
            FftAnalyzer::new(4, 1000.0, WindowKind::Hann, SpectrumScale::Magnitude, false),
            Err(DspError::InvalidFftLength(_))
        ));
    }
}
