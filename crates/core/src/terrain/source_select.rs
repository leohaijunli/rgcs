//! DEM source resolution for an area of interest (plan §6.4 **H6**,
//! operator instruction 2026-10-17): given the candidate sources the app can
//! reach, pick the **highest-precision one that covers the AOI** — the tier
//! order is ① an HRDEM session (1 m DTM+DSM, per-project) → ② local
//! processed LiDAR tiles (1 m, cm-grade DTM+DSM) → ③ GeoTIFF fallbacks
//! (CDEM/Copernicus ~30 m, DTM-only) → ④ GLO-30 download (a *suggestion* —
//! the download itself stays a command, plan rule 13).
//!
//! Pure strategy: no IO, no network — the caller assembles candidates from
//! discovery, the prepared-survey status and config, and this module ranks
//! them. Table-driven tests pin the priority, the precision ordering within
//! a tier, and the no-coverage fallback.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{DemBounds, DemSource};

/// One candidate source the resolver may choose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct SourceCandidate {
    /// Stable identifier the caller uses to act on the choice (a tile folder,
    /// a prepared-session folder, ...).
    pub id: String,
    /// Which tier the candidate belongs to.
    pub source: DemSource,
    /// Nominal ground resolution, metres (1 m LiDAR, ~30 m CDEM/GLO-30).
    pub resolution_m: f64,
    /// Coverage — a candidate that does not cover the AOI is never chosen
    /// unless the resolver is allowed to fall through to the download tier.
    pub bounds: DemBounds,
    /// Whether the candidate carries a real DSM (the clearance check needs
    /// one; a DTM-only source is usable but downranked within its tier).
    pub has_dsm: bool,
    /// Acquisition date (`dem.json`), for the age the chip warns about.
    pub acquisition: Option<String>,
}

/// What the resolver decided.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SourceSelection {
    /// A covering candidate won (its `id` acts on the caller's map).
    Use(SourceCandidate),
    /// Nothing covering exists in any tier: the caller should offer the
    /// GLO-30 download for the AOI (tier ④) — the resolver never downloads.
    SuggestDownload,
}

/// Rank a source tier (lower wins): HRDEM session → processed LiDAR →
/// GeoTIFF fallback → (the download suggestion is not a candidate tier).
#[must_use]
pub fn tier_of(source: DemSource) -> u8 {
    match source {
        DemSource::Hrdem => 0,
        DemSource::Lidar => 1,
        DemSource::Geotiff => 2,
    }
}

