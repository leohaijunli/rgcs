//! Bounded full-log plot snapshots. ULog decoding and DSP stay in core; this
//! adapter scans an independent reader so history never moves the playhead.

use std::collections::HashMap;

use maggcs_core::calib::MagCalibration;
use maggcs_core::inspector::{Session, Trace};
use maggcs_core::ulog::replay::UlogSource;
use serde::Serialize;

use crate::inspector_service::TraceFrame;

/// First/last, raw min/max, filtered min/max, and one gap per time bucket.
const POINTS_PER_BUCKET: usize = 7;
const MAX_TRACES: usize = 256;
const MAX_POINTS_PER_TRACE: usize = 65_536;
const MAX_TOTAL_POINTS: usize = 1_048_576;

#[derive(Debug, Serialize)]
pub struct UlogHistory {
    pub name: String,
    pub duration_ms: f64,
    pub traces: Vec<TraceFrame>,
}

#[derive(Clone, Copy)]
struct Point {
    order: usize,
    t: f64,
    raw: f64,
    filtered: f64,
}

#[derive(Default, Clone)]
struct Bucket {
    first: Option<Point>,
    last: Option<Point>,
    raw_min: Option<Point>,
    raw_max: Option<Point>,
    filtered_min: Option<Point>,
    filtered_max: Option<Point>,
    gap: Option<Point>,
}

impl Bucket {
    fn push(&mut self, p: Point) {
        if self.first.is_none_or(|old| p.t < old.t) {
            self.first = Some(p);
        }
        if self.last.is_none_or(|old| p.t >= old.t) {
            self.last = Some(p);
        }
        if p.raw.is_finite() {
            if self.raw_min.is_none_or(|old| p.raw < old.raw) {
                self.raw_min = Some(p);
            }
            if self.raw_max.is_none_or(|old| p.raw > old.raw) {
                self.raw_max = Some(p);
            }
        }
        if p.filtered.is_finite() {
            if self
                .filtered_min
                .is_none_or(|old| p.filtered < old.filtered)
            {
                self.filtered_min = Some(p);
            }
            if self
                .filtered_max
                .is_none_or(|old| p.filtered > old.filtered)
            {
                self.filtered_max = Some(p);
            }
        }
        if !p.raw.is_finite() && !p.filtered.is_finite() && self.gap.is_none() {
            self.gap = Some(p);
        }
    }

    fn points(&self) -> impl Iterator<Item = Point> {
        [
            self.first,
            self.last,
            self.raw_min,
            self.raw_max,
            self.filtered_min,
            self.filtered_max,
            self.gap,
        ]
        .into_iter()
        .flatten()
    }
}

struct TraceBuckets {
    buckets: Vec<Bucket>,
    next_order: usize,
}

impl TraceBuckets {
    fn push(&mut self, duration_ms: f64, t: f64, raw: f64, filtered: f64) {
        if !t.is_finite() {
            return;
        }
        let i = if duration_ms > 0.0 {
            ((t.max(0.0) / duration_ms) * self.buckets.len() as f64) as usize
        } else {
            0
        }
        .min(self.buckets.len() - 1);
        self.buckets[i].push(Point {
            order: self.next_order,
            t,
            raw,
            filtered,
        });
        self.next_order = self.next_order.saturating_add(1);
    }

    fn frame(self, trace_id: String) -> TraceFrame {
        let mut points: Vec<_> = self.buckets.iter().flat_map(Bucket::points).collect();
        points.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.order.cmp(&b.order)));
        points.dedup_by_key(|p| p.order);
        TraceFrame {
            trace_id,
            t: points.iter().map(|p| p.t).collect(),
            raw: points.iter().map(|p| p.raw).collect(),
            filtered: points.iter().map(|p| p.filtered).collect(),
        }
    }
}

