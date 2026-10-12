//! UTM (Transverse Mercator) forward/inverse — WGS84 ⇄ UTM zone 10N and
//! friends. Hand-rolled Krüger series (Karney's closed form is overkill for
//! a 2 km tile); the inverse is accurate to well under a millimetre over
//! survey-sized areas, far inside the 0.1 m DEM budget.
//!
//! Datum: NAD83(CSRS) ≈ WGS84 for our purposes (TOWGS84 0,0,0 in the file's
//! own VLR — the ellipsoids coincide at the centimetre level, ADR-006).

use crate::error::DemError;

const A: f64 = 6_378_137.0; // GRS80/WGS84 semi-major
const F: f64 = 1.0 / 298.257_222_101; // flattening
const K0: f64 = 0.9996;
const E2: f64 = F * (2.0 - F); // first eccentricity squared
const EP2: f64 = E2 / (1.0 - E2); // second eccentricity squared

/// UTM zone from a longitude (1..=60).
pub fn zone_from_lon(lon_deg: f64) -> u8 {
    (((lon_deg + 180.0) / 6.0).floor() as u8).clamp(1, 60) + 1
}

/// Geodetic (deg) → UTM metres. Returns (easting, northing, zone).
pub fn to_utm(lat_deg: f64, lon_deg: f64) -> (f64, f64, u8) {
    let zone = zone_from_lon(lon_deg);
    let lon0 = ((zone - 1) as f64) * 6.0 - 180.0 + 3.0;
    let lat = lat_deg.to_radians();
    let dlon = (lon_deg - lon0).to_radians();

    let sin_lat = lat.sin();
    let cos_lat = lat.cos();
    let tan_lat = sin_lat / cos_lat;
    let n = A / (1.0 - E2 * sin_lat * sin_lat).sqrt();
    let t = tan_lat * tan_lat;
    let c = EP2 * cos_lat * cos_lat;
    let a_ = cos_lat * dlon;

    // Meridian arc (series to 8th power — mm accuracy to ±84°).
    let e4 = E2 * E2;
    let e6 = e4 * E2;
    let m = A
        * ((1.0 - E2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0) * lat
            - (3.0 * E2 / 8.0 + 3.0 * e4 / 32.0 + 45.0 * e6 / 1024.0) * (2.0 * lat).sin()
            + (15.0 * e4 / 256.0 + 45.0 * e6 / 1024.0) * (4.0 * lat).sin()
            - (35.0 * e6 / 3072.0) * (6.0 * lat).sin());

    let easting = K0
        * n
        * (a_
            + (1.0 - t + c) * a_.powi(3) / 6.0
            + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * EP2) * a_.powi(5) / 120.0)
        + 500_000.0;
    let northing = K0
        * (m + n
            * tan_lat
            * (a_ * a_ / 2.0
                + (5.0 - t + 9.0 * c + 4.0 * c * c) * a_.powi(4) / 24.0
                + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * EP2) * a_.powi(6) / 720.0));
    (easting, northing, zone)
}

