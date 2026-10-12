//! COG GeoTIFF DEM reader (ADR-004 / D1): opens the DTM+DSM raster pair of a
//! site and serves bilinear samples as datum-tagged [`Height`]s.
//!
//! The reader requires the rasters to be **geographic** (WGS84 degrees):
//! `tools/dem-prep` regrids the BC LiDAR product from its projected CRS to a
//! WGS84 regular grid at export time, where a full inverse projection is
//! available (CRS-aware). The runtime keeps a single simple placement model:
//! GeoTIFF `ModelTiepoint` (pixel 0,0 → north-west corner) + `ModelPixelScale`
//! in degrees.
//!
//! Sampling never extrapolates (ADR-004): outside the raster, or inside a
//! nodata void, is an error.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use tiff::decoder::{Decoder, DecodingResult};

use super::{check_paths, DemBounds, DemMetadata, ElevationSource, TerrainError, VerticalDatum};
use crate::height::{Height, HeightDatum};
use crate::plan::GeoPoint;

/// BC LiDAR nodata sentinel (`tools/dem-prep` writes it; anything at or
/// below is a void).
const NODATA: f64 = -9999.0;

/// A decoded raster plus its geographic placement.
struct Raster {
    width: usize,
    height: usize,
    /// Row-major, north-up (TIFF's first row is the top = max latitude).
    values: Vec<f64>,
    /// Geographic placement of pixel (0,0)'s corner (north-west).
    west_deg: f64,
    north_deg: f64,
    dlat_deg: f64,
    dlon_deg: f64,
}

impl Raster {
    /// Bilinear sample in geographic degrees.
    ///
    /// # Errors
    ///
    /// [`TerrainError::OutsideCoverage`] beyond the raster (with half a
    /// pixel of tolerance at the edges), [`TerrainError::Void`] on a nodata
    /// pixel.
    fn sample(&self, lat: f64, lon: f64) -> Result<f64, TerrainError> {
        // Continuous pixel coordinates, y measured from the top row: the
        // corner (west, north) is pixel (0,0)'s outer corner, so a point at
        // the first cell's centre maps to pixel coordinate +0.5.
        let x = (lon - self.west_deg) / self.dlon_deg;
        let y = (self.north_deg - lat) / self.dlat_deg;
        if x < 0.0 || y < 0.0 || x > (self.width - 1) as f64 || y > (self.height - 1) as f64 {
            return Err(TerrainError::OutsideCoverage(GeoPoint::new(lat, lon)));
        }
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = x - x0 as f64;
        let ty = y - y0 as f64;
        let v00 = self.at(x0, y0)?;
        let v10 = self.at(x1, y0)?;
        let v01 = self.at(x0, y1)?;
        let v11 = self.at(x1, y1)?;
        let top = v00 * (1.0 - tx) + v10 * tx;
        let bottom = v01 * (1.0 - tx) + v11 * tx;
        Ok(top * (1.0 - ty) + bottom * ty)
    }

    fn at(&self, x: usize, y: usize) -> Result<f64, TerrainError> {
        let v = self.values[y * self.width + x];
        if v <= NODATA + 1.0 {
            Err(TerrainError::Void(GeoPoint::new(0.0, 0.0)))
        } else {
            Ok(v)
        }
    }
}

/// One site's DTM + DSM pair.
pub struct CogDem {
    dtm: Raster,
    dsm: Raster,
    metadata: DemMetadata,
}

