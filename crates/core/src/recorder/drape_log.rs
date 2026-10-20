//! The drape time-series log (plan §5B DR7): one JSON object per line in
//! `drape.jsonl`, next to the tlog and the event log it contextualises.
//!
//! The first line is a header object (`"drape_meta"`) carrying the
//! `spec_hash` and DEM reference so a summary is always reproducible against
//! the exact plan that was flown; sample lines follow (`"drape_sample"`).
//! A truncated tail line (a crash mid-write) is skipped by the reader — the
//! plan's acceptance: "the file remains parseable after an abnormal exit".

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::RecorderError;

/// The file name inside a session directory.
pub const DRAPE_FILE: &str = "drape.jsonl";

/// One monitor sample (the scalars DR7 archives; alerts already flow through
/// the alert-center bridge into `events.jsonl`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DrapeSampleRecord {
    /// Epoch microseconds (the step's wall clock).
    pub utc_us: u64,
    /// The compiled-route projection, metres.
    pub s_star_m: Option<f64>,
    pub line_id: Option<String>,
    pub ez_m: Option<f64>,
    pub e_agl_m: Option<f64>,
    pub cross_track_m: Option<f64>,
    pub clearance_m: Option<f64>,
    pub lookahead_clearance_m: Option<f64>,
    pub e_dem_median_m: Option<f64>,
    /// `fresh`/`stale`/`outside_dem`.
    pub fresh: String,
    /// The latched-critical state at this sample.
    pub latched_critical: bool,
    /// Cumulative in-band fraction at this sample.
    pub in_band_ratio: f64,
}

impl Default for DrapeSampleRecord {
    fn default() -> Self {
        Self {
            utc_us: 0,
            s_star_m: None,
            line_id: None,
            ez_m: None,
            e_agl_m: None,
            cross_track_m: None,
            clearance_m: None,
            lookahead_clearance_m: None,
            e_dem_median_m: None,
            fresh: "fresh".into(),
            latched_critical: false,
            in_band_ratio: 0.0,
        }
    }
}

/// The header line: which plan the series belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrapeMeta {
    /// SHA-256 of the `DrapeSpec` the baseline was solved with.
    pub spec_hash: String,
    /// The DEM provenance hash.
    pub dem_hash: String,
    /// Epoch microseconds the series started.
    pub started_utc_us: u64,
}

/// One parsed line of `drape.jsonl`.
#[derive(Debug, Clone, PartialEq)]
pub enum DrapeLine {
    Meta(DrapeMeta),
    Sample(DrapeSampleRecord),
}

/// Read a `drape.jsonl`, tolerating a truncated tail (crash mid-write).
///
/// # Errors
///
/// [`RecorderError::Io`] when the file cannot be read at all.
pub fn read_drape_log(path: &Path) -> Result<Vec<DrapeLine>, RecorderError> {
    let text = fs::read_to_string(path)?;
    Ok(parse_drape_log(&text))
}

/// Parse the file body: `meta` first, samples after; a truncated last line is
/// dropped, never fatal (the acceptance rule).
#[must_use]
pub fn parse_drape_log(text: &str) -> Vec<DrapeLine> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(parsed) = try_parse_line(line) else {
            // A torn tail (crash mid-write) or a future schema: skip it.
            continue;
        };
        out.push(parsed);
    }
    out
}

fn try_parse_line(line: &str) -> Option<DrapeLine> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("drape_meta").is_some() {
        let meta: DrapeMeta = serde_json::from_value(value.get("drape_meta")?.clone()).ok()?;
        return Some(DrapeLine::Meta(meta));
    }
    let sample: DrapeSampleRecord =
        serde_json::from_value(value.get("drape_sample")?.clone()).ok()?;
    Some(DrapeLine::Sample(sample))
}

/// Serialise one sample line (the writer appends a trailing newline).
///
/// # Errors
///
/// Serialization failure (practically never).
pub fn sample_to_line(sample: &DrapeSampleRecord) -> Result<String, RecorderError> {
    let mut text = serde_json::to_string(&serde_json::json!({ "drape_sample": sample }))?;
    text.push('\n');
    Ok(text)
}

/// Serialise the meta header line.
///
/// # Errors
///
/// Serialization failure (practically never).
pub fn meta_to_line(meta: &DrapeMeta) -> Result<String, RecorderError> {
    let mut text = serde_json::to_string(&serde_json::json!({ "drape_meta": meta }))?;
    text.push('\n');
    Ok(text)
}

/// Append one line to the session's `drape.jsonl` (creates the file with the
/// meta header when absent). Writer-thread only.
pub(super) fn write_line(dir: &Path, meta: &DrapeMeta, line: &str) -> std::io::Result<()> {
    let path = dir.join(DRAPE_FILE);
    let needs_header = !path.exists();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    if needs_header {
        let header = meta_to_line(meta).map_err(std::io::Error::other)?;
        file.write_all(header.as_bytes())?;
    }
    file.write_all(line.as_bytes())?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(utc: u64, e_agl: f64) -> DrapeSampleRecord {
        DrapeSampleRecord {
            utc_us: utc,
            s_star_m: Some(utc as f64 / 200.0),
            line_id: Some("S1".into()),
            e_agl_m: Some(e_agl),
            in_band_ratio: 0.5,
            ..DrapeSampleRecord::default()
        }
    }

    #[test]
    fn the_log_round_trips_and_the_meta_travels_with_it() {
        let meta = DrapeMeta {
            spec_hash: "ab12".into(),
            dem_hash: "cd34".into(),
            started_utc_us: 1_000,
        };
        let mut text = meta_to_line(&meta).expect("meta");
        for i in 0..3 {
            text.push_str(&sample_to_line(&sample(1_000 + i * 200, 1.5)).expect("sample"));
        }
        let parsed = parse_drape_log(&text);
        assert_eq!(parsed.len(), 4);
        assert!(matches!(&parsed[0], DrapeLine::Meta(m) if m.spec_hash == "ab12"));
        for line in parsed[1..].iter() {
            assert!(matches!(line, DrapeLine::Sample(_)));
        }
    }

    #[test]
    fn a_truncated_tail_line_is_skipped_never_fatal() {
        let mut text = meta_to_line(&DrapeMeta {
            spec_hash: "x".into(),
            dem_hash: "y".into(),
            started_utc_us: 0,
        })
        .expect("meta");
        text.push_str(&sample_to_line(&sample(200, 1.0)).expect("sample"));
        // A torn line: the crash happened mid-write.
        text.push_str("{\"drape_sample\":{\"utc_us\":40");
        let parsed = parse_drape_log(&text);
        assert_eq!(parsed.len(), 2, "meta + one whole sample only");
    }

    #[test]
    fn future_or_unknown_lines_are_skipped() {
        let text = "{\"drape_future\":{\"x\":1}}\nnot json at all\n";
        assert!(parse_drape_log(text).is_empty());
    }
}
