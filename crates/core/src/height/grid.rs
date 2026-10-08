//! Regular-grid geoid model (ADR-006 groundwork).
//!
//! [`GeoidGridModel`] looks up the geoid undulation `N` from one regular
//! latitude/longitude grid per [`HeightDatum`] and bilinearly interpolates
//! between the four surrounding nodes. Grids are row-major with latitude
//! increasing first (`index = lat_index * nlon + lon_index`).
//!
//! Coverage is explicit: a query outside a grid — or a datum with no grid —
//! returns [`HeightError`] rather than extrapolating. ADR-004 treats DEM voids
//! the same way, and a silently extrapolated undulation would bias every height
//! near the survey edge.
//!
//! The real EGM96 / CGG2013 grids are loaded in Phase 3 (the COG/GeoTIFF reader
//! is ADR-004's job); this module only implements the interpolation and the
//! `GeoidModel` surface so the datum chain can be tested now.

use std::collections::BTreeMap;

use super::{GeoidModel, HeightDatum, HeightError};

/// Shape and placement of a [`GeoidGrid`].
///
/// Kept as plain public fields so a grid file reader (Phase 3) can hand over
/// the header it parsed without a long positional constructor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoidGridSpec {
    /// Datum the values describe.
    pub datum: HeightDatum,
    /// Latitude of the first row (bottom/least latitude), degrees.
    pub lat0_deg: f64,
    /// Longitude of the first column (west/least longitude), degrees.
    pub lon0_deg: f64,
    /// Row spacing in degrees; must be positive.
    pub lat_step_deg: f64,
    /// Column spacing in degrees; must be positive.
    pub lon_step_deg: f64,
    /// Number of rows (>= 2).
    pub nlat: usize,
    /// Number of columns (>= 2).
    pub nlon: usize,
}

/// A regular latitude/longitude grid of undulation values, in meters.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoidGrid {
    spec: GeoidGridSpec,
    /// Row-major undulation values, `nlat * nlon` finite entries.
    values: Vec<f64>,
}

impl GeoidGrid {
    /// Build a grid, validating shape, spacing and values.
    ///
    /// # Errors
    ///
    /// Returns [`HeightError::InvalidGrid`] if `nlat`/`nlon` is below 2, a step
    /// is not positive and finite, `values.len() != nlat * nlon`, or any value
    /// is not finite.
    pub fn new(spec: GeoidGridSpec, values: Vec<f64>) -> Result<Self, HeightError> {
        let GeoidGridSpec {
            datum,
            lat0_deg,
            lon0_deg,
            lat_step_deg,
            lon_step_deg,
            nlat,
            nlon,
        } = spec;
        if nlat < 2 || nlon < 2 {
            return Err(HeightError::InvalidGrid(format!(
                "{datum:?}: need at least 2x2 nodes, got {nlat}x{nlon}"
            )));
        }
        if !lat_step_deg.is_finite() || lat_step_deg <= 0.0 {
            return Err(HeightError::InvalidGrid(format!(
                "{datum:?}: latitude step must be positive, got {lat_step_deg}"
            )));
        }
        if !lon_step_deg.is_finite() || lon_step_deg <= 0.0 {
            return Err(HeightError::InvalidGrid(format!(
                "{datum:?}: longitude step must be positive, got {lon_step_deg}"
            )));
        }
        if !lat0_deg.is_finite() || !lon0_deg.is_finite() {
            return Err(HeightError::InvalidGrid(format!(
                "{datum:?}: origin must be finite, got ({lat0_deg}, {lon0_deg})"
            )));
        }
        if values.len() != nlat * nlon {
            return Err(HeightError::InvalidGrid(format!(
                "{datum:?}: expected {} values, got {}",
                nlat * nlon,
                values.len()
            )));
        }
        if let Some(bad) = values.iter().position(|v| !v.is_finite()) {
            return Err(HeightError::InvalidGrid(format!(
                "{datum:?}: value {bad} is not finite"
            )));
        }
        Ok(Self { spec, values })
    }

    /// Build a constant grid; convenient for tests and flat reference models.
    pub fn constant(spec: GeoidGridSpec, undulation_m: f64) -> Result<Self, HeightError> {
        Self::new(spec, vec![undulation_m; spec.nlat * spec.nlon])
    }

    /// Datum this grid describes.
    pub fn datum(&self) -> HeightDatum {
        self.spec.datum
    }

    /// Shape and placement of this grid.
    pub fn spec(&self) -> GeoidGridSpec {
        self.spec
    }