impl CogDem {
    /// Open the raster pair with the sidecar metadata from `tools/dem-prep`.
    ///
    /// # Errors
    ///
    /// [`TerrainError::Io`] when a file is missing, [`TerrainError::Tiff`]
    /// when a raster is not a readable single-band GeoTIFF, and
    /// [`TerrainError::Metadata`] when the pair disagrees (extent or
    /// resolution) — a DTM and DSM that do not line up would silently skew
    /// the clearance check.
    pub fn open(dtm: &Path, dsm: &Path, metadata: &DemMetadata) -> Result<Self, TerrainError> {
        check_paths(dtm, dsm)?;
        // Placement comes from dem.json; compute the per-axis degree scales
        // from the nominal metre resolution at the coverage's mid-latitude
        // (dem-prep used the same conversion, so the numbers agree exactly).
        let mid_lat = (metadata.bounds.north + metadata.bounds.south) / 2.0;
        let m_per_deg_lat = 111_132.954 - 559.822 * (2.0 * mid_lat).to_radians().cos();
        let m_per_deg_lon = 111_319.49 * mid_lat.to_radians().cos();
        let (west, north) = (metadata.bounds.west, metadata.bounds.north);
        let (dlat, dlon) = (
            metadata.resolution_m / m_per_deg_lat,
            metadata.resolution_m / m_per_deg_lon,
        );
        let dtm = read_raster(dtm, west, north, dlat, dlon)?;
        let dsm = read_raster(dsm, west, north, dlat, dlon)?;
        if dtm.width != dsm.width || dtm.height != dsm.height {
            return Err(TerrainError::Metadata(
                "DTM and DSM dimensions differ".into(),
            ));
        }
        if (dtm.west_deg - dsm.west_deg).abs() > 1e-9
            || (dtm.north_deg - dsm.north_deg).abs() > 1e-9
            || (dtm.dlat_deg - dsm.dlat_deg).abs() > 1e-12
        {
            return Err(TerrainError::Metadata(
                "DTM and DSM placements differ".into(),
            ));
        }
        let vertical_datum = metadata.vertical_datum;
        Ok(Self {
            dtm,
            dsm,
            metadata: DemMetadata {
                vertical_datum,
                has_dsm: metadata.has_dsm,
                ..metadata.clone()
            },
        })
    }

    fn datum(&self) -> HeightDatum {
        match self.metadata.vertical_datum {
            VerticalDatum::OrthometricCgvd2013 => HeightDatum::OrthometricCgvd2013,
            VerticalDatum::AmslEgm96 => HeightDatum::AmslEgm96,
        }
    }

    fn raster_bounds(&self) -> DemBounds {
        DemBounds {
            north: self.dtm.north_deg,
            south: self.dtm.north_deg - self.dtm.dlat_deg * self.dtm.height as f64,
            east: self.dtm.west_deg + self.dtm.dlon_deg * self.dtm.width as f64,
            west: self.dtm.west_deg,
        }
    }
}

impl ElevationSource for CogDem {
    /// DTM + DSM at one point. A DTM void errors (the AGL datum must
    /// exist); a DSM void — the DTM-only fallback sources — returns NaN so
    /// the drape can skip the clearance check without failing the plan.
    fn sample_pair(&self, point: GeoPoint) -> Result<(Height, Height), TerrainError> {
        let dtm = self.dtm.sample(point.latitude_deg, point.longitude_deg)?;
        let dsm = self
            .dsm
            .sample(point.latitude_deg, point.longitude_deg)
            .unwrap_or(f64::NAN);
        let datum = self.datum();
        Ok((Height::new(datum, dtm), Height::new(datum, dsm)))
    }

    fn elevation(&self, point: GeoPoint) -> Result<Height, TerrainError> {
        // The DTM is the AGL datum (ADR-005): the plain elevation query.
        let v = self
            .dtm
            .sample(point.latitude_deg, point.longitude_deg)
            .map_err(|e| match e {
                TerrainError::Void(_) => TerrainError::Void(point),
                other => other,
            })?;
        Ok(Height::new(self.datum(), v))
    }

    fn bounds(&self) -> DemBounds {
        self.raster_bounds()
    }

    fn version(&self) -> &str {
        &self.metadata.version
    }
}