/// Scan the whole recording, retaining time-bucket extrema rather than a
/// rolling tail. `current` cancels a replaced request/source between records.
pub(crate) fn collect(
    mut source: UlogSource,
    name: String,
    traces: Vec<Trace>,
    calibration: Option<MagCalibration>,
    max_points: usize,
    current: impl Fn() -> bool,
) -> Result<UlogHistory, String> {
    if max_points < POINTS_PER_BUCKET || traces.len() > MAX_TRACES {
        return Err(format!(
            "history needs at least {POINTS_PER_BUCKET} points and at most {MAX_TRACES} traces"
        ));
    }
    // A calibrated mag source can add one derived trace per requested trace.
    let output_traces = traces.len().saturating_mul(2).max(1);
    let points_per_trace = max_points
        .min(MAX_POINTS_PER_TRACE)
        .min(MAX_TOTAL_POINTS / output_traces);
    let bucket_count = points_per_trace / POINTS_PER_BUCKET;
    let duration_ms = source.duration_ms();
    let mut session = Session::new();
    session.set_mag_calibration(calibration);
    session.set_traces(traces).map_err(|e| e.to_string())?;
    let subscribed = session.subscribe_ids();
    let mut columns: HashMap<String, TraceBuckets> = HashMap::new();
    loop {
        if !current() {
            return Err("ULog history request was replaced".into());
        }
        let Some(step) = source.next_step().map_err(|e| e.to_string())? else {
            break;
        };
        for sample in &step.samples {
            if !subscribed.contains(&sample.id) {
                continue;
            }
            for sample in session.ingest(sample) {
                columns
                    .entry(sample.trace_id)
                    .or_insert_with(|| TraceBuckets {
                        buckets: vec![Bucket::default(); bucket_count],
                        next_order: 0,
                    })
                    .push(duration_ms, sample.t_ms, sample.raw, sample.filtered);
            }
        }
    }
    let mut traces: Vec<_> = columns.into_iter().map(|(id, b)| b.frame(id)).collect();
    traces.sort_by(|a, b| a.trace_id.cmp(&b.trace_id));
    Ok(UlogHistory {
        name,
        duration_ms,
        traces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use maggcs_core::inspector::{AlgoConfig, AnalyzerSource};
    use maggcs_core::ulog::ULOG_MAGIC;
    use std::io::Cursor;

    fn recording(samples: usize) -> UlogSource {
        fn record(bytes: &mut Vec<u8>, tag: u8, payload: &[u8]) {
            bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
            bytes.push(tag);
            bytes.extend_from_slice(payload);
        }
        let mut bytes = ULOG_MAGIC.to_vec();
        bytes.push(1);
        bytes.extend_from_slice(&0u64.to_le_bytes());
        record(&mut bytes, b'F', b"sensor:uint64_t timestamp;float x;");
        let mut topic = vec![0, 7, 0];
        topic.extend_from_slice(b"sensor");
        record(&mut bytes, b'A', &topic);
        for i in 0..samples {
            let mut body = 7u16.to_le_bytes().to_vec();
            body.extend_from_slice(&(i as u64 * 1000).to_le_bytes());
            let value = match i {
                1000 => 9999.0,
                179_000 => -9999.0,
                _ => (i as f32 / 100.0).sin(),
            };
            body.extend_from_slice(&value.to_le_bytes());
            record(&mut bytes, b'D', &body);
        }
        UlogSource::from_reader(Cursor::new(bytes)).expect("ULog")
    }

    fn trace(pipeline: Vec<AlgoConfig>) -> Trace {
        Trace {
            id: "x".into(),
            signal: recording(1)
                .signals()
                .into_iter()
                .find(|s| s.field == "x")
                .expect("x signal"),
            pipeline,
            analyzer: None,
            analyzer_source: AnalyzerSource::Raw,
        }
    }

    #[test]
    fn whole_dense_log_preserves_both_ends_and_early_late_peaks_with_bounded_points() {
        let history = collect(
            recording(180_001),
            "dense.ulg".into(),
            vec![trace(vec![])],
            None,
            256,
            || true,
        )
        .expect("history");
        assert_eq!(history.duration_ms, 180_000.0);
        let x = &history.traces[0];
        assert_eq!(x.t.first(), Some(&0.0));
        assert_eq!(x.t.last(), Some(&180_000.0));
        assert!(
            x.t.len() <= 256,
            "bounded even beyond the 65,536 live-ring cap"
        );
        assert!(x.raw.contains(&9999.0));
        assert!(x.raw.contains(&-9999.0));
        assert!(x.t.windows(2).all(|t| t[0] <= t[1]));
    }

    #[test]
    fn snapshot_runs_the_core_filter_chain_and_keeps_short_log_data() {
        let history = collect(
            recording(20),
            "short.ulg".into(),
            vec![trace(vec![AlgoConfig {
                algorithm: "lpf2".into(),
                params: vec![("fc_hz".into(), 5.0)],
            }])],
            None,
            256,
            || true,
        )
        .expect("filtered history");
        let x = &history.traces[0];
        assert_eq!(x.t.len(), 20);
        assert_eq!(x.raw.len(), x.filtered.len());
        assert!(x.filtered.iter().any(|v| v.is_finite()));
        assert!(x
            .raw
            .iter()
            .zip(&x.filtered)
            .any(|(a, b)| (a - b).abs() > 0.001));
    }

    #[test]
    fn invalid_limits_and_replaced_requests_are_reported() {
        assert!(collect(
            recording(20),
            "test".into(),
            vec![trace(vec![])],
            None,
            0,
            || true
        )
        .is_err());
        let error = collect(
            recording(20),
            "test".into(),
            vec![trace(vec![])],
            None,
            256,
            || false,
        )
        .expect_err("replaced request");
        assert!(error.contains("replaced"));
    }
}
