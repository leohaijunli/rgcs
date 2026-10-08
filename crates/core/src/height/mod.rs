//! Datum-tagged heights (ADR-006).
//!
//! Three datums are in use:
//! - [`HeightDatum::EllipsoidWgs84`] — ellipsoid height `h` (Cesium).
//! - [`HeightDatum::AmslEgm96`] — AMSL as reported by PX4
//!   (`MAV_FRAME_GLOBAL`), computed `h − N(EGM96)`.
//! - [`HeightDatum::OrthometricCgvd2013`] — CGVD2013 orthometric `H`
//!   (BC LiDAR), with `H + N(CGG2013) = h`.
//!
//! Absolute heights must be represented as [`Height`], never as bare `f64`.
//! All datum conversions live in this module. The grid interpolation is in
//! [`grid`]; the real EGM96 / CGG2013 grids are loaded in Phase 3.

pub mod grid;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

/// Height datum tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HeightDatum {
    /// WGS84 ellipsoid height `h`.
    EllipsoidWgs84,
    /// AMSL (PX4 `MAV_FRAME_GLOBAL`): `h − N(EGM96)`.
    AmslEgm96,
    /// CGVD2013 orthometric height `H` (BC LiDAR): `H + N(CGG2013) = h`.
    OrthometricCgvd2013,
}

/// A datum-tagged absolute height in meters.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Height {
    pub datum: HeightDatum,
    pub meters: f64,
}

impl Height {
    /// Create a tagged height.
    pub fn new(datum: HeightDatum, meters: f64) -> Self {
        Self { datum, meters }
    }

    /// Height value in meters.
    pub fn meters(&self) -> f64 {
        self.meters
    }

    /// Datum tag of this height.
    pub fn datum(&self) -> HeightDatum {
        self.datum
    }

    /// Convert to `target` datum using the given geoid model.
    ///
    /// The geoid undulation varies with horizontal position, so the WGS84
    /// latitude/longitude of the measurement is required.
    ///
    /// Implementation lands in Phase 3 (CGG2013/EGM96 grid interpolation);
    /// the signature and error surface are final.
    pub fn convert(
        &self,
        target: HeightDatum,
        latitude_deg: f64,
        longitude_deg: f64,
        model: &dyn GeoidModel,
    ) -> Result<Height, HeightError> {
        if target == self.datum {
            return Ok(*self);
        }
        let target_ellipsoid = self.to_ellipsoid(latitude_deg, longitude_deg, model)?;
        let meters = match target {
            HeightDatum::EllipsoidWgs84 => target_ellipsoid,
            HeightDatum::AmslEgm96 => {
                let n = model.undulation_m(latitude_deg, longitude_deg, HeightDatum::AmslEgm96)?;
                target_ellipsoid - n
            }
            HeightDatum::OrthometricCgvd2013 => {
                let n = model.undulation_m(
                    latitude_deg,
                    longitude_deg,
                    HeightDatum::OrthometricCgvd2013,
                )?;
                target_ellipsoid - n
            }
        };
        Ok(Height::new(target, meters))
    }

    /// Convert to ellipsoid height `h`.
    fn to_ellipsoid(
        self,
        latitude_deg: f64,
        longitude_deg: f64,
        model: &dyn GeoidModel,
    ) -> Result<f64, HeightError> {
        match self.datum {
            HeightDatum::EllipsoidWgs84 => Ok(self.meters),
            HeightDatum::AmslEgm96 | HeightDatum::OrthometricCgvd2013 => {
                let n = model.undulation_m(latitude_deg, longitude_deg, self.datum)?;
                Ok(self.meters + n)
            }
        }
    }
}

/// Source of geoid undulation values (CGG2013, EGM96 grids in Phase 3).
pub trait GeoidModel: Send + Sync {
    /// Human-readable model identifier.
    fn name(&self) -> &str;

    /// Geoid undulation `N` (meters) at the given WGS84 latitude/longitude
    /// for the given datum. Positive N means the datum surface lies above the
    /// ellipsoid.
    fn undulation_m(
        &self,
        latitude_deg: f64,
        longitude_deg: f64,
        datum: HeightDatum,
    ) -> Result<f64, HeightError>;
}

pub use grid::{GeoidGrid, GeoidGridModel};

/// Errors from height conversion or geoid-grid construction.
#[derive(Debug, Error)]
pub enum HeightError {
    /// Geoid undulation unavailable for the datum at the requested location.
    #[error("geoid undulation unavailable for datum {datum:?} at ({lat:.6}, {lon:.6})")]
    UndulationUnavailable {
        datum: HeightDatum,
        lat: f64,
        lon: f64,
    },
    /// The geoid model is not loaded.
    #[error("geoid model not loaded: {0}")]
    ModelNotLoaded(String),
    /// A geoid grid is malformed (shape, spacing or a non-finite value).
    #[error("invalid geoid grid: {0}")]
    InvalidGrid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConstGeoid(f64, f64);

    impl GeoidModel for ConstGeoid {
        fn name(&self) -> &str {
            "test-const"
        }

        fn undulation_m(
            &self,
            latitude_deg: f64,
            longitude_deg: f64,
            datum: HeightDatum,
        ) -> Result<f64, HeightError> {
            let n = match datum {
                HeightDatum::AmslEgm96 => self.0,
                HeightDatum::OrthometricCgvd2013 => self.1,
                HeightDatum::EllipsoidWgs84 => {
                    return Err(HeightError::UndulationUnavailable {
                        datum,
                        lat: latitude_deg,
                        lon: longitude_deg,
                    })
                }
            };
            Ok(n)
        }
    }

    #[test]
    fn identity_conversion_is_free() {
        let h = Height::new(HeightDatum::AmslEgm96, 100.0);
        let model = ConstGeoid(20.0, 15.0);
        assert_eq!(
            h.convert(HeightDatum::AmslEgm96, 49.25, -123.0, &model)
                .unwrap(),
            h
        );
    }

    #[test]
    fn amsl_to_ellipsoid_adds_undulation() {
        let h = Height::new(HeightDatum::AmslEgm96, 123.4);
        let model = ConstGeoid(20.0, 15.0);
        let out = h
            .convert(HeightDatum::EllipsoidWgs84, 49.25, -123.0, &model)
            .expect("convert");
        assert!((out.meters() - 143.4).abs() < 1e-9);
        assert_eq!(out.datum(), HeightDatum::EllipsoidWgs84);
    }

    #[test]
    fn ellipsoid_to_cgvd2013_subtracts_undulation() {
        let h = Height::new(HeightDatum::EllipsoidWgs84, 200.0);
        let model = ConstGeoid(20.0, 15.0);
        let out = h
            .convert(HeightDatum::OrthometricCgvd2013, 49.25, -123.0, &model)
            .expect("convert");
        assert!((out.meters() - 185.0).abs() < 1e-9);
        assert_eq!(out.datum(), HeightDatum::OrthometricCgvd2013);
    }
}
