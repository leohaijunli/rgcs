//! HRDEM (CanElevation) per-project fetch: STAC query plus a `/vsicurl/`
//! window crop of the native EPSG:3979 COG pair (plan §6 H1, ADR-018).
//!
//! The AOI is the survey rectangle plus a configurable margin on every side
//! ([`DEFAULT_BUFFER_M`]); the **total** window is capped at [`AOI_LIMIT_M`]
//! (decision 3, 2026-10-18 revision: download a band wider than the survey so
//! the drape never runs off the DEM edge). The STAC query must resolve to
//! exactly one project —
//! an AOI crossing project boundaries is refused, never stitched (the
//! collection has no edgematching between projects; decision 2).
//!
//! Output is the **native-CRS window** (no reprojection, no resampling —
//! those are H2/H3, executed by `gdalwarp` with its own policy) plus a
//! provenance JSON recording the item, asset hrefs, fetch time, license,
//! GDAL/PROJ versions and output hashes. Repeat runs with the same request
//! hit the on-disk cache.
//!
//! GDAL is invoked as a subprocess (`gdal_translate`/`gdaltransform`), never
//! linked, so building dem-prep needs no libgdal headers (ADR-018 addendum).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// STAC search endpoint of the NRCan datacube (ADR-018).
const STAC_SEARCH_URL: &str = "https://datacube.services.geo.ca/stac/api/search";
/// The per-project HRDEM collection (not the 1 m/2 m national mosaics).
const HRDEM_COLLECTION: &str = "hrdem-lidar";
/// Hard cap for the **total** window side (core survey rectangle plus the
/// buffer on both sides), metres. Decision 3 revised 2026-10-18: the fetched
/// band is deliberately larger than the survey so a plan that grows to the
/// area edge (or a little past) still has DEM under every waypoint. 2 km of
/// 1 m HRDEM is ~16 MB per band; the COG window crop (H1) range-reads only
/// this window, so the 71 GB project file is never downloaded whole.
pub const AOI_LIMIT_M: f64 = 2000.0;
/// Default margin added on every side of the survey rectangle (drape margins,
/// sensor footprint, plan slop), metres.
pub const DEFAULT_BUFFER_M: f64 = 250.0;
/// Default core survey rectangle side when the caller names no size (the UI
/// always passes an explicit width/height), metres.
pub const DEFAULT_SURVEY_M: f64 = 100.0;
/// OGL – Canada 2.0 covers the CanElevation series (ADR-018).
const LICENSE_NAME: &str = "Open Government Licence - Canada 2.0";
const LICENSE_URL: &str = "https://open.canada.ca/en/open-government-licence-canada";
/// Request cap for the STAC search; Victoria-area AOIs resolve to one item.
const STAC_PAGE_LIMIT: usize = 10;
/// ADR-018 §D: NAD83(CSRS) ≡ WGS 84 horizontally (null transformation,
/// EPSG:1842 semantics). PROJ's *default* pick for EPSG:3979→EPSG:4326 is the
/// seven-parameter EPSG:1946, which §D explicitly rejects — so every
/// transform in this module runs between two proj strings that share the
/// GRS80 ellipsoid and carry no datum identity, making a datum step
/// structurally impossible. Each provenance records what PROJ would have
/// picked by default (`proj_default_operation`), unused.
const NULL_GEOGRAPHIC_SRS: &str = "+proj=longlat +ellps=GRS80 +no_defs";
const NULL_LAMBERT_3979_SRS: &str = "+proj=lcc +lat_1=49 +lat_2=77 +lat_0=49 \
                                     +lon_0=-95 +x_0=0 +y_0=0 +ellps=GRS80 +units=m +no_defs";
/// Output target SRS label: WGS 84, per §D applied as the null transform.
const TARGET_WGS84_SRS: &str = "EPSG:4326";
/// Resampling per product (H3, *verify* 17 measured 2026-10-17): the DTM
/// (smooth ground) uses `bilinear`; the DSM uses footprint-**max** so a
/// sharp surface peak is never averaged away (measured: bilinear turned a
/// single-cell 25.0 m spike into 22.74 m, while `-r max` reproduced 25.0
/// exactly, kept nodata cells untouched and invented no values — so the
/// plan's "write custom post-processing if needed" fallback is not needed).
const DTM_RESAMPLING: &str = "bilinear";
const DSM_RESAMPLING: &str = "max";
/// Nodata is explicit on both sides of the warp: source cells at −32767
/// never contribute (no fake peaks, no bilinear contamination), and cells
/// with no valid source coverage stay −32767 — never filled (§5B rule 7).
const HRDEM_NODATA: &str = "-32767";
/// Default WGS84 grid resolution in metres (decision 3: keep 1 m, no
/// downsampling of the 1 m source).
pub const DEFAULT_RESOLUTION_M: f64 = 1.0;

/// Everything that can stop an HRDEM fetch. Every variant is actionable from
/// the CLI: the message names the refused request or the missing tool.
#[derive(Debug)]
pub enum HrdemError {
    /// The buffered AOI exceeds [`AOI_LIMIT_M`].
    AoiTooLarge {
        width_m: f64,
        height_m: f64,
        limit_m: f64,
    },
    /// No `hrdem-lidar` item intersects the buffered AOI.
    NoCoverage,
    /// The AOI intersects more than one project; stitching is not allowed
    /// (decision 2). `ids` lists the conflicting items to choose from.
    MultipleProjects { ids: Vec<String> },
    /// Network/HTTP failure (curl spawn or non-2xx exit).
    Offline(String),
    /// The STAC response could not be parsed.
    Stac(String),
    /// An item is missing a usable DTM or DSM GeoTIFF asset.
    Asset {
        item_id: String,
        missing: &'static str,
    },
    /// A required GDAL/PROJ tool was not found on `PATH` or in an
    /// autodetected prefix (see [`crate::gdal`]).
    GdalMissing { tool: &'static str },
    /// A GDAL/PROJ invocation failed.
    Gdal { tool: String, detail: String },
    /// Local filesystem failure.
    Io(String),
}

impl std::fmt::Display for HrdemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AoiTooLarge {
                width_m,
                height_m,
                limit_m,
            } => write!(
                f,
                "survey window {width_m} m x {height_m} m (plus buffer on every side) exceeds \
                 the {limit_m} m window limit (plan decision 3); shrink the survey area"
            ),
            Self::NoCoverage => {
                write!(f, "no hrdem-lidar project covers the requested AOI")
            }
            Self::MultipleProjects { ids } => write!(
                f,
                "the AOI crosses project boundaries ({}); choose a single project \
                 by shrinking the AOI — cross-project stitching is refused: {}",
                ids.len(),
                ids.join(", ")
            ),
            Self::Offline(m) => write!(f, "offline or STAC unreachable: {m}"),
            Self::Stac(m) => write!(f, "malformed STAC response: {m}"),
            Self::Asset { item_id, missing } => write!(
                f,
                "STAC item {item_id} has no usable {missing} GeoTIFF asset"
            ),
            Self::GdalMissing { tool } => write!(
                f,
                "{tool} not found — dem-prep shells out to the GDAL/PROJ \
                 command-line tools (ADR-018). Install GDAL, or point {} at a \
                 GDAL prefix (the directory holding bin/ and share/proj)",
                crate::gdal::PREFIX_ENV
            ),
            Self::Gdal { tool, detail } => write!(f, "{tool} failed: {detail}"),
            Self::Io(m) => write!(f, "filesystem error: {m}"),
        }
    }
}

impl std::error::Error for HrdemError {}

/// A lat/lon box in degrees (WGS84), west/east lon, south/north lat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AoiBounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// One resolved `hrdem-lidar` STAC item with the two raster assets we need.
#[derive(Debug, Clone, PartialEq)]
pub struct HrdemItem {
    pub id: String,
    pub acquisition_datetime: Option<String>,
    pub dtm_href: String,
    pub dsm_href: String,
}

/// The fetch request. `width_m`/`height_m` are the core survey area; the
/// actual window adds `buffer_m` on every side. `resolution_m` is the target
/// WGS84 grid resolution (decision 3 keeps the 1 m source resolution).
#[derive(Debug, Clone, PartialEq)]
pub struct HrdemRequest {
    pub center_lat: f64,
    pub center_lon: f64,
    pub width_m: f64,
    pub height_m: f64,
    pub buffer_m: f64,
    pub resolution_m: f64,
    pub out: PathBuf,
}

/// Metres per degree latitude used for the AOI envelope; the crop itself is
/// computed by `gdaltransform`, so this only needs envelope accuracy.
const METERS_PER_DEGREE: f64 = 111_132.954;

impl HrdemRequest {
    /// Validate the AOI against the hard cap and build the request.
    ///
    /// # Errors
    ///
    /// [`HrdemError::AoiTooLarge`] when the buffered window exceeds
    /// [`AOI_LIMIT_M`].
    pub fn new(
        center_lat: f64,
        center_lon: f64,
        width_m: f64,
        height_m: f64,
        buffer_m: f64,
        out: impl Into<PathBuf>,
    ) -> Result<Self, HrdemError> {
        if !(center_lat).is_finite()
            || !(center_lon).is_finite()
            || !(width_m).is_finite()
            || !(height_m).is_finite()
            || !(buffer_m).is_finite()
            || width_m <= 0.0
            || height_m <= 0.0
            || buffer_m < 0.0
        {
            return Err(HrdemError::Stac(
                "AOI values must be finite numbers".to_string(),
            ));
        }
        if width_m + 2.0 * buffer_m > AOI_LIMIT_M || height_m + 2.0 * buffer_m > AOI_LIMIT_M {
            return Err(HrdemError::AoiTooLarge {
                width_m,
                height_m,
                limit_m: AOI_LIMIT_M,
            });
        }
        Ok(Self {
            center_lat,
            center_lon,
            width_m,
            height_m,
            buffer_m,
            resolution_m: DEFAULT_RESOLUTION_M,
            out: out.into(),
        })
    }

    /// Set the WGS84 output grid resolution (default [`DEFAULT_RESOLUTION_M`]).
    ///
    /// # Errors
    ///
    /// [`HrdemError::Stac`] for non-finite or non-positive values.
    pub fn resolution(mut self, resolution_m: f64) -> Result<Self, HrdemError> {
        if !resolution_m.is_finite() || resolution_m <= 0.0 {
            return Err(HrdemError::Stac(
                "resolution must be a finite positive number".to_string(),
            ));
        }
        self.resolution_m = resolution_m;
        Ok(self)
    }