/// Pick the best source for `aoi` among `candidates` (H6):
///
/// 1. **Coverage first** — a candidate must contain the AOI centre (the same
///    rule `pick_tile` has always used; an AOI spanning tiles is the
///    operator's clipping problem, reported by the drape).
/// 2. **Tier order** — HRDEM session beats processed LiDAR beats GeoTIFF,
///    regardless of resolution numbers (a 1 m GeoTIFF would still be tier ③).
/// 3. **Within a tier** — finer resolution wins; a tie prefers the candidate
///    with a real DSM; a further tie prefers the more recent acquisition,
///    then the lexicographically first id (deterministic).
///
/// No covering candidate at all → [`SourceSelection::SuggestDownload`].
#[must_use]
pub fn select_source(candidates: &[SourceCandidate], aoi: &DemBounds) -> SourceSelection {
    let cx = (aoi.north + aoi.south) / 2.0;
    let cy = (aoi.east + aoi.west) / 2.0;
    let covering: Vec<&SourceCandidate> = candidates
        .iter()
        .filter(|c| {
            c.bounds.north >= cx
                && cx >= c.bounds.south
                && c.bounds.east >= cy
                && cy >= c.bounds.west
        })
        .collect();
    let best = covering.into_iter().min_by(|a, b| {
        tier_of(a.source)
            .cmp(&tier_of(b.source))
            .then(
                a.resolution_m
                    .partial_cmp(&b.resolution_m)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(b.has_dsm.cmp(&a.has_dsm))
            .then(b.acquisition.cmp(&a.acquisition))
            .then(a.id.cmp(&b.id))
    });
    match best {
        Some(c) => SourceSelection::Use(c.clone()),
        None => SourceSelection::SuggestDownload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100 m AOI centred at (48.5, -123.4).
    fn aoi() -> DemBounds {
        DemBounds {
            north: 48.5 + 0.0005,
            south: 48.5 - 0.0005,
            east: -123.4 + 0.0007,
            west: -123.4 - 0.0007,
        }
    }

    fn candidate(
        id: &str,
        source: DemSource,
        res: f64,
        covers: bool,
        has_dsm: bool,
    ) -> SourceCandidate {
        SourceCandidate {
            id: id.to_string(),
            source,
            resolution_m: res,
            bounds: if covers {
                aoi()
            } else {
                // Far away: nothing near the AOI centre.
                DemBounds {
                    north: 50.0,
                    south: 49.9,
                    east: -122.0,
                    west: -122.1,
                }
            },
            has_dsm,
            acquisition: None,
        }
    }

    #[test]
    fn the_tier_order_beats_everything() {
        // A coarse GeoTIFF, a processed LiDAR tile and an HRDEM session all
        // cover: the HRDEM session wins even though the LiDAR tile is finer
        // on paper (tier ① > ② > ③).
        let candidates = vec![
            candidate("geotiff", DemSource::Geotiff, 30.0, true, false),
            candidate("lidar", DemSource::Lidar, 1.0, true, true),
            candidate("hrdem", DemSource::Hrdem, 1.0, true, true),
        ];
        match select_source(&candidates, &aoi()) {
            SourceSelection::Use(c) => assert_eq!(c.id, "hrdem"),
            other => panic!("expected a use, got {other:?}"),
        }
    }

    #[test]
    fn finer_resolution_wins_within_a_tier() {
        let candidates = vec![
            candidate("lidar-2m", DemSource::Lidar, 2.0, true, true),
            candidate("lidar-1m", DemSource::Lidar, 1.0, true, true),
        ];
        match select_source(&candidates, &aoi()) {
            SourceSelection::Use(c) => assert_eq!(c.id, "lidar-1m"),
            other => panic!("expected a use, got {other:?}"),
        }
    }

    #[test]
    fn a_real_dsm_breaks_a_resolution_tie() {
        let candidates = vec![
            candidate("dsm-only", DemSource::Lidar, 1.0, true, false),
            candidate("with-dsm", DemSource::Lidar, 1.0, true, true),
        ];
        match select_source(&candidates, &aoi()) {
            SourceSelection::Use(c) => assert_eq!(c.id, "with-dsm"),
            other => panic!("expected a use, got {other:?}"),
        }
    }

    #[test]
    fn a_more_recent_acquisition_breaks_a_full_tie() {
        let mut older = candidate("older", DemSource::Lidar, 1.0, true, true);
        older.acquisition = Some("2018-01-01".into());
        let mut newer = candidate("newer", DemSource::Lidar, 1.0, true, true);
        newer.acquisition = Some("2021-06-01".into());
        match select_source(&[older, newer], &aoi()) {
            SourceSelection::Use(c) => assert_eq!(c.id, "newer"),
            other => panic!("expected a use, got {other:?}"),
        }
    }

    #[test]
    fn non_covering_candidates_are_ignored_even_in_better_tiers() {
        // The HRDEM session covers a different area: the local LiDAR tile wins.
        let candidates = vec![
            candidate("hrdem-elsewhere", DemSource::Hrdem, 1.0, false, true),
            candidate("lidar-here", DemSource::Lidar, 1.0, true, true),
        ];
        match select_source(&candidates, &aoi()) {
            SourceSelection::Use(c) => assert_eq!(c.id, "lidar-here"),
            other => panic!("expected a use, got {other:?}"),
        }
    }

    #[test]
    fn nothing_covering_suggests_the_glo30_download() {
        let candidates = vec![
            candidate("lidar-elsewhere", DemSource::Lidar, 1.0, false, true),
            candidate("geotiff-elsewhere", DemSource::Geotiff, 30.0, false, false),
        ];
        assert_eq!(
            select_source(&candidates, &aoi()),
            SourceSelection::SuggestDownload
        );
        assert_eq!(select_source(&[], &aoi()), SourceSelection::SuggestDownload);
    }

    #[test]
    fn the_choice_is_deterministic() {
        let candidates = vec![
            candidate("b", DemSource::Lidar, 1.0, true, true),
            candidate("a", DemSource::Lidar, 1.0, true, true),
        ];
        match select_source(&candidates, &aoi()) {
            SourceSelection::Use(c) => assert_eq!(c.id, "a", "id tiebreak"),
            other => panic!("expected a use, got {other:?}"),
        }
    }
}
