//! The session: a set of traces, each with its own [`Pipeline`] and optional
//! [`Analyzer`], fed by the tap's sample stream (plan §5, "一条 Trace 一份
//! Pipeline/Analyzer").
//!
//! - `set_traces` replaces the trace set; an unchanged trace keeps its running
//!   filter state ("热更新,尽量不丢状态"), a changed one is rebuilt.
//! - `ingest` routes one sample to its trace, applies the pipeline, and emits
//!   the raw + filtered sample. A stream gap (`dt > GAP_FACTOR × 标称 dt`)
//!   resets the filter and emits a `NaN` sample so the curve breaks (SDI shows
//!   the same gap).
//! - The per-trace sample rate is an EMA; the coefficients are redesigned only
//!   when fs drifts past [`FS_RETUNE_FRACTION`].

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dsp::pipeline::Pipeline;
use crate::dsp::registry;
use crate::dsp::{Analyzer, DspError, ParamValues, SpectrumFrame};
use crate::signals::{SignalId, SignalSample};

/// A sample gap longer than `GAP_FACTOR` times the nominal dt breaks the curve.
pub const GAP_FACTOR: f64 = 3.0;
/// EMA smoothing for the per-trace sample-rate estimate.
const FS_EMA_ALPHA: f64 = 0.1;
/// Rebuild filter coefficients when fs drifts by more than this fraction.
const FS_RETUNE_FRACTION: f64 = 0.05;
/// Fallback sample rate used until two samples establish an estimate.
const NOMINAL_FS_HZ: f64 = 100.0;
/// Minimum usable sample rate, Hz.
const MIN_FS_HZ: f64 = 0.01;

/// Stable identity for one trace, chosen by the frontend so it survives
/// workspace reloads and link reconnects. Two traces may share one signal.
pub type TraceId = String;

/// Errors from configuring or running the inspector session.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SessionError {
    #[error("unknown algorithm: {0}")]
    UnknownAlgorithm(String),
    #[error("invalid parameters: {0}")]
    InvalidParams(String),
    #[error("a trace with id {0:?} already exists")]
    DuplicateTrace(TraceId),
    #[error("algorithm {0} is an analyzer, not a filter")]
    NotAFilter(String),
    #[error("algorithm {0} is a filter, not an analyzer")]
    NotAnAnalyzer(String),
}

impl From<DspError> for SessionError {
    fn from(e: DspError) -> Self {
        match e {
            DspError::UnknownAlgorithm(id) => Self::UnknownAlgorithm(id),
            other => Self::InvalidParams(other.to_string()),
        }
    }
}

/// One algorithm bound to parameters, as configured by the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AlgoConfig {
    /// A registry algorithm id (`lpf2`, `fft`, …).
    pub algorithm: String,
    /// Parameter key/value pairs for that algorithm.
    pub params: Vec<(String, f64)>,
}

impl AlgoConfig {
    fn param_values(&self) -> ParamValues {
        let mut p = ParamValues::new();
        for (k, v) in &self.params {
            p.set(k, *v);
        }
        p
    }
}

/// The full configuration of one plotted trace: a stable id, the source signal
/// plus the filter chain and optional analyzer to apply to it. Several traces
/// may bind the same signal with different filters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Trace {
    /// Stable identity within one plot window (see [`TraceId`]).
    pub id: TraceId,
    pub signal: SignalId,
    /// Ordered filter stages applied to every sample (`detrend → LPF`).
    pub pipeline: Vec<AlgoConfig>,
    /// An optional windowed analyzer (the FFT) on the raw signal.
    pub analyzer: Option<AlgoConfig>,
    /// Whether the analyzer window sees the raw signal or the filtered output.
    #[serde(default)]
    pub analyzer_source: AnalyzerSource,
}

/// What the analyzer window is fed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzerSource {
    /// The value straight off the link (default).
    #[default]
    Raw,
    /// The pipeline output (falls back to raw when there is no pipeline).
    Filtered,
}

/// One plot window: a stable id, a title, and the traces drawn on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Plot {
    pub id: String,
    pub title: String,
    pub traces: Vec<Trace>,
}

