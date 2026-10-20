//! The DSP algorithm registry: self-describing descriptors the frontend turns
//! into parameter forms, and a factory that builds a processor or analyzer.
//!
//! Adding an algorithm is one `AlgorithmDescriptor` in [`REGISTRY`]; the
//! frontend never hard-codes an algorithm id.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::biquad::{biquad_params, Biquad, BiquadKind};
use super::detrend::{detrend_params, Detrend};
use super::fft::{fft_params, FftAnalyzer};
use super::moving_average::{moving_average_params, MovingAverage};
use super::param::{ParamSpec, ParamValues};
use super::{Analyzer, DspError, Processor};

/// Whether an algorithm filters a stream or transforms a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AlgoKind {
    Processor,
    Analyzer,
}

/// Serializable algorithm description sent to the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AlgorithmInfo {
    pub id: String,
    pub name: String,
    pub kind: AlgoKind,
    pub params: Vec<ParamSpec>,
}

/// A built algorithm, type-erased until the caller picks a trait.
pub enum BuiltAlgorithm {
    Processor(Box<dyn Processor>),
    Analyzer(Box<dyn Analyzer>),
}

pub struct AlgorithmDescriptor {
    pub info: AlgorithmInfo,
    pub factory: fn(&ParamValues, f64) -> Result<BuiltAlgorithm, DspError>,
}

fn make_biquad_lp(p: &ParamValues, fs: f64) -> Result<BuiltAlgorithm, DspError> {
    make_biquad(BiquadKind::LowPass, p, fs)
}

fn make_biquad_hp(p: &ParamValues, fs: f64) -> Result<BuiltAlgorithm, DspError> {
    make_biquad(BiquadKind::HighPass, p, fs)
}

fn make_biquad(kind: BiquadKind, p: &ParamValues, fs: f64) -> Result<BuiltAlgorithm, DspError> {
    let mut b = Biquad::new(kind, 5.0, std::f64::consts::FRAC_1_SQRT_2, fs.max(1.0))?;
    b.configure(p, fs)?;
    Ok(BuiltAlgorithm::Processor(Box::new(b)))
}

fn make_moving_average(p: &ParamValues, fs: f64) -> Result<BuiltAlgorithm, DspError> {
    let mut ma = MovingAverage::new(10)?;
    ma.configure(p, fs)?;
    Ok(BuiltAlgorithm::Processor(Box::new(ma)))
}

fn make_detrend(p: &ParamValues, fs: f64) -> Result<BuiltAlgorithm, DspError> {
    let mut d = Detrend::new(100, true)?;
    d.configure(p, fs)?;
    Ok(BuiltAlgorithm::Processor(Box::new(d)))
}

fn make_fft(p: &ParamValues, fs: f64) -> Result<BuiltAlgorithm, DspError> {
    use super::fft::{SpectrumScale, WindowKind, WINDOW_NAMES};
    let n = p.get("n").unwrap_or(1024.0) as usize;
    let window_idx = p.get("window").unwrap_or(1.0) as usize;
    let window = match WINDOW_NAMES.get(window_idx).copied().unwrap_or("hann") {
        "rectangular" => WindowKind::Rectangular,
        "hann" => WindowKind::Hann,
        "hamming" => WindowKind::Hamming,
        "blackman" => WindowKind::Blackman,
        "flat_top" => WindowKind::FlatTop,
        _ => WindowKind::Hann,
    };
    let scale = match p.get("scale").unwrap_or(0.0) as usize {
        1 => SpectrumScale::Psd,
        2 => SpectrumScale::Decibels,
        _ => SpectrumScale::Magnitude,
    };
    let detrend = p.boolean("detrend");
    let an = FftAnalyzer::new(n, fs, window, scale, detrend)?;
    Ok(BuiltAlgorithm::Analyzer(Box::new(an)))
}

/// The algorithm registry, in display order.
pub static REGISTRY: LazyLock<Vec<AlgorithmDescriptor>> = LazyLock::new(|| {
    vec![
        AlgorithmDescriptor {
            info: AlgorithmInfo {
                id: "lpf2".into(),
                name: "2nd-order low-pass".into(),
                kind: AlgoKind::Processor,
                params: biquad_params(),
            },
            factory: make_biquad_lp,
        },
        AlgorithmDescriptor {
            info: AlgorithmInfo {
                id: "hpf2".into(),
                name: "2nd-order high-pass".into(),
                kind: AlgoKind::Processor,
                params: biquad_params(),
            },
            factory: make_biquad_hp,
        },
        AlgorithmDescriptor {
            info: AlgorithmInfo {
                id: "moving_average".into(),
                name: "Moving average".into(),
                kind: AlgoKind::Processor,
                params: moving_average_params(),
            },
            factory: make_moving_average,
        },
        AlgorithmDescriptor {
            info: AlgorithmInfo {
                id: "detrend".into(),
                name: "Detrend".into(),
                kind: AlgoKind::Processor,
                params: detrend_params(),
            },
            factory: make_detrend,
        },
        AlgorithmDescriptor {
            info: AlgorithmInfo {
                id: "fft".into(),
                name: "Realtime FFT".into(),
                kind: AlgoKind::Analyzer,
                params: fft_params(),
            },
            factory: make_fft,
        },
    ]
});

/// All algorithm descriptions, for the frontend parameter forms.
pub fn list_algorithms() -> Vec<AlgorithmInfo> {
    REGISTRY.iter().map(|d| d.info.clone()).collect()
}

/// Build an algorithm by id.
pub fn create(id: &str, params: &ParamValues, fs_hz: f64) -> Result<BuiltAlgorithm, DspError> {
    REGISTRY
        .iter()
        .find(|d| d.info.id == id)
        .map(|d| (d.factory)(params, fs_hz))
        .unwrap_or(Err(DspError::UnknownAlgorithm(id.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_all_algorithms_with_params() {
        let algos = list_algorithms();
        assert!(algos.len() >= 5, "registry size");
        for a in &algos {
            assert!(!a.id.is_empty(), "id");
            assert!(!a.name.is_empty(), "name");
            assert!(!a.params.is_empty(), "{} has params", a.id);
        }
    }

    #[test]
    fn factory_builds_a_processor_and_an_analyzer() {
        let mut p = ParamValues::new();
        p.set_all(&[("fc_hz", 5.0), ("q", std::f64::consts::FRAC_1_SQRT_2)]);
        let lp = create("lpf2", &p, 100.0).expect("lpf2");
        assert!(
            matches!(lp, BuiltAlgorithm::Processor(_)),
            "lpf2 is a processor"
        );

        let fft = create("fft", &ParamValues::new(), 1000.0).expect("fft");
        assert!(
            matches!(fft, BuiltAlgorithm::Analyzer(_)),
            "fft is an analyzer"
        );
    }

    #[test]
    fn unknown_algorithm_is_rejected() {
        assert!(matches!(
            create("nope", &ParamValues::new(), 1000.0),
            Err(DspError::UnknownAlgorithm(_))
        ));
    }
}