    /// The buffered AOI envelope in degrees.
    #[must_use]
    pub fn aoi_bounds(&self) -> AoiBounds {
        let half_ew_m = self.width_m / 2.0 + self.buffer_m;
        let half_ns_m = self.height_m / 2.0 + self.buffer_m;
        let dlat = half_ns_m / METERS_PER_DEGREE;
        let dlon = half_ew_m / (METERS_PER_DEGREE * self.center_lat.to_radians().cos());
        AoiBounds {
            west: self.center_lon - dlon,
            east: self.center_lon + dlon,
            south: self.center_lat - dlat,
            north: self.center_lat + dlat,
        }
    }

    /// Stable hash of the normalized request: the cache key and the value
    /// recorded in the provenance JSON. Changes to any field change the hash.
    #[must_use]
    pub fn spec_hash(&self) -> String {
        let mut hasher = Sha256::new();
        let normalized = format!(
            "hrdem-v1|{:.7}|{:.7}|{:.3}|{:.3}|{:.3}|{:.3}",
            self.center_lat,
            self.center_lon,
            self.width_m,
            self.height_m,
            self.buffer_m,
            self.resolution_m
        );
        hasher.update(normalized.as_bytes());
        format!("{:016x}", hasher.finalize())
    }

    /// The per-AOI output directory under `out`, named after the request so a
    /// repeat run finds the cache.
    #[must_use]
    pub fn session_dir(&self) -> PathBuf {
        self.out.join(format!(
            "{:.6}_{:.6}_{}x{}",
            self.center_lat, self.center_lon, self.width_m, self.height_m
        ))
    }

    /// Pre-warp estimate of the WGS84 grid size (px), ±1 for `-tap`
    /// snapping. Upper bound at the 45°-rotation worst case: the native
    /// window is axis-aligned in EPSG:3979 but tilted in lon/lat (Lambert
    /// grid convergence, ~25° at the Victoria AOI), so the output covers
    /// the rotated rectangle's bounding box, not the core area itself.
    #[must_use]
    pub fn estimated_wgs84_dims(&self) -> (u32, u32) {
        let half_x = self.width_m / 2.0 + self.buffer_m;
        let half_y = self.height_m / 2.0 + self.buffer_m;
        let span_m = (half_x + half_y) * std::f64::consts::SQRT_2;
        let px = (span_m / self.resolution_m).ceil() as u32;
        (px, px)
    }
}

/// A cached file entry in the provenance JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HrdemFile {
    pub path: String,
    pub sha256: String,
}

/// Provenance written next to the rasters (H1 fetch + H2 reprojection; H4
/// extends the core `dem.json` metadata). Everything needed to re-resolve
/// the source and to audit the fetch is recorded here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HrdemProvenance {
    pub schema_version: u32,
    pub source: String,
    pub stac_api: String,
    pub collection: String,
    pub item_id: String,
    pub acquisition_datetime: Option<String>,
    pub dtm_href: String,
    pub dsm_href: String,
    pub fetched_utc: String,
    pub license: String,
    pub license_url: String,
    pub horizontal_crs: String,
    pub vertical_datum: String,
    /// ADR-018 §D: NAD83(CSRS) ↔ WGS84 uses the null transform; the ~1 m
    /// horizontal residual is accepted and recorded, not silently applied.
    pub datum_note: String,
    /// The datum behaviour actually used (proj strings sharing GRS80, so no
    /// datum step can occur). Recorded for audit.
    pub datum_pipeline: String,
    /// What PROJ would pick by default for EPSG:3979→EPSG:4326 (recorded,
    /// NOT used — typically the seven-parameter EPSG:1946 that §D rejects).
    pub proj_default_operation: String,
    pub aoi_center_lat: f64,
    pub aoi_center_lon: f64,
    pub aoi_core_m: [f64; 2],
    pub aoi_buffer_m: f64,
    /// Target WGS84 grid resolution in metres (decision 3: keep 1 m).
    pub resolution_m: f64,
    /// Resampling actually applied (H2 placeholder `near`; H3 owns the
    /// policy: DTM bilinear, DSM footprint-max).
    pub resampling: String,
    /// WGS84 bounds of the reprojected pair: `[west, south, east, north]`.
    pub wgs84_bounds: [f64; 4],
    pub spec_hash: String,
    pub gdal_version: String,
    pub proj_version: String,
    /// The reprojected WGS84 pair the ground station loads.
    pub dtm: HrdemFile,
    pub dsm: HrdemFile,
    /// The native EPSG:3979 window crops (reprojection inputs).
    pub native_dtm: HrdemFile,
    pub native_dsm: HrdemFile,
}

impl HrdemProvenance {
    pub const SCHEMA_VERSION: u32 = 1;
}

/// Query the STAC API for `hrdem-lidar` items intersecting the AOI.
///
/// # Errors
///
/// [`HrdemError::Offline`] on network failure, [`HrdemError::Stac`] on a
/// malformed response, [`HrdemError::Asset`] when an item lacks the DTM/DSM
/// rasters.
pub fn query_stac(aoi: &AoiBounds) -> Result<Vec<HrdemItem>, HrdemError> {
    let body = serde_json::json!({
        "collections": [HRDEM_COLLECTION],
        "intersects": {
            "type": "Polygon",
            "coordinates": [[
                [aoi.west, aoi.south],
                [aoi.east, aoi.south],
                [aoi.east, aoi.north],
                [aoi.west, aoi.north],
                [aoi.west, aoi.south],
            ]],
        },
        "limit": STAC_PAGE_LIMIT,
    })
    .to_string();
    let output = run_curl_post_json(STAC_SEARCH_URL, &body)?;
    parse_stac_features(&output)
}

