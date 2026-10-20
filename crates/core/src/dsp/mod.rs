//! Digital signal processing for the Signal Inspector (ADR-015).
//!
//! Pure, dependency-free algorithms: a biquad low/high-pass, a moving
//! average, a sliding-window detrend and a radix-2 real FFT analyzer, plus a
//! self-describing registry the frontend renders parameter forms from
//! (Signal Inspector plan §5). Nothing here touches MAVLink or the UI: a
//! processor processes one sample and returns one sample, an analyzer buffers
//! a window and returns spectral frames, and the pipeline chains processors.
//!
//! Acceptance is numeric: the biquad meets the −3.01 dB cutoff and ~−40 dB/dec
//! rolloff, the moving average matches an O(N) reference, the detrend removes a
//! linear ramp, and the FFT locates a sine's peak within 1 %. Golden vectors
//! can be regenerated with `tools/gen_dsp_golden.py` (SciPy).

pub mod biquad;
pub mod detrend;
pub mod error;
pub mod fft;
pub mod moving_average;
pub mod param;
pub mod pipeline;
pub mod registry;

pub use biquad::{Biquad, BiquadKind};
pub use error::DspError;
pub use moving_average::MovingAverage;
pub use param::{ParamKind, ParamSpec, ParamValues};
pub use pipeline::Pipeline;
pub use registry::{create, list_algorithms, AlgoKind, AlgorithmDescriptor, AlgorithmInfo};

/// A single-sample streaming processor (LPF, HPF, moving average, detrend).
///
/// `configure` re-designs the filter from parameters, ideally without dropping
/// running state (a parameter change mid-flight takes effect on the next
/// sample); `reset` clears all state.
pub trait Processor: Send {
    fn configure(&mut self, params: &ParamValues, fs_hz: f64) -> Result<(), DspError>;
    fn reset(&mut self);
    fn process(&mut self, x: f64) -> f64;
}

/// A windowed time-to-frequency analyzer (the FFT). Time-domain samples are
/// pushed continuously; when a full window has been collected a spectrum frame
/// becomes available from [`Self::poll`].
pub trait Analyzer: Send {
    fn push(&mut self, t: f64, x: f64);
    fn poll(&mut self) -> Option<SpectrumFrame>;
}

/// A spectrum computed by an [`Analyzer`] over one window.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SpectrumFrame {
    /// Assumed sample rate, Hz.
    pub fs: f64,
    /// FFT length (power of two).
    pub n: usize,
    /// Bin width, Hz (`fs / n`).
    pub delta_f: f64,
    /// Nyquist frequency, Hz.
    pub nyquist: f64,
    /// One value per bin (magnitude, PSD or dB per the analyzer scale).
    pub bins: Vec<f64>,
    /// Index of the strongest bin.
    pub peak_bin: usize,
    /// Frequency of the strongest bin, Hz.
    pub peak_freq_hz: f64,
    /// Value of the strongest bin.
    pub peak_value: f64,
}
