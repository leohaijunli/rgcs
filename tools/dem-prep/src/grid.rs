//! Gridding: point cloud → DTM/DSM rasters, regridded to WGS84.

use crate::error::DemError;
use crate::utm;

/// A raster in UTM metres (the native grid the LiDAR is delivered on).
pub struct UtmGrid {
    pub min_x: f64,
    pub min_y: f64,
    pub resolution_m: f64,
    pub width: usize,
    pub height: usize,
    /// Per-cell aggregates.
    pub dtm_sum: Vec<f64>,
    pub dtm_n: Vec<u32>,
    pub dsm_max: Vec<f32>,
}

impl UtmGrid {
    /// Empty grid covering the given UTM extent.
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64, resolution_m: f64) -> Self {
        let width = (((max_x - min_x) / resolution_m).ceil() as usize).max(1);
        let height = (((max_y - min_y) / resolution_m).ceil() as usize).max(1);
        Self {
            min_x,
            min_y,
            resolution_m,
            width,
            height,
            dtm_sum: vec![0.0; width * height],
            dtm_n: vec![0; width * height],
            dsm_max: vec![f32::NEG_INFINITY; width * height],
        }
    }

    fn index(&self, x: f64, y: f64) -> Option<usize> {
        let cx = ((x - self.min_x) / self.resolution_m).floor() as isize;
        let cy = ((y - self.min_y) / self.resolution_m).floor() as isize;
        if cx < 0 || cy < 0 || cx as usize >= self.width || cy as usize >= self.height {
            return None;
        }
        Some(cy as usize * self.width + cx as usize)
    }

    /// Fold one point into the aggregates.
    pub fn add(&mut self, p: crate::las::LasPoint) {
        if let Some(i) = self.index(p.x, p.y) {
            if p.class == crate::las::CLASS_GROUND {
                self.dtm_sum[i] += p.z;
                self.dtm_n[i] += 1;
            }
            let dsm = &mut self.dsm_max[i];
            if p.z as f32 > *dsm {
                *dsm = p.z as f32;
            }
        }
    }
}

/// A geographic (WGS84) raster: what the GCS reader consumes.
pub struct GeoRaster {
    pub width: usize,
    pub height: usize,
    /// North-up row-major, first row = max latitude.
    pub values: Vec<f32>,
    /// Geographic placement of pixel (0,0)'s corner (north-west): consumed
    /// by main.rs for dem.json (single source of truth for the reader).
    pub west: f64,
    pub north: f64,
}

/// Resample a UTM aggregate grid onto a WGS84 regular grid.
///
/// `aggregate` maps a UTM cell to a value (`None` = void). The output grid
/// spans the tile's geographic bounds at `resolution_deg`; each output cell
/// samples the UTM grid bilinearly in metre space (inverse-project the cell
/// centre once).
///
/// # Errors
///
/// [`DemError::Projection`] when the inverse projection fails.
pub fn resample_to_wgs84(
    grid: &UtmGrid,
    resolution_deg: (f64, f64),
    aggregate: impl Fn(usize) -> Option<f64>,
    zone: u8,
) -> Result<GeoRaster, DemError> {
    // Geographic bounds: the **envelope of all four UTM corners**. A UTM
    // rectangle is rotated in geographic space by the meridian convergence,
    // so projecting only SW/NE (as this used to) clips the east/west edges
    // by ~7-8 m on a BC sheet.
    let max_x = grid.min_x + grid.width as f64 * grid.resolution_m;
    let max_y = grid.min_y + grid.height as f64 * grid.resolution_m;
    let corners = [
        utm::from_utm(grid.min_x, grid.min_y, zone)?,
        utm::from_utm(max_x, grid.min_y, zone)?,
        utm::from_utm(grid.min_x, max_y, zone)?,
        utm::from_utm(max_x, max_y, zone)?,
    ];
    let west = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
    let east = corners
        .iter()
        .map(|c| c.1)
        .fold(f64::NEG_INFINITY, f64::max);
    let south = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
    let north = corners
        .iter()
        .map(|c| c.0)
        .fold(f64::NEG_INFINITY, f64::max);

    let width = (((east - west) / resolution_deg.1).ceil() as usize).max(2);
    let height = (((north - south) / resolution_deg.0).ceil() as usize).max(2);

    let mut values = vec![f32::NAN; width * height];
    for row in 0..height {
        for col in 0..width {
            // North-up: row 0 = max latitude. Sample the cell centre.
            let lat = north - (row as f64 + 0.5) * resolution_deg.0;
            let lon = west + (col as f64 + 0.5) * resolution_deg.1;
            let (x, y) = utm::to_utm_zone(lat, lon, zone);
            let cx = ((x - grid.min_x) / grid.resolution_m).floor() as isize;
            let cy = ((y - grid.min_y) / grid.resolution_m).floor() as isize;
            let value =
                if cx < 0 || cy < 0 || cx as usize >= grid.width || cy as usize >= grid.height {
                    None
                } else {
                    aggregate(cy as usize * grid.width + cx as usize)
                };
            if let Some(v) = value {
                values[row * width + col] = v as f32;
            }
        }
    }
    Ok(GeoRaster {
        width,
        height,
        values,
        west,
        north,
    })
}

