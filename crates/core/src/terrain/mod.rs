//! Terrain elevation sources (ADR-004, comprehensive plan D1): a local COG
//! GeoTIFF DEM is the default and the only required source — field sites
//! often have no internet, and survey-grade planning needs deterministic,
//! versioned terrain data (Cesium ion streaming is explicitly out of scope
//! here).
//!
//! The surface follows ADR-004's `ElevationSource` sketch; sampling never
//! extrapolates: a point outside the raster — or inside a void — is an
//! error (the same principle as the geoid grid in `core::height::grid`).
//!
//! Both rasters of a site are opened together: the DTM (bare earth, the AGL
//! datum for S3 draping) and the DSM (surface incl. vegetation, the min-
//! clearance check of AGENTS.md / ADR-005).

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::height::Height;
use crate::plan::GeoPoint;

pub mod cog;
pub mod drape_compile;
pub mod drape_solver;
pub mod drape_spec;

pub use cog::CogDem;

/// The `dem.json` sidecar `tools/dem-prep` writes next to the rasters. It is
/// the version chain ADR-004 requires (mission archives reference the DEM
/// they were planned against) plus the placement the reader needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct DemMetadata {
    /// Content hash (sha256, first 16 hex chars) of the raster pair.
    pub version: String,
    /// CRS the rasters are gridded in (e.g. `"EPSG:26910"`).
    pub raster_crs: String,
    /// Nominal grid spacing, metres.
    pub resolution_m: f64,
    /// WGS84 coverage of the raster pair.
    pub bounds: DemBounds,
    /// Vertical datum of the pixel values (BC LiDAR: CGVD2013 orthometric).
    pub vertical_datum: VerticalDatum,
    /// Whether the pair carries a real DSM. Fallback sources (CDEM /
    /// Copernicus) are DTM-only: the drape then skips the clearance check.
    #[serde(default)]
    pub has_dsm: bool,
    /// Whether the pair carries a real DTM. Only HRDEM's northern
    /// DSM-only coverage sets this false: AGL is then really
    /// height-above-surface and the planning panel must say so (§6.5).
    #[serde(default = "default_has_dtm")]
    pub has_dtm: bool,
    /// What produced the raster pair — drives the auto-pick priority
    /// (LiDAR preferred for the low-altitude drape) and the report label.
    #[serde(default)]
    pub source: DemSource,
    /// Exact per-axis pixel angles from the writer, if known (H4). The
    /// reader prefers these over its mid-latitude conversion — the
    /// constant drifts (~0.06–0.17 %) which is visible at 1 m.
    #[serde(default)]
    pub dlat_deg: Option<f64>,
    #[serde(default)]
    pub dlon_deg: Option<f64>,
    /// The `hrdem-lidar` STAC item this pair was cut from (H1).
    #[serde(default)]
    pub stac_item_id: Option<String>,
    /// Project name of the HRDEM mosaic (per-project; no edgematching).
    #[serde(default)]
    pub project_name: Option<String>,
    /// Acquisition date (range) of the source survey.
    #[serde(default)]
    pub acquisition: Option<String>,
    /// CRS the source was delivered in before reprojection (HRDEM:
    /// `EPSG:3979` NAD83(CSRS); the WGS84 grid applies ADR-018 §D's null
    /// transform).
    #[serde(default)]
    pub original_crs: Option<String>,
    /// sha256 of the session's `provenance.json` (the full audit record:
    /// asset hrefs, fetch time, licence, tool versions).
    #[serde(default)]
    pub provenance_sha256: Option<String>,
    #[serde(default)]
    pub gdal_version: Option<String>,
    #[serde(default)]
    pub proj_version: Option<String>,
}

/// Old `dem.json` files predate `has_dtm` — they always carried a DTM.
#[must_use]
fn default_has_dtm() -> bool {
    true
}

/// Where a DEM tile came from (written by `tools/dem-prep`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DemSource {
    /// Airborne LiDAR (BC provincial product): ~1 m, cm-grade DTM + DSM.
    #[default]
    Lidar,
    /// GeoTIFF fallback (CDEM / Copernicus): ~30 m, DTM-only, no DSM.
    Geotiff,
    /// NRCan HRDEM (`hrdem-lidar`, plan H0–H4/ADR-018): 1 m DTM + DSM,
    /// CGVD2013, fetched per project over STAC + `/vsicurl/`.
    Hrdem,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct DemBounds {
    pub north: f64,
    pub south: f64,
    pub east: f64,
    pub west: f64,
}

