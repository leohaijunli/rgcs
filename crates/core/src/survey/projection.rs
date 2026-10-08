//! Local tangent-plane projection for pattern geometry.
//!
//! Survey and calibration patterns are easier to reason about in metres than in
//! degrees. This is an equirectangular ("flat earth") projection around an
//! origin: exact enough over a survey block (sub-metre at a few kilometres) and
//! dependency-free. WS-C can swap in UTM 10 when the `geo` crate lands; the
//! API here is the seam.

use crate::plan::GeoPoint;

/// WGS84 semi-major axis (metres).
const SEMI_MAJOR_M: f64 = 6_378_137.0;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// Local east/north projection around an origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalProjection {
    lat0_deg: f64,
    lon0_deg: f64,
    cos_lat0: f64,
}

impl LocalProjection {
    /// Create a projection centred on `origin`.
    pub fn new(origin: GeoPoint) -> Self {
        Self {
            lat0_deg: origin.latitude_deg,
            lon0_deg: origin.longitude_deg,
            cos_lat0: (origin.latitude_deg * DEG_TO_RAD).cos(),
        }
    }

    /// Latitude/longitude to local `(east_m, north_m)`.
    pub fn to_local(&self, p: GeoPoint) -> (f64, f64) {
        let east = (p.longitude_deg - self.lon0_deg) * DEG_TO_RAD * SEMI_MAJOR_M * self.cos_lat0;
        let north = (p.latitude_deg - self.lat0_deg) * DEG_TO_RAD * SEMI_MAJOR_M;
        (east, north)
    }

    /// Local `(east_m, north_m)` back to latitude/longitude.
    pub fn to_geo(&self, east_m: f64, north_m: f64) -> GeoPoint {
        let latitude_deg = self.lat0_deg + north_m / (SEMI_MAJOR_M * DEG_TO_RAD);
        let longitude_deg = if self.cos_lat0.abs() < f64::EPSILON {
            self.lon0_deg
        } else {
            self.lon0_deg + east_m / (SEMI_MAJOR_M * self.cos_lat0 * DEG_TO_RAD)
        };
        GeoPoint::new(latitude_deg, longitude_deg)
    }
}

/// Rotate a local `(east, north)` vector by `azimuth_deg` (clockwise from north).
pub fn rotate(east_m: f64, north_m: f64, azimuth_deg: f64) -> (f64, f64) {
    let a = azimuth_deg * DEG_TO_RAD;
    let (sin, cos) = a.sin_cos();
    (east_m * cos + north_m * sin, -east_m * sin + north_m * cos)
}

/// Unit vector for `azimuth_deg` (clockwise from north) as `(east, north)`.
pub fn unit(azimuth_deg: f64) -> (f64, f64) {
    rotate(0.0, 1.0, azimuth_deg)
}