/// Run `curl` to POST `body` and capture stdout (the existing fetch.rs
/// convention: dem-prep shells out rather than linking an HTTP stack).
fn run_curl_post_json(url: &str, body: &str) -> Result<String, HrdemError> {
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "60",
            "--header",
            "Content-Type: application/json",
            "--data",
        ])
        .arg(body)
        .arg(url)
        .output()
        .map_err(|e| HrdemError::Offline(format!("could not spawn curl: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(HrdemError::Offline(if stderr.is_empty() {
            format!("HTTP request failed (curl exit {})", output.status)
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Minimal serde shapes for the STAC search response.
#[derive(Deserialize)]
struct StacSearch {
    #[serde(default)]
    features: Vec<StacFeature>,
}

#[derive(Deserialize)]
struct StacFeature {
    id: String,
    #[serde(default)]
    properties: StacProperties,
    #[serde(default)]
    assets: std::collections::BTreeMap<String, StacAsset>,
}

#[derive(Deserialize, Default)]
struct StacProperties {
    #[serde(default)]
    datetime: Option<String>,
}

#[derive(Deserialize)]
struct StacAsset {
    href: String,
}

/// Parse the STAC search response into items with resolved DTM/DSM assets.
/// Duplicate ids (relisted pages) collapse; asset resolution requires the
/// `dtm`/`dsm` keys (the documented layout) or, as a fallback, a unique
/// `.tif` asset whose href ends in `-dtm.tif`/`-dsm.tif`.
fn parse_stac_features(raw: &str) -> Result<Vec<HrdemItem>, HrdemError> {
    let parsed: StacSearch = serde_json::from_str(raw)
        .map_err(|e| HrdemError::Stac(format!("not a STAC search response: {e}")))?;
    let mut items: Vec<HrdemItem> = Vec::new();
    for feature in parsed.features {
        if items.iter().any(|item: &HrdemItem| item.id == feature.id) {
            continue;
        }
        let dtm_href =
            resolve_asset_href(&feature, "dtm", "-dtm.tif").ok_or(HrdemError::Asset {
                item_id: feature.id.clone(),
                missing: "DTM",
            })?;
        let dsm_href =
            resolve_asset_href(&feature, "dsm", "-dsm.tif").ok_or(HrdemError::Asset {
                item_id: feature.id.clone(),
                missing: "DSM",
            })?;
        items.push(HrdemItem {
            id: feature.id,
            acquisition_datetime: feature.properties.datetime,
            dtm_href,
            dsm_href,
        });
    }
    Ok(items)
}

/// Asset href by key, with a suffix fallback over the remaining `.tif`
/// assets (accepts exactly one match — ambiguity is an error, not a guess).
fn resolve_asset_href(feature: &StacFeature, key: &str, suffix: &str) -> Option<String> {
    if let Some(asset) = feature.assets.get(key) {
        return Some(asset.href.clone());
    }
    let matches: Vec<&StacAsset> = feature
        .assets
        .values()
        .filter(|asset| asset.href.ends_with(suffix) && asset.href.ends_with(".tif"))
        .collect();
    match matches.as_slice() {
        [asset] => Some(asset.href.clone()),
        _ => None,
    }
}

/// Require exactly one covering project (decision 2: no stitching).
///
/// # Errors
///
/// [`HrdemError::NoCoverage`] when nothing intersects,
/// [`HrdemError::MultipleProjects`] when more than one project does.
pub fn select_single_item(items: Vec<HrdemItem>) -> Result<HrdemItem, HrdemError> {
    match items.len() {
        0 => Err(HrdemError::NoCoverage),
        1 => Ok(items.into_iter().next().expect("len checked")),
        _ => Err(HrdemError::MultipleProjects {
            ids: items.into_iter().map(|item| item.id).collect(),
        }),
    }
}

/// Run `gdaltransform` over N points with explicit SRS strings, returning the
/// transformed `(x, y)` pairs. Used with the null-transform strings only
/// (ADR-018 §D): identical ellipsoid on both sides → pure projection change.
fn gdaltransform_points(
    s_srs: &str,
    t_srs: &str,
    points: &[(f64, f64)],
) -> Result<Vec<(f64, f64)>, HrdemError> {
    check_tool("gdaltransform")?;
    let mut child = Command::new("gdaltransform")
        .args(["-s_srs", s_srs, "-t_srs", t_srs])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| HrdemError::Gdal {
            tool: "gdaltransform".to_string(),
            detail: format!("spawn failed: {e}"),
        })?;
    let mut stdin = child.stdin.take().expect("stdin piped");
    for (x, y) in points {
        writeln!(stdin, "{x} {y}").map_err(|e| HrdemError::Gdal {
            tool: "gdaltransform".to_string(),
            detail: format!("could not feed the points: {e}"),
        })?;
    }
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| HrdemError::Gdal {
        tool: "gdaltransform".to_string(),
        detail: format!("wait failed: {e}"),
    })?;
    if !output.status.success() {
        return Err(HrdemError::Gdal {
            tool: "gdaltransform".to_string(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(points.len());
    lines
        .map(|line| parse_xy_line(Some(line), "point"))
        .collect::<Result<Vec<_>, _>>()
}

/// The AOI centre (WGS84 lon/lat, treated as NAD83(CSRS) per §D) in EPSG:3979
/// map metres, computed with the null transform.
///
/// The window itself is the **axis-aligned rectangle** around that centre —
/// half core size plus buffer per axis — not the envelope of the four
/// projected corners: this far west of the central meridian the Lambert
/// grid convergence is ~25°, so the geographic box's projected envelope
/// would be a tilted quadrilateral whose bounding rectangle is far larger
/// and covers the wrong shape. An axis-aligned window also matches the
/// H0 fixture semantics (ADR-018: a 140 × 140 window for a 100 m core plus
/// a 20 m buffer).
fn transform_center_3979(lat: f64, lon: f64) -> Result<(f64, f64), HrdemError> {
    let mut out = gdaltransform_points(NULL_GEOGRAPHIC_SRS, NULL_LAMBERT_3979_SRS, &[(lon, lat)])?;
    Ok(out.pop().expect("one point in, one point out"))
}

/// Parse one `gdaltransform` output line into `(x, y)`.
fn parse_xy_line(line: Option<&str>, which: &str) -> Result<(f64, f64), HrdemError> {
    let line = line.ok_or_else(|| HrdemError::Gdal {
        tool: "gdaltransform".to_string(),
        detail: format!("missing the {which} in the output"),
    })?;
    let mut fields = line.split_whitespace().map(|f| f.parse::<f64>());
    match (fields.next(), fields.next()) {
        (Some(Ok(x)), Some(Ok(y))) => Ok((x, y)),
        _ => Err(HrdemError::Gdal {
            tool: "gdaltransform".to_string(),
            detail: format!("unexpected output line for the {which}: {line}"),
        }),
    }
}

/// WGS84 pixel sizes (degrees) for a metric resolution at a latitude:
/// `(dlon, dlat)`. Spherical approximation of the metre-per-degree scale;
/// the residual is far below the ADR-018 datum residual (~1 m).
#[must_use]
pub fn wgs84_pixel_sizes(resolution_m: f64, lat: f64) -> (f64, f64) {
    (
        resolution_m / (METERS_PER_DEGREE * lat.to_radians().cos()),
        resolution_m / METERS_PER_DEGREE,
    )
}

/// The WGS84 bounds `[west, south, east, north]` of the axis-aligned
/// EPSG:3979 window, computed with the null transform (four corners, one
/// `gdaltransform` call).
fn window_bounds_wgs84(cx: f64, cy: f64, half_x: f64, half_y: f64) -> Result<[f64; 4], HrdemError> {
    let corners = [
        (cx - half_x, cy + half_y),
        (cx + half_x, cy + half_y),
        (cx + half_x, cy - half_y),
        (cx - half_x, cy - half_y),
    ];
    let out = gdaltransform_points(NULL_LAMBERT_3979_SRS, NULL_GEOGRAPHIC_SRS, &corners)?;
    let lons: Vec<f64> = out.iter().map(|(x, _)| *x).collect();
    let lats: Vec<f64> = out.iter().map(|(_, y)| *y).collect();
    Ok([
        lons.iter().cloned().fold(f64::MAX, f64::min),
        lats.iter().cloned().fold(f64::MAX, f64::min),
        lons.iter().cloned().fold(f64::MIN, f64::max),
        lats.iter().cloned().fold(f64::MIN, f64::max),
    ])
}

/// Record what PROJ would pick **by default** for EPSG:3979 → EPSG:4326
/// near this AOI (audit only — the pipeline is NOT used; §D rejects the
/// default, typically the seven-parameter EPSG:1946).
fn probe_proj_default_operation(aoi: &AoiBounds) -> String {
    let bbox = format!(
        "{:.5},{:.5},{:.5},{:.5}",
        aoi.west, aoi.south, aoi.east, aoi.north
    );
    match Command::new("projinfo")
        .args([
            "-s",
            "EPSG:3979",
            "-t",
            "EPSG:4326",
            "--bbox",
            &bbox,
            "-o",
            "PROJ",
        ])
        .output()
    {
        Ok(output) => parse_projinfo_default(&String::from_utf8_lossy(&output.stdout)),
        Err(_) => "unknown (projinfo not available)".to_string(),
    }
}

/// Extract the summary line of "Operation No. 1" from a `projinfo -o PROJ`
/// response (pure; unit-tested against captured real output).
fn parse_projinfo_default(raw: &str) -> String {
    match raw.split("Operation No. 1:").nth(1) {
        Some(rest) => rest
            .split("PROJ string:")
            .next()
            .unwrap_or(rest)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "unknown (no candidate operation listed)".to_string()),
        None => {
            if raw.trim().is_empty() {
                "unknown (projinfo returned nothing)".to_string()
            } else {
                "unknown (unrecognized projinfo output)".to_string()
            }
        }
    }
}

/// Return `dir` when it holds a complete cached tileset for `spec_hash`:
/// provenance with a matching hash plus the WGS84 pair and the native
/// window. Anything else — missing files, corrupt provenance, a stale hash —
/// is a cache miss so `fetch_tileset` refetches rather than serving wrong
/// data.
fn cache_lookup(dir: &Path, spec_hash: &str) -> Option<PathBuf> {
    let provenance_path = dir.join("provenance.json");
    if !(provenance_path.is_file()
        && dir.join("dtm.tif").is_file()
        && dir.join("dsm.tif").is_file()
        && dir.join("dem.json").is_file()
        && dir.join("native/dtm-3979.tif").is_file()
        && dir.join("native/dsm-3979.tif").is_file())
    {
        return None;
    }
    let existing = std::fs::read_to_string(provenance_path).ok()?;
    let cached: HrdemProvenance = serde_json::from_str(&existing).ok()?;
    (cached.spec_hash == spec_hash).then(|| dir.to_path_buf())
}

fn check_tool(tool: &'static str) -> Result<(), HrdemError> {
    let missing = Command::new(tool)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .err();
    // A tool that exists but rejects --version still spawns successfully.
    if missing.is_some() {
        return Err(HrdemError::GdalMissing { tool });
    }
    Ok(())
}

/// `gdalinfo --version` → e.g. "GDAL 3.13.3, released 2025-…".
fn gdal_version() -> String {
    tool_version_line("gdalinfo").unwrap_or_else(|| "unknown".to_string())
}

/// PROJ version: `proj --version` prints its release line (on stderr for
/// PROJ 9); `projinfo` is the fallback.
fn proj_version() -> String {
    tool_version_line("proj")
        .or_else(|| tool_version_line("projinfo"))
        .unwrap_or_else(|| "unknown".to_string())
}

/// First non-empty line of the tool's `--version` output, stdout first then
/// stderr — PROJ 9 binaries print their release line on stderr.
fn tool_version_line(tool: &str) -> Option<String> {
    let output = Command::new(tool).arg("--version").output().ok()?;
    for stream in [&output.stdout, &output.stderr] {
        if let Some(line) = String::from_utf8_lossy(stream)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
        {
            return Some(line.to_string());
        }
    }
    None
}

fn sha256_file(path: &Path) -> Result<String, HrdemError> {
    let bytes =
        std::fs::read(path).map_err(|e| HrdemError::Io(format!("read {}: {e}", path.display())))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Fetch (or reuse from cache) the HRDEM tileset for `req`: the native
/// EPSG:3979 window pair plus the reprojected WGS84 pair the ground station
/// loads.
///
/// Returns the session directory holding `dtm.tif`/`dsm.tif` (WGS84),
/// `native/dtm-3979.tif`/`native/dsm-3979.tif` and `provenance.json`. A
/// cached directory whose provenance `spec_hash` matches is reused without
/// network or GDAL access.
///
/// # Errors
///
/// See [`HrdemError`]: every refusal (oversized AOI, no coverage,
/// cross-project, offline, missing GDAL, non-null datum behaviour) leaves
/// no partial output behind.
pub fn fetch_tileset(req: &HrdemRequest) -> Result<PathBuf, HrdemError> {
    let dir = req.session_dir();
    if let Some(cached) = cache_lookup(&dir, &req.spec_hash()) {
        return Ok(cached);
    }

    let item = select_single_item(query_stac(&req.aoi_bounds())?)?;
    // Axis-aligned window around the projected centre (see
    // `transform_center_3979` for why not the corner envelope).
    let (cx, cy) = transform_center_3979(req.center_lat, req.center_lon)?;
    let half_x = req.width_m / 2.0 + req.buffer_m;
    let half_y = req.height_m / 2.0 + req.buffer_m;
    check_tool("gdal_translate")?;
    check_tool("gdalwarp")?;

    // Stage into a temp sibling, then move: a failure must leave no
    // half-written tileset that a later run could mistake for a cache hit.
    let staging = dir.with_extension("tmp-hrdem");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(staging.join("native"))
        .map_err(|e| HrdemError::Io(format!("mkdir {}: {e}", staging.display())))?;
    let native = stage_window_crop(
        &staging,
        &item,
        cx - half_x,
        cy + half_y,
        cx + half_x,
        cy - half_y,
    )?;
    let bounds = window_bounds_wgs84(cx, cy, half_x, half_y)?;
    let wgs84 = reproject_to_wgs84(&staging, req, &native, bounds)?;

    let provenance = HrdemProvenance {
        schema_version: HrdemProvenance::SCHEMA_VERSION,
        source: "hrdem-lidar".to_string(),
        stac_api: STAC_SEARCH_URL.to_string(),
        collection: HRDEM_COLLECTION.to_string(),
        item_id: item.id.clone(),
        acquisition_datetime: item.acquisition_datetime.clone(),
        dtm_href: item.dtm_href.clone(),
        dsm_href: item.dsm_href.clone(),
        fetched_utc: now_utc(),
        license: LICENSE_NAME.to_string(),
        license_url: LICENSE_URL.to_string(),
        horizontal_crs: format!("{TARGET_WGS84_SRS} (WGS84, per ADR-018 §D null transform)"),
        vertical_datum: "CGVD2013 (EPSG:6647), orthometric, metres".to_string(),
        datum_note: "NAD83(CSRS) to WGS84 via the null transform per ADR-018 §D \
                     (~1 m horizontal residual, accepted and recorded)"
            .to_string(),
        datum_pipeline: format!(
            "null (proj strings sharing GRS80: {} -> {})",
            NULL_LAMBERT_3979_SRS, NULL_GEOGRAPHIC_SRS
        ),
        proj_default_operation: probe_proj_default_operation(&req.aoi_bounds()),
        aoi_center_lat: req.center_lat,
        aoi_center_lon: req.center_lon,
        aoi_core_m: [req.width_m, req.height_m],
        aoi_buffer_m: req.buffer_m,
        resolution_m: req.resolution_m,
        resampling: format!("dtm={DTM_RESAMPLING}, dsm={DSM_RESAMPLING}"),
        wgs84_bounds: bounds,
        spec_hash: req.spec_hash(),
        gdal_version: gdal_version(),
        proj_version: proj_version(),
        dtm: HrdemFile {
            path: "dtm.tif".to_string(),
            sha256: sha256_file(&wgs84.dtm)?,
        },
        dsm: HrdemFile {
            path: "dsm.tif".to_string(),
            sha256: sha256_file(&wgs84.dsm)?,
        },
        native_dtm: HrdemFile {
            path: "native/dtm-3979.tif".to_string(),
            sha256: sha256_file(&native.dtm)?,
        },
        native_dsm: HrdemFile {
            path: "native/dsm-3979.tif".to_string(),
            sha256: sha256_file(&native.dsm)?,
        },
    };
    let provenance_json =
        serde_json::to_string_pretty(&provenance).map_err(|e| HrdemError::Io(e.to_string()))?;
    std::fs::write(staging.join("provenance.json"), &provenance_json)
        .map_err(|e| HrdemError::Io(format!("write provenance: {e}")))?;
    // dem.json (H4) — the core `DemMetadata` the ground station opens the
    // pair through; hashes the provenance JSON just written.
    write_dem_json(&staging, req, &provenance, &provenance_json)?;

    // Publish: destination never contains a partial tileset.
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| HrdemError::Io(format!("replace {}: {e}", dir.display())))?;
    }
    std::fs::rename(&staging, &dir)
        .map_err(|e| HrdemError::Io(format!("publish {}: {e}", dir.display())))?;
    Ok(dir)
}

/// Crop the remote COG pair over `/vsicurl/` into `staging/native`; no
/// reprojection, no resampling (the native grid feeds `gdalwarp`).
fn stage_window_crop(
    staging: &Path,
    item: &HrdemItem,
    ulx: f64,
    uly: f64,
    lrx: f64,
    lry: f64,
) -> Result<StagedPair, HrdemError> {
    let dtm = staging.join("native/dtm-3979.tif");
    let dsm = staging.join("native/dsm-3979.tif");
    window_crop_one("gdal_translate", &item.dtm_href, &dtm, ulx, uly, lrx, lry)?;
    window_crop_one("gdal_translate", &item.dsm_href, &dsm, ulx, uly, lrx, lry)?;
    Ok(StagedPair { dtm, dsm })
}

struct StagedPair {
    dtm: PathBuf,
    dsm: PathBuf,
}

/// Reproject the native window pair to a WGS84 regular grid with
/// `gdalwarp`, forcing the ADR-018 §D null datum transform via the
/// proj-string source SRS (GDAL would otherwise let PROJ pick the
/// seven-parameter EPSG:1946 from the file's EPSG:3979 tags). DTM and DSM
/// share one `-te`/`-tr`/`-tap` parameter set → strictly the same grid,
/// asserted after the warp; the resampling differs per product (H3):
/// `bilinear` for the DTM, footprint-max for the DSM.
fn reproject_to_wgs84(
    staging: &Path,
    req: &HrdemRequest,
    native: &StagedPair,
    bounds: [f64; 4],
) -> Result<StagedPair, HrdemError> {
    let dtm = staging.join("dtm.tif");
    let dsm = staging.join("dsm.tif");
    warp_one(&native.dtm, &dtm, req, bounds, DTM_RESAMPLING)?;
    warp_one(&native.dsm, &dsm, req, bounds, DSM_RESAMPLING)?;
    let dtm_dims = raster_dims(&dtm)?;
    let dsm_dims = raster_dims(&dsm)?;
    if dtm_dims != dsm_dims {
        return Err(HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: format!(
                "DTM and DSM landed on different grids ({dtm_dims:?} vs {dsm_dims:?}) — \
                 refusing to publish a mismatched pair"
            ),
        });
    }
    Ok(StagedPair { dtm, dsm })
}

/// One `gdalwarp` pass: native EPSG:3979 window → WGS84 regular grid with
/// the product's resampling policy (H3) and explicit nodata both sides.
fn warp_one(
    src: &Path,
    dest: &Path,
    req: &HrdemRequest,
    bounds: [f64; 4],
    resampling: &str,
) -> Result<(), HrdemError> {
    let (dlon, dlat) = wgs84_pixel_sizes(req.resolution_m, req.center_lat);
    let [west, south, east, north] = bounds;
    let output = Command::new("gdalwarp")
        .args([
            "-s_srs",
            NULL_LAMBERT_3979_SRS,
            "-t_srs",
            TARGET_WGS84_SRS,
            "-srcnodata",
            HRDEM_NODATA,
            "-dstnodata",
            HRDEM_NODATA,
            "-te",
        ])
        .arg(format!("{west:.9}"))
        .arg(format!("{south:.9}"))
        .arg(format!("{east:.9}"))
        .arg(format!("{north:.9}"))
        .arg("-tr")
        .arg(format!("{dlon:.12e}"))
        .arg(format!("{dlat:.12e}"))
        .args(["-tap", "-r", resampling, "-of", "GTiff"])
        .args([
            "-co",
            "COMPRESS=LZW",
            "-co",
            "TILED=YES",
            "-co",
            "BIGTIFF=IF_SAFER",
        ])
        .arg(src)
        .arg(dest)
        .output()
        .map_err(|e| HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: format!("spawn failed: {e}"),
        })?;
    if !output.status.success() {
        return Err(HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(())
}

/// Raster dimensions `(width, height)` via the `tiff` crate — no GDAL
/// needed for the same-grid assertion.
pub fn raster_dims(path: &Path) -> Result<(u32, u32), HrdemError> {
    let file = std::fs::File::open(path)
        .map_err(|e| HrdemError::Io(format!("open {}: {e}", path.display())))?;
    let mut decoder = tiff::decoder::Decoder::new(std::io::BufReader::new(file)).map_err(|e| {
        HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: format!("decode {}: {e}", path.display()),
        }
    })?;
    decoder.dimensions().map_err(|e| HrdemError::Gdal {
        tool: "gdalwarp".to_string(),
        detail: format!("dimensions of {}: {e}", path.display()),
    })
}

/// Read the cached session's readiness for this AOI (U9): parse the session
/// `dem.json` when present and report the rasters' size. Pure filesystem —
/// no STAC, no GDAL — so the ground station can call it offline (an already
/// fetched session reads as ready even with the network down).
#[must_use]
pub fn read_status(req: &HrdemRequest) -> maggcs_core::terrain::SurveyPrepStatus {
    use maggcs_core::terrain::{DemMetadata, SurveyPrepState, SurveyPrepStatus};
    let dir = req.session_dir();
    let folder = Some(dir.display().to_string());
    let datum_verified =
        maggcs_core::height::vertical_datum_chain_available(req.center_lat, req.center_lon);

    let meta: Option<DemMetadata> = std::fs::read_to_string(dir.join("dem.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let dtm = dir.join("dtm.tif");
    let dsm = dir.join("dsm.tif");

    match meta {
        Some(m) if dtm.is_file() && dsm.is_file() => {
            let bytes = std::fs::metadata(&dtm).map(|x| x.len()).unwrap_or(0)
                + std::fs::metadata(&dsm).map(|x| x.len()).unwrap_or(0);
            SurveyPrepStatus {
                state: SurveyPrepState::Ready,
                folder,
                project_name: m.project_name,
                acquisition: m.acquisition,
                resolution_m: Some(m.resolution_m),
                vertical_datum: Some(m.vertical_datum),
                has_dtm: m.has_dtm,
                has_dsm: m.has_dsm,
                bounds: Some(m.bounds),
                bytes: Some(bytes),
                datum_verified,
                message: None,
            }
        }
        _ if dir.is_dir() => SurveyPrepStatus {
            state: SurveyPrepState::Partial,
            folder,
            project_name: None,
            acquisition: None,
            resolution_m: None,
            vertical_datum: None,
            has_dtm: false,
            has_dsm: false,
            bounds: None,
            bytes: None,
            datum_verified,
            message: Some(
                "the cached session is incomplete — re-run prepare to rebuild it".to_string(),
            ),
        },
        _ => SurveyPrepStatus {
            state: SurveyPrepState::Absent,
            folder,
            project_name: None,
            acquisition: None,
            resolution_m: None,
            vertical_datum: None,
            has_dtm: false,
            has_dsm: false,
            bounds: None,
            bytes: None,
            datum_verified,
            message: Some("not prepared for this area yet".to_string()),
        },
    }
}

/// The warped raster's own placement as gdalwarp wrote it into the GeoTIFF
/// tags: `(dlon, dlat, west, north, width, height)`. These — not the
/// requested parameters — are what `dem.json` records (H4: exact per-axis
/// pixel angles so the runtime reader never falls back to its mid-latitude
/// constant).
struct WarpedPlacement {
    dlon: f64,
    dlat: f64,
    west: f64,
    north: f64,
    width: u32,
    height: u32,
}

fn warped_placement(path: &Path) -> Result<WarpedPlacement, HrdemError> {
    let file = std::fs::File::open(path)
        .map_err(|e| HrdemError::Io(format!("open {}: {e}", path.display())))?;
    let mut decoder = tiff::decoder::Decoder::new(std::io::BufReader::new(file)).map_err(|e| {
        HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: format!("decode {}: {e}", path.display()),
        }
    })?;
    let (width, height) = decoder.dimensions().map_err(|e| HrdemError::Gdal {
        tool: "gdalwarp".to_string(),
        detail: format!("dimensions of {}: {e}", path.display()),
    })?;
    let scale = decoder
        .get_tag_f64_vec(tiff::tags::Tag::ModelPixelScaleTag)
        .map_err(|e| HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: format!("pixel scale of {}: {e}", path.display()),
        })?;
    let tie = decoder
        .get_tag_f64_vec(tiff::tags::Tag::ModelTiepointTag)
        .map_err(|e| HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: format!("tiepoint of {}: {e}", path.display()),
        })?;
    if scale.len() < 2 || tie.len() < 6 || scale[0] <= 0.0 || scale[1] <= 0.0 {
        return Err(HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: format!("missing placement tags in {}", path.display()),
        });
    }
    Ok(WarpedPlacement {
        dlon: scale[0],
        dlat: scale[1],
        west: tie[3],
        north: tie[4],
        width,
        height,
    })
}

