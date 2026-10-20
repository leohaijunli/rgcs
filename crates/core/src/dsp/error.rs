//! Errors from DSP processors and analyzers.

use thiserror::Error;

/// Errors produced while configuring or running a processor/analyzer.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum DspError {
    /// A parameter value is missing or unusable.
    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    /// A cutoff at or above Nyquist cannot be designed (the filter would alias).
    #[error("cutoff {fc} Hz must be below fs/2 = {nyquist} Hz")]
    CutoffAtOrAboveNyquist { fc: f64, nyquist: f64 },

    /// A requested algorithm id is not in the registry.
    #[error("unknown algorithm: {0}")]
    UnknownAlgorithm(String),

    /// An FFT length is not a supported power of two.
    #[error("FFT length must be a power of two in 16..=8192, got {0}")]
    InvalidFftLength(usize),
}