/// One processed sample streamed to the inspector window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceSample {
    pub trace_id: TraceId,
    /// Host-aligned time in milliseconds.
    pub t_ms: f64,
    pub raw: f64,
    /// Filtered value; `NaN` when the trace has no pipeline or across a gap.
    pub filtered: f64,
}

/// Streaming per-trace sample-rate estimate (EMA of the sample dt).
#[derive(Debug, Clone, Default)]
struct FsEstimator {
    ema_dt_ms: Option<f64>,
    last_t_ms: Option<f64>,
}

impl FsEstimator {
    /// Update with a sample time and return the (possibly fresh) rate estimate.
    /// Returns `None` until a second sample establishes a first dt.
    fn observe(&mut self, t_ms: f64) -> Option<f64> {
        if let Some(last) = self.last_t_ms {
            let dt = (t_ms - last).abs();
            if dt > 0.0 {
                self.ema_dt_ms = Some(match self.ema_dt_ms {
                    None => dt,
                    Some(ema) => ema + FS_EMA_ALPHA * (dt - ema),
                });
            }
        }
        self.last_t_ms = Some(t_ms);
        self.fs_hz()
    }

    fn fs_hz(&self) -> Option<f64> {
        self.ema_dt_ms.filter(|dt| *dt > 0.0).map(|dt| 1000.0 / dt)
    }

