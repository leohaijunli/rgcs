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
    /// One loaded calibration is applied to each complete magnetometer source,
    /// but no source shares component or dirty state with another.
    mag_calibration: Option<crate::calib::MagCalibration>,
    mag_by_source: HashMap<SignalId, MagAccumulator>,
}

/// Prefix of every Rust-derived calibrated total-field trace id. The complete
/// source follows it, so multiple nodes/messages cannot share a frontend ring.
pub const MAG_TOTAL_TRACE_PREFIX: &str = "mag_total_cal";

/// Stable trace id for one source's calibrated total field. `field` is ignored:
/// x/y/z from one `(system, component, message)` must resolve to the same id.
pub fn mag_total_trace_id(source: &SignalId) -> TraceId {
    format!(
        "{MAG_TOTAL_TRACE_PREFIX}:{}:{}:{}",
        source.system_id, source.component_id, source.message_id
    )
}

fn mag_source(signal: &SignalId) -> SignalId {
    SignalId::new(signal.system_id, signal.component_id, signal.message_id, "")
}

/// Latest-triple state for the calibrated total-field curve. Emits exactly
/// one derived sample per complete triple (dirty flags). There is one instance
/// of this struct per full MAVLink source identity.
#[derive(Default)]
struct MagAccumulator {
    x: Option<f64>,
    y: Option<f64>,
    z: Option<f64>,
    dx: bool,
    dy: bool,
    dz: bool,
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

    /// The signal ids the tap must subscribe to (the union of the traces),
    /// plus the three mag fields when a calibration is loaded (the derived
    /// total needs them even if no trace plots the axes themselves).
    pub fn subscribe_ids(&self) -> HashSet<SignalId> {
        let mut ids: HashSet<SignalId> = self.by_signal.keys().cloned().collect();
        if self.mag_calibration.is_some() {
            // Subscribe a complete triple for every represented source. This
            // must not choose only the first source: one plot can contain all
            // three SCALED_IMU instances (or multiple MAVLink nodes).
            let mut sources: HashSet<SignalId> = self
                .by_signal
                .keys()
                .filter(|s| matches!(s.field.as_str(), "xmag" | "ymag" | "zmag" | "mag_total"))
                .map(mag_source)
                .collect();
            if sources.is_empty() {
                // Preserve the inert fallback used before a live/ULog trace is
                // selected; the real source replaces it on `set_traces`.
                sources.insert(SignalId::new(0, 0, 0, ""));
            }
            for source in sources {
                for field in ["xmag", "ymag", "zmag"] {
                    ids.insert(SignalId {
                        field: field.to_string(),
                        ..source.clone()
                    });
                }
            }
        }
        ids
    }

    /// Load (or clear with `None`) the magnetometer calibration. When set and
    /// the trace set carries the xmag/ymag/zmag fields, `ingest` derives the
    /// calibrated total `|A·(m_raw − b)|` under a source-aware trace id from
    /// [`mag_total_trace_id`]. Clearing drops the derived curves.
    pub fn set_mag_calibration(&mut self, cal: Option<crate::calib::MagCalibration>) {
        self.mag_calibration = cal;
        // Never complete a triple with components received across a calibration
        // change or after calibration was cleared and later reloaded.
        self.mag_by_source.clear();
    }

    /// Whether a mag calibration is loaded.
    pub fn mag_calibration_loaded(&self) -> bool {
        self.mag_calibration.is_some()
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
        // A trace whose filter cannot be designed at the observed rate (e.g. a
        // cutoff at or above Nyquist for a slow stream) must not take the whole
        // trace set down: it keeps streaming raw and we report why afterwards.
        let mut first_error: Option<SessionError> = None;
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
                    existing.pipeline = match build_pipeline(&existing.cfg, fs) {
                        Ok(p) => p,
                        Err(e) => {
                            first_error.get_or_insert(e);
                            Pipeline::new()
                        }
                    };
                    existing.analyzer = match build_analyzer(existing.cfg.analyzer.as_ref(), fs) {
                        Ok(a) => a,
                        Err(e) => {
                            first_error.get_or_insert(e);
                            None
                        }
                    };
                    existing.fs_built = 0.0; // reconfigure on the next sample
                    next.insert(existing.cfg.id.clone(), existing);
                }
                None => {
                    let fs = NOMINAL_FS_HZ;
                    let pipeline = match build_pipeline(&cfg, fs) {
                        Ok(p) => p,
                        Err(e) => {
                            first_error.get_or_insert(e);
                            Pipeline::new()
                        }
                    };
                    let analyzer = match build_analyzer(cfg.analyzer.as_ref(), fs) {
                        Ok(a) => a,
                        Err(e) => {
                            first_error.get_or_insert(e);
                            None
                        }
                    };
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
        match first_error {
            Some(e) => Err(e),
            None => Ok(self.subscribe_ids()),
        }
    }

