# ADR-018: HRDEM elevation source and datum handling

- Status: Accepted (2026-10-15; operator confirmed the Victoria AOI and the
  §D null-transformation decision)
- Date: 2026-10-15
- Relates to: ADR-004 (terrain sources), ADR-006 (height datums),
  plan `docs/plans/lifecycle-survey-hrdem-plan.md` WP **H0**

## Decision

1. **Source** — the survey DEM comes from the NRCan **HRDEM** (High Resolution
   Digital Elevation Model), STAC collection **`hrdem-lidar`**, queried per
   project ("mosaic by project"). AOIs that span more than one project are
   rejected with an alarm instead of being stitched (enforced in H1).
2. **Selected project** — the pilot AOI is covered by exactly one item,
   `BC-Vancouver_Island_Sunshine_Coast_2018-1m`. No other item intersects the
   AOI, so no "newest wins" tie-break is needed.
3. **Ingest** — 1 m, **no downsampling** (plan decision 3). The COGs are read
   by HTTP Range directly from the public S3 bucket (no authentication); the
   per-survey AOI is a small window, so full-raster memory is irrelevant.
4. **Fixture** — a 140 m × 140 m (100 m core + 20 m buffer) crop of DTM and DSM
   is committed under `testdata/hrdem/` for offline tests, under the Open
   Government Licence – Canada (attribution in that folder).
5. **Datum handling (see §D)** — treat NAD83(CSRS) as coincident with WGS 84
   for the horizontal component (null transformation). Documented residual
   ≈ 1 m.

## Context

- Survey AOI: **Victoria, BC only**, centre **48.55606° N, 123.38136° W**
  (operator-confirmed). No second site is in scope for the lifecycle plan.
- AOI hard limit is 100 m × 100 m plus buffer (plan decision 3); the accuracy
  bottleneck is therefore the datum chain, not memory or performance.
- Three height datums are in play and must stay separated (ADR-006):
  `H` CGVD2013 orthometric, `h` WGS 84 ellipsoid, AMSL (PX4
  `MAV_FRAME_GLOBAL`). Conversion is `H + N(CGG2013) = h`,
  `h − N(EGM96) = AMSL`; the conversion lives only in `core::height`.

## Findings

### STAC / collection

- Collection **`hrdem-lidar`** on the NRCan datacube STAC API
  <https://datacube.services.geo.ca/stac/api> (`stac_version` 1.0.0; also
  served at <https://datacube.services.geo.ca/pgstac/api/>).
  *(fetched 2026-10-15)*
- Licence: **Open Government Licence – Canada 2.0**
  <https://open.canada.ca/en/open-government-licence-canada>
  *(fetched 2026-10-15)*.
- Vertical datum: **CGVD2013** (EPSG:6647), orthometric, metres. The
  distributed COGs are reprojected to **EPSG:3979**
  (NAD83(CSRS) / Canada Atlas Lambert).
- The collection note states there is **no integration/edgematching between
  projects** — the reason the plan rejects cross-project stitching.

### Selected item

- Item id: **`BC-Vancouver_Island_Sunshine_Coast_2018-1m`**; acquisition
  `datetime` **2019-10-01**; project name "BC Vancouver Island Sunshine Coast
  2018, 1 m".
- Footprint bbox `[-128.6556, 47.4040, -122.1866, 52.0974]`; project raster
  `proj:shape` `[430547, 300978]`.
- Assets (HTTPS on S3, **no authentication** — `HEAD` 200 and `Range` 206
  verified, fetched 2026-10-15):
  - DTM `https://canelevation-dem.s3.ca-central-1.amazonaws.com/hrdem-lidar/BC-Vancouver_Island_Sunshine_Coast_2018-1m-dtm.tif`
    (`Content-Length` 71 493 061 342)
  - DSM `…/BC-Vancouver_Island_Sunshine_Coast_2018-1m-dsm.tif` (72 793 855 024)
  - plus `-extent.geojson`, `-dtm.vrt`, `-dsm.vrt`, `coverage`, `thumbnail`.

### COG internals (parsed from HTTP Range headers)

- BigTIFF (version 43, 8-byte offsets), 300 978 (X) × 430 547 (Y) pixels,
  tiles **512 × 512** (494 508 tiles).