/// Write the session's `dem.json` (the core `DemMetadata`): the file the
/// ground station's `CogDem` opens the pair through. Placement comes from
/// the rasters' own tags; the provenance hash ties the metadata to the full
/// audit record (H4).
fn write_dem_json(
    staging: &Path,
    req: &HrdemRequest,
    provenance: &HrdemProvenance,
    provenance_json: &str,
) -> Result<(), HrdemError> {
    let placement = warped_placement(&staging.join("dtm.tif"))?;
    let dsm_dims = raster_dims(&staging.join("dsm.tif"))?;
    if (dsm_dims.0, dsm_dims.1) != (placement.width, placement.height) {
        return Err(HrdemError::Gdal {
            tool: "gdalwarp".to_string(),
            detail: "DTM and DSM placements disagree before writing dem.json".to_string(),
        });
    }
    let dtm_bytes = std::fs::read(staging.join("dtm.tif"))
        .map_err(|e| HrdemError::Io(format!("read dtm: {e}")))?;
    let dsm_bytes = std::fs::read(staging.join("dsm.tif"))
        .map_err(|e| HrdemError::Io(format!("read dsm: {e}")))?;
    let metadata = maggcs_core::terrain::DemMetadata {
        version: crate::cog::version_of(&[&dtm_bytes, &dsm_bytes]),
        raster_crs: "EPSG:4326".to_string(),
        resolution_m: req.resolution_m,
        bounds: maggcs_core::terrain::DemBounds {
            north: placement.north,
            south: placement.north - placement.dlat * f64::from(placement.height),
            east: placement.west + placement.dlon * f64::from(placement.width),
            west: placement.west,
        },
        vertical_datum: maggcs_core::terrain::VerticalDatum::OrthometricCgvd2013,
        // The Victoria item is DTM + DSM (ADR-018); northern DSM-only
        // coverage writes has_dtm=false when it is ever fetched.
        has_dsm: true,
        has_dtm: true,
        source: maggcs_core::terrain::DemSource::Hrdem,
        dlat_deg: Some(placement.dlat),
        dlon_deg: Some(placement.dlon),
        stac_item_id: Some(provenance.item_id.clone()),
        project_name: Some(provenance.item_id.clone()),
        acquisition: provenance.acquisition_datetime.clone(),
        original_crs: Some("EPSG:3979 (NAD83(CSRS) / Canada Atlas Lambert)".to_string()),
        provenance_sha256: Some(format!("{:x}", Sha256::digest(provenance_json.as_bytes()))),
        gdal_version: Some(provenance.gdal_version.clone()),
        proj_version: Some(provenance.proj_version.clone()),
    };
    let json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| HrdemError::Io(format!("serialize dem.json: {e}")))?;
    std::fs::write(staging.join("dem.json"), json)
        .map_err(|e| HrdemError::Io(format!("write dem.json: {e}")))
}