/// Vertical datums `tools/dem-prep` emits. The BC LiDAR product is
/// orthometric CGVD2013 (`H`); the runtime converts to the datum a caller
/// needs through `core::height` (the `H + N(CGG2013) = h` chain, ADR-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum VerticalDatum {
    OrthometricCgvd2013,
    AmslEgm96,
}

/// Readiness of a "prepare survey area" request (U9/H9) for one AOI. Drives
/// the Area step card's colour and next-step text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SurveyPrepState {
    /// The cached session holds both rasters and valid metadata; ready to
    /// plan drape/terrain-following against it (offline included).
    Ready,
    /// A session folder exists but is incomplete (a raster or `dem.json`
    /// missing), so it is not usable yet.
    Partial,
    /// No cached session for this AOI — prepare it before flying.
    Absent,
}

/// U9: what the Area step's "prepare survey area" card shows. Assembled from
/// the cached session's `dem.json`; no network and no GDAL, so it works
/// offline (an absent-but-cached tile still reads as [`SurveyPrepState::Ready`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SurveyPrepStatus {
    pub state: SurveyPrepState,
    /// The cached session folder (`tools/dem-prep hrdem` output), when one
    /// exists.
    pub folder: Option<String>,
    /// HRDEM project/mosaic name the tile was cut from.
    pub project_name: Option<String>,
    /// Acquisition date (range) of the source survey.
    pub acquisition: Option<String>,
    pub resolution_m: Option<f64>,
    pub vertical_datum: Option<VerticalDatum>,
    pub has_dtm: bool,
    pub has_dsm: bool,
    /// WGS84 coverage of the raster pair.
    pub bounds: Option<DemBounds>,
    /// Total bytes of the raster pair (dtm.tif + dsm.tif), when present.
    pub bytes: Option<u64>,
    /// H7: the CGG2013/EGM96 chain is available to convert the tile's
    /// CGVD2013 heights to AMSL (blocks the drape when false).
    pub datum_verified: bool,
    /// Human-readable readiness reason or failure + next step, when there is
    /// something the operator must act on.
    pub message: Option<String>,
}

/// Errors from terrain sources (ADR-004: no extrapolation, no void fill).
#[derive(Debug, thiserror::Error)]
pub enum TerrainError {
    #[error("point outside the DEM coverage")]
    OutsideCoverage(GeoPoint),
    #[error("DEM void")]
    Void(GeoPoint),
    #[error("DEM metadata mismatch: {0}")]
    Metadata(String),
    #[error("DEM file error: {0}")]
    Io(String),
    #[error("GeoTIFF error: {0}")]
    Tiff(String),
}

/// ADR-004's elevation surface. Implemented by [`CogDem`]; a tile cache /
/// network source would implement the same trait later.
pub trait ElevationSource: Send + Sync {
    /// Elevation at the point, in the raster's own vertical datum (tagged).
    ///
    /// # Errors
    ///
    /// [`TerrainError::OutsideCoverage`] / [`TerrainError::Void`] rather
    /// than extrapolating or silently filling.
    fn elevation(&self, point: GeoPoint) -> Result<Height, TerrainError>;

    /// WGS84 coverage of the source.
    fn bounds(&self) -> DemBounds;

    /// The DEM version (the `dem.json` content hash) for mission archives.
    fn version(&self) -> &str;

    /// DTM and DSM at one point (both datum-tagged). The pair is what
    /// draping needs: the DTM sets the AGL datum, the DSM the clearance.
    ///
    /// # Errors
    ///
    /// Same contract as [`ElevationSource::elevation`].
    fn sample_pair(&self, point: GeoPoint) -> Result<(Height, Height), TerrainError>;
}