    /// Grid extent as `(min_lat, min_lon, max_lat, max_lon)` in degrees.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let s = self.spec;
        (
            s.lat0_deg,
            s.lon0_deg,
            s.lat0_deg + s.lat_step_deg * (s.nlat - 1) as f64,
            s.lon0_deg + s.lon_step_deg * (s.nlon - 1) as f64,
        )
    }

    /// Undulation at a node, for tests and inspection.
    fn node(&self, i: usize, j: usize) -> f64 {
        self.values[i * self.spec.nlon + j]
    }

    /// Interpolate the undulation at `lat`/`lon`, or an error outside coverage.
    fn undulation(&self, lat_deg: f64, lon_deg: f64) -> Result<f64, HeightError> {
        if !lat_deg.is_finite() || !lon_deg.is_finite() {
            return Err(self.unavailable(lat_deg, lon_deg));
        }
        let (min_lat, min_lon, max_lat, max_lon) = self.bounds();
        // Tolerate a hair outside the corners (grid files store rounded bounds).
        const EPS_DEG: f64 = 1e-9;
        if lat_deg < min_lat - EPS_DEG
            || lat_deg > max_lat + EPS_DEG
            || lon_deg < min_lon - EPS_DEG
            || lon_deg > max_lon + EPS_DEG
        {
            return Err(self.unavailable(lat_deg, lon_deg));
        }

        let s = self.spec;
        let fi = ((lat_deg - s.lat0_deg) / s.lat_step_deg).clamp(0.0, (s.nlat - 1) as f64);
        let fj = ((lon_deg - s.lon0_deg) / s.lon_step_deg).clamp(0.0, (s.nlon - 1) as f64);
        let i0 = (fi.floor() as usize).min(s.nlat - 2);
        let j0 = (fj.floor() as usize).min(s.nlon - 2);
        let ti = (fi - i0 as f64).clamp(0.0, 1.0);
        let tj = (fj - j0 as f64).clamp(0.0, 1.0);

        let v00 = self.node(i0, j0);
        let v01 = self.node(i0, j0 + 1);
        let v10 = self.node(i0 + 1, j0);
        let v11 = self.node(i0 + 1, j0 + 1);
        let south = v00 + (v01 - v00) * tj;
        let north = v10 + (v11 - v10) * tj;
        Ok(south + (north - south) * ti)
    }

    fn unavailable(&self, lat: f64, lon: f64) -> HeightError {
        HeightError::UndulationUnavailable {
            datum: self.spec.datum,
            lat,
            lon,
        }
    }
}

/// A [`GeoidModel`] backed by one [`GeoidGrid`] per datum.
#[derive(Debug, Clone)]
pub struct GeoidGridModel {
    name: String,
    grids: BTreeMap<HeightDatum, GeoidGrid>,
}

impl GeoidGridModel {
    /// Create an empty model (no datum covered yet).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            grids: BTreeMap::new(),
        }
    }

    /// Add or replace the grid for its datum. Returns `self` for chaining.
    pub fn with_grid(mut self, grid: GeoidGrid) -> Self {
        self.grids.insert(grid.datum(), grid);
        self
    }

    /// Whether a grid is loaded for `datum`.
    pub fn covers(&self, datum: HeightDatum) -> bool {
        self.grids.contains_key(&datum)
    }
}

impl GeoidModel for GeoidGridModel {
    fn name(&self) -> &str {
        &self.name
    }

