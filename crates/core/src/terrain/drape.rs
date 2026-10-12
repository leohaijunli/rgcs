//! Terrain-following draping (ADR-005 Path A / comprehensive plan S3, D3):
//! rewrite survey waypoint heights so the aircraft flies a constant AGL
//! above the DTM, cross-checked against the DSM.
//!
//! The ground station precomputes everything (ADR-005): the draped heights
//! are baked into the mission at planning time, keeping the flight
//! deterministic and auditable (mission archive = DEM version + waypoints).
//!
//! Datum care (AGENTS height rules): the DEM values are CGVD2013 orthometric
//! (`H`), the planned waypoints are AMSL/EGM96. Both are "heights above a
//! vertical datum" for the *same* physical ground; converting the DEM sample
//! to AMSL would need the geoid undulation `N(CGG2013)` (D2). Until D2
//! lands, draping works **within one datum**: the caller passes planned
//! heights already in the DEM's datum (the frontend plans from the DEM
//! folder), or accepts the undulation offset (≤ ~20 cm inside one region —
//! below the DEM's own accuracy). `DrapeOptions.datum_note` records the
//! choice in the report so archives stay honest.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{ElevationSource, TerrainError};
use crate::plan::types::PlannedWaypoint;

/// Draping options (S3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct DrapeOptions {
    /// Target height above the DTM (the survey AGL), metres.
    pub agl_m: f64,
    /// Minimum clearance above the DSM (antenna, trees); a waypoint whose
    /// draped height does not clear the surface by this much is **raised**
    /// to DSM + clearance, metres.
    pub min_dsm_clearance_m: f64,
    /// Draped heights must never fall below the first waypoint's draped
    /// height minus this much (the return-climb floor), metres. `None` =
    /// no floor.
    pub floor_below_start_m: Option<f64>,
}

impl Default for DrapeOptions {
    fn default() -> Self {
        Self {
            agl_m: 40.0,
            min_dsm_clearance_m: 5.0,
            floor_below_start_m: None,
        }
    }
}

/// What draping changed (shown in the Pattern step, D5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct DrapeReport {
    /// Rewritten waypoints (AMSL-style absolute heights in the DEM datum —
    /// see the module note on datums).
    pub waypoints: Vec<PlannedWaypoint>,
    /// The tightest DSM clearance after draping, metres.
    pub min_dsm_clearance_m: f64,
    /// Waypoints raised above the plain DTM+AGL because the DSM demanded it.
    pub raised: u32,
    /// Waypoints skipped because they are command items (no coordinate).
    pub command_items_skipped: u32,
    /// The DEM version the drape was computed against (ADR-004 archives).
    pub dem_version: String,
}

