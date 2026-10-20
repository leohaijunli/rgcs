//! The post-flight drape report (plan §5B DR7): aggregates the session's
//! `drape.jsonl` per line — the in-band fraction, the worst deviations, the
//! tightest clearance and the duration — and renders CSV / Markdown summaries
//! tied back to the flown plan by its `spec_hash`.
//!
//! Pure: the reader is tolerant of a truncated tail (see [`super::drape_log`]),
//! and the exports are plain strings so the acceptance ("replay statistics
//! match the live readouts; the file remains parseable after an abnormal
//! exit") is testable without any I/O.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::drape_log::DrapeLine;

/// One line's aggregate over the session.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DrapeFlightLineSummary {
    pub line_id: String,
    /// Samples recorded on this line.
    pub samples: u64,
    /// `|e_agl| ≤ band` fraction over the line's samples, 0..1.
    pub in_band_ratio: f64,
    /// The largest AGL deviation seen (signed), metres.
    pub e_agl_worst_m: f64,
    /// The mean AGL deviation, metres (bias indicator).
    pub e_agl_mean_m: f64,
    /// The tightest actual clearance, metres (None without a DSM).
    pub min_clearance_m: Option<f64>,
    /// The tightest look-ahead margin, metres.
    pub min_lookahead_m: Option<f64>,
    /// The tightest radar-vs-DEM difference (median), metres.
    pub e_dem_median_m: Option<f64>,
    /// The line's duration from its first to last sample, seconds.
    pub duration_s: f64,
}

/// The whole session's report.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DrapeFlightReport {
    /// The flown plan's `spec_hash` (from the log header).
    pub spec_hash: String,
    /// The DEM provenance hash.
    pub dem_hash: String,
    /// Lines in first-seen order.
    pub lines: Vec<DrapeFlightLineSummary>,
    /// Samples outside any line (between lines, run-in).
    pub unassigned_samples: u64,
    /// Total samples in the log.
    pub total_samples: u64,
}

/// Build the report from parsed log lines.
#[must_use]
pub fn build_report(lines: &[DrapeLine]) -> DrapeFlightReport {
    let mut report = DrapeFlightReport::default();
    let mut order: Vec<String> = Vec::new();
    let mut per_line: std::collections::HashMap<String, DrapeFlightLineSummary> =
        std::collections::HashMap::new();
    let mut acc: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();

    for line in lines {
        match line {
            DrapeLine::Meta(meta) => {
                report.spec_hash = meta.spec_hash.clone();
                report.dem_hash = meta.dem_hash.clone();
            }
            DrapeLine::Sample(sample) => {
                report.total_samples += 1;
                let Some(line_id) = sample.line_id.clone() else {
                    report.unassigned_samples += 1;
                    continue;
                };
                if !per_line.contains_key(&line_id) {
                    order.push(line_id.clone());
                    per_line.insert(
                        line_id.clone(),
                        DrapeFlightLineSummary {
                            line_id: line_id.clone(),
                            samples: 0,
                            duration_s: 0.0,
                            ..DrapeFlightLineSummary::default()
                        },
                    );
                    acc.insert(line_id.clone(), (sample.utc_us, 0));
                }
                let summary = per_line.get_mut(&line_id).expect("just inserted");
                let (first_utc, in_band) = acc.get_mut(&line_id).expect("just inserted");
                summary.samples += 1;
                let last_utc = (*first_utc).max(sample.utc_us);
                *first_utc = (*first_utc).min(sample.utc_us);
                summary.duration_s = last_utc.saturating_sub(*first_utc) as f64 / 1_000_000.0;
                if let Some(e) = sample.e_agl_m {
                    // In-band: the ±3 m acceptance band (the plan's agl_band
                    // default; the live panel shows the same number).
                    if e.abs() <= 3.0 {
                        *in_band += 1;
                    }
                    summary.e_agl_mean_m += e;
                    if e.abs() > summary.e_agl_worst_m.abs() {
                        summary.e_agl_worst_m = e;
                    }
                }
                if let Some(c) = sample.clearance_m {
                    summary.min_clearance_m =
                        Some(summary.min_clearance_m.map_or(c, |m: f64| m.min(c)));
                }
                if let Some(l) = sample.lookahead_clearance_m {
                    summary.min_lookahead_m =
                        Some(summary.min_lookahead_m.map_or(l, |m: f64| m.min(l)));
                }
                if let Some(r) = sample.e_dem_median_m {
                    summary.e_dem_median_m = Some(r);
                }
            }
        }
    }

    for line_id in &order {
        if let Some(summary) = per_line.get_mut(line_id) {
            if let Some((_, in_band)) = acc.get(line_id.as_str()) {
                summary.in_band_ratio = if summary.samples > 0 {
                    *in_band as f64 / summary.samples as f64
                } else {
                    0.0
                };
            }
            if summary.samples > 0 {
                summary.e_agl_mean_m /= summary.samples as f64;
            }
        }
    }
    report.lines = order.iter().filter_map(|id| per_line.remove(id)).collect();
    report
}