/// Validate a metadata/raster pair before sampling anything.
pub(crate) fn check_paths(dtm: &Path, dsm: &Path) -> Result<(), TerrainError> {
    for path in [dtm, dsm] {
        if !path.is_file() {
            return Err(TerrainError::Io(format!(
                "missing DEM raster {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// One tile in the folder-level `index.json` `tools/dem-prep` writes (N1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct TileIndexEntry {
    /// Tile output folder name, relative to the index folder.
    pub folder: String,
    pub bounds: DemBounds,
    /// `dem.json` version (sha256[..16]).
    pub version: String,
    /// Whether the tile pair carries a real DSM (fallback sources: DTM-only).
    #[serde(default)]
    pub has_dsm: bool,
    /// Data source — the auto-pick prefers LiDAR for the drape.
    #[serde(default)]
    pub source: DemSource,
}

/// Read the folder-level `index.json` a multi-tile dem-prep run writes (N1).
///
/// # Errors
///
/// [`TerrainError::Io`] when the file is missing, [`TerrainError::Metadata`]
/// when it is not a valid tile list.
pub fn load_tile_index(dir: &Path) -> Result<Vec<TileIndexEntry>, TerrainError> {
    let path = dir.join("index.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| TerrainError::Io(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| TerrainError::Metadata(format!("{}: {e}", path.display())))
}

/// Pick the tile whose bounds contain the centre of the query area (N1
/// auto-select), preferring a **LiDAR** tile over a GeoTIFF fallback when
/// both cover it (the low-altitude drape needs the cm-grade DTM + DSM).
/// Returns `None` when the area is outside every tile — the caller then
/// surfaces a hint.
pub fn pick_tile<'a>(tiles: &'a [TileIndexEntry], query: &DemBounds) -> Option<&'a TileIndexEntry> {
    let cx = (query.north + query.south) / 2.0;
    let cy = (query.east + query.west) / 2.0;
    let covers = |t: &'a TileIndexEntry| {
        t.bounds.north >= cx && cx >= t.bounds.south && t.bounds.east >= cy && cy >= t.bounds.west
    };
    tiles
        .iter()
        .find(|t| covers(t) && t.source == DemSource::Lidar)
        .or_else(|| tiles.iter().find(|t| covers(t)))
}

#[cfg(test)]
mod tile_index_tests {
    use super::*;

    fn tile(folder: &str, n: f64, s: f64, e: f64, w: f64) -> TileIndexEntry {
        tile_src(folder, n, s, e, w, DemSource::Lidar)
    }

    fn tile_src(folder: &str, n: f64, s: f64, e: f64, w: f64, source: DemSource) -> TileIndexEntry {
        TileIndexEntry {
            folder: folder.to_string(),
            bounds: DemBounds {
                north: n,
                south: s,
                east: e,
                west: w,
            },
            version: "v1".to_string(),
            has_dsm: true,
            source,
        }
    }

    #[test]
    fn picks_covering_tile() {
        let tiles = vec![
            tile("a", 48.7, 48.6, -123.3, -123.5),
            tile("b", 48.6, 48.5, -123.3, -123.5),
        ];
        let q = DemBounds {
            north: 48.55,
            south: 48.52,
            east: -123.4,
            west: -123.45,
        };
        assert_eq!(pick_tile(&tiles, &q).unwrap().folder, "b");
    }

    #[test]
    fn lidar_wins_over_geotiff_for_the_same_area() {
        // Two tiles covering the same query centre: the LiDAR one must win
        // even though the GeoTIFF tile sorts first.
        let tiles = vec![
            tile_src("geotiff", 48.7, 48.6, -123.3, -123.5, DemSource::Geotiff),
            tile_src("lidar", 48.7, 48.6, -123.3, -123.5, DemSource::Lidar),
        ];
        let q = DemBounds {
            north: 48.65,
            south: 48.63,
            east: -123.4,
            west: -123.43,
        };
        assert_eq!(pick_tile(&tiles, &q).unwrap().folder, "lidar");
    }

    #[test]
    fn geotiff_is_the_fallback_when_only_it_covers() {
        let tiles = vec![tile_src(
            "geotiff",
            48.7,
            48.6,
            -123.3,
            -123.5,
            DemSource::Geotiff,
        )];
        let q = DemBounds {
            north: 48.65,
            south: 48.63,
            east: -123.4,
            west: -123.43,
        };
        assert_eq!(pick_tile(&tiles, &q).unwrap().folder, "geotiff");
    }

    #[test]
    fn missing_source_defaults_to_lidar() {
        // Old dem.json / index.json entries have no `source` field: serde
        // must default them to LiDAR (the existing BC data is LiDAR).
        let raw = r#"{"folder":"s","bounds":{"north":1.0,"south":0.0,"east":1.0,"west":0.0},"version":"v","has_dsm":true}"#;
        let entry: TileIndexEntry = serde_json::from_str(raw).expect("parse");
        assert_eq!(entry.source, DemSource::Lidar);

        let meta = r#"{"version":"v","raster_crs":"EPSG:4326","resolution_m":1.0,"bounds":{"north":1.0,"south":0.0,"east":1.0,"west":0.0},"vertical_datum":"orthometric_cgvd2013","has_dsm":true}"#;
        let m: DemMetadata = serde_json::from_str(meta).expect("parse");
        assert_eq!(m.source, DemSource::Lidar);
        // H4 backward compatibility: old files predate has_dtm and the
        // exact-angle/provenance fields — they must default, not fail.
        assert!(m.has_dtm, "old tiles always carried a DTM");
        assert_eq!(m.dlat_deg, None);
        assert_eq!(m.dlon_deg, None);
        assert_eq!(m.stac_item_id, None);
        assert_eq!(m.provenance_sha256, None);
    }

    #[test]
    fn hrdem_dem_json_round_trips_with_exact_angles() {
        // The shape dem-prep writes for an HRDEM session (H1–H4): every H4
        // field present, the new source tag, and a has_dtm=false case for
        // northern DSM-only coverage.
        let m = DemMetadata {
            version: "abc123".into(),
            raster_crs: "EPSG:4326".into(),
            resolution_m: 1.0,
            bounds: DemBounds {
                north: 48.5569,
                south: 48.5552,
                east: -123.3804,
                west: -123.3826,
            },
            vertical_datum: VerticalDatum::OrthometricCgvd2013,
            has_dsm: true,
            has_dtm: true,
            source: DemSource::Hrdem,
            dlat_deg: Some(8.998_231_074e-6),
            dlon_deg: Some(1.359_481_608_8e-5),
            stac_item_id: Some("BC-Vancouver_Island_Sunshine_Coast_2018-1m".into()),
            project_name: Some("BC Vancouver Island Sunshine Coast 2018, 1 m".into()),
            acquisition: Some("2019-10-01T12:00:00Z".into()),
            original_crs: Some("EPSG:3979 (NAD83(CSRS) / Canada Atlas Lambert)".into()),
            provenance_sha256: Some("deadbeef".into()),
            gdal_version: Some("GDAL 3.10.3".into()),
            proj_version: Some("Rel. 9.6.2".into()),
        };
        let json = serde_json::to_string(&m).expect("serialize");
        let back: DemMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, m);
        assert!(json.contains(r#""source":"hrdem""#));
    }

    #[test]
    fn dem_json_source_hrdem_tag_parses() {
        let raw = r#"{"version":"v","raster_crs":"EPSG:4326","resolution_m":1.0,
            "bounds":{"north":1.0,"south":0.0,"east":1.0,"west":0.0},
            "vertical_datum":"orthometric_cgvd2013","has_dsm":true,
            "source":"hrdem","has_dtm":false}"#;
        let m: DemMetadata = serde_json::from_str(raw).expect("parse");
        assert_eq!(m.source, DemSource::Hrdem);
        assert!(!m.has_dtm, "DSM-only HRDEM coverage must be readable");
    }

    #[test]
    fn outside_all_tiles_is_none() {
        let tiles = vec![tile("a", 48.7, 48.6, -123.3, -123.5)];
        let q = DemBounds {
            north: 50.1,
            south: 50.0,
            east: -120.0,
            west: -120.1,
        };
        assert!(pick_tile(&tiles, &q).is_none());
    }

    #[test]
    fn empty_index_is_none() {
        let q = DemBounds {
            north: 48.6,
            south: 48.5,
            east: -123.4,
            west: -123.5,
        };
        assert!(pick_tile(&[], &q).is_none());
    }

    #[test]
    fn loads_written_index() {
        let dir = std::env::temp_dir().join(format!("maggcs-n1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tiles = vec![tile("sidney", 48.7, 48.6, -123.2, -123.5)];
        let path = dir.join("index.json");
        std::fs::write(&path, serde_json::to_string_pretty(&tiles).unwrap()).unwrap();
        let loaded = load_tile_index(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].folder, "sidney");
        assert_eq!(loaded[0].version, "v1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_index_is_io_error() {
        let dir = std::env::temp_dir().join("maggcs-n1-missing");
        let err = load_tile_index(&dir).unwrap_err();
        assert!(matches!(err, TerrainError::Io(_)));
    }
}
