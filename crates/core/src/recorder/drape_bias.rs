//! The post-flight DEM bias estimate (plan §5B DR9, operator decision
//! 2026-10-18: **no in-flight re-drape** — assessment only).
//!
//! The aircraft carries a lidar ranger (`DISTANCE_SENSOR`), so the drape
//! monitor's `e_dem = agl_radar − agl_dtm` window is a real measurement of
//! how far the DEM sits from the truth under the flown route. This module
//! turns a session's `drape.jsonl` samples into one robust estimate — median
//! and MAD over the radar-bearing samples — with an uncertainty that grows
//! with spread and shrinks with sample count, and the `vertical_offset`
//! candidate the operator may type into the *next* plan by hand.
//!
//! Never applied automatically; never in flight.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::drape_log::DrapeSampleRecord;

/// Too few radar-bearing samples to say anything — refuse to invent a number.
pub const MIN_SAMPLES: usize = 10;

/// One session's estimate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DemBiasEstimate {
    /// The DEM's signed bias: median of `e_dem = agl_radar − agl_dtm`, metres
    /// (positive = the radar read higher than the DEM ⇒ the DEM is low).
    pub bias_m: f64,
    /// 1-sigma uncertainty of the median (`1.4826·MAD/√n`), metres.
    pub uncertainty_m: f64,
    /// The `vertical_offset` candidate for the NEXT plan, metres: `-bias`
    /// (a DEM reading low ⇒ fly higher to compensate).
    pub vertical_offset_candidate_m: f64,
    /// Radar-bearing samples the estimate rests on.
    pub samples: usize,
    /// The lines those samples came from (bias may be terrain-dependent).
    pub line_ids: Vec<String>,
    /// The flown plan the samples belong to.
    pub spec_hash: String,
}

impl Default for DemBiasEstimate {
    fn default() -> Self {
        Self {
            bias_m: 0.0,
            uncertainty_m: 0.0,
            vertical_offset_candidate_m: 0.0,
            samples: 0,
            line_ids: Vec::new(),
            spec_hash: String::new(),
        }
    }
}

/// The outcome: an estimate, or why there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DemBiasOutcome {
    Estimate(DemBiasEstimate),
    /// Not enough radar-bearing samples (`< MIN_SAMPLES`) — no number beats
    /// an invented one.
    InsufficientSamples {
        samples: usize,
    },
    /// No radar at all on this session (the aircraft's lidar never fed the
    /// monitor).
    NoRadar,
}

/// Estimate the session's DEM bias from the drape log's radar samples.
///
/// Robust by construction: the median shrugs off outliers (a rangefinder
/// dropout reads as a spike, not a shift), and the MAD-derived uncertainty
/// says how tightly the samples cluster.
#[must_use]
pub fn estimate_bias(meta_spec_hash: &str, samples: &[DrapeSampleRecord]) -> DemBiasOutcome {
    let radar: Vec<&DrapeSampleRecord> = samples
        .iter()
        .filter(|s| s.e_dem_median_m.is_some() && s.fresh == "fresh")
        .collect();
    if radar.is_empty() {
        return DemBiasOutcome::NoRadar;
    }
    // Deduplicate: e_dem_median_m repeats within the sliding window; one
    // value per distinct (line, s*) keeps the statistics honest.
    let mut values: Vec<f64> = Vec::new();
    let mut line_ids: Vec<String> = Vec::new();
    for s in &radar {
        let v = s.e_dem_median_m.unwrap_or_default();
        values.push(v);
        if !line_ids.contains(&s.line_id.clone().unwrap_or_default()) {
            if let Some(id) = &s.line_id {
                line_ids.push(id.clone());
            }
        }
    }
    if values.len() < MIN_SAMPLES {
        return DemBiasOutcome::InsufficientSamples {
            samples: values.len(),
        };
    }
    let (median, mad) = median_mad(&values);
    let n = values.len() as f64;
    // Standard error of the median from the MAD (Gaussian convention):
    // 1.4826·MAD is the sigma-equivalent; /√n scales to the mean's.
    let sigma = 1.4826 * mad;
    let uncertainty = sigma / n.sqrt();
    DemBiasOutcome::Estimate(DemBiasEstimate {
        bias_m: median,
        uncertainty_m: uncertainty,
        vertical_offset_candidate_m: -median,
        samples: values.len(),
        line_ids,
        spec_hash: meta_spec_hash.into(),
    })
}