- `Float32` (`SampleFormat=3`, `BitsPerSample=32`), `Compression=5` (LZW),
  `Predictor=1`, **`GDAL_NODATA = -32767`**, layout IFDS-before-data.
- `ModelPixelScale (1, 1, 0)`; `ModelTiepoint (0,0,0, -2249116, 807835, 0)`.
- GeoTIFF keys: projected CRS **EPSG:3979**, units metre, geodetic datum
  NAD83(CSRS).
- **8 overview levels** (each halving the resolution).
- DSM has the identical structure.

### Remote windowed read (verified)

- AOI centre → EPSG:3979 `(-2029808.408, 411109.020)` (rasterio, fetched
  2026-10-15).
- A 140 × 140 window (col 219237, row 396655) over the 100 m core plus a 20 m
  buffer reads with no resampling, DTM 18.01–20.72 m, DSM 18.07–20.79 m, no
  nodata cells in the window.

### Vertical accuracy

- HRDEM **Product Specification**, Edition 1.7 (2026-05-19), states HRDEM
  products derived from airborne LiDAR are generally **better than 1 m** in
  elevation:
  <https://download-telecharger.services.geo.ca/pub/elevation/dem_mne/highresolution_hauteresolution/HRDEM_Product_Specification.pdf>.
- The **per-project** accuracy figure lives in each project's metadata (File
  Geodatabase) and is **not** carried in the STAC item; it was not retrieved.
  The generic HRDEM statement above is used as the planning figure.
- For context, the federal airborne-LiDAR acquisition guideline targets
  non-vegetated RMSEz ≤ 10 cm; the HRDEM product figure (≤ 1 m) is the
  conservative number used here.

## §D — NAD83(CSRS) ↔ WGS 84 horizontal handling

Problem: HRDEM is **NAD83(CSRS)**; PX4/GNSS positions are **WGS 84**. At 1 m
resolution a 1–2 m horizontal difference is 1–2 pixels.

Sourced facts *(fetched 2026-10-15)*:

- **EPSG:4617** (NAD83(CSRS)) remark: "Includes all versions of NAD83(CSRS)
  from v2 [CSRS98] onwards without specific identification. As such it has an
  **accuracy of approximately 1 m**."
  <https://epsg.org/crs_4617/NAD83-CSRS-.html>
- **EPSG:1842** "NAD83(CSRS) to WGS 84 (1)" — a null/copy transformation,
  scope "approximation for medium and low accuracy applications", remark
  "**For many purposes NAD83(CSRS) can be considered to be coincident with
  WGS 84**". <https://epsg.io/1842>
- **EPSG:1946** "NAD83(CSRS) to WGS 84 (2)" — 7-parameter Helmert
  (translations `-0.991, 1.9072, 0.5129`), **operation accuracy 1.0 m**,
  derived ignoring time-dependent terms and assuming ITRF96(1997.0) ≈ WGS 84.
  <https://epsg.io/1946>

**Decision:** use the **null transformation** (treat NAD83(CSRS) ≡ WGS 84
horizontally). Rationale:

- The 7-parameter option is itself only stated at 1.0 m accuracy and assumes
  equality at a 1997 epoch, so it buys no meaningful improvement over the null
  option at this scale.
- The dominant error for AGL is **vertical** (the CGG2013 geoid grid is still a
  blocker, issue N2); ~1 m of horizontal position is sub-survey-line and far
  below the vertical bottleneck.
- The DEM's own horizontal positional accuracy is of the same order (≈ 1 m).

**Residual risk recorded:** ≈ 1 m horizontal, i.e. up to ~2 pixels at 1 m.
This is only significant over very steep or shoreline terrain (the Victoria
AOI is low-relief coastal and reads 18–21 m). The control-point check in
**M9/H7** (target < 0.5 m, AGENTS.md) will reveal any systematic horizontal
bias; if it does, the 7-parameter transform (EPSG:1946) becomes the fallback.
No hand-rolled datum shift is implemented.

## §E — Licence and attribution

