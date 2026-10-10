//! Second-order IIR low/high-pass filters (biquad).
//!
//! Designed by bilinear transform with frequency prewarping, Direct Form II
//! Transposed. A second-order Butterworth has −3.01 dB at the cutoff and
//! approximately −40 dB/decade rolloff; the coefficients are the standard
//! RBJ audio-EQ formulas (same filter `scipy.signal.butter(2, …)` produces).

use super::param::{ParamSpec, ParamValues};
use super::{DspError, Processor};

/// Cutoff parameter key.
pub const PARAM_FC_HZ: &str = "fc_hz";
/// Q parameter key (default 1/√2 = Butterworth).
pub const PARAM_Q: &str = "q";

/// Which biquad to design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiquadKind {
    LowPass,
    HighPass,
}

/// Direct Form II Transposed biquad state.
#[derive(Debug, Clone)]
pub struct Biquad {
    kind: BiquadKind,
    /// Coefficient signature; only redesign when it changes.
    sig: (f64, f64),
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    /// Last finite input, used to re-seed the state on a live retune.
    last_x: f64,
    /// False until the first finite sample seeds the state (step init).
    primed: bool,
}

/// The transfer function evaluated on the unit circle: `|H(e^{j2π f/fs})|`.
fn magnitude_at(b: &Biquad, f_hz: f64, fs: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f_hz / fs;
    let (s, c) = w.sin_cos(); // sin_cos yields (sin, cos)
                              // z^-1 = cos(w) - j sin(w), z^-2 = cos(2w) - j sin(2w)
    let c2 = 2.0 * c * c - 1.0;
    let s2 = 2.0 * s * c;
    let num_re = b.b0 + b.b1 * c + b.b2 * c2;
    let num_im = -(b.b1 * s + b.b2 * s2);
    let den_re = 1.0 + b.a1 * c + b.a2 * c2;
    let den_im = -(b.a1 * s + b.a2 * s2);
    (num_re * num_re + num_im * num_im).sqrt() / (den_re * den_re + den_im * den_im).sqrt()
}

/// Design a biquad for a cutoff at `fc_hz` with quality `q` at sample rate `fs`.
fn design(kind: BiquadKind, fc_hz: f64, q: f64, fs: f64) -> Result<Biquad, DspError> {
    if !fc_hz.is_finite() || fc_hz <= 0.0 {
        return Err(DspError::InvalidParam(format!(
            "fc_hz must be positive, got {fc_hz}"
        )));
    }
    if fc_hz >= fs / 2.0 {
        return Err(DspError::CutoffAtOrAboveNyquist {
            fc: fc_hz,
            nyquist: fs / 2.0,
        });
    }
    if !q.is_finite() || q <= 0.0 {
        return Err(DspError::InvalidParam(format!(
            "q must be positive, got {q}"
        )));
    }
    // Bilinear transform with prewarping (RBJ).
    let k = (std::f64::consts::PI * fc_hz / fs).tan();
    let norm = 1.0 / (1.0 + k / q + k * k);
    let (b0, b1, b2) = match kind {
        BiquadKind::LowPass => (k * k * norm, 2.0 * k * k * norm, k * k * norm),
        BiquadKind::HighPass => (norm, -2.0 * norm, norm),
    };
    let a1 = 2.0 * (k * k - 1.0) * norm;
    let a2 = (1.0 - k / q + k * k) * norm;
    Ok(Biquad {
        kind,
        sig: (fc_hz, q),
        b0,
        b1,
        b2,
        a1,
        a2,
        x1: 0.0,
        x2: 0.0,
        last_x: 0.0,
        primed: false,
    })
}

impl Biquad {
    /// Build a configured biquad directly.
    pub fn new(kind: BiquadKind, fc_hz: f64, q: f64, fs_hz: f64) -> Result<Self, DspError> {
        design(kind, fc_hz, q, fs_hz)
    }

    /// Magnitude response in dB at `f_hz` (for tests and the UI readout).
    pub fn magnitude_db(&self, f_hz: f64, fs: f64) -> f64 {
        20.0 * magnitude_at(self, f_hz, fs).log10()
    }
}

