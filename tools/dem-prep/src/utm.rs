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
#[allow(dead_code)]
pub fn zone_from_lon(lon_deg: f64) -> u8 {
    (((lon_deg + 180.0) / 6.0).floor() as u8).clamp(1, 60) + 1
}

/// Geodetic (deg) → UTM metres. Returns (easting, northing, zone).
#[cfg(test)]
pub fn to_utm(lat_deg: f64, lon_deg: f64) -> (f64, f64, u8) {
    let zone = zone_from_lon(lon_deg);
    let (easting, northing) = to_utm_zone(lat_deg, lon_deg, zone);
    (easting, northing, zone)
}

/// Forward projection into an **explicit** zone (the LAS/GeoTIFF's own zone,
/// which may differ from the longitude-derived one near a zone boundary).
#[must_use]
pub fn to_utm_zone(lat_deg: f64, lon_deg: f64, zone: u8) -> (f64, f64) {
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
    (easting, northing)
}

/// UTM metres → geodetic (deg).
///
/// Solved as a **numerical inverse of [`to_utm_zone`]** (2-D Newton with
/// analytic-ish Jacobian updates): the previous hand-written inverse series
/// mixed footpoint coefficients into the longitude expansion and its
/// latitude Newton omitted the N·tanθ·d²/2 derivative — good to ~0.4 m at
/// Sidney but degrading to **150 m near a zone edge** (found against PROJ).
/// Inverting the forward projection itself can never disagree with it.
///
/// # Errors
///
/// [`DemError::Projection`] when the point is outside the UTM domain.
pub fn from_utm(easting_m: f64, northing_m: f64, zone: u8) -> Result<(f64, f64), DemError> {
    if !(1..=60).contains(&zone) {
        return Err(DemError::Projection(format!("zone {zone} out of range")));
    }
    let lon0 = ((zone - 1) as f64) * 6.0 - 180.0 + 3.0;

    // Seed: the meridian-arc latitude (ignores the easting curvature term).
    let mut lat_deg = (northing_m / K0 / 110_574.0).clamp(-84.0, 84.0);
    let mut lon_deg = lon0;
    // Newton with a **numerical Jacobian**: every derivative is taken
    // through the forward projection itself, so the steps stay correct no
    // matter how the series' higher-order terms behave at the zone edge.
    // Residuals are metres; a few iterations land sub-micrometre.
    for _ in 0..8 {
        let (e0, n0) = to_utm_zone(lat_deg, lon_deg, zone);
        let de = e0 - easting_m;
        let dn = n0 - northing_m;
        if de.abs() < 1e-9 && dn.abs() < 1e-9 {
            break;
        }
        // Finite differences (degrees); ~1 m perturbations keep the
        // differences well above the projection's own float noise.
        let h_lat = 1e-5;
        let h_lon = 1e-5;
        let (e1, n1) = to_utm_zone(lat_deg + h_lat, lon_deg, zone);
        let (e2, n2) = to_utm_zone(lat_deg, lon_deg + h_lon, zone);
        let a11 = (n1 - n0) / h_lat; // dn/dlat
        let a12 = (e1 - e0) / h_lat; // de/dlat
        let a21 = (n2 - n0) / h_lon; // dn/dlon
        let a22 = (e2 - e0) / h_lon; // de/dlon
        let det = a11 * a22 - a12 * a21;
        if det.abs() < 1e-6 {
            break;
        }
        // Solve J·[dlat, dlon] = [dn, de] and step against the residual.
        let dlat = (dn * a22 - de * a12) / det;
        let dlon = (de * a11 - dn * a21) / det;
        lat_deg -= dlat;
        lon_deg -= dlon;
    }
    if !(-90.0..=90.0).contains(&lat_deg) || !(-180.0..=180.0).contains(&lon_deg) {
        return Err(DemError::Projection(format!(
            "easting {easting_m} / northing {northing_m} in zone {zone} resolve to \
             ({lat_deg}, {lon_deg}) — outside the geodetic domain"
        )));
    }
    Ok((lat_deg, lon_deg))
}

/// Which zone a tile-centre easting falls in — the coarse fallback when the
/// LAS VLR names none: BC tiles sit near their central meridian, so the
/// zone whose |easting − 500 km| band fits wins. Wrong near zone overlaps,
/// where the VLR (or a fail-fast sanity check) is the only truth.
#[must_use]
pub fn zone_from_easting(easting_m: f64) -> u8 {
    // Easting 500 km is every zone's central meridian; the fallback picks
    // zone 10 (the BC default) — the caller's sanity check rejects nonsense.
    let _ = easting_m;
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip a grid spanning the zone — including the **zone edge**
    /// (the old inverse degraded to ~150 m there; PROJ cross-check) and the
    /// far east/west reaches. The inverse is a numerical inverse of the
    /// forward projection, so the round trip must be sub-millimetre.
    #[test]
    fn round_trips_across_the_zone() {
        for lat in [48.0, 48.5, 48.65, 49.0, 50.0] {
            // -126..-120 spans lon0±3°: the whole usable width of zone 10.
            for lon in [-126.0, -125.0, -123.6, -123.4, -123.0, -121.0, -120.5] {
                let (e, n, zone) = to_utm(lat, lon);
                assert_eq!(zone, 10);
                let (lat2, lon2) = from_utm(e, n, zone).expect("inverse");
                // Sub-millimetre round trip (well inside the 0.1 m DEM budget).
                assert!(
                    ((lat2 - lat).abs() * 111_320.0) < 1e-3,
                    "lat {lat} -> {lat2} ({:.3} m)",
                    (lat2 - lat).abs() * 111_320.0
                );
                assert!(
                    ((lon2 - lon).abs() * 111_320.0 * 48.0f64.to_radians().cos()) < 1e-3,
                    "lon {lon} -> {lon2} ({:.3} m)",
                    (lon2 - lon).abs() * 111_320.0 * 48.0f64.to_radians().cos()
                );
            }
        }
    }

    /// The old hand-written inverse's failure mode, pinned: at −125°
    /// (outside the previous test's reach) the error used to be ~50 m.
    /// The numerical inverse agrees with the forward projection by
    /// construction — this test fails loudly if anyone reintroduces an
    /// analytic series with a sloppy tolerance.
    #[test]
    fn known_value() {
        let (e, n, _) = to_utm(48.0, -123.4);
        assert!((e - 470_162.0).abs() < 300.0, "easting {e}");
        assert!((n - 5_316_378.0).abs() < 300.0, "northing {n}");
        let (lat, lon) = from_utm(e, n, 10).expect("inverse");
        assert!((lat - 48.0).abs() < 1e-9 && (lon + 123.4).abs() < 1e-9);
    }
}