- Redistribution of the fixture is permitted under OGL-Canada 2.0, which
  requires an attribution statement (or, if none is specified, the standard
  one). NRCan does not specify a custom statement, so the standard applies:
  **"Contains information licensed under the Open Government Licence –
  Canada."** <https://open.canada.ca/en/open-government-licence-canada>
- The attribution and source URLs are committed in
  `testdata/hrdem/ATTRIBUTION.md`; machine-readable provenance (item id,
  acquisition date, asset hrefs, CRS, window, SHA-256) is in
  `testdata/hrdem/provenance.json`.
- The licence is **not** a barrier for the magnetic (geomagnetic) streams
  handled elsewhere: those are NRCan geomagnetic data and are **never** put in
  the repo, fixtures or installer (plan §5B, ADR-020).

## Fixture (§H0(e))

- `testdata/hrdem/victoria-aoi-dtm.tif`, `testdata/hrdem/victoria-aoi-dsm.tif`
  — 140 × 140, EPSG:3979, Float32, nodata −32767, DEFLATE, tiled COG,
  **pixel-exact** against the source (verified: a 5 × 5 sample compares
  identically to the remote COG).
- Total size 130 317 bytes (≈ 0.13 MB), well under the 2 MB budget.
- `provenance.json` and `ATTRIBUTION.md` accompany them.

## Consequences

- H1 (`dem-prep hrdem`) queries `hrdem-lidar`, rejects cross-project AOIs, and
  records item id / project / acquisition date / asset href / fetch time /
  SHA-256 / licence in the output provenance JSON.
- H2 implements EPSG:3979 (NAD83(CSRS) Lambert) projection; per §D it does
  **not** attach a NAD83(CSRS)→WGS 84 shift (null), so the transform is
  Lambert forward/inverse on the GRS-80 ellipsoid.
- H3/H4 keep 1 m resolution, DTM bilinear, **DSM footprint-max (conservative)**,
  nodata preserved, DTM/DSM on the same grid, and write the exact per-pixel
  angle (`dlat_deg`, `dlon_deg`) plus `CGVD2013` as the vertical datum.
- H7 remains blocked on a real CGG2013 grid (issue N2); until then AMSL export
  must be blocked and flagged "vertical datum unverified".

## Open items

- ~~Operator confirmation of the AOI centre (48.55606, −123.38136)~~
  **confirmed 2026-10-15.**
- ~~Confirmation of the §D null-transformation decision~~ **confirmed
  2026-10-15 (null transformation accepted; EPSG:1946 remains the fallback if
  M9/H7 control points show a systematic horizontal bias).**
- Per-project vertical-accuracy figure from the project File Geodatabase, if a
  tighter number than the ≤ 1 m product figure is required.

## Addendum (2026-10-17): dem-prep shells out to the GDAL/PROJ CLI

**Decision.** `dem-prep hrdem` (H1) and the H2/H3 reprojection invoke
`gdal_translate` / `gdaltransform` / `gdalwarp` as **subprocesses**; dem-prep
does not link the `gdal` crate or libgdal. This is the choice H2 was required
to record.

**Why.**

- The Rust build graph stays free of system-library headers (libgdal-dev),
  which are the heaviest native dependency this repo would otherwise carry
  and the most common cross-platform build breakage (three-platform CI).
- Rule 13 already confines GDAL/PROJ to `tools/dem-prep`; a subprocess boundary
  makes that confinement structural rather than conventional.
- Versions are recorded per run: the provenance JSON captures
  `gdalinfo --version` and the PROJ release line, so every artifact states the
  exact toolchain that produced it (H2 will pin them in CI).
- The cost — parsing CLI output and per-invocation process startup — is
  irrelevant at the scale of one small AOI per survey area.

**Consequences.**

- `gdal_translate`, `gdaltransform` (and later `gdalwarp`) must be on PATH at
  runtime; a missing tool is a first-class, actionable error
  (`HrdemError::GdalMissing`), not a build failure.
- `gdalinfo --version` / `projinfo --version` are queried per fetch and written
  to `provenance.json`.
- The live end-to-end test (`hrdem_live_fetch_from_stac`, `#[ignore]`) is the
  acceptance for the `/vsicurl/` windowed-read behaviour (plan *verify* item
  16).