impl Processor for Biquad {
    fn configure(&mut self, params: &ParamValues, fs_hz: f64) -> Result<(), DspError> {
        let fc = params.finite(PARAM_FC_HZ)?;
        let q = params
            .get(PARAM_Q)
            .unwrap_or(std::f64::consts::FRAC_1_SQRT_2);
        if self.sig == (fc, q) {
            return Ok(()); // unchanged: keep running state
        }
        let mut redesigned = design(self.kind, fc, q, fs_hz)?;
        // A live retune must not restart the curve from zero, and carrying
        // the raw delay line shifts its meaning under the new coefficients
        // (a several-percent transient on a DC-offset signal). Instead,
        // re-seed the state at the last input under the *new* coefficients:
        // the output stays on the signal.
        if self.primed {
            redesigned.x1 = (1.0 - redesigned.b0) * self.last_x;
            redesigned.x2 = (redesigned.b2 - redesigned.a2) * self.last_x;
            redesigned.primed = true;
        }
        *self = redesigned;
        Ok(())
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.last_x = 0.0;
        self.primed = false;
    }

    fn process(&mut self, x: f64) -> f64 {
        // A gap invalidates the running state: pass it through and re-prime
        // from the next finite sample, instead of letting NaN poison the
        // delay line forever.
        if !x.is_finite() {
            self.x1 = 0.0;
            self.x2 = 0.0;
            self.primed = false;
            return x;
        }
        self.last_x = x;
        // Step initialization: the first sample seeds the delay line as if
        // the input had sat at that value forever, so the filtered curve
        // starts *at* the signal instead of ramping up from zero (operator
        // report: a 22 000 nT mag bias took seconds to converge).
        if !self.primed {
            self.x1 = (1.0 - self.b0) * x;
            self.x2 = (self.b2 - self.a2) * x;
            self.primed = true;
        }
        // Direct Form II Transposed.
        let y = self.b0 * x + self.x1;
        self.x1 = self.b1 * x - self.a1 * y + self.x2;
        self.x2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Parameter spec shared by the low- and high-pass descriptors.
pub fn biquad_params() -> Vec<ParamSpec> {
    vec![
        ParamSpec::float(PARAM_FC_HZ, "Cutoff", "Hz", 0.1, 5000.0, 5.0),
        ParamSpec::float(PARAM_Q, "Q", "", 0.1, 10.0, std::f64::consts::FRAC_1_SQRT_2),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_at(fc: f64, q: f64, fs: f64, f: f64, kind: BiquadKind) -> f64 {
        let b = Biquad::new(kind, fc, q, fs).unwrap();
        b.magnitude_db(f, fs)
    }

    #[test]
    fn cutoff_is_minus_3_dot_01_db_within_tolerance() {
        let fs = 1000.0;
        for fc in [2.0, 5.0, 20.0, 120.0] {
            let lp = gain_at(
                fc,
                std::f64::consts::FRAC_1_SQRT_2,
                fs,
                fc,
                BiquadKind::LowPass,
            );
            let hp = gain_at(
                fc,
                std::f64::consts::FRAC_1_SQRT_2,
                fs,
                fc,
                BiquadKind::HighPass,
            );
            assert!((lp + 3.0103).abs() < 0.1, "LP cutoff gain {lp} dB");
            assert!((hp + 3.0103).abs() < 0.1, "HP cutoff gain {hp} dB");
        }
    }

    #[test]
    fn lowpass_rolls_off_about_40_db_per_decade() {
        let fs = 1000.0;
        let fc = 10.0;
        let at_100 = gain_at(
            fc,
            std::f64::consts::FRAC_1_SQRT_2,
            fs,
            100.0,
            BiquadKind::LowPass,
        );
        assert!(
            (-42.0..=-38.0).contains(&at_100),
            "gain at 10*fc = {at_100} dB"
        );
        let at_500 = gain_at(
            fc,
            std::f64::consts::FRAC_1_SQRT_2,
            fs,
            500.0,
            BiquadKind::LowPass,
        );
        assert!(at_500 < -70.0, "gain at fs/2 = {at_500} dB");
    }

    #[test]
    fn d_c_and_near_nyquist_are_mirrored() {
        let fs = 1000.0;
        let fc = 50.0;
        let lp0 = gain_at(
            fc,
            std::f64::consts::FRAC_1_SQRT_2,
            fs,
            1e-9,
            BiquadKind::LowPass,
        );
        let hp0 = gain_at(
            fc,
            std::f64::consts::FRAC_1_SQRT_2,
            fs,
            1e-9,
            BiquadKind::HighPass,
        );
        assert!((lp0 - 0.0).abs() < 1e-6, "LP DC gain {lp0} dB");
        assert!(hp0 < -60.0, "HP DC gain {hp0} dB");
    }

    #[test]
    fn rejects_cutoff_at_or_above_nyquist() {
        let fs = 100.0;
        assert!(matches!(
            Biquad::new(
                BiquadKind::LowPass,
                50.0,
                std::f64::consts::FRAC_1_SQRT_2,
                fs
            ),
            Err(DspError::CutoffAtOrAboveNyquist { .. })
        ));
        assert!(matches!(
            Biquad::new(
                BiquadKind::LowPass,
                60.0,
                std::f64::consts::FRAC_1_SQRT_2,
                fs
            ),
            Err(DspError::CutoffAtOrAboveNyquist { .. })
        ));
        assert!(matches!(
            Biquad::new(
                BiquadKind::LowPass,
                0.0,
                std::f64::consts::FRAC_1_SQRT_2,
                fs
            ),
            Err(DspError::InvalidParam(_))
        ));
    }

    #[test]
    fn steady_state_d_c_of_lowpass_is_the_input() {
        let fs = 1000.0;
        let mut b = Biquad::new(
            BiquadKind::LowPass,
            5.0,
            std::f64::consts::FRAC_1_SQRT_2,
            fs,
        )
        .unwrap();
        let mut sum = 0.0f64;
        for (i, _) in (0..20_000).enumerate() {
            let y = b.process(1.0);
            if i >= 1000 {
                sum += y;
            }
        }
        let dc = sum / (20_000.0 - 1000.0);
        assert!((dc - 1.0).abs() < 1e-4, "steady DC = {dc}");
    }

    #[test]
    fn hot_retune_keeps_running_without_nan() {
        let fs = 1000.0;
        let mut b = Biquad::new(
            BiquadKind::LowPass,
            5.0,
            std::f64::consts::FRAC_1_SQRT_2,
            fs,
        )
        .unwrap();
        let mut p = ParamValues::new();
        p.set_all(&[
            (PARAM_FC_HZ, 5.0),
            (PARAM_Q, std::f64::consts::FRAC_1_SQRT_2),
        ]);
        let mut out = 0.0f64;
        for i in 0..10_000 {
            let x = (i as f64 * 0.01).sin();
            out = out.max(b.process(x));
            if i == 5_000 {
                p.set(PARAM_FC_HZ, 40.0);
                b.configure(&p, fs).unwrap();
            }
            assert!(out.is_finite(), "output went NaN at sample {i}");
        }
    }

    #[test]
    fn first_output_starts_at_the_signal_not_zero() {
        // Operator report: a filtered mag trace ramped from 0 toward the
        // 22 000 nT bias for seconds. Step init must make the very first
        // filtered sample equal the raw value, for both kinds.
        let fs = 100.0;
        for kind in [BiquadKind::LowPass, BiquadKind::HighPass] {
            let mut b = Biquad::new(kind, 5.0, std::f64::consts::FRAC_1_SQRT_2, fs).unwrap();
            let y0 = b.process(22_000.0);
            assert!(
                (y0 - 22_000.0).abs() < 1e-6,
                "{kind:?}: first output {y0} != first sample"
            );
        }
    }

    #[test]
    fn gap_reseeds_instead_of_poisoning_the_state() {
        let fs = 100.0;
        let mut b = Biquad::new(
            BiquadKind::LowPass,
            5.0,
            std::f64::consts::FRAC_1_SQRT_2,
            fs,
        )
        .unwrap();
        for _ in 0..50 {
            b.process(100.0);
        }
        // A NaN gap passes through …
        assert!(b.process(f64::NAN).is_nan());
        // … and the next finite sample restarts *at* the signal, not at 0,
        // and not stuck at NaN.
        let y = b.process(100.0);
        assert!((y - 100.0).abs() < 1e-6, "post-gap output {y}");
        let y2 = b.process(100.0);
        assert!(y2.is_finite(), "state stayed poisoned: {y2}");
    }

    #[test]
    fn retune_keeps_the_curve_near_the_signal() {
        // A live cutoff change must not restart the curve from zero (the
        // redesign used to wipe the delay line despite claiming otherwise).
        let fs = 100.0;
        let mut b = Biquad::new(
            BiquadKind::LowPass,
            5.0,
            std::f64::consts::FRAC_1_SQRT_2,
            fs,
        )
        .unwrap();
        for _ in 0..200 {
            b.process(5_000.0);
        }
        let mut p = ParamValues::new();
        p.set_all(&[
            (PARAM_FC_HZ, 10.0),
            (PARAM_Q, std::f64::consts::FRAC_1_SQRT_2),
        ]);
        b.configure(&p, fs).unwrap();
        let y = b.process(5_000.0);
        assert!((y - 5_000.0).abs() < 1.0, "retune restarted from zero: {y}");
    }
}