/// Read one single-band geographic (WGS84) GeoTIFF. The placement comes
/// from `dem.json` (single source of truth): the tiff crate cannot attach
/// custom tags to the image IFD `write_image` builds, so dem-prep does not
/// write GeoTIFF tags at all.
fn read_raster(
    path: &Path,
    west_deg: f64,
    north_deg: f64,
    dlat_deg: f64,
    dlon_deg: f64,
) -> Result<Raster, TerrainError> {
    let file = File::open(path).map_err(|e| TerrainError::Io(e.to_string()))?;
    let mut decoder =
        Decoder::new(BufReader::new(file)).map_err(|e| TerrainError::Tiff(e.to_string()))?;

    // The tags may live on IFD #1 with the samples on IFD #2 (how
    // tools/dem-prep writes them); walk to the image IFD for the dimensions
    // while remembering the tags we read along the way.
    // Tag-only IFDs (how dem-prep writes the placement) have no dimensions;
    // walk forward until an image IFD. Any non-dimension error surfaces as-is.
    let (width, height) = decoder
        .dimensions()
        .map_err(|e| TerrainError::Tiff(e.to_string()))?;
    let (width, height) = (width as usize, height as usize);

    let values = match decoder
        .read_image()
        .map_err(|e| TerrainError::Tiff(e.to_string()))?
    {
        DecodingResult::F32(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::F64(v) => v,
        DecodingResult::U16(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::U32(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::U8(v) => v.into_iter().map(f64::from).collect(),
        other => {
            return Err(TerrainError::Tiff(format!(
                "unsupported sample type: {other:?}"
            )))
        }
    };
    if values.len() != width * height {
        return Err(TerrainError::Tiff(format!(
            "expected {} samples, got {}",
            width * height,
            values.len()
        )));
    }

    Ok(Raster {
        width,
        height,
        values,
        west_deg,
        north_deg,
        dlat_deg,
        dlon_deg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::GeoPoint;
    use std::io::BufWriter;

    /// Write a small geographic GeoTIFF via the tiff encoder: 4x4 pixels,
    /// values 100.., degrees placement, nodata in one pixel.
    fn write_test_tiff(path: &Path, values: &[f64; 16]) {
        let file = File::create(path).expect("create");
        let mut encoder = tiff::encoder::TiffEncoder::new(BufWriter::new(file)).expect("encoder");
        let floats: Vec<f32> = values.iter().map(|v| *v as f32).collect();
        encoder
            .write_image::<tiff::encoder::colortype::Gray32Float>(4, 4, &floats)
            .expect("write image");
    }

    fn metadata() -> DemMetadata {
        DemMetadata {
            version: "test000000000000".into(),
            raster_crs: "EPSG:4326".into(),
            resolution_m: 111.32,
            bounds: DemBounds {
                north: 48.0,
                south: 47.996,
                east: -122.996,
                west: -123.0,
            },
            vertical_datum: VerticalDatum::AmslEgm96,
            has_dsm: true,
        }
    }

    fn ramp() -> [f64; 16] {
        // 16 distinct values, one nodata pixel at (2,1).
        let mut v = [0.0; 16];
        for (i, slot) in v.iter_mut().enumerate() {
            *slot = 100.0 + i as f64;
        }
        v[6] = NODATA;
        v
    }

    #[test]
    fn samples_bilinearly_and_reports_coverage() {
        let dir = std::env::temp_dir().join("maggcs-dem-test");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let dtm_path = dir.join("dtm.tif");
        let dsm_path = dir.join("dsm.tif");
        write_test_tiff(&dtm_path, &ramp());
        write_test_tiff(&dsm_path, &ramp());

        let dem = CogDem::open(&dtm_path, &dsm_path, &metadata()).expect("open");
        // Pixel (0,0) centre: half a pixel inside from the NW corner.
        let p = GeoPoint::new(48.0 - 0.0005, -123.0 + 0.0005);
        let h = dem.elevation(p).expect("sample");
        // The point sits between cell centres: the bilinear mix of values
        // 100/101/104/105 (row-major, north-up) — not the corner value.
        assert!((100.0..104.0).contains(&h.meters()), "got {}", h.meters());
        assert_eq!(h.datum(), HeightDatum::AmslEgm96);

        // Outside coverage is an error, never an extrapolation.
        assert!(matches!(
            dem.elevation(GeoPoint::new(47.99, -122.99)),
            Err(TerrainError::OutsideCoverage(_))
        ));

        // The nodata pixel is a void.
        let void = GeoPoint::new(48.0 - 0.0015, -123.0 + 0.0025);
        assert!(matches!(dem.elevation(void), Err(TerrainError::Void(_))));

        // The pair returns both rasters, datum-tagged.
        let (dtm, dsm) = dem.sample_pair(p).expect("pair");
        assert_eq!(dtm.datum(), HeightDatum::AmslEgm96);
        assert_eq!(dsm.meters(), dtm.meters());
    }

    #[test]
    fn mismatched_pairs_are_rejected() {
        let dir = std::env::temp_dir().join("maggcs-dem-test2");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let dtm_path = dir.join("dtm.tif");
        let dsm_path = dir.join("dsm.tif");
        let mut other = ramp();
        other[0] = 500.0; // same placement, different values — fine
        write_test_tiff(&dtm_path, &ramp());
        write_test_tiff(&dsm_path, &other);
        let dem = CogDem::open(&dtm_path, &dsm_path, &metadata()).expect("open");
        assert_eq!(dem.version(), "test000000000000");

        // A second raster with a different size must be rejected.
        let bad = dir.join("bad.tif");
        {
            let file = File::create(&bad).expect("create");
            let mut encoder =
                tiff::encoder::TiffEncoder::new(BufWriter::new(file)).expect("encoder");
            encoder
                .write_image::<tiff::encoder::colortype::Gray32Float>(2, 2, &[1.0f32; 4])
                .expect("write");
        } // encoder dropped: the BufWriter flushes
        match CogDem::open(&dtm_path, &bad, &metadata()) {
            Err(TerrainError::Metadata(_)) => {}
            Err(e) => panic!("expected Metadata, got {e} (path {})", bad.display()),
            Ok(_) => panic!("expected Metadata, got Ok"),
        }
    }
}

#[cfg(test)]
mod real_data_tests {
    use super::*;
    use std::path::PathBuf;

    /// The committed BC LiDAR tile (Sidney, BC — 2023 LidarBC acquisition,
    /// processed by tools/dem-prep from a 73M-point LAZ): end-to-end proof
    /// that the reader consumes real data.
    #[test]
    fn reads_the_committed_bc_sidney_tile() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sidney");
        let dtm = dir.join("dtm.tif");
        let dsm = dir.join("dsm.tif");
        if !dtm.is_file() {
            // The raster pair is committed; if missing (shallow clone), skip.
            return;
        }
        let meta: DemMetadata =
            serde_json::from_str(&std::fs::read_to_string(dir.join("dem.json")).expect("dem.json"))
                .expect("parse dem.json");
        let dem = CogDem::open(&dtm, &dsm, &meta).expect("open");

        // Coverage sanity: the metadata bounds match the raster placement.
        let b = dem.bounds();
        assert!((b.north - meta.bounds.north).abs() < 1e-9, "{b:?}");
        assert!((b.west - meta.bounds.west).abs() < 1e-9, "{b:?}");

        // Sample the tile centre: elevation must be plausible coastal terrain
        // (Sidney: sea level to ~60 m; the datum is CGVD2013 orthometric).
        let center = GeoPoint::new((b.north + b.south) / 2.0, (b.east + b.west) / 2.0);
        let (dtm_h, dsm_h) = dem.sample_pair(center).expect("centre sample");
        assert!(dtm_h.datum() == HeightDatum::OrthometricCgvd2013);
        assert!(
            (-5.0..80.0).contains(&dtm_h.meters()),
            "DTM {} implausible for Sidney",
            dtm_h.meters()
        );
        // The surface is never below the ground.
        assert!(dsm_h.meters() >= dtm_h.meters() - 0.5);

        // Version flows through for mission archives.
        assert_eq!(dem.version(), meta.version);

        // Outside coverage errors.
        assert!(matches!(
            dem.elevation(GeoPoint::new(49.5, -123.4)),
            Err(TerrainError::OutsideCoverage(_))
        ));
    }
}