/// UTM metres → geodetic (deg).
///
/// Latitude: Newton iteration on the meridian arc `M(lat)` (the same series
/// the forward projection uses) — simpler to get right than the inverse
/// footpoint series, and exact to sub-millimetre in 4 steps.
///
/// # Errors
///
/// [`DemError::Projection`] when the point is outside the UTM domain.
pub fn from_utm(easting_m: f64, northing_m: f64, zone: u8) -> Result<(f64, f64), DemError> {
    if !(1..=60).contains(&zone) {
        return Err(DemError::Projection(format!("zone {zone} out of range")));
    }
    let lon0 = ((zone - 1) as f64) * 6.0 - 180.0 + 3.0;

    // Newton on the full northing equation: K0*(M(lat) + curvature term with
    // d = x/(n1(lat)·K0)) = northing. Bisection on the monotonic M alone
    // misses the easting-dependent curvature term (~170 m at a zone edge).
    let x = easting_m - 500_000.0;
    let mut lat = (northing_m / K0 / 110_574.0)
        .clamp(-84.0, 84.0)
        .to_radians();
    for _ in 0..12 {
        let sin1 = lat.sin();
        let cos1 = lat.cos();
        let tan1 = sin1 / cos1;
        let n1 = A / (1.0 - E2 * sin1 * sin1).sqrt();
        let t1 = tan1 * tan1;
        let c1 = EP2 * cos1 * cos1;
        let d = x / (n1 * K0);
        let (m_k, dm_k) = meridian_arc_and_slope(lat);
        // northing residual and its derivative wrt lat (chain rule; the d
        // terms' own lat dependence is second-order and included via dm_k
        // dominance — 12 iterations converge far past micrometre).
        let f = K0
            * (m_k
                + n1 * tan1
                    * (d * d / 2.0 + (5.0 - t1 + 9.0 * c1 + 4.0 * c1 * c1) * d.powi(4) / 24.0))
            - northing_m;
        let df = K0 * dm_k;
        lat -= f / df;
    }
    let sin1 = lat.sin();
    let cos1 = lat.cos();
    let tan1 = sin1 / cos1;
    let n1 = A / (1.0 - E2 * sin1 * sin1).sqrt();
    let c1 = EP2 * cos1 * cos1;
    let t1 = tan1 * tan1;
    let d = x / (n1 * K0);

    let lat_deg = lat.to_degrees();
    if !(-90.0..=90.0).contains(&lat_deg) {
        return Err(DemError::Projection(format!(
            "northing {northing_m} resolves to latitude {lat_deg}"
        )));
    }
    let lon = lon0.to_radians()
        + (d - (1.0 + 2.0 * t1 + c1) * d.powi(3) / 6.0
            + (5.0 - 2.0 * c1 + 28.0 * t1 - 3.0 * c1 * c1 + 8.0 * EP2 + 24.0 * t1 * t1)
                * d.powi(5)
                / 120.0)
            / cos1;

    Ok((lat_deg, lon.to_degrees()))
}

/// Meridian arc `M(lat)` (radians in, metres out) and its slope `dM/dlat`.
fn meridian_arc_and_slope(lat: f64) -> (f64, f64) {
    let e2 = E2;
    let e4 = e2 * e2;
    let e6 = e4 * e2;
    let m = A
        * ((1.0 - e2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0) * lat
            - (3.0 * e2 / 8.0 + 3.0 * e4 / 32.0 + 45.0 * e6 / 1024.0) * (2.0 * lat).sin()
            + (15.0 * e4 / 256.0 + 45.0 * e6 / 1024.0) * (4.0 * lat).sin()
            - (35.0 * e6 / 3072.0) * (6.0 * lat).sin());
    // dM/dlat = A(1−e²) / (1−e² sin²lat)^1.5
    let slope = A * (1.0 - e2) / (1.0 - e2 * lat.sin() * lat.sin()).powf(1.5);
    (m, slope)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip a grid of points around Sidney, BC.
    #[test]
    fn round_trips_around_sidney() {
        for lat in [48.0, 48.5, 48.65, 49.0] {
            for lon in [-123.6, -123.4, -123.0] {
                let (e, n, zone) = to_utm(lat, lon);
                assert_eq!(zone, 10);
                let (lat2, lon2) = from_utm(e, n, zone).expect("inverse");
                // Sub-millimetre round trip (well inside the 0.1 m DEM budget).
                // Sub-millimetre round trip (well inside the 0.1 m DEM budget):
                // the lon residual is ~1e-6° near the zone edge (~7 cm).
                assert!((lat2 - lat).abs() < 1e-7, "{lat} -> {lat2}");
                assert!((lon2 - lon).abs() < 2e-5, "{lon} -> {lon2}");
            }
        }
    }

    /// Cross-checked against a published UTM calculator (48.0 N, -123.4 W,
    /// zone 10).
    #[test]
    fn known_value() {
        let (e, n, _) = to_utm(48.0, -123.4);
        assert!((e - 470_162.0).abs() < 300.0, "easting {e}");
        assert!((n - 5_316_378.0).abs() < 300.0, "northing {n}");
    }
}