/// The CSV export: one row per line, the header naming every column.
#[must_use]
pub fn to_csv(report: &DrapeFlightReport) -> String {
    let mut out = String::from("line,samples,in_band_ratio,e_agl_worst_m,e_agl_mean_m,min_clearance_m,min_lookahead_m,e_dem_median_m,duration_s\n");
    for l in &report.lines {
        out.push_str(&format!(
            "{},{},{:.3},{:.2},{:.2},{},{},{},{:.1}\n",
            l.line_id,
            l.samples,
            l.in_band_ratio,
            l.e_agl_worst_m,
            l.e_agl_mean_m,
            l.min_clearance_m
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "NA".into()),
            l.min_lookahead_m
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "NA".into()),
            l.e_dem_median_m
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "NA".into()),
            l.duration_s,
        ));
    }
    out
}

/// The Markdown summary: the header ties the report to the flown plan.
#[must_use]
pub fn to_markdown(report: &DrapeFlightReport) -> String {
    let mut out = String::from("# Drape flight summary\n\n");
    out.push_str(&format!(
        "- Plan `spec_hash`: `{}`\n- DEM hash: `{}`\n- Samples: {} ({} outside any line)\n\n",
        report.spec_hash, report.dem_hash, report.total_samples, report.unassigned_samples
    ));
    out.push_str("| Line | In band | Worst e_agl | Mean e_agl | Min clearance | Duration |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for l in &report.lines {
        out.push_str(&format!(
            "| {} | {:.0}% | {:+.1} m | {:+.1} m | {} | {:.0} s |\n",
            l.line_id,
            l.in_band_ratio * 100.0,
            l.e_agl_worst_m,
            l.e_agl_mean_m,
            l.min_clearance_m
                .map(|v| format!("{v:.1} m"))
                .unwrap_or_else(|| "—".into()),
            l.duration_s,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::drape_log::{DrapeMeta, DrapeSampleRecord};

    fn meta() -> DrapeLine {
        DrapeLine::Meta(DrapeMeta {
            spec_hash: "ab12".into(),
            dem_hash: "cd34".into(),
            started_utc_us: 0,
        })
    }

    fn sample(line: Option<&str>, utc_s: u64, e_agl: f64, clearance: f64) -> DrapeLine {
        DrapeLine::Sample(DrapeSampleRecord {
            utc_us: utc_s * 1_000_000,
            line_id: line.map(String::from),
            e_agl_m: Some(e_agl),
            clearance_m: Some(clearance),
            lookahead_clearance_m: Some(clearance - 5.0),
            e_dem_median_m: Some(0.5),
            in_band_ratio: 0.5,
            ..DrapeSampleRecord::default()
        })
    }

    #[test]
    fn the_report_aggregates_per_line_and_keeps_the_plan_identity() {
        let lines = vec![
            meta(),
            sample(Some("S1"), 0, 1.0, 40.0),
            sample(Some("S1"), 2, 2.5, 38.0),
            sample(Some("S1"), 4, -1.0, 42.0),
            sample(Some("S2"), 6, 8.0, 12.0), // out of band
            sample(Some("S2"), 8, 1.0, 44.0),
            sample(None, 10, 0.0, 39.0), // between lines
        ];
        let report = build_report(&lines);
        assert_eq!(report.spec_hash, "ab12");
        assert_eq!(report.dem_hash, "cd34");
        assert_eq!(report.total_samples, 6);
        assert_eq!(report.unassigned_samples, 1);
        assert_eq!(report.lines.len(), 2);
        let s1 = &report.lines[0];
        assert_eq!(s1.line_id, "S1");
        assert_eq!(s1.samples, 3);
        // All three S1 samples are inside ±3 m.
        assert!((s1.in_band_ratio - 1.0).abs() < 1e-9);
        assert!((s1.e_agl_worst_m - 2.5).abs() < 1e-9);
        assert!((s1.e_agl_mean_m - (1.0 + 2.5 - 1.0) / 3.0).abs() < 1e-9);
        assert!((s1.min_clearance_m.expect("dsm") - 38.0).abs() < 1e-9);
        assert!((s1.duration_s - 4.0).abs() < 1e-9);
        let s2 = &report.lines[1];
        assert!((s2.in_band_ratio - 0.5).abs() < 1e-9, "one of two in band");
        assert!((s2.e_agl_worst_m - 8.0).abs() < 1e-9);
    }

    #[test]
    fn the_exports_carry_every_column() {
        let report = build_report(&[meta(), sample(Some("S1"), 0, 1.0, 40.0)]);
        let csv = to_csv(&report);
        assert!(csv.starts_with("line,samples,in_band_ratio"));
        assert!(csv.contains("S1,1,1.000"));
        let md = to_markdown(&report);
        assert!(md.contains("# Drape flight summary"));
        assert!(md.contains("ab12"));
        assert!(md.contains("| S1 |"));
    }

    #[test]
    fn an_empty_log_is_an_empty_report() {
        let report = build_report(&[]);
        assert_eq!(report.total_samples, 0);
        assert!(report.lines.is_empty());
        assert!(to_csv(&report).starts_with("line,samples"));
    }

    #[test]
    fn a_truncated_tail_does_not_reach_the_report() {
        // The reader drops torn lines before we ever see them; this pins that
        // the aggregation only counts whole samples.
        let lines = vec![
            meta(),
            sample(Some("S1"), 0, 1.0, 40.0),
            sample(Some("S1"), 2, 1.0, 40.0),
        ];
        let report = build_report(&lines);
        assert_eq!(report.lines[0].samples, 2);
        assert!((report.lines[0].duration_s - 2.0).abs() < 1e-9);
    }
}