/// DTM value of a cell: mean ground height; `None` when the cell saw no
/// ground points (a void — left as nodata, the reader rejects it rather than
/// extrapolating).
pub fn dtm_value(grid: &UtmGrid, i: usize) -> Option<f64> {
    let n = grid.dtm_n[i];
    (n > 0).then(|| grid.dtm_sum[i] / f64::from(n))
}

/// DSM value of a cell: highest hit; `None` when empty.
pub fn dsm_value(grid: &UtmGrid, i: usize) -> Option<f64> {
    let v = grid.dsm_max[i];
    (v > f32::NEG_INFINITY).then(|| f64::from(v))
}

/// Fill single-cell nodata holes in a geographic raster with the mean of the
/// (up to 8) neighbours that have data — a smoothing pass, not a void fill:
/// multi-cell voids stay void (the reader rejects them, ADR-004).
pub fn fill_single_cell_holes(raster: &mut GeoRaster) {
    let (w, h) = (raster.width, raster.height);
    let src = raster.values.clone();
    for row in 0..h {
        for col in 0..w {
            let i = row * w + col;
            if !src[i].is_nan() {
                continue;
            }
            let mut sum = 0.0f64;
            let mut n = 0u32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (r, c) = (row as i32 + dy, col as i32 + dx);
                    if r < 0 || c < 0 || r as usize >= h || c as usize >= w {
                        continue;
                    }
                    let v = src[r as usize * w + c as usize];
                    if !v.is_nan() {
                        sum += f64::from(v);
                        n += 1;
                    }
                }
            }
            if n >= 6 {
                raster.values[i] = (sum / f64::from(n)) as f32;
            }
        }
    }
}

/// Resample a **raster** UTM band (BC LiDAR DTM product tiles) onto the
/// WGS84 grid the runtime reader consumes. Unlike the point-cloud
/// [`resample_to_wgs84`], the aggregate is nearest-cell lookup of the
/// source values (the product is already a gridded DEM); voids stay void.
///
/// The output placement is the four-corner geographic envelope (the UTM
/// rectangle is rotated by the meridian convergence), same as the
/// point-cloud resampler.
///
/// # Errors
///
/// [`DemError::Projection`] when a corner fails the inverse projection.
pub fn resample_raster_to_wgs84(
    band: &crate::geotiff::ProjectedBand,
    resolution_deg: (f64, f64),
    zone: u8,
) -> Result<GeoRaster, DemError> {
    let max_e = band.min_e + band.width as f64 * band.res_m;
    let min_n = band.max_n - band.height as f64 * band.res_m;
    let corners = [
        crate::utm::from_utm(band.min_e, min_n, zone)?,
        crate::utm::from_utm(max_e, min_n, zone)?,
        crate::utm::from_utm(band.min_e, band.max_n, zone)?,
        crate::utm::from_utm(max_e, band.max_n, zone)?,
    ];
    let west = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
    let east = corners
        .iter()
        .map(|c| c.1)
        .fold(f64::NEG_INFINITY, f64::max);
    let south = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
    let north = corners
        .iter()
        .map(|c| c.0)
        .fold(f64::NEG_INFINITY, f64::max);

    let width = (((east - west) / resolution_deg.1).ceil() as usize).max(2);
    let height = (((north - south) / resolution_deg.0).ceil() as usize).max(2);
    let mut values = vec![f32::NAN; width * height];
    for row in 0..height {
        for col in 0..width {
            // North-up, sample the cell centre.
            let lat = north - (row as f64 + 0.5) * resolution_deg.0;
            let lon = west + (col as f64 + 0.5) * resolution_deg.1;
            let (x, y) = crate::utm::to_utm_zone(lat, lon, zone);
            let cx = ((x - band.min_e) / band.res_m).floor() as isize;
            let cy = ((band.max_n - y) / band.res_m).floor() as isize;
            if cx < 0 || cy < 0 || cx as usize >= band.width || cy as usize >= band.height {
                continue;
            }
            let v = band.values[cy as usize * band.width + cx as usize];
            if v.is_finite() {
                values[row * width + col] = v as f32;
            }
        }
    }
    Ok(GeoRaster {
        width,
        height,
        values,
        west,
        north,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::las::LasPoint;

    /// A 10x10 m tile at 1 m resolution with one ground point at the centre:
    /// the WGS84 resample must land it, not produce all-void.
    #[test]
    fn resample_lands_points() {
        let mut grid = UtmGrid::new(468_000.0, 5_388_000.0, 468_010.0, 5_388_010.0, 1.0);
        grid.add(LasPoint {
            x: 468_005.0,
            y: 5_388_005.0,
            z: 42.0,
            class: crate::las::CLASS_GROUND,
        });
        grid.add(LasPoint {
            x: 468_005.5,
            y: 5_388_005.5,
            z: 55.0,
            class: 1,
        });
        let (lat, lon) = utm::from_utm(468_005.0, 5_388_005.0, 10).expect("inv");
        println!("centre: {lat} {lon}");
        let geo = resample_to_wgs84(
            &grid,
            (
                1.0 / 111_320.0,
                1.0 / (111_320.0 * 48.65f64.to_radians().cos()),
            ),
            |i| dtm_value(&grid, i),
            10,
        )
        .expect("resample");
        let filled = geo.values.iter().filter(|v| !v.is_nan()).count();
        assert!(filled > 0, "all cells void — resampler missed the grid");
    }
}