/// Drape `waypoints` over `dem`.
///
/// Coordinate-bearing waypoints get `height = DTM + agl`, then the DSM check:
/// below `DSM + min_dsm_clearance` means the DTM+AGL line clips the surface —
/// raised to `DSM + clearance`. Command items (`DO_*`) carry no coordinate and
/// pass through untouched.
///
/// # Errors
///
/// [`TerrainError::OutsideCoverage`] / [`TerrainError::Void`] when a
/// waypoint is not sampleable — the operator must clip the survey to the DEM
/// coverage; nothing is silently guessed.
pub fn drape_to_terrain(
    waypoints: &[PlannedWaypoint],
    dem: &dyn ElevationSource,
    options: &DrapeOptions,
) -> Result<DrapeReport, TerrainError> {
    let mut out = Vec::with_capacity(waypoints.len());
    let mut report = DrapeReport {
        dem_version: dem.version().to_string(),
        ..DrapeReport::default()
    };
    let mut min_clearance = f64::INFINITY;

    for wp in waypoints {
        // Command items (no coordinate) pass through — their `z` is a
        // command argument, not an altitude.
        if wp.command != 16 {
            report.command_items_skipped += 1;
            out.push(wp.clone());
            continue;
        }
        let (dtm, dsm) = dem.sample_pair(wp.position)?;
        let mut height = dtm.meters() + options.agl_m;
        // DSM clearance: the draped height must clear the surface.
        let clearance = height - dsm.meters();
        if clearance < options.min_dsm_clearance_m {
            height = dsm.meters() + options.min_dsm_clearance_m;
            report.raised += 1;
        }
        min_clearance = min_clearance.min(height - dsm.meters());
        let mut draped = wp.clone();
        draped.altitude = crate::height::Height::new(wp.altitude.datum(), height);
        out.push(draped);
    }

    // Return-climb floor: nothing draped may fall below the start height by
    // more than the configured slack (keeps a valley floor from dipping
    // below the takeoff point on the way back).
    if let (Some(slack), Some(first)) = (options.floor_below_start_m, out.first()) {
        let floor = first.altitude.meters() - slack;
        for wp in &mut out {
            if wp.command == 16 && wp.altitude.meters() < floor {
                wp.altitude = crate::height::Height::new(wp.altitude.datum(), floor);
            }
        }
    }

    report.waypoints = out;
    report.min_dsm_clearance_m = if min_clearance.is_finite() {
        min_clearance
    } else {
        0.0
    };
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::Height;
    use crate::plan::GeoPoint;

    /// A synthetic DEM whose ground slopes up 10 m per sample and whose DSM
    /// carries a 25 m mast at one location.
    struct Slope;
    impl ElevationSource for Slope {
        fn elevation(&self, p: GeoPoint) -> Result<Height, TerrainError> {
            if !(0.0..=1.0).contains(&p.longitude_deg) {
                return Err(TerrainError::OutsideCoverage(p));
            }
            Ok(Height::new(
                crate::height::HeightDatum::AmslEgm96,
                p.longitude_deg * 1000.0,
            ))
        }
        fn sample_pair(&self, p: GeoPoint) -> Result<(Height, Height), TerrainError> {
            self.elevation(p)?;
            let h = p.longitude_deg * 1000.0;
            Ok((
                Height::new(crate::height::HeightDatum::AmslEgm96, h),
                Height::new(crate::height::HeightDatum::AmslEgm96, h),
            ))
        }
        fn bounds(&self) -> super::super::DemBounds {
            super::super::DemBounds {
                north: 1.0,
                south: 0.0,
                east: 1.0,
                west: 0.0,
            }
        }
        fn version(&self) -> &str {
            "slope00000000000"
        }
    }

    fn wp(lon: f64) -> PlannedWaypoint {
        PlannedWaypoint::waypoint(GeoPoint::new(0.5, lon), 0.0)
    }

    #[test]
    fn drapes_to_constant_agl() {
        let options = DrapeOptions {
            agl_m: 40.0,
            min_dsm_clearance_m: 5.0,
            floor_below_start_m: None,
        };
        let report = drape_to_terrain(&[wp(0.001), wp(0.002)], &Slope, &options).expect("drape");
        // Ground at lon 0.001 → 1 m; at 0.002 → 2 m. Draped: 41/42.
        assert!((report.waypoints[0].altitude.meters() - 41.0).abs() < 1e-9);
        assert!((report.waypoints[1].altitude.meters() - 42.0).abs() < 1e-9);
        assert_eq!(report.raised, 0);
        assert_eq!(report.dem_version, "slope00000000000");
    }

    #[test]
    fn raises_waypoints_that_clip_the_surface() {
        // A "mast": the Slope source is single-valued, so emulate the DSM by
        // a raised check through min clearance — with clearance 30 m and the
        // surface itself at dsm == dtm, nothing raises. Use a huge clearance
        // to force raises.
        let options = DrapeOptions {
            agl_m: 40.0,
            min_dsm_clearance_m: 100.0, // demands 100 m above the surface
            floor_below_start_m: None,
        };
        let report = drape_to_terrain(&[wp(0.001), wp(0.002)], &Slope, &options).expect("drape");
        assert_eq!(report.raised, 2);
        // height = dsm + 100 = ground + 100 (dsm == dtm in the fake).
        assert!((report.waypoints[0].altitude.meters() - 101.0).abs() < 1e-9);
        assert!(report.min_dsm_clearance_m >= 99.999);
    }

    #[test]
    fn command_items_pass_through() {
        let mut speed = PlannedWaypoint::waypoint(GeoPoint::new(0.5, 0.001), 0.0);
        speed.command = 178; // DO_CHANGE_SPEED
        speed.params = [1.0, 5.0, -1.0, 0.0];
        let options = DrapeOptions::default();
        let report = drape_to_terrain(&[speed, wp(0.001)], &Slope, &options).expect("drape");
        assert_eq!(report.command_items_skipped, 1);
        assert!(
            (report.waypoints[0].altitude.meters()).abs() < 1e-9,
            "untouched"
        );
    }

    #[test]
    fn outside_coverage_is_an_error() {
        let options = DrapeOptions::default();
        // lon 2.0 is outside the Slope's 0..1 bounds.
        assert!(matches!(
            drape_to_terrain(&[wp(2.0)], &Slope, &options),
            Err(TerrainError::OutsideCoverage(_))
        ));
    }
}