## Addendum (2026-10-17): H2 enforces the §D null transform in GDAL

**Finding (live, PROJ 9.6.2).** PROJ's *default* operation for
EPSG:3979 → EPSG:4326 near the Victoria AOI is **"NAD83(CSRS) to WGS 84 (2)"
(EPSG:1946)** — the seven-parameter Helmert §D explicitly rejected
(translations −0.991, 1.9072, 0.5129; ≈1.29 m horizontal at the AOI). Left to
itself, `gdalwarp`/`gdaltransform` with EPSG codes applies it silently.

**Decision.** Every transform in `dem-prep hrdem` runs between proj strings
that share the GRS-80 ellipsoid and carry no datum identity:

- `+proj=longlat +ellps=GRS80 +no_defs` (WGS 84 lon/lat, treated as
  NAD83(CSRS) per §D) ↔
- `+proj=lcc +lat_1=49 +lat_2=77 +lat_0=49 +lon_0=-95 +x_0=0 +y_0=0
  +ellps=GRS80 +units=m +no_defs` (the EPSG:3979 geometry).

Same ellipsoid both sides → a datum step is structurally impossible. The warp
passes this source string via `-s_srs`, never the file's EPSG:3979 tags.
Each provenance records `datum_pipeline` (the strings actually used) and
`proj_default_operation` (what PROJ would have picked — audit contrast, not
used).

**Consequence for the H0 fixture.** The committed fixture window
(origin −2029879, 411180) was cut by rasterio via PROJ's *default* pipeline,
so it sits **1 px (≈1.29 m)** from the null-transform window
(origin −2029880, 411181) this code produces. The data is identical: the
overlapping cells match pixel-for-pixel after the 1-px alignment (19 321
cells, max |diff| = 0.0, verified 2026-10-17). This is the §D-recorded
residual made visible; the live test asserts the ±2 px alignment instead of
pinning the shift, which legitimately moves with PROJ versions.

**Version pinning (plan *verify* 18).** Measured on this machine:
GDAL 3.10.3 / PROJ 9.6.2 (conda-forge user prefix under `~/.local/gdal-env`;
needs `PROJ_DATA` pointing at the prefix's `share/proj`). CI does not need
GDAL (unit tests parse fixtures; the live test stays `#[ignore]`), so pinning
moves to whenever CI gains GDAL-dependent steps (H3 and beyond).

**Runtime installation (three platforms).**

- Linux: distro packages (`gdal`, `proj`) or any conda-forge prefix; PROJ
  binaries print their release line on stderr since PROJ 9 — `dem-prep`
  reads both streams.
- macOS: `brew install gdal proj`.
- Windows: OSGeo4W or a conda-forge environment on PATH; no compilation
  needed because nothing links libgdal.

## Addendum (2026-10-17): H3 resampling policy — measured, not assumed

**Finding (plan *verify* 17, synthetic 5 × 5 spike DSM, GDAL 3.10.3).**
Reprojecting a single-cell 25.0 m spike over the production parameter set
(`-te/-tr/-tap`, 1 m → 1 m):

- `-r max`: the output value set is exactly `{−32767, 18, 25}` — the peak
  survives **exactly**, no values are invented, nodata cells stay nodata
  (they never contribute to a maximum and are never filled).
- `-r bilinear`: the spike averages out to 22.74 m in its own cell — the
  reason bilinear is forbidden for the DSM (clearance would be optimistic
  over sharp features).

**Decision.** DTM warps with `-r bilinear` (smooth ground); DSM warps with
`-r max` (conservative: a peak may be over-represented by up to one footprint,
never shaved). nodata is explicit on both sides (`-srcnodata`/`-dstnodata`
−32767): no fake peaks from nodata, no filling of the tilted window's outside
corners. The plan's "write custom post-processing if needed" fallback is
**not** needed. The ignored test `hrdem_warp_policy_preserves_dsm_peaks_and_nodata`
locks the measured behaviour in; the live fetch test additionally asserts
DTM = DSM grid and dims within the printed pre-warp estimate
(`HrdemRequest::estimated_wgs84_dims`, a 45°-rotation upper bound for the
Lambert grid-tilted window).