    /// Drop the EMA (a gap polluted it) but keep the last sample time.
    fn clear_ema(&mut self) {
        self.ema_dt_ms = None;
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

struct RunningTrace {
    cfg: Trace,
    pipeline: Pipeline,
    analyzer: Option<Box<dyn Analyzer>>,
    fs: FsEstimator,
    /// Sample rate the pipeline was last configured with (0 = never).
    fs_built: f64,
}

/// The session: the inspector's trace set and its running DSP state, keyed by
/// [`TraceId`] so several traces may share one signal.
#[derive(Default)]
pub struct Session {
    traces: HashMap<TraceId, RunningTrace>,
    /// `signal → trace ids`, for routing one live sample to every trace on it.
    by_signal: HashMap<SignalId, Vec<TraceId>>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.traces.is_empty()
    }

    pub fn len(&self) -> usize {
        self.traces.len()
    }

    /// The signal ids the tap must subscribe to (the union of the traces).
    pub fn subscribe_ids(&self) -> HashSet<SignalId> {
        self.by_signal.keys().cloned().collect()
    }

    /// Replace the trace set: adds new traces, rebuilds changed ones (keeping
    /// the running filter state when id and configuration are unchanged), and
    /// drops removed traces. Several traces may bind the same signal. Returns
    /// the new subscription set.
    pub fn set_traces(
        &mut self,
        configs: impl IntoIterator<Item = Trace>,
    ) -> Result<HashSet<SignalId>, SessionError> {
        let mut next: HashMap<TraceId, RunningTrace> = HashMap::new();
        for cfg in configs {
            if next.contains_key(&cfg.id) {
                return Err(SessionError::DuplicateTrace(cfg.id));
            }
            match self.traces.remove(&cfg.id) {
                Some(existing) if existing.cfg == cfg => {
                    // Unchanged configuration: keep the running DSP state.
                    next.insert(cfg.id, existing);
                }
                Some(mut existing) => {
                    // Rebuild the DSP but keep the fs estimate.
                    let fs = existing.fs.fs_hz().unwrap_or(NOMINAL_FS_HZ);
                    existing.cfg = cfg;
                    existing.pipeline = build_pipeline(&existing.cfg, fs)?;
                    existing.analyzer = build_analyzer(existing.cfg.analyzer.as_ref(), fs)?;
                    existing.fs_built = 0.0; // reconfigure on the next sample
                    next.insert(existing.cfg.id.clone(), existing);
                }
                None => {
                    let fs = NOMINAL_FS_HZ;
                    let pipeline = build_pipeline(&cfg, fs)?;
                    let analyzer = build_analyzer(cfg.analyzer.as_ref(), fs)?;
                    next.insert(
                        cfg.id.clone(),
                        RunningTrace {
                            cfg,
                            pipeline,
                            analyzer,
                            fs: FsEstimator::default(),
                            fs_built: 0.0,
                        },
                    );
                }
            }
        }
        // Rebuild the signal → trace-id routing table for the new set.
        let mut by_signal: HashMap<SignalId, Vec<TraceId>> = HashMap::new();
        for (id, trace) in &next {
            by_signal
                .entry(trace.cfg.signal.clone())
                .or_default()
                .push(id.clone());
        }
        self.traces = next;
        self.by_signal = by_signal;
        Ok(self.subscribe_ids())
    }

    /// Apply one live sample to every trace that subscribes to its signal.
    /// Returns zero samples when none does; otherwise the raw+filtered sample
    /// per trace and, when a stream gap is detected, a leading `NaN` sample
    /// that breaks the curve.
    pub fn ingest(&mut self, sample: &SignalSample) -> Vec<TraceSample> {
        let Some(ids) = self.by_signal.get(&sample.id).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(ids.len() * 2);
        for id in ids {
            if let Some(trace) = self.traces.get_mut(&id) {
                process(trace, id, sample, &mut out);
            }
        }
        out
    }

    /// Collect a fresh spectrum frame from every analyzer, tagged with its
    /// trace id.
    pub fn poll_spectra(&mut self) -> Vec<(TraceId, SpectrumFrame)> {
        let mut frames = Vec::new();
        for (id, trace) in self.traces.iter_mut() {
            if let Some(analyzer) = &mut trace.analyzer {
                if let Some(frame) = analyzer.poll() {
                    frames.push((id.clone(), frame));
                }
            }
        }
        frames
    }

    /// Drop every trace (the window closed).
    pub fn clear(&mut self) {
        self.traces.clear();
        self.by_signal.clear();
    }

    /// Keep the configurations but drop all filter/analyzer state (reconnect).
    pub fn reset(&mut self) {
        for trace in self.traces.values_mut() {
            trace.pipeline.reset();
            trace.fs.reset();
            trace.fs_built = 0.0;
        }
    }
}

/// Apply one sample to one running trace, appending its output to `out`.
fn process(
    trace: &mut RunningTrace,
    trace_id: TraceId,
    sample: &SignalSample,
    out: &mut Vec<TraceSample>,
) {
    // The prior rate estimate and the new dt decide the gap check before the
    // estimate absorbs this sample's dt.
    let dt_ms = trace.fs.last_t_ms.map(|last| (sample.t_ms - last).abs());
    let fs_before = trace.fs.fs_hz();
    let fs_est = trace.fs.observe(sample.t_ms);
    if let (Some(dt), Some(fs)) = (dt_ms, fs_before) {
        let nominal_ms = 1000.0 / fs.max(MIN_FS_HZ);
        if dt > GAP_FACTOR * nominal_ms {
            trace.pipeline.reset();
            trace.fs.clear_ema();
            trace.fs_built = 0.0;
            out.push(TraceSample {
                trace_id: trace_id.clone(),
                t_ms: trace.fs.last_t_ms.unwrap_or(sample.t_ms) - nominal_ms,
                raw: f64::NAN,
                filtered: f64::NAN,
            });
        }
    }
    let mut filtered = f64::NAN;
    if !trace.pipeline.is_empty() {
        let fs = fs_est.unwrap_or(NOMINAL_FS_HZ);
        let retune = trace.fs_built <= 0.0
            || (fs - trace.fs_built).abs() / trace.fs_built.max(MIN_FS_HZ) > FS_RETUNE_FRACTION;
        if retune && trace.pipeline.configure(fs).is_ok() {
            trace.fs_built = fs;
        }
        filtered = trace.pipeline.process(sample.value);
    }
    let source = trace.cfg.analyzer_source;
    if let Some(analyzer) = &mut trace.analyzer {
        let x = match source {
            AnalyzerSource::Raw => sample.value,
            // A pipeline that produced a finite value feeds the FFT; otherwise
            // (no pipeline, or across a gap) fall back to the raw sample.
            AnalyzerSource::Filtered if filtered.is_finite() => filtered,
            AnalyzerSource::Filtered => sample.value,
        };
        analyzer.push(sample.t_ms / 1000.0, x);
    }
    out.push(TraceSample {
        trace_id,
        t_ms: sample.t_ms,
        raw: sample.value,
        filtered,
    });
}

/// Build a filter chain from a trace's stage list.
fn build_pipeline(cfg: &Trace, fs_hz: f64) -> Result<Pipeline, SessionError> {
    let mut pipeline = Pipeline::new();
    for stage in &cfg.pipeline {
        match registry::create(&stage.algorithm, &stage.param_values(), fs_hz)? {
            registry::BuiltAlgorithm::Processor(p) => {
                pipeline.push(p, stage.param_values());
            }
            registry::BuiltAlgorithm::Analyzer(_) => {
                return Err(SessionError::NotAFilter(stage.algorithm.clone()));
            }
        }
    }
    Ok(pipeline)
}

/// Build an optional analyzer from a trace's analyzer config.
fn build_analyzer(
    config: Option<&AlgoConfig>,
    fs_hz: f64,
) -> Result<Option<Box<dyn Analyzer>>, SessionError> {
    let Some(config) = config else {
        return Ok(None);
    };
    match registry::create(&config.algorithm, &config.param_values(), fs_hz)? {
        registry::BuiltAlgorithm::Analyzer(a) => Ok(Some(a)),
        registry::BuiltAlgorithm::Processor(_) => {
            Err(SessionError::NotAnAnalyzer(config.algorithm.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(msg: u32, field: &str) -> SignalId {
        SignalId::new(1, 1, msg, field)
    }

    fn sample(id: &SignalId, t_ms: f64, value: f64) -> SignalSample {
        SignalSample {
            id: id.clone(),
            t_ms,
            value,
        }
    }

    fn lpf(fc: f64) -> AlgoConfig {
        AlgoConfig {
            algorithm: "lpf2".into(),
            params: vec![
                ("fc_hz".into(), fc),
                ("q".into(), std::f64::consts::FRAC_1_SQRT_2),
            ],
        }
    }

    /// A one-trace config; the id is the field name so tests read clearly.
    fn trace(signal: SignalId, pipeline: Vec<AlgoConfig>) -> Trace {
        let id = signal.field.clone();
        trace_id(&id, signal, pipeline)
    }

    fn trace_id(id: &str, signal: SignalId, pipeline: Vec<AlgoConfig>) -> Trace {
        Trace {
            id: id.to_string(),
            signal,
            pipeline,
            analyzer: None,
            analyzer_source: AnalyzerSource::Raw,
        }
    }

    #[test]
    fn set_traces_returns_the_subscription_union() {
        let mut s = Session::new();
        let ids = s
            .set_traces(vec![
                trace(sig(30, "roll"), vec![]),
                trace(sig(30, "pitch"), vec![lpf(5.0)]),
            ])
            .unwrap();
        assert_eq!(ids.len(), 2, "union size");
        assert!(ids.contains(&sig(30, "roll")));
        assert!(ids.contains(&sig(30, "pitch")));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn duplicate_trace_id_is_rejected() {
        let mut s = Session::new();
        // Two traces with the same id in one call are rejected.
        assert!(matches!(
            s.set_traces(vec![
                trace_id("a", sig(30, "roll"), vec![]),
                trace_id("a", sig(30, "pitch"), vec![]),
            ]),
            Err(SessionError::DuplicateTrace(_))
        ));
    }

    #[test]
    fn the_same_signal_can_back_two_traces() {
        let mut s = Session::new();
        let ids = s
            .set_traces(vec![
                trace_id("raw", sig(30, "roll"), vec![]),
                trace_id("lp", sig(30, "roll"), vec![lpf(5.0)]),
            ])
            .unwrap();
        assert_eq!(ids.len(), 1, "one signal subscribed once");
        assert!(ids.contains(&sig(30, "roll")));
        assert_eq!(s.len(), 2, "but two traces run");
        let out = s.ingest(&sample(&sig(30, "roll"), 0.0, 1.0));
        assert_eq!(out.len(), 2, "both traces emit a sample");
        let mut out_ids: Vec<&str> = out.iter().map(|o| o.trace_id.as_str()).collect();
        out_ids.sort();
        assert_eq!(out_ids, vec!["lp", "raw"]);
    }

    #[test]
    fn unknown_algorithm_is_rejected() {
        let mut s = Session::new();
        let cfg = Trace {
            id: "roll".into(),
            signal: sig(30, "roll"),
            pipeline: vec![AlgoConfig {
                algorithm: "nope".into(),
                params: vec![],
            }],
            analyzer: None,
            analyzer_source: AnalyzerSource::Raw,
        };
        assert!(matches!(
            s.set_traces(vec![cfg]),
            Err(SessionError::UnknownAlgorithm(_))
        ));
    }

    #[test]
    fn analyzer_in_a_pipeline_slot_is_rejected() {
        let mut s = Session::new();
        let cfg = Trace {
            id: "roll".into(),
            signal: sig(30, "roll"),
            pipeline: vec![AlgoConfig {
                algorithm: "fft".into(),
                params: vec![],
            }],
            analyzer: None,
            analyzer_source: AnalyzerSource::Raw,
        };
        assert!(matches!(
            s.set_traces(vec![cfg]),
            Err(SessionError::NotAFilter(_))
        ));
    }

    #[test]
    fn ingest_routes_to_the_right_trace_and_filters() {
        let mut s = Session::new();
        s.set_traces(vec![
            trace(sig(30, "roll"), vec![lpf(20.0)]),
            trace(sig(30, "pitch"), vec![]),
        ])
        .unwrap();
        for i in 0..100 {
            let t = i as f64 * 10.0; // 100 Hz
            let roll = s.ingest(&sample(&sig(30, "roll"), t, (i as f64 * 0.1).sin()));
            assert_eq!(roll.len(), 1, "one filtered sample");
            assert!(roll[0].filtered.is_finite(), "lpf filters");
            let pitch = s.ingest(&sample(&sig(30, "pitch"), t, i as f64));
            assert_eq!(pitch.len(), 1);
            assert!(pitch[0].filtered.is_nan(), "no pipeline -> NaN");
            assert_eq!(pitch[0].raw, i as f64);
        }
    }

    #[test]
    fn unsubscribed_signals_produce_no_samples() {
        let mut s = Session::new();
        s.set_traces(vec![trace(sig(30, "roll"), vec![])]).unwrap();
        assert!(s.ingest(&sample(&sig(30, "pitch"), 0.0, 1.0)).is_empty());
    }

    #[test]
    fn a_gap_breaks_the_curve_and_resets_the_filter() {
        let mut s = Session::new();
        s.set_traces(vec![trace(sig(30, "roll"), vec![lpf(10.0)])])
            .unwrap();
        for i in 0..10 {
            s.ingest(&sample(&sig(30, "roll"), i as f64 * 10.0, 1.0));
        }
        let out = s.ingest(&sample(&sig(30, "roll"), 100_000.0, 1.0));
        assert_eq!(out.len(), 2, "a NaN break plus the sample");
        assert!(out[0].raw.is_nan() && out[0].filtered.is_nan(), "break");
        assert!(out[1].filtered.is_finite(), "filter restarts after the gap");
    }

    #[test]
    fn hot_update_keeps_state_when_config_unchanged() {
        let mut s = Session::new();
        s.set_traces(vec![trace(sig(30, "roll"), vec![lpf(5.0)])])
            .unwrap();
        for i in 0..100 {
            s.ingest(&sample(&sig(30, "roll"), i as f64 * 10.0, 1.0));
        }
        s.set_traces(vec![trace(sig(30, "roll"), vec![lpf(5.0)])])
            .unwrap();
        let out = s.ingest(&sample(&sig(30, "roll"), 1000.0, 1.0));
        assert_eq!(out.len(), 1);
        let v = out[0].filtered;
        assert!(v.is_finite() && v > 0.9, "steady state preserved, got {v}");
    }

    #[test]
    fn changing_params_rebuilds_the_pipeline() {
        let mut s = Session::new();
        s.set_traces(vec![trace(sig(30, "roll"), vec![lpf(5.0)])])
            .unwrap();
        s.set_traces(vec![trace(sig(30, "roll"), vec![lpf(20.0)])])
            .unwrap();
        assert_eq!(s.len(), 1, "rebuilt in place");
        let out = s.ingest(&sample(&sig(30, "roll"), 0.0, 1.0));
        assert_eq!(out.len(), 1);
        assert!(out[0].filtered.is_finite());
    }

    #[test]
    fn reset_keeps_configs_and_drops_filter_state() {
        let mut s = Session::new();
        s.set_traces(vec![trace(sig(30, "roll"), vec![lpf(5.0)])])
            .unwrap();
        for i in 0..100 {
            s.ingest(&sample(&sig(30, "roll"), i as f64 * 10.0, 1.0));
        }
        s.reset();
        assert_eq!(s.len(), 1, "configs kept");
        let out = s.ingest(&sample(&sig(30, "roll"), 1000.0, 1.0));
        assert!(out[0].filtered.is_finite(), "filter restarts clean");
    }

    #[test]
    fn clear_drops_everything() {
        let mut s = Session::new();
        s.set_traces(vec![trace(sig(30, "roll"), vec![])]).unwrap();
        s.clear();
        assert!(s.is_empty());
        assert!(s.subscribe_ids().is_empty());
    }

    #[test]
    fn analyzer_produces_a_spectrum_frame() {
        let mut s = Session::new();
        s.set_traces(vec![Trace {
            id: "roll".into(),
            signal: sig(30, "roll"),
            pipeline: vec![],
            analyzer: Some(AlgoConfig {
                algorithm: "fft".into(),
                params: vec![("n".into(), 64.0)],
            }),
            analyzer_source: AnalyzerSource::Raw,
        }])
        .unwrap();
        for i in 0..64 {
            s.ingest(&sample(
                &sig(30, "roll"),
                i as f64 * 10.0,
                (i as f64 * 0.1).sin(),
            ));
        }
        let frames = s.poll_spectra();
        assert_eq!(frames.len(), 1, "one analyzer, one frame");
        assert_eq!(frames[0].0, "roll", "tagged with the trace id");
        assert_eq!(frames[0].1.n, 64);
        assert!(frames[0].1.bins.len() >= 32);
    }

    #[test]
    fn analyzer_can_run_on_the_filtered_signal() {
        let mut s = Session::new();
        s.set_traces(vec![Trace {
            id: "roll".into(),
            signal: sig(30, "roll"),
            pipeline: vec![lpf(20.0)],
            analyzer: Some(AlgoConfig {
                algorithm: "fft".into(),
                params: vec![("n".into(), 64.0)],
            }),
            analyzer_source: AnalyzerSource::Filtered,
        }])
        .unwrap();
        for i in 0..64 {
            s.ingest(&sample(
                &sig(30, "roll"),
                i as f64 * 10.0,
                (i as f64 * 0.1).sin(),
            ));
        }
        let frames = s.poll_spectra();
        assert_eq!(frames.len(), 1, "filtered-source analyzer still produces a frame");
        assert_eq!(frames[0].1.n, 64);
    }

    #[test]
    fn analyzer_built_with_a_processor_slot_is_rejected() {
        let mut s = Session::new();
        let cfg = Trace {
            id: "roll".into(),
            signal: sig(30, "roll"),
            pipeline: vec![],
            analyzer: Some(AlgoConfig {
                algorithm: "lpf2".into(),
                params: vec![
                    ("fc_hz".into(), 5.0),
                    ("q".into(), std::f64::consts::FRAC_1_SQRT_2),
                ],
            }),
            analyzer_source: AnalyzerSource::Raw,
        };
        let result = s.set_traces(vec![cfg]);
        assert!(matches!(result, Err(SessionError::NotAnAnalyzer(_))));
    }
}