fn window_crop_one(
    tool: &'static str,
    href: &str,
    dest: &Path,
    ulx: f64,
    uly: f64,
    lrx: f64,
    lry: f64,
) -> Result<(), HrdemError> {
    let source = format!("/vsicurl/{href}");
    let output = Command::new(tool)
        .args(["-of", "COG", "-projwin"])
        .arg(format!("{ulx}"))
        .arg(format!("{uly}"))
        .arg(format!("{lrx}"))
        .arg(format!("{lry}"))
        .arg("-co")
        .arg("COMPRESS=LZW")
        .arg(&source)
        .arg(dest)
        .output()
        .map_err(|e| HrdemError::Gdal {
            tool: tool.to_string(),
            detail: format!("spawn failed: {e}"),
        })?;
    if !output.status.success() {
        return Err(HrdemError::Gdal {
            tool: tool.to_string(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(())
}

fn now_utc() -> String {
    // dem-prep is a build tool: RFC 3339 via `date` keeps the dependency
    // graph empty. A missing `date` degrades the timestamp, not the fetch.
    Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%dT%H:%M:%SZ")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_search_response() -> String {
        r#"{
            "type": "FeatureCollection",
            "features": [
                {
                    "id": "BC-Vancouver_Island_Sunshine_Coast_2018-1m",
                    "bbox": [-128.65, 47.40, -122.18, 52.09],
                    "type": "Feature",
                    "properties": {"datetime": "2019-10-01T12:00:00Z"},
                    "assets": {
                        "dsm": {"href": "https://example.test/a-dsm.tif"},
                        "dtm": {"href": "https://example.test/a-dtm.tif"},
                        "extent": {"href": "https://example.test/a-extent.geojson"}
                    }
                }
            ]
        }"#
        .to_string()
    }

    #[test]
    fn aoi_cap_counts_the_buffer_against_the_total() {
        // The cap is on the whole window: 1800 m core + 2 x 250 m buffer = 2300 m.
        let err = HrdemRequest::new(48.5, -123.4, 1800.0, 1000.0, 250.0, "out").unwrap_err();
        assert!(matches!(
            err,
            HrdemError::AoiTooLarge {
                width_m: 1800.0,
                height_m: 1000.0,
                limit_m: AOI_LIMIT_M,
            }
        ));
        // 1500 m core + 2 x 250 m buffer = 2000 m exactly: allowed.
        assert!(HrdemRequest::new(48.5, -123.4, 1500.0, 1000.0, 250.0, "out").is_ok());
        // A comfortable survey area with the default margin is fine.
        assert!(HrdemRequest::new(48.5, -123.4, 300.0, 300.0, DEFAULT_BUFFER_M, "out").is_ok());
    }

    #[test]
    fn aoi_cap_rejects_nonpositive_or_nan() {
        assert!(HrdemRequest::new(48.5, -123.4, 0.0, 10.0, 0.0, "out").is_err());
        assert!(HrdemRequest::new(48.5, -123.4, 10.0, -1.0, 0.0, "out").is_err());
        assert!(HrdemRequest::new(f64::NAN, -123.4, 10.0, 10.0, 0.0, "out").is_err());
        assert!(HrdemRequest::new(48.5, -123.4, 10.0, 10.0, -1.0, "out").is_err());
    }

    #[test]
    fn aoi_bounds_include_the_buffer() {
        let req = HrdemRequest::new(48.556, -123.381, 100.0, 100.0, 20.0, "out").unwrap();
        let aoi = req.aoi_bounds();
        // 70 m half-width (50 m core + 20 m buffer) at this latitude.
        let expect_dlat = 70.0 / METERS_PER_DEGREE;
        let expect_dlon = 70.0 / (METERS_PER_DEGREE * 48.556f64.to_radians().cos());
        assert!((aoi.north - req.center_lat - expect_dlat).abs() < 1e-12);
        assert!((req.center_lon - aoi.west - expect_dlon).abs() < 1e-12);
        assert!(aoi.south < req.center_lat);
        assert!(aoi.east > req.center_lon);
    }

    #[test]
    fn spec_hash_is_stable_and_sensitive() {
        let a = HrdemRequest::new(48.556, -123.381, 100.0, 100.0, 20.0, "out").unwrap();
        let b = HrdemRequest::new(48.556, -123.381, 100.0, 100.0, 20.0, "other").unwrap();
        assert_eq!(
            a.spec_hash(),
            b.spec_hash(),
            "out dir must not change the hash"
        );
        let c = HrdemRequest::new(48.556, -123.381, 100.0, 100.0, 30.0, "out").unwrap();
        assert_ne!(a.spec_hash(), c.spec_hash());
        let d = HrdemRequest::new(48.5561, -123.381, 100.0, 100.0, 20.0, "out").unwrap();
        assert_ne!(a.spec_hash(), d.spec_hash());
        // The WGS84 output resolution is part of the cached spec.
        let e = a.clone().resolution(0.5).unwrap();
        assert_ne!(a.spec_hash(), e.spec_hash());
        assert!(a.clone().resolution(f64::NAN).is_err());
        assert!(a.clone().resolution(0.0).is_err());
    }

    #[test]
    fn wgs84_pixel_sizes_match_metres_at_the_latitude() {
        let (dlon, dlat) = wgs84_pixel_sizes(1.0, 48.556);
        // 1 m north-south is one dlat step everywhere; east-west shrinks
        // with cos(lat).
        assert!((dlat * METERS_PER_DEGREE - 1.0).abs() < 1e-9);
        assert!(
            (dlon * METERS_PER_DEGREE * 48.556f64.to_radians().cos() - 1.0).abs() < 1e-9,
            "dlon must span 1 m of longitude at the AOI latitude"
        );
        assert!(dlon > dlat, "east-west degrees per metre are larger");
        // Halving the resolution halves both.
        let (dlon2, dlat2) = wgs84_pixel_sizes(0.5, 48.556);
        assert!((dlon2 - dlon / 2.0).abs() < 1e-15);
        assert!((dlat2 - dlat / 2.0).abs() < 1e-15);
    }

    #[test]
    fn estimated_wgs84_dims_bounds_the_rotated_window() {
        let req = HrdemRequest::new(48.556, -123.381, 100.0, 100.0, 20.0, "out").unwrap();
        // 45° worst case: (70 + 70) * sqrt(2) ≈ 198 px at 1 m.
        let (w, h) = req.estimated_wgs84_dims();
        assert_eq!((w, h), (198, 198));
        // Finer resolution scales up; the estimate is per-axis symmetric.
        let fine = req.clone().resolution(0.5).unwrap();
        assert_eq!(fine.estimated_wgs84_dims(), (396, 396));
        // Smaller core with no buffer shrinks accordingly: 2 · 25 · sqrt(2).
        let small = HrdemRequest::new(48.556, -123.381, 50.0, 50.0, 0.0, "out").unwrap();
        assert_eq!(small.estimated_wgs84_dims(), (71, 71));
    }

    #[test]
    fn projinfo_default_operation_is_extracted_for_audit() {
        // Captured from projinfo -s EPSG:3979 -t EPSG:4326 (PROJ 9.6.2,
        // 2026-10-17): the default pick is the seven-parameter EPSG:1946
        // that ADR-018 §D rejects — exactly what the provenance must record.
        let raw = "Candidate operations found: 2\n\
                   -------------------------------------\n\
                   Operation No. 1:\n\n\
                   unknown id, Inverse of Canada Atlas Lambert + NAD83(CSRS) to WGS 84 (2), 1 m, Canada - onshore and offshore - Alberta; British Columbia; Manitoba.\n\n\
                   PROJ string:\n\
                   +proj=pipeline\n  +step +inv +proj=lcc\n";
        assert_eq!(
            parse_projinfo_default(raw),
            "unknown id, Inverse of Canada Atlas Lambert + NAD83(CSRS) to WGS 84 (2), 1 m, Canada - onshore and offshore - Alberta; British Columbia; Manitoba."
        );
        assert_eq!(
            parse_projinfo_default(""),
            "unknown (projinfo returned nothing)"
        );
        assert_eq!(
            parse_projinfo_default("some other failure text"),
            "unknown (unrecognized projinfo output)"
        );
        assert_eq!(
            parse_projinfo_default("Operation No. 1:\n\n\nPROJ string:"),
            "unknown (no candidate operation listed)"
        );
    }

    #[test]
    fn parse_stac_response_single_item() {
        let items = parse_stac_features(&sample_search_response()).expect("parse");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "BC-Vancouver_Island_Sunshine_Coast_2018-1m");
        assert_eq!(
            items[0].acquisition_datetime.as_deref(),
            Some("2019-10-01T12:00:00Z")
        );
        assert_eq!(items[0].dtm_href, "https://example.test/a-dtm.tif");
        assert_eq!(items[0].dsm_href, "https://example.test/a-dsm.tif");
    }

    #[test]
    fn parse_stac_dedupes_relisted_items() {
        let one = sample_search_response();
        let parsed: serde_json::Value = serde_json::from_str(&one).expect("json");
        let mut two = parsed.clone();
        two["features"]
            .as_array_mut()
            .expect("features")
            .push(parsed["features"][0].clone());
        let items = parse_stac_features(&two.to_string()).expect("parse");
        assert_eq!(items.len(), 1, "the same item relisted must dedupe");
    }

    #[test]
    fn select_single_item_rejects_cross_project_and_empty() {
        let err = select_single_item(Vec::new()).unwrap_err();
        assert!(matches!(err, HrdemError::NoCoverage));
        let two = vec![
            HrdemItem {
                id: "A-1m".to_string(),
                acquisition_datetime: None,
                dtm_href: "https://example.test/a-dtm.tif".to_string(),
                dsm_href: "https://example.test/a-dsm.tif".to_string(),
            },
            HrdemItem {
                id: "B-1m".to_string(),
                acquisition_datetime: None,
                dtm_href: "https://example.test/b-dtm.tif".to_string(),
                dsm_href: "https://example.test/b-dsm.tif".to_string(),
            },
        ];
        match select_single_item(two) {
            Err(HrdemError::MultipleProjects { ids }) => {
                assert_eq!(ids, vec!["A-1m".to_string(), "B-1m".to_string()]);
            }
            other => panic!("expected MultipleProjects, got {other:?}"),
        }
    }

    #[test]
    fn missing_or_ambiguous_assets_are_errors() {
        let no_dtm = r#"{"features":[{"id":"x","properties":{},"assets":{
            "dsm":{"href":"https://example.test/x-dsm.tif"}}}]}"#;
        assert!(matches!(
            parse_stac_features(no_dtm),
            Err(HrdemError::Asset { missing: "DTM", .. })
        ));
        let ambiguous = r#"{"features":[{"id":"x","properties":{},
            "assets":{"a":{"href":"https://e.test/x-dtm.tif"},
                       "b":{"href":"https://e.test/y-dtm.tif"}}}]}"#;
        assert!(matches!(
            parse_stac_features(ambiguous),
            Err(HrdemError::Asset { .. })
        ));
    }

    #[test]
    fn empty_features_mean_no_coverage() {
        assert!(matches!(
            parse_stac_features(r#"{"type":"FeatureCollection","features":[]}"#),
            Ok(items) if items.is_empty()
        ));
        assert!(matches!(
            parse_stac_features("not json"),
            Err(HrdemError::Stac(_))
        ));
    }

    #[test]
    fn asset_fallback_matches_by_href_suffix() {
        // Asset keys renamed upstream: the unique -dtm.tif/-dsm.tif assets
        // still resolve; guessing beyond that is an error.
        let renamed = r#"{"features":[{"id":"x","properties":{},
            "assets":{"dem-dtm":{"href":"https://e.test/x-dtm.tif"},
                      "dem-dsm":{"href":"https://e.test/x-dsm.tif"}}}]}"#;
        let items = parse_stac_features(renamed).expect("parse");
        assert_eq!(items[0].dtm_href, "https://e.test/x-dtm.tif");
        assert_eq!(items[0].dsm_href, "https://e.test/x-dsm.tif");
    }

    #[test]
    fn provenance_round_trips() {
        let provenance = HrdemProvenance {
            schema_version: HrdemProvenance::SCHEMA_VERSION,
            source: "hrdem-lidar".to_string(),
            stac_api: STAC_SEARCH_URL.to_string(),
            collection: HRDEM_COLLECTION.to_string(),
            item_id: "BC-Vancouver_Island_Sunshine_Coast_2018-1m".to_string(),
            acquisition_datetime: Some("2019-10-01T12:00:00Z".to_string()),
            dtm_href: "https://example.test/a-dtm.tif".to_string(),
            dsm_href: "https://example.test/a-dsm.tif".to_string(),
            fetched_utc: "2026-10-17T00:00:00Z".to_string(),
            license: LICENSE_NAME.to_string(),
            license_url: LICENSE_URL.to_string(),
            horizontal_crs: "EPSG:4326 (WGS84, per ADR-018 §D null transform)".to_string(),
            vertical_datum: "CGVD2013 (EPSG:6647), orthometric, metres".to_string(),
            datum_note: "null transform per ADR-018 §D".to_string(),
            datum_pipeline: "null (proj strings sharing GRS80)".to_string(),
            proj_default_operation: "unknown id, Inverse of Canada Atlas Lambert \
                                     + NAD83(CSRS) to WGS 84 (2), 1 m, ..."
                .to_string(),
            aoi_center_lat: 48.556,
            aoi_center_lon: -123.381,
            aoi_core_m: [100.0, 100.0],
            aoi_buffer_m: 20.0,
            resolution_m: 1.0,
            resampling: format!("dtm={DTM_RESAMPLING}, dsm={DSM_RESAMPLING}"),
            wgs84_bounds: [-123.3823, 48.5554, -123.3804, 48.5567],
            spec_hash: "abc123".to_string(),
            gdal_version: "GDAL 3.13.3".to_string(),
            proj_version: "9.8.1".to_string(),
            dtm: HrdemFile {
                path: "dtm.tif".to_string(),
                sha256: "deadbeef".to_string(),
            },
            dsm: HrdemFile {
                path: "dsm.tif".to_string(),
                sha256: "cafebabe".to_string(),
            },
            native_dtm: HrdemFile {
                path: "native/dtm-3979.tif".to_string(),
                sha256: "beefdead".to_string(),
            },
            native_dsm: HrdemFile {
                path: "native/dsm-3979.tif".to_string(),
                sha256: "babecafe".to_string(),
            },
        };
        let json = serde_json::to_string(&provenance).expect("serialize");
        let back: HrdemProvenance = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, provenance);
    }

    #[test]
    fn cache_hit_returns_without_gdal_or_network() {
        let dir = std::env::temp_dir().join(format!(
            "maggcs-hrdem-cache-{}-{}",
            std::process::id(),
            now_utc().replace(['-', ':'], "")
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let req = HrdemRequest::new(48.556, -123.381, 100.0, 100.0, 20.0, &dir).unwrap();
        let session = req.session_dir();
        std::fs::create_dir_all(session.join("native")).expect("mkdir native");
        std::fs::write(session.join("dtm.tif"), b"dtm").expect("write dtm");
        std::fs::write(session.join("dsm.tif"), b"dsm").expect("write dsm");
        std::fs::write(session.join("native/dtm-3979.tif"), b"dtm").expect("write native dtm");
        std::fs::write(session.join("native/dsm-3979.tif"), b"dsm").expect("write native dsm");
        let provenance = serde_json::json!({
            "schema_version": 1,
            "source": "hrdem-lidar",
            "stac_api": STAC_SEARCH_URL,
            "collection": HRDEM_COLLECTION,
            "item_id": "cached",
            "acquisition_datetime": null,
            "dtm_href": "https://example.test/a-dtm.tif",
            "dsm_href": "https://example.test/a-dsm.tif",
            "fetched_utc": "2026-10-17T00:00:00Z",
            "license": LICENSE_NAME,
            "license_url": LICENSE_URL,
            "horizontal_crs": "EPSG:4326",
            "vertical_datum": "CGVD2013",
            "datum_note": "",
            "datum_pipeline": "null",
            "proj_default_operation": "recorded",
            "aoi_center_lat": req.center_lat,
            "aoi_center_lon": req.center_lon,
            "aoi_core_m": [req.width_m, req.height_m],
            "aoi_buffer_m": req.buffer_m,
            "resolution_m": req.resolution_m,
            "resampling": "dtm=bilinear, dsm=max",
            "wgs84_bounds": [0.0, 0.0, 0.0, 0.0],
            "spec_hash": req.spec_hash(),
            "gdal_version": "unknown",
            "proj_version": "unknown",
            "dtm": {"path": "dtm.tif", "sha256": "x"},
            "dsm": {"path": "dsm.tif", "sha256": "y"},
            "native_dtm": {"path": "native/dtm-3979.tif", "sha256": "x"},
            "native_dsm": {"path": "native/dsm-3979.tif", "sha256": "y"},
        });
        std::fs::write(
            session.join("provenance.json"),
            serde_json::to_string(&provenance).expect("serialize"),
        )
        .expect("write provenance");
        std::fs::write(session.join("dem.json"), b"{}").expect("write dem.json");
        // A matching hash with the complete file set is a hit: no network
        // and no GDAL are needed for a cache hit, on any machine.
        assert_eq!(
            cache_lookup(&session, &req.spec_hash()),
            Some(session.clone())
        );
        // A session without dem.json (pre-H4 layout) is a miss.
        std::fs::remove_file(session.join("dem.json")).expect("remove dem.json");
        assert_eq!(cache_lookup(&session, &req.spec_hash()), None);
        std::fs::write(session.join("dem.json"), b"{}").expect("restore dem.json");
        // A different buffer changes the hash: the stale cache must miss so
        // the fetch runs again instead of serving wrong data.
        let stale = HrdemRequest::new(48.556, -123.381, 100.0, 100.0, 30.0, &dir).unwrap();
        assert_eq!(cache_lookup(&session, &stale.spec_hash()), None);
        // Incomplete tilesets (missing raster, corrupt provenance) miss.
        std::fs::remove_file(session.join("dsm.tif")).expect("remove dsm");
        assert_eq!(cache_lookup(&session, &req.spec_hash()), None);
        std::fs::write(session.join("dem.json"), b"{}").expect("write dem.json");
        std::fs::write(session.join("dsm.tif"), b"dsm").expect("restore dsm");
        std::fs::remove_file(session.join("native/dtm-3979.tif")).expect("remove native dtm");
        assert_eq!(cache_lookup(&session, &req.spec_hash()), None);
        std::fs::write(session.join("native/dtm-3979.tif"), b"dtm").expect("restore native dtm");
        std::fs::write(session.join("provenance.json"), b"{ not json").expect("corrupt");
        assert_eq!(cache_lookup(&session, &req.spec_hash()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// U9: `read_status` reports absent → partial → ready purely from disk,
    /// so the Area step card works offline. Location matches the H0 fixture
    /// AOI so the height-datum chain is available.
    #[test]
    fn read_status_tracks_the_cached_session() {
        use maggcs_core::terrain::{DemMetadata, SurveyPrepState};
        let dir = std::env::temp_dir().join(format!(
            "maggcs-hrdem-status-{}-{}",
            std::process::id(),
            now_utc().replace(['-', ':'], "")
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let req = HrdemRequest::new(48.556, -123.381, 100.0, 100.0, 20.0, &dir).unwrap();

        // No session folder yet: absent.
        let absent = read_status(&req);
        assert_eq!(absent.state, SurveyPrepState::Absent);
        assert!(!absent.has_dtm && !absent.has_dsm);
        assert!(absent.datum_verified, "the fixture AOI is inside the grids");
        assert!(absent.message.is_some(), "absent must say what to do");

        // A folder without the pair: partial.
        let session = req.session_dir();
        std::fs::create_dir_all(&session).expect("mkdir session");
        assert_eq!(read_status(&req).state, SurveyPrepState::Partial);

        // A complete pair + dem.json: ready, with the metadata surfaced.
        std::fs::write(session.join("dtm.tif"), b"dtm-bytes").expect("dtm");
        std::fs::write(session.join("dsm.tif"), b"dsm").expect("dsm");
        let meta = DemMetadata {
            version: "abc123".to_string(),
            raster_crs: "EPSG:4326".to_string(),
            resolution_m: 1.0,
            bounds: maggcs_core::terrain::DemBounds {
                north: 48.5565,
                south: 48.5555,
                east: -123.3805,
                west: -123.3815,
            },
            vertical_datum: maggcs_core::terrain::VerticalDatum::OrthometricCgvd2013,
            has_dsm: true,
            has_dtm: true,
            source: maggcs_core::terrain::DemSource::Hrdem,
            dlat_deg: None,
            dlon_deg: None,
            stac_item_id: Some("item-1".to_string()),
            project_name: Some("BC-Vancouver_Island".to_string()),
            acquisition: Some("2019-10-01T12:00:00Z".to_string()),
            original_crs: None,
            provenance_sha256: None,
            gdal_version: None,
            proj_version: None,
        };
        std::fs::write(
            session.join("dem.json"),
            serde_json::to_string(&meta).expect("serialize"),
        )
        .expect("write dem.json");

        let ready = read_status(&req);
        assert_eq!(ready.state, SurveyPrepState::Ready);
        assert_eq!(ready.project_name.as_deref(), Some("BC-Vancouver_Island"));
        assert_eq!(ready.acquisition.as_deref(), Some("2019-10-01T12:00:00Z"));
        assert_eq!(ready.resolution_m, Some(1.0));
        assert!(ready.has_dtm && ready.has_dsm);
        assert_eq!(ready.bytes, Some(9 + 3), "dtm + dsm sizes");
        assert!(ready.bounds.is_some());
        assert!(ready.message.is_none(), "ready needs no next step");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live end-to-end over the real STAC API and `/vsicurl/` (ADR-018 H0
    /// fixture AOI): native window fetch (H1) + WGS84 reprojection (H2).
    /// Ignored by default: needs network + GDAL installed.
    /// Run with: `cargo test -p dem-prep hrdem_live -- --ignored --nocapture`
    /// H3 acceptance (*verify* 17): through the **production warp args**,
    /// `gdalwarp -r max` keeps a single-cell DSM peak exactly, never invents
    /// values, and leaves nodata untouched; `bilinear` (the DTM policy)
    /// interpolates but never blends nodata into valid terrain. Synthetic
    /// raster, no network — local GDAL only.
    /// Run with: `cargo test -p dem-prep hrdem_warp_policy -- --ignored --nocapture`
    #[test]
    #[ignore = "local GDAL required (plan H3 verification)"]
    fn hrdem_warp_policy_preserves_dsm_peaks_and_nodata() {
        let dir = std::env::temp_dir().join(format!(
            "maggcs-hrdem-warp-{}-{}",
            std::process::id(),
            now_utc().replace(['-', ':'], "")
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");

        // 5x5 grid in EPSG:3979: a -32767 ring around base 18.0 terrain
        // with one single-cell 25.0 spike.
        let grid = format!(
            "ncols 5\nnrows 5\nxllcorner -2029810\nyllcorner 411105\ncellsize 1\n\
             NODATA_value -32767\n{ring}\n{inner}\n{spike}\n{inner}\n{ring}\n",
            ring = "-32767 -32767 -32767 -32767 -32767",
            inner = "-32767 18.0 18.0 18.0 -32767",
            spike = "-32767 18.0 25.0 18.0 -32767",
        );
        let grd = dir.join("spike.grd");
        std::fs::write(&grd, grid).expect("write AAIGrid");
        let src = dir.join("spike3979.tif");
        let out = std::process::Command::new("gdal_translate")
            .args([
                "-q",
                "-of",
                "GTiff",
                "-a_srs",
                "EPSG:3979",
                "-a_ullr",
                "-2029810",
                "411110",
                "-2029805",
                "411105",
                "-a_nodata",
                HRDEM_NODATA,
            ])
            .arg(&grd)
            .arg(&src)
            .output()
            .expect("gdal_translate available (ignored test)");
        assert!(
            out.status.success(),
            "gdal_translate failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // A request centred on the synthetic window (null-transform centre).
        let (lat, lon) = gdaltransform_points(
            NULL_LAMBERT_3979_SRS,
            NULL_GEOGRAPHIC_SRS,
            &[(-2029807.5, 411107.5)],
        )
        .expect("centre transform")
        .pop()
        .map(|(x, y)| (y, x))
        .expect("one point");
        let req = HrdemRequest::new(lat, lon, 5.0, 5.0, 0.0, &dir)
            .expect("request")
            .resolution(1.0)
            .expect("resolution");
        let bounds = window_bounds_wgs84(-2029807.5, 411107.5, 2.5, 2.5).expect("bounds");
        let native = StagedPair {
            dtm: src.clone(),
            dsm: src,
        };
        let staged = reproject_to_wgs84(&dir, &req, &native, bounds).expect("warp");

        let (dtm_w, dtm_h, dtm_vals) = read_f32_grid(&staged.dtm);
        let (dsm_w, dsm_h, dsm_vals) = read_f32_grid(&staged.dsm);
        assert_eq!(
            (dtm_w, dtm_h),
            (dsm_w, dsm_h),
            "DTM and DSM must share one grid"
        );

        // DSM (max): the peak survives exactly; no invented values; the
        // nodata ring is not filled and never becomes a fake peak.
        let mut has_peak = false;
        let mut has_nodata = false;
        for v in &dsm_vals {
            if *v == -32767.0 {
                has_nodata = true;
            } else {
                assert!(
                    (*v - 18.0).abs() < 1e-6 || (*v - 25.0).abs() < 1e-6,
                    "DSM max invented the value {v}"
                );
                has_peak |= (*v - 25.0).abs() < 1e-6;
            }
        }
        assert!(has_peak, "the 25.0 spike must survive -r max");
        assert!(has_nodata, "nodata cells must stay nodata, not be filled");

        // DTM (bilinear): interpolates (values strictly between 18 and 25
        // are expected), but nodata never blends into valid terrain and the
        // source envelope is never exceeded.
        for v in &dtm_vals {
            assert!(
                *v == -32767.0 || (18.0..=25.0).contains(v),
                "bilinear must stay within the source envelope (nodata or [18, 25]), got {v}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "live network + GDAL required (plan H1/H2 verification)"]
    fn hrdem_live_fetch_from_stac() {
        let out = std::env::temp_dir().join("maggcs-hrdem-live");
        let _ = std::fs::remove_dir_all(&out);
        let req = HrdemRequest::new(48.556_06, -123.381_36, 100.0, 100.0, 20.0, &out).unwrap();
        let dir = fetch_tileset(&req).expect("live fetch");
        let provenance: HrdemProvenance = serde_json::from_str(
            &std::fs::read_to_string(dir.join("provenance.json")).expect("read provenance"),
        )
        .expect("provenance parses");
        assert_eq!(
            provenance.item_id,
            "BC-Vancouver_Island_Sunshine_Coast_2018-1m"
        );
        assert!(provenance.gdal_version.starts_with("GDAL"));
        assert!(provenance.proj_version.contains('.'));
        // §D audit: the PROJ default (seven-parameter EPSG:1946) must be
        // recorded, and it must NOT be the pipeline we used.
        assert!(
            provenance
                .proj_default_operation
                .contains("NAD83(CSRS) to WGS 84 (2)"),
            "audit record should name PROJ's default 7-param pick: {}",
            provenance.proj_default_operation
        );
        assert!(provenance.datum_pipeline.contains("null"));

        // H1: the native axis-aligned window is (core + 2*buffer) m per
        // side: 1 m cells → 140 px per side (projwin may snap by ≤1 px).
        // Also guards the grid-convergence pitfall: a corner-envelope window
        // would come out ~66 x 187 px.
        for raster in ["native/dtm-3979.tif", "native/dsm-3979.tif"] {
            let dims = raster_dims(&dir.join(raster)).expect("native dims");
            assert!(
                (139..=141).contains(&dims.0) && (139..=141).contains(&dims.1),
                "{raster}: expected ~140x140 px, got {:?}",
                dims
            );
        }

        // H2: the WGS84 pair is on one strictly identical grid, sized for
        // the ~140 m window at the requested resolution.
        let dtm_dims = raster_dims(&dir.join("dtm.tif")).expect("dtm dims");
        let dsm_dims = raster_dims(&dir.join("dsm.tif")).expect("dsm dims");
        assert_eq!(dtm_dims, dsm_dims, "DTM and DSM must share one grid");
        // H3: the actual grid must not exceed the pre-warp estimate.
        let (est_w, est_h) = req.estimated_wgs84_dims();
        assert!(
            dtm_dims.0 <= est_w + 1 && dtm_dims.1 <= est_h + 1,
            "warped dims {dtm_dims:?} exceed the estimate ({est_w} x {est_h})"
        );
        let (dlon, dlat) = wgs84_pixel_sizes(req.resolution_m, req.center_lat);
        let [west, south, east, north] = provenance.wgs84_bounds;
        let expect_w = ((east - west) / dlon).round();
        let expect_h = ((north - south) / dlat).round();
        assert!(
            (f64::from(dtm_dims.0) - expect_w).abs() <= 2.0
                && (f64::from(dtm_dims.1) - expect_h).abs() <= 2.0,
            "warped dims {dtm_dims:?} should match the bounds span \
             ({expect_w} x {expect_h} px)"
        );

        // H2 control point (§D), part 1 — placement: the warped pair's own
        // placement tags must reproduce the null-transform bounding box of
        // the native window (tap may snap outward by ≤1 pixel). Note this
        // compares bbox to bbox: the native window is tilted ~25° in
        // lon/lat (Lambert grid convergence), so its NW *corner* is not the
        // bbox NW corner.
        let placement = placement_tags(&dir.join("dtm.tif")).expect("dtm placement");
        let (w_px, h_px) = raster_dims(&dir.join("dtm.tif")).expect("dtm dims");
        let (dlon, dlat) = wgs84_pixel_sizes(req.resolution_m, req.center_lat);
        let (west, north) = placement.origin;
        let east = west + f64::from(w_px) * placement.pixel.0;
        let south = north - f64::from(h_px) * placement.pixel.1;
        let [te_w, te_s, te_e, te_n] = provenance.wgs84_bounds;
        for (actual, expected, label, step) in [
            (west, te_w, "west", placement.pixel.0),
            (east, te_e, "east", placement.pixel.0),
            (south, te_s, "south", placement.pixel.1),
            (north, te_n, "north", placement.pixel.1),
        ] {
            assert!(
                (actual - expected).abs() <= 1.5 * step,
                "warped {label} {actual} deviates from the null-transform bounds \
                 {expected} by more than 1.5 px"
            );
        }
        // The requested pixel sizes are honoured.
        for (actual, expected) in [(placement.pixel.0, dlon), (placement.pixel.1, dlat)] {
            assert!(
                (actual - expected).abs() <= 1e-9,
                "warped pixel size {actual} != requested {expected}"
            );
        }

        // H2 control point (§D), part 2 — discrimination: PROJ's *default*
        // pipeline (seven-parameter EPSG:1946) and our forced null must
        // differ by exactly the residual §D recorded (~1.3 m at this AOI).
        // If this ever changes, PROJ's default changed — re-audit ADR-018
        // before touching the expectation.
        let via_default = gdaltransform_points(
            "EPSG:4326",
            "EPSG:3979",
            &[(req.center_lon, req.center_lat)],
        )
        .expect("default pipeline transform")
        .pop()
        .expect("one point");
        let via_null =
            transform_center_3979(req.center_lat, req.center_lon).expect("null pipeline transform");
        let residual = (
            (via_default.0 - via_null.0).abs(),
            (via_default.1 - via_null.1).abs(),
        );
        assert!(
            residual.0 > 0.05 && residual.0 < 5.0 && residual.1 > 0.05 && residual.1 < 5.0,
            "PROJ default vs null residual {residual:?} m is outside the \
             ADR-018 §D recorded scale (~1 m) — re-audit the datum decision"
        );

        // H2 value check: the cell covering the AOI centre carries the
        // native centre value (nearest resampling, low-relief AOI).
        let placement = placement_tags(&dir.join("dtm.tif")).expect("dtm placement");
        let native = read_center_value(&dir.join("native/dtm-3979.tif"));
        let warped = sample_wgs84_at(
            &dir.join("dtm.tif"),
            &placement,
            req.center_lon,
            req.center_lat,
        );
        assert!(
            (native - warped).abs() <= 0.5,
            "warped centre value {warped} deviates from native {native} by > 0.5 m"
        );

        // H4: the session carries a core `dem.json`; CogDem opens the
        // pair through it and resolves grid nodes exactly (the exact pixel
        // angles, not the mid-latitude constant).
        let meta: maggcs_core::terrain::DemMetadata = serde_json::from_str(
            &std::fs::read_to_string(dir.join("dem.json")).expect("read dem.json"),
        )
        .expect("dem.json parses");
        assert_eq!(meta.source, maggcs_core::terrain::DemSource::Hrdem);
        assert_eq!(
            meta.stac_item_id.as_deref(),
            Some("BC-Vancouver_Island_Sunshine_Coast_2018-1m")
        );
        assert!(meta.has_dtm && meta.has_dsm);
        assert_eq!(
            meta.vertical_datum,
            maggcs_core::terrain::VerticalDatum::OrthometricCgvd2013
        );
        let (w, h, dtm_direct) = read_f32_grid(&dir.join("dtm.tif"));
        use maggcs_core::terrain::ElevationSource as _;
        let dem =
            maggcs_core::terrain::CogDem::open(&dir.join("dtm.tif"), &dir.join("dsm.tif"), &meta)
                .expect("CogDem opens the HRDEM session");
        let dlat = meta.dlat_deg.expect("exact dlat");
        let dlon = meta.dlon_deg.expect("exact dlon");
        // Grid nodes (a pixel-corner coordinate resolves a single cell
        // exactly): compare the runtime sample against the raw raster.
        let mut compared = 0;
        // Corners of the tilted window are legitimately nodata (outside the
        // native footprint); the central band is valid — cover both.
        for (col, row) in [
            (0usize, 0usize),
            (w / 2, h / 2),
            (w.saturating_sub(2), h.saturating_sub(2)),
            (w / 2, h / 4),
            (w / 2, 3 * h / 4),
            (w / 4, h / 2),
        ] {
            let lon = meta.bounds.west + col as f64 * dlon;
            let lat = meta.bounds.north - row as f64 * dlat;
            let direct = dtm_direct[row * w + col];
            match dem.elevation(maggcs_core::plan::GeoPoint::new(lat, lon)) {
                Ok(height) => {
                    assert!(
                        (height.meters() - f64::from(direct)).abs() < 1e-4,
                        "sample at ({col},{row}) = {} vs direct {direct}",
                        height.meters()
                    );
                    compared += 1;
                }
                Err(maggcs_core::terrain::TerrainError::Void(_)) => {
                    assert_eq!(direct, -32767.0, "only nodata cells may refuse");
                }
                Err(other) => panic!("unexpected sample error: {other:?}"),
            }
        }
        assert!(compared >= 3, "expected mostly-valid cells, got {compared}");

        // §D vs H0 fixture: the committed fixture window was cut from the
        // PROJ *default* pipeline (rasterio, seven-parameter EPSG:1946);
        // this code cuts via the §D null transform, so the windows sit a
        // couple of pixels apart. The data itself must be identical: some
        // shift in ±2 px aligns the two windows pixel-for-pixel.
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/hrdem/victoria-aoi-dtm.tif");
        let fetched = read_f32_grid(&dir.join("native/dtm-3979.tif"));
        let reference = read_f32_grid(&fixture);
        let aligned = align_shift(fetched, reference, 2);
        assert!(
            aligned,
            "fetched window does not align with the H0 fixture within ±2 px"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    /// Full Float32 grid `(width, height, values)` of a small raster.
    fn read_f32_grid(path: &Path) -> (usize, usize, Vec<f32>) {
        let file =
            std::fs::File::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
        let mut decoder =
            tiff::decoder::Decoder::new(std::io::BufReader::new(file)).expect("decode raster");
        let (width, height) = decoder.dimensions().expect("dims");
        let image = decoder.read_image().expect("read image");
        let tiff::decoder::DecodingResult::F32(values) = image else {
            panic!("expected Float32 samples in {}", path.display());
        };
        (width as usize, height as usize, values)
    }

    /// True when shifting `fetched` by some `(dx, dy)` within ±`max_shift`
    /// makes it match `reference` cell-for-cell over the overlap. Records
    /// the §D null-vs-default residual (~1 px) without pinning the exact
    /// shift, which legitimately moves with PROJ versions.
    fn align_shift(
        (fw, fh, fv): (usize, usize, Vec<f32>),
        (rw, rh, rv): (usize, usize, Vec<f32>),
        max_shift: i32,
    ) -> bool {
        assert_eq!((fw, fh), (rw, rh), "grid sizes must match to compare");
        for dy in -max_shift..=max_shift {
            for dx in -max_shift..=max_shift {
                let mut aligned = true;
                'cells: for r in 0..rh {
                    for c in 0..rw {
                        let (fc, fr) = (c as i32 + dx, r as i32 + dy);
                        if fc < 0 || fr < 0 || fc >= fw as i32 || fr >= fh as i32 {
                            continue;
                        }
                        let a = rv[r * rw + c];
                        let b = fv[fr as usize * fw + fc as usize];
                        if a == -32767.0 && b == -32767.0 {
                            continue;
                        }
                        if (a - b).abs() > 1e-6 {
                            aligned = false;
                            break 'cells;
                        }
                    }
                }
                if aligned {
                    return true;
                }
            }
        }
        false
    }

    /// `(dlon, dlat, west, north)` GeoTIFF placement of a raster.
    struct Placement {
        pixel: (f64, f64),
        origin: (f64, f64),
    }

    fn placement_tags(path: &Path) -> Result<Placement, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut decoder = tiff::decoder::Decoder::new(std::io::BufReader::new(file))
            .map_err(|e| e.to_string())?;
        let scale = decoder
            .get_tag_f64_vec(tiff::tags::Tag::ModelPixelScaleTag)
            .map_err(|e| e.to_string())?;
        let tie = decoder
            .get_tag_f64_vec(tiff::tags::Tag::ModelTiepointTag)
            .map_err(|e| e.to_string())?;
        if scale.len() < 2 || tie.len() < 6 {
            return Err("missing placement tags".to_string());
        }
        Ok(Placement {
            pixel: (scale[0], scale[1]),
            origin: (tie[3], tie[4]),
        })
    }

    /// The value at the centre of a small single-band Float32 raster.
    fn read_center_value(path: &Path) -> f64 {
        let file = std::fs::File::open(path).expect("open raster");
        let mut decoder =
            tiff::decoder::Decoder::new(std::io::BufReader::new(file)).expect("decode raster");
        let (width, height) = decoder.dimensions().expect("dims");
        let image = decoder.read_image().expect("read image");
        let tiff::decoder::DecodingResult::F32(values) = image else {
            panic!("expected Float32 samples");
        };
        let idx = (height as usize / 2) * width as usize + width as usize / 2;
        f64::from(values[idx])
    }

    /// Nearest-cell sample of a WGS84 raster at a lon/lat.
    fn sample_wgs84_at(path: &Path, placement: &Placement, lon: f64, lat: f64) -> f64 {
        let file = std::fs::File::open(path).expect("open raster");
        let mut decoder =
            tiff::decoder::Decoder::new(std::io::BufReader::new(file)).expect("decode raster");
        let (width, height) = decoder.dimensions().expect("dims");
        let image = decoder.read_image().expect("read image");
        let tiff::decoder::DecodingResult::F32(values) = image else {
            panic!("expected Float32 samples");
        };
        let (dlon, dlat) = placement.pixel;
        let col = ((lon - placement.origin.0) / dlon).floor() as usize;
        let row = ((placement.origin.1 - lat) / dlat).floor() as usize;
        assert!(
            col < width as usize && row < height as usize,
            "point outside raster"
        );
        f64::from(values[row * width as usize + col])
    }
}