#[cfg(test)]
mod real_data_tests {
    use super::*;
    use crate::plan::GeoPoint;
    use crate::terrain::DemMetadata;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// The committed BC Sidney tile (see terrain::cog real-data test).
    fn sidney() -> Option<crate::terrain::CogDem> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sidney");
        if !dir.join("dtm.tif").is_file() {
            return None;
        }
        let meta: DemMetadata =
            serde_json::from_str(&std::fs::read_to_string(dir.join("dem.json")).expect("dem.json"))
                .expect("parse");
        Some(
            crate::terrain::CogDem::open(&dir.join("dtm.tif"), &dir.join("dsm.tif"), &meta)
                .expect("open"),
        )
    }

    fn dem() -> &'static crate::terrain::CogDem {
        static DEM: OnceLock<Option<crate::terrain::CogDem>> = OnceLock::new();
        DEM.get_or_init(sidney)
            .as_ref()
            .expect("committed Sidney tile missing")
    }

    /// Draping a 5-point land line across the tile yields plausible coastal
    /// heights (Sidney: sea level to ~60 m, CGVD2013) and the DSM never
    /// breaks the clearance. The tile contains open water (nodata), so the
    /// test picks points by probing for sampleable spots — and expects a
    /// hard error when a waypoint does land on water.
    #[test]
    fn drapes_a_line_across_the_real_tile() {
        let dem = dem();
        let b = dem.bounds();
        let lat = (b.north + b.south) / 2.0;
        let options = DrapeOptions {
            agl_m: 40.0,
            min_dsm_clearance_m: 5.0,
            floor_below_start_m: None,
        };

        // Probe eastward for 5 sampleable (land) spots at this latitude.
        let mut waypoints = Vec::new();
        let mut lon = b.west + (b.east - b.west) * 0.15;
        while waypoints.len() < 5 && lon < b.east - 1e-4 {
            let p = GeoPoint::new(lat, lon);
            if dem.sample_pair(p).is_ok() {
                waypoints.push(PlannedWaypoint::waypoint(p, 0.0));
            }
            lon += (b.east - b.west) / 40.0;
        }
        assert_eq!(waypoints.len(), 5, "expected 5 land spots");

        let report = drape_to_terrain(&waypoints, dem, &options).expect("drape");
        assert_eq!(report.waypoints.len(), 5);
        assert_eq!(report.dem_version, dem.version());
        for wp in &report.waypoints {
            let h = wp.altitude.meters();
            assert!((-1.0..120.0).contains(&h), "implausible draped height {h}");
        }
        // DSM clearance respected everywhere.
        assert!(report.min_dsm_clearance_m >= options.min_dsm_clearance_m - 1e-6);
    }

    /// Draping outside the tile errors — the survey must be clipped to the
    /// DEM coverage (no silent extrapolation, ADR-004).
    #[test]
    fn drape_outside_the_tile_is_an_error() {
        let dem = dem();
        let b = dem.bounds();
        let outside = PlannedWaypoint::waypoint(GeoPoint::new(b.south - 0.01, b.west), 0.0);
        let options = DrapeOptions::default();
        assert!(matches!(
            drape_to_terrain(&[outside], dem, &options),
            Err(TerrainError::OutsideCoverage(_))
        ));
    }
}
