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
) -> Result<GeoRaster, DemError> {
    // Tile's geographic bounds: project the four UTM corners.
    let (lat0, lon0) = utm::from_utm(grid.min_x, grid.min_y, 10)?;
    let (lat1, lon1) = utm::from_utm(
        grid.min_x + grid.width as f64 * grid.resolution_m,
        grid.min_y + grid.height as f64 * grid.resolution_m,
        10,
    )?;
    let west = lon0.min(lon1);
    let east = lon0.max(lon1);
    let south = lat0.min(lat1);
    let north = lat0.max(lat1);

    let width = (((east - west) / resolution_deg.1).ceil() as usize).max(2);
    let height = (((north - south) / resolution_deg.0).ceil() as usize).max(2);

    let mut values = vec![f32::NAN; width * height];
    for row in 0..height {
        for col in 0..width {
            // North-up: row 0 = max latitude. Sample the cell centre.
            let lat = north - (row as f64 + 0.5) * resolution_deg.0;
            let lon = west + (col as f64 + 0.5) * resolution_deg.1;
            let (x, y, _) = utm::to_utm(lat, lon);
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
        )
        .expect("resample");
        let filled = geo.values.iter().filter(|v| !v.is_nan()).count();
        assert!(filled > 0, "all cells void — resampler missed the grid");
    }
}