    fn undulation_m(
        &self,
        latitude_deg: f64,
        longitude_deg: f64,
        datum: HeightDatum,
    ) -> Result<f64, HeightError> {
        let grid = self.grids.get(&datum).ok_or_else(|| {
            HeightError::ModelNotLoaded(format!("{}: no {datum:?} grid", self.name))
        })?;
        grid.undulation(latitude_deg, longitude_deg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::Height;

    /// 3 rows x 4 cols starting at (10, 20) with 1 degree steps.
    fn spec() -> GeoidGridSpec {
        GeoidGridSpec {
            datum: HeightDatum::AmslEgm96,
            lat0_deg: 10.0,
            lon0_deg: 20.0,
            lat_step_deg: 1.0,
            lon_step_deg: 1.0,
            nlat: 3,
            nlon: 4,
        }
    }

    /// N(i, j) = 2*i + 3*j.
    fn ramp_grid() -> GeoidGrid {
        let s = spec();
        let mut values = Vec::with_capacity(s.nlat * s.nlon);
        for i in 0..s.nlat {
            for j in 0..s.nlon {
                values.push(2.0 * i as f64 + 3.0 * j as f64);
            }
        }
        GeoidGrid::new(s, values).expect("valid grid")
    }

    fn flat_spec(nlat: usize, nlon: usize) -> GeoidGridSpec {
        GeoidGridSpec {
            datum: HeightDatum::AmslEgm96,
            lat0_deg: 45.0,
            lon0_deg: -125.0,
            lat_step_deg: 0.25,
            lon_step_deg: 0.25,
            nlat,
            nlon,
        }
    }

    #[test]
    fn constant_grid_returns_its_value() {
        let grid = GeoidGrid::constant(flat_spec(4, 5), 7.5).expect("grid");
        assert!((grid.undulation(45.3, -124.4).unwrap() - 7.5).abs() < 1e-12);
    }

    #[test]
    fn bilinear_interpolation_is_exact_for_a_linear_field() {
        let grid = ramp_grid();
        // Interior point: lat_index 0.5, lon_index 1.25 -> 2*0.5 + 3*1.25 = 4.75.
        let n = grid.undulation(10.5, 21.25).unwrap();
        assert!((n - 4.75).abs() < 1e-12, "N = {n}");
    }

    #[test]
    fn corner_and_edge_nodes_are_reproduced() {
        let grid = ramp_grid();
        assert!((grid.undulation(10.0, 20.0).unwrap() - 0.0).abs() < 1e-12);
        assert!((grid.undulation(12.0, 23.0).unwrap() - (4.0 + 9.0)).abs() < 1e-12);
        // Exactly on the far edge (clamped indices).
        assert!((grid.undulation(10.0, 23.0).unwrap() - 9.0).abs() < 1e-12);
    }

    #[test]
    fn outside_coverage_is_an_error_never_extrapolated() {
        let grid = ramp_grid();
        let err = grid.undulation(9.5, 20.0).unwrap_err();
        assert!(matches!(
            err,
            HeightError::UndulationUnavailable {
                datum: HeightDatum::AmslEgm96,
                ..
            }
        ));
    }

    #[test]
    fn missing_datum_reports_which_model_is_not_loaded() {
        let model = GeoidGridModel::new("test").with_grid(ramp_grid());
        assert!(model.covers(HeightDatum::AmslEgm96));
        assert!(!model.covers(HeightDatum::OrthometricCgvd2013));
        let err = model
            .undulation_m(11.0, 21.0, HeightDatum::OrthometricCgvd2013)
            .unwrap_err();
        assert!(matches!(err, HeightError::ModelNotLoaded(_)));
    }

    #[test]
    fn invalid_grids_are_rejected() {
        // nlat < 2
        let e = GeoidGrid::new(GeoidGridSpec { nlat: 1, ..spec() }, vec![0.0; 4]);
        assert!(matches!(e, Err(HeightError::InvalidGrid(_))));

        // non-positive step
        let e = GeoidGrid::new(
            GeoidGridSpec {
                lat_step_deg: 0.0,
                ..spec()
            },
            vec![0.0; 12],
        );
        assert!(matches!(e, Err(HeightError::InvalidGrid(_))));

        // value count mismatch
        let e = GeoidGrid::new(spec(), vec![0.0; 3]);
        assert!(matches!(e, Err(HeightError::InvalidGrid(_))));

        // non-finite value
        let mut values = vec![0.0; 12];
        values[5] = f64::NAN;
        let e = GeoidGrid::new(spec(), values);
        assert!(matches!(e, Err(HeightError::InvalidGrid(_))));
    }

    #[test]
    fn datum_chain_uses_the_interpolated_undulation() {
        let model = GeoidGridModel::new("test")
            .with_grid(GeoidGrid::constant(flat_spec(5, 5), 20.0).unwrap());
        let amsl = Height::new(HeightDatum::AmslEgm96, 100.0);
        let ellipsoid = amsl
            .convert(HeightDatum::EllipsoidWgs84, 45.5, -124.5, &model)
            .expect("convert");
        assert!((ellipsoid.meters() - 120.0).abs() < 1e-9);

        // Round trip back to AMSL must land on the original height.
        let back = ellipsoid
            .convert(HeightDatum::AmslEgm96, 45.5, -124.5, &model)
            .expect("convert back");
        assert!((back.meters() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn conversion_outside_coverage_fails_closed() {
        let model = GeoidGridModel::new("test")
            .with_grid(GeoidGrid::constant(flat_spec(5, 5), 20.0).unwrap());
        let amsl = Height::new(HeightDatum::AmslEgm96, 100.0);
        let err = amsl
            .convert(HeightDatum::EllipsoidWgs84, 10.0, 10.0, &model)
            .unwrap_err();
        assert!(matches!(err, HeightError::UndulationUnavailable { .. }));
    }
}