    /// Apply one live sample to every trace that subscribes to its signal.
    /// Returns zero samples when none does; otherwise the raw+filtered sample
    /// per trace and, when a stream gap is detected, a leading `NaN` sample
    /// that breaks the curve.
    pub fn ingest(&mut self, sample: &SignalSample) -> Vec<TraceSample> {
        let mut out: Vec<TraceSample> = Vec::new();
        // With a calibration loaded, mag samples are buffered and emitted
        // calibrated once the full triple is in (A·(m−b) mixes the axes), so
        // the raw values never reach a trace; without a calibration the raw
        // (nT-normalized) samples flow to their traces below.
        let defer_mag = self.mag_calibration.is_some()
            && matches!(
                sample.id.field.as_str(),
                "xmag" | "ymag" | "zmag" | "mag_total"
            );
        if !defer_mag {
            if let Some(ids) = self.by_signal.get(&sample.id).cloned() {
                out.reserve(ids.len() * 2);
                for id in ids {
                    if let Some(trace) = self.traces.get_mut(&id) {
                        process(trace, id, sample, &mut out);
                    }
                }
            }
        }
        // Calibrated mag values: track the triple on every mag sample and,
        // once all three axes are fresh, emit (plan §9 − the math is here,
        // in Rust, not in the frontend):
        //  - the calibrated components to the traces on xmag/ymag/zmag,
        //  - the calibrated total to traces on the raw `mag_total` signal, and
        //  - one source-aware derived total series.
        let complete = if matches!(sample.id.field.as_str(), "xmag" | "ymag" | "zmag") {
            let Some(cal) = self.mag_calibration.as_ref() else {
                return out;
            };
            let source = mag_source(&sample.id);
            let mag = self.mag_by_source.entry(source.clone()).or_default();
            match sample.id.field.as_str() {
                "xmag" => {
                    mag.x = Some(sample.value);
                    mag.dx = true;
                }
                "ymag" => {
                    mag.y = Some(sample.value);
                    mag.dy = true;
                }
                "zmag" => {
                    mag.z = Some(sample.value);
                    mag.dz = true;
                }
                _ => {}
            }
            if let (true, true, true, Some(x), Some(y), Some(z)) =
                (mag.dx, mag.dy, mag.dz, mag.x, mag.y, mag.z)
            {
                let c = cal.apply([x, y, z], None);
                mag.dx = false;
                mag.dy = false;
                mag.dz = false;
                Some((source, c))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((node, c)) = complete {
            let total = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            for (axis, value) in [("xmag", c[0]), ("ymag", c[1]), ("zmag", c[2])] {
                let cal_samp = SignalSample {
                    id: SignalId {
                        field: axis.to_string(),
                        ..node.clone()
                    },
                    t_ms: sample.t_ms,
                    value,
                };
                if let Some(ids) = self.by_signal.get(&cal_samp.id).cloned() {
                    for id in ids {
                        if let Some(trace) = self.traces.get_mut(&id) {
                            process(trace, id, &cal_samp, &mut out);
                        }
                    }
                }
            }
            let tot_samp = SignalSample {
                id: SignalId {
                    field: "mag_total".to_string(),
                    ..node.clone()
                },
                t_ms: sample.t_ms,
                value: total,
            };
            if let Some(ids) = self.by_signal.get(&tot_samp.id).cloned() {
                for id in ids {
                    if let Some(trace) = self.traces.get_mut(&id) {
                        process(trace, id, &tot_samp, &mut out);
                    }
                }
            }
            out.push(TraceSample {
                trace_id: mag_total_trace_id(&node),
                t_ms: sample.t_ms,
                raw: total,
                filtered: f64::NAN,
            });
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
        self.mag_by_source.clear();
    }

    /// Keep the configurations but drop all filter/analyzer state (reconnect).
    pub fn reset(&mut self) {
        for trace in self.traces.values_mut() {
            trace.pipeline.reset();
            trace.fs.reset();
            trace.fs_built = 0.0;
        }
        self.mag_by_source.clear();
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
mod mag_cal_tests {
    use super::*;
    use crate::calib::CalibrationDomain;

    fn session() -> Session {
        Session::new()
    }

    // Minimal identity calibration (A = I, b = 0).
    fn identity() -> crate::calib::MagCalibration {
        crate::calib::MagCalibration {
            a: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            b: [0.0, 0.0, 0.0],
            k: None,
            domain: CalibrationDomain::Px4Raw,
        }
    }

    fn mag_sample_from(
        system_id: u8,
        component_id: u8,
        message_id: u32,
        field: &str,
        value: f64,
        t_ms: f64,
    ) -> crate::signals::SignalSample {
        crate::signals::SignalSample {
            id: SignalId {
                system_id,
                component_id,
                message_id,
                field: field.to_string(),
            },
            t_ms,
            value,
        }
    }

    fn mag_sample(field: &str, value: f64, t_ms: f64) -> crate::signals::SignalSample {
        mag_sample_from(1, 1, 105, field, value, t_ms)
    }

    fn total_id(system_id: u8, component_id: u8, message_id: u32) -> String {
        mag_total_trace_id(&SignalId::new(system_id, component_id, message_id, ""))
    }

    #[test]
    fn calibrated_total_is_emitted_once_all_axes_arrive() {
        let mut s = session();
        s.set_mag_calibration(Some(identity()));
        // Two axes only: nothing derived yet.
        assert!(s
            .ingest(&mag_sample("xmag", 3.0, 10.0))
            .iter()
            .all(|o| o.trace_id != total_id(1, 1, 105)));
        assert!(s
            .ingest(&mag_sample("ymag", 4.0, 10.0))
            .iter()
            .all(|o| o.trace_id != total_id(1, 1, 105)));
        // Third axis: total = sqrt(9+16+25) = sqrt(50) with identity cal.
        let out = s.ingest(&mag_sample("zmag", 5.0, 10.0));
        let total = out
            .iter()
            .find(|o| o.trace_id == total_id(1, 1, 105))
            .expect("derived total emitted");
        assert!((total.raw - (3f64 * 3.0 + 4.0 * 4.0 + 5.0 * 5.0).sqrt()).abs() < 1e-9);
        assert!(total.raw.is_finite());
    }

    #[test]
    fn calibration_applies_offset_and_matrix() {
        let mut s = session();
        let mut cal = identity();
        cal.b = [1.0, 1.0, 1.0]; // (m - 1)
        cal.a = [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]]; // 2(m-1)
        s.set_mag_calibration(Some(cal));
        s.ingest(&mag_sample("xmag", 1.0, 0.0)); // -> 0
        s.ingest(&mag_sample("ymag", 2.0, 0.0)); // -> 2
        let out = s.ingest(&mag_sample("zmag", 3.0, 0.0)); // -> 4
        let total = out
            .iter()
            .find(|o| o.trace_id == total_id(1, 1, 105))
            .expect("total");
        assert!((total.raw - (0f64 + 4.0 + 16.0).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn clearing_calibration_stops_the_derived_trace() {
        let mut s = session();
        s.set_mag_calibration(Some(identity()));
        s.ingest(&mag_sample("xmag", 1.0, 0.0));
        s.ingest(&mag_sample("ymag", 1.0, 0.0));
        s.ingest(&mag_sample("zmag", 1.0, 0.0));
        assert!(s.mag_calibration_loaded());
        s.set_mag_calibration(None);
        assert!(!s.mag_calibration_loaded());
        assert!(s
            .ingest(&mag_sample("xmag", 2.0, 1.0))
            .iter()
            .all(|o| o.trace_id != total_id(1, 1, 105)));
    }

    #[test]
    fn one_derived_sample_per_complete_triple() {
        let mut s = session();
        s.set_mag_calibration(Some(identity()));
        for field in ["xmag", "ymag", "zmag"] {
            s.ingest(&mag_sample(field, 1.0, 10.0));
        }
        // Same-axis repeat before the other axes refresh: no extra emit.
        let out = s.ingest(&mag_sample("xmag", 2.0, 20.0));
        assert!(out.iter().all(|o| o.trace_id != total_id(1, 1, 105)));
        // Completing the triple again emits exactly one derived sample.
        s.ingest(&mag_sample("ymag", 1.0, 20.0));
        let out = s.ingest(&mag_sample("zmag", 1.0, 20.0));
        assert_eq!(
            out.iter()
                .filter(|o| o.trace_id == total_id(1, 1, 105))
                .count(),
            1
        );
    }

    #[test]
    fn subscribe_ids_includes_mag_fields_when_calibrated() {
        let s = session();
        assert!(!s.subscribe_ids().iter().any(|id| id.field == "xmag"));
        let mut s2 = session();
        s2.set_mag_calibration(Some(identity()));
        assert!(s2.subscribe_ids().iter().any(|id| id.field == "xmag"));
        assert!(s2.subscribe_ids().iter().any(|id| id.field == "ymag"));
        assert!(s2.subscribe_ids().iter().any(|id| id.field == "zmag"));
    }

    #[test]
    fn calibration_calibrates_the_plotted_component_and_total_traces() {
        // With a calibration loaded, the traces on xmag/ymag/zmag and the raw
        // mag_total signal receive the A·(m−b) values, and the raw values never
        // flow (the calibration is applied everywhere, not just the derived
        // total series).
        let mut s = session();
        let trace = |id: &str, field: &str| Trace {
            id: id.to_string(),
            signal: SignalId::new(1, 1, 105, field),
            pipeline: Vec::new(),
            analyzer: None,
            analyzer_source: AnalyzerSource::Raw,
        };
        s.set_traces([
            trace("x", "xmag"),
            trace("y", "ymag"),
            trace("z", "zmag"),
            trace("tot", "mag_total"),
        ])
        .expect("traces");
        // m_cal = 2·(m − 1).
        let mut cal = identity();
        cal.b = [1.0, 1.0, 1.0];
        cal.a = [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]];
        s.set_mag_calibration(Some(cal));

        // Component samples are buffered until the triple is complete.
        assert!(
            s.ingest(&mag_sample("xmag", 2.0, 0.0)).is_empty(),
            "raw xmag must not reach a trace while calibrated"
        );
        assert!(
            s.ingest(&mag_sample("ymag", 3.0, 0.0)).is_empty(),
            "raw ymag must not reach a trace while calibrated"
        );
        let out = s.ingest(&mag_sample("zmag", 4.0, 0.0));

        let value = |id: &str| {
            out.iter()
                .find(|o| o.trace_id == id)
                .map(|o| o.raw)
                .unwrap_or(f64::NAN)
        };
        assert!(
            (value("x") - 2.0).abs() < 1e-9,
            "calibrated x: {}",
            value("x")
        );
        assert!(
            (value("y") - 4.0).abs() < 1e-9,
            "calibrated y: {}",
            value("y")
        );
        assert!(
            (value("z") - 6.0).abs() < 1e-9,
            "calibrated z: {}",
            value("z")
        );
        let total: f64 = (4.0f64 + 16.0 + 36.0).sqrt();
        assert!(
            (value("tot") - total).abs() < 1e-9,
            "calibrated mag_total: {}",
            value("tot")
        );
        let derived = out
            .iter()
            .find(|o| o.trace_id == total_id(1, 1, 105))
            .expect("derived total");
        assert!((derived.raw - total).abs() < 1e-9);
        assert!(
            s.ingest(&mag_sample("mag_total", 999.0, 0.0)).is_empty(),
            "the source's uncalibrated synthetic total must not follow the calibrated total"
        );
    }

    #[test]
    fn subscribe_ids_adopt_the_real_mag_node_when_a_trace_exists() {
        // On a live link the mag triple must be subscribed with the vehicle's
        // own system/component/message ids, not the zero wildcard, so the tap
        // forwards the samples the calibration needs.
        let mut s = session();
        s.set_traces([Trace {
            id: "tot".into(),
            signal: SignalId::new(250, 1, 105, "mag_total"),
            pipeline: Vec::new(),
            analyzer: None,
            analyzer_source: AnalyzerSource::Raw,
        }])
        .expect("traces");
        s.set_mag_calibration(Some(identity()));
        let ids = s.subscribe_ids();
        assert!(
            ids.contains(&SignalId::new(250, 1, 105, "xmag")),
            "xmag subscribed with the real node"
        );
        assert!(ids.contains(&SignalId::new(250, 1, 105, "zmag")));
        assert!(!ids.contains(&SignalId::new(0, 0, 0, "xmag")));
    }

    #[test]
    fn calibration_subscribes_every_represented_magnetometer_source() {
        let mut s = session();
        let trace = |id: &str, message_id: u32| Trace {
            id: id.into(),
            signal: SignalId::new(7, 1, message_id, "xmag"),
            pipeline: Vec::new(),
            analyzer: None,
            analyzer_source: AnalyzerSource::Raw,
        };
        s.set_traces([
            trace("imu1", crate::signals::SCALED_IMU_ID),
            trace("imu2", crate::signals::SCALED_IMU2_ID),
            trace("imu3", crate::signals::SCALED_IMU3_ID),
        ])
        .expect("traces");
        s.set_mag_calibration(Some(identity()));
        let ids = s.subscribe_ids();
        for message_id in [
            crate::signals::SCALED_IMU_ID,
            crate::signals::SCALED_IMU2_ID,
            crate::signals::SCALED_IMU3_ID,
        ] {
            for field in ["xmag", "ymag", "zmag"] {
                assert!(ids.contains(&SignalId::new(7, 1, message_id, field)));
            }
        }
    }

    #[test]
    fn scaled_imu_triples_and_imu3_offset_are_source_isolated() {
        let mut s = session();
        s.set_mag_calibration(Some(identity()));
        let imu1 = crate::signals::SCALED_IMU_ID;
        let imu2 = crate::signals::SCALED_IMU2_ID;
        let imu3 = crate::signals::SCALED_IMU3_ID;

        // Interleave all components. A shared accumulator would form mixed
        // triples here; source-keyed accumulators emit only on their own zmag.
        for (message_id, value) in [(imu1, 3.0), (imu2, 6.0), (imu3, 5.0)] {
            assert!(s
                .ingest(&mag_sample_from(1, 1, message_id, "xmag", value, 10.0))
                .is_empty());
        }
        for (message_id, value) in [(imu1, 4.0), (imu2, 8.0), (imu3, 12.0)] {
            assert!(s
                .ingest(&mag_sample_from(1, 1, message_id, "ymag", value, 10.0))
                .is_empty());
        }
        let imu3_out = s.ingest(&mag_sample_from(1, 1, imu3, "zmag", 0.0, 10.0));
        assert_eq!(imu3_out.len(), 1);
        assert_eq!(imu3_out[0].trace_id, total_id(1, 1, imu3));
        assert!((imu3_out[0].raw - 13.0).abs() < 1e-9);
        let imu1_out = s.ingest(&mag_sample_from(1, 1, imu1, "zmag", 0.0, 10.0));
        let imu2_out = s.ingest(&mag_sample_from(1, 1, imu2, "zmag", 0.0, 10.0));
        assert_eq!(imu1_out[0].trace_id, total_id(1, 1, imu1));
        assert_eq!(imu1_out[0].raw, 5.0);
        assert_eq!(imu2_out[0].trace_id, total_id(1, 1, imu2));
        assert_eq!(imu2_out[0].raw, 10.0);

        // Change only IMU3. It emits only IMU3's ring; IMU1/2 emit nothing and
        // retain no component values from this cycle.
        let mut changed = Vec::new();
        for (field, value) in [("xmag", 105.0), ("ymag", 12.0), ("zmag", 0.0)] {
            changed.extend(s.ingest(&mag_sample_from(1, 1, imu3, field, value, 20.0)));
        }
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].trace_id, total_id(1, 1, imu3));
        assert!((changed[0].raw - (105.0f64.powi(2) + 12.0f64.powi(2)).sqrt()).abs() < 1e-9);
        assert!(changed
            .iter()
            .all(|sample| sample.trace_id != total_id(1, 1, imu1)
                && sample.trace_id != total_id(1, 1, imu2)));
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
    fn a_filter_that_cannot_be_designed_keeps_the_trace_streaming_raw() {
        let mut s = Session::new();
        let signal = sig(30, "roll");
        // A slow stream (1 Hz) makes the registry's 5 Hz default low-pass sit
        // above Nyquist, so the stage cannot be designed at the observed rate.
        s.set_traces(vec![trace(signal.clone(), vec![])]).unwrap();
        for i in 0..10 {
            s.ingest(&sample(&signal, i as f64 * 1000.0, i as f64));
        }
        let err = s
            .set_traces(vec![trace(signal.clone(), vec![lpf(5.0)])])
            .expect_err("cutoff above Nyquist is rejected");
        assert!(matches!(err, SessionError::InvalidParams(_)), "got {err:?}");
        // The trace is still there and still streams its raw signal.
        assert_eq!(s.len(), 1, "trace kept despite the bad filter");
        let out = s.ingest(&sample(&signal, 10_000.0, 42.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].raw, 42.0, "raw keeps flowing");
        assert!(
            out[0].filtered.is_nan(),
            "the undesignable stage is dropped"
        );
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
        assert_eq!(
            frames.len(),
            1,
            "filtered-source analyzer still produces a frame"
        );
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