/// Median and MAD (median absolute deviation).
fn median_mad(values: &[f64]) -> (f64, f64) {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let mut dev: Vec<f64> = sorted.iter().map(|v| (v - median).abs()).collect();
    dev.sort_by(f64::total_cmp);
    (median, dev[dev.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::drape_log::DrapeSampleRecord;

    fn sample(line: Option<&str>, e_dem: Option<f64>, fresh: bool) -> DrapeSampleRecord {
        DrapeSampleRecord {
            line_id: line.map(String::from),
            e_dem_median_m: e_dem,
            fresh: if fresh {
                "fresh".into()
            } else {
                "stale".into()
            },
            ..DrapeSampleRecord::default()
        }
    }

    #[test]
    fn a_known_bias_is_recovered_within_the_tolerance() {
        // +2.5 m injected (the radar reads 2.5 m above the DEM's AGL) with
        // Gaussian-ish noise and a few hard outliers.
        let samples: Vec<DrapeSampleRecord> = (0..200)
            .map(|i| {
                let noise = ((i % 7) as f64 - 3.0) * 0.05;
                let outlier = if i % 47 == 0 { 30.0 } else { 0.0 };
                sample(Some("S1"), Some(2.5 + noise + outlier), true)
            })
            .collect();
        let DemBiasOutcome::Estimate(est) = estimate_bias("hash", &samples) else {
            panic!("expected an estimate");
        };
        assert!(
            (est.bias_m - 2.5).abs() < 0.1,
            "bias {} vs injected 2.5",
            est.bias_m
        );
        assert!((est.vertical_offset_candidate_m + 2.5).abs() < 0.1);
        assert!(est.uncertainty_m > 0.0 && est.uncertainty_m < 1.0);
        assert_eq!(est.samples, 200);
        assert_eq!(est.spec_hash, "hash");
    }

    #[test]
    fn too_few_samples_refuses_to_invent_a_number() {
        let samples: Vec<_> = (0..5)
            .map(|_| sample(Some("S1"), Some(1.0), true))
            .collect();
        assert!(matches!(
            estimate_bias("h", &samples),
            DemBiasOutcome::InsufficientSamples { samples: 5 }
        ));
    }

    #[test]
    fn no_radar_and_stale_only_samples_are_neither_estimate_nor_insufficient() {
        assert!(matches!(estimate_bias("h", &[]), DemBiasOutcome::NoRadar));
        let stale: Vec<_> = (0..50)
            .map(|_| sample(Some("S1"), Some(1.0), false))
            .collect();
        assert!(matches!(
            estimate_bias("h", &stale),
            DemBiasOutcome::NoRadar
        ));
    }

    #[test]
    fn a_zero_bias_flies_as_a_zero_candidate() {
        let samples: Vec<_> = (0..40)
            .map(|_| sample(Some("S1"), Some(0.0), true))
            .collect();
        let DemBiasOutcome::Estimate(est) = estimate_bias("h", &samples) else {
            panic!("estimate");
        };
        assert!(est.bias_m.abs() < 1e-9);
        assert!(est.vertical_offset_candidate_m.abs() < 1e-9);
        // Zero MAD (constant samples): the uncertainty is zero — the samples
        // agree perfectly.
        assert!(est.uncertainty_m < 0.05);
    }

    #[test]
    fn the_lines_that_fed_the_estimate_are_listed() {
        let samples: Vec<_> = (0..12)
            .map(|i| {
                sample(
                    if i % 2 == 0 { Some("S1") } else { Some("S2") },
                    Some(1.0),
                    true,
                )
            })
            .collect();
        let DemBiasOutcome::Estimate(est) = estimate_bias("h", &samples) else {
            panic!("estimate");
        };
        assert_eq!(est.line_ids, vec!["S1".to_string(), "S2".to_string()]);
    }
}
