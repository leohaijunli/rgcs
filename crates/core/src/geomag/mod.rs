//! World Magnetic Model 2025 (WMM2025) synthesis — the offline answer to
//! "what is the local geomagnetic direction at these coordinates?"
//! (operator request: survey lines parallel to the local field).
//!
//! The model is NOAA/NCEI public domain; the committed coefficient file is
//! byte-identical to `WMM2025.COF` from the official WMM2025COF.zip
//! (https://www.ncei.noaa.gov/sites/default/files/2024-12/WMM2025COF.zip,
//! fetched 2026-10-17; epoch 2025.0, valid 2025.0–2030.0). Cite:
//! NOAA NCEI Geomagnetic Modeling Team; British Geological Survey, 2024:
//! World Magnetic Model 2025. https://doi.org/10.25921/aqfd-sd83.
//!
//! Pure spherical-harmonic synthesis (~250 lines, zero dependencies): the
//! official `WMM2025_TEST_VALUES.txt` vectors pin the implementation.

use std::sync::OnceLock;

/// WMM reference radius (metres) — part of the model definition.
const A_REF_M: f64 = 6_371_200.0;
/// WGS84 ellipsoid.
const WGS84_A: f64 = 6_378_137.0;
const WGS84_F: f64 = 1.0 / 298.257_223_563;
/// Model degree.
const N_MAX: usize = 12;
/// Validity window (decimal years): outside, the field is refused — a stale
/// model silently drifting is worse than a loud error.
pub const EPOCH: f64 = 2025.0;
pub const VALID_UNTIL: f64 = 2030.0;

/// The committed WMM2025 coefficient file (NOAA public domain).
const COF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/geomag/WMM2025.COF"
));

/// Everything that can stop a field computation.
#[derive(Debug, thiserror::Error)]
pub enum GeomagError {
    #[error("magnetic model coefficients failed to parse: {0}")]
    Coefficients(String),
    #[error(
        "magnetic model out of date: WMM2025 is valid {EPOCH}–{VALID_UNTIL}, asked for {year}"
    )]
    OutOfWindow { year: f64 },
    #[error("latitude must be -90..=90 degrees, got {0}")]
    BadLatitude(f64),
    #[error("declination is undefined at the magnetic pole (H ≈ 0)")]
    AtPole,
}

/// One Gauss coefficient set: `g/h[n][m]` at the epoch plus secular rates.
struct Coeffs {
    g: [[f64; N_MAX + 1]; N_MAX + 1],
    h: [[f64; N_MAX + 1]; N_MAX + 1],
    gdot: [[f64; N_MAX + 1]; N_MAX + 1],
    hdot: [[f64; N_MAX + 1]; N_MAX + 1],
}

fn coeffs() -> Result<&'static Coeffs, GeomagError> {
    static COEFFS: OnceLock<Result<Coeffs, GeomagError>> = OnceLock::new();
    COEFFS
        .get_or_init(parse_cof)
        .as_ref()
        .map_err(|e| GeomagError::Coefficients(e.to_string()))
}

/// Parse the official `WMM.COF` format: header line, 90 coefficient lines
/// (`n m g h gdot hdot`), two `999…` terminator lines.
fn parse_cof() -> Result<Coeffs, GeomagError> {
    let bad = |m: &str| Err(GeomagError::Coefficients(m.to_string()));
    let mut c = Coeffs {
        g: [[0.0; N_MAX + 1]; N_MAX + 1],
        h: [[0.0; N_MAX + 1]; N_MAX + 1],
        gdot: [[0.0; N_MAX + 1]; N_MAX + 1],
        hdot: [[0.0; N_MAX + 1]; N_MAX + 1],
    };
    let mut count = 0;
    for line in COF.lines() {
        if line.trim_start().starts_with("9999") {
            break;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            // The header line (epoch / model name / release date).
            continue;
        }
        let (Ok(n), Ok(m)) = (fields[0].parse::<usize>(), fields[1].parse::<usize>()) else {
            continue;
        };
        if n == 0 || n > N_MAX || m > n {
            return bad(&format!("bad degree/order: {n} {m}"));
        }
        let vals: Vec<f64> = fields[2..6]
            .iter()
            .map(|v| v.parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|e| GeomagError::Coefficients(format!("bad coefficient: {e}")))?;
        c.g[n][m] = vals[0];
        c.h[n][m] = vals[1];
        c.gdot[n][m] = vals[2];
        c.hdot[n][m] = vals[3];
        count += 1;
    }
    if count != 90 {
        return bad(&format!("expected 90 coefficient lines, got {count}"));
    }
    Ok(c)
}

/// The magnetic field components the WMM defines, geodetic (X north, Y east,
/// Z **down**, all nT), plus declination/inclination/total.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeomagField {
    pub x_nt: f64,
    pub y_nt: f64,
    pub z_nt: f64,
    pub h_nt: f64,
    pub f_nt: f64,
    /// Declination in degrees, east positive — the "local geomagnetic
    /// direction" survey lines can be aligned with.
    pub declination_deg: f64,
    pub inclination_deg: f64,
}

/// Field at a geodetic position. `height_m` is above the WGS84 ellipsoid
/// (AMSL differs by the geoid, which moves D by a negligible amount — the
/// official note puts it well under the model's own uncertainty).
///
/// # Errors
///
/// [`GeomagError::OutOfWindow`] outside the model's validity window,
/// [`GeomagError::BadLatitude`] / [`GeomagError::AtPole`] for degenerate
/// positions. Never extrapolates in time.
pub fn field(
    lat_deg: f64,
    lon_deg: f64,
    height_m: f64,
    decimal_year: f64,
) -> Result<GeomagField, GeomagError> {
    if !(EPOCH..VALID_UNTIL).contains(&decimal_year) {
        return Err(GeomagError::OutOfWindow { year: decimal_year });
    }
    if !(-90.0..=90.0).contains(&lat_deg) {
        return Err(GeomagError::BadLatitude(lat_deg));
    }
    let c = coeffs()?;
    let dt = decimal_year - EPOCH;

    // Geodetic → geocentric spherical (WGS84): ECEF from the prime
    // vertical radius Rp (the meridional radius has no role here).
    let e2 = WGS84_F * (2.0 - WGS84_F);
    let sin_phi = lat_deg.to_radians().sin();
    let cos_phi = lat_deg.to_radians().cos();
    let rp = WGS84_A / (1.0 - e2 * sin_phi * sin_phi).sqrt();
    let p = (rp + height_m) * cos_phi;
    let z = (rp * (1.0 - e2) + height_m) * sin_phi;
    let r = (p * p + z * z).sqrt();
    let sin_t = z / r; // geocentric latitude sine
    let cos_t = p / r;
    let delta = (sin_phi * cos_t - cos_phi * sin_t).atan2(cos_phi * cos_t + sin_phi * sin_t);

    // Schmidt semi-normalized Legendre P̄nm(sin t) and dP̄/dt.
    let (p_bar, dp_bar) = legendre(sin_t, cos_t);

    // Sum the spherical-harmonic field in the geocentric frame.
    let lon = lon_deg.to_radians();
    let rho = A_REF_M / r;
    let mut rho_np2 = rho * rho; // ρ^(n+2), starting at n=0 (unused) — advanced per n
    let mut b_n = 0.0; // north' (increasing t)
    let mut b_e = 0.0; // east
    let mut b_u = 0.0; // up' (outward radial)
    for n in 1..=N_MAX {
        rho_np2 *= rho;
        for m in 0..=n {
            let (g, h) = (c.g[n][m] + c.gdot[n][m] * dt, c.h[n][m] + c.hdot[n][m] * dt);
            let cos_ml = (m as f64 * lon).cos();
            let sin_ml = (m as f64 * lon).sin();
            let gc = g * cos_ml + h * sin_ml;
            let gs = g * sin_ml - h * cos_ml;
            b_n += -rho_np2 * gc * dp_bar[n][m];
            b_u += rho_np2 * (n as f64 + 1.0) * gc * p_bar[n][m];
            if m > 0 {
                b_e += rho_np2 * m as f64 * gs * p_bar[n][m];
            }
        }
    }
    // The east component carries a 1/cos(t) that is numerically unstable at
    // the poles; clamp the denominator instead of dividing by ~0.
    b_e /= cos_t.abs().max(1e-10);

    // Rotate to the geodetic frame (delta = geodetic − geocentric latitude).
    let (sin_d, cos_d) = (delta.sin(), delta.cos());
    let x = b_n * cos_d - b_u * sin_d;
    let y = b_e;
    let z_down = -(b_n * sin_d + b_u * cos_d);
    let h = (x * x + y * y).sqrt();
    let f = (h * h + z_down * z_down).sqrt();
    if h < 1.0 {
        return Err(GeomagError::AtPole);
    }
    Ok(GeomagField {
        x_nt: x,
        y_nt: y,
        z_nt: z_down,
        h_nt: h,
        f_nt: f,
        declination_deg: y.atan2(x).to_degrees(),
        inclination_deg: z_down.atan2(h).to_degrees(),
    })
}

/// Schmidt semi-normalized associated Legendre functions P̄nm(sin t) and
/// dP̄nm/dt: `([n][m])` arrays for n = 0..=12. The argument is x = sin(t)
/// (t = geocentric latitude), so P̄11 = cos t; the general three-term
/// recursion covers all n > m (P(n−2,m) terms vanish via zeros).
fn legendre(sin_t: f64, cos_t: f64) -> (Vec<[f64; N_MAX + 1]>, Vec<[f64; N_MAX + 1]>) {
    let mut p = vec![[0.0; N_MAX + 1]; N_MAX + 1];
    let mut dp = vec![[0.0; N_MAX + 1]; N_MAX + 1];
    p[0][0] = 1.0;
    p[1][1] = cos_t;
    dp[1][1] = -sin_t;
    for n in 2..=N_MAX {
        let k = ((2 * n - 1) as f64 / (2 * n) as f64).sqrt();
        p[n][n] = k * cos_t * p[n - 1][n - 1];
        dp[n][n] = k * (cos_t * dp[n - 1][n - 1] - sin_t * p[n - 1][n - 1]);
    }
    for n in 1..=N_MAX {
        for m in 0..n {
            let kn = (2 * n - 1) as f64;
            let norm = ((n * n - m * m) as f64).sqrt();
            let k2 = (((n - 1) * (n - 1) - m * m) as f64).sqrt();
            let pm1 = p[n - 1][m];
            let dpm1 = dp[n - 1][m];
            // p[n-2][m] is zero when m > n-2 (the arrays start zeroed and
            // are filled in n order), which the k2 factor zeroes anyway.
            let pm2 = if m <= n && n >= 2 { p[n - 2][m] } else { 0.0 };
            let dpm2 = if m <= n && n >= 2 { dp[n - 2][m] } else { 0.0 };
            p[n][m] = (kn * sin_t * pm1 - k2 * pm2) / norm;
            dp[n][m] = (kn * (cos_t * pm1 + sin_t * dpm1) - k2 * dpm2) / norm;
        }
    }
    (p, dp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The official WMM2025 test values (fields 1–11; X Y Z H F I D).
    #[test]
    fn official_test_values_match() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/geomag/WMM2025_TEST_VALUES.txt"
        ));
        let mut cases = 0;
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 11 || f[0].starts_with('#') {
                continue;
            }
            let v: Vec<f64> = f[..11].iter().map(|x| x.parse().unwrap()).collect();
            let (date, h_km, lat, lon, x, y, z, h, f_tot, inc, dec) = (
                v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10],
            );
            let field = field(lat, lon, h_km * 1000.0, date).expect("compute");
            assert!(
                (field.x_nt - x).abs() <= 0.1,
                "X at ({lat},{lon},{date}): {} vs {x}",
                field.x_nt
            );
            assert!((field.y_nt - y).abs() <= 0.1, "Y: {} vs {y}", field.y_nt);
            assert!((field.z_nt - z).abs() <= 0.1, "Z: {} vs {z}", field.z_nt);
            assert!((field.h_nt - h).abs() <= 0.1, "H: {} vs {h}", field.h_nt);
            assert!(
                (field.f_nt - f_tot).abs() <= 0.1,
                "F: {} vs {f_tot}",
                field.f_nt
            );
            assert!(
                (field.inclination_deg - inc).abs() <= 0.005,
                "I: {} vs {inc}",
                field.inclination_deg
            );
            assert!(
                (field.declination_deg - dec).abs() <= 0.005,
                "D: {} vs {dec}",
                field.declination_deg
            );
            cases += 1;
        }
        assert_eq!(cases, 12, "the file carries 12 official vectors");
    }

    /// The pilot AOI's declination, pinned for the ∥ B line-azimuth feature:
    /// Victoria, BC is ~+15.7°E at the epoch (westward drift ~0.2°/yr).
    #[test]
    fn victoria_declination_is_pinned() {
        let f = field(48.556_06, -123.381_36, 0.0, 2026.8).expect("inside the window");
        assert!(
            (f.declination_deg - 15.0).abs() < 1.0,
            "expected ≈ +15°E, got {}",
            f.declination_deg
        );
        assert!(f.f_nt > 45_000.0 && f.f_nt < 60_000.0, "F = {}", f.f_nt);
    }

    /// Outside the validity window the model refuses — a silently drifting
    /// field is worse than a loud error (and DR-era surveys archive D).
    #[test]
    fn out_of_window_is_refused() {
        assert!(matches!(
            field(48.0, -123.0, 0.0, 2030.5),
            Err(GeomagError::OutOfWindow { .. })
        ));
        assert!(matches!(
            field(48.0, -123.0, 0.0, 2024.0),
            Err(GeomagError::OutOfWindow { .. })
        ));
        assert!(matches!(
            field(91.0, 0.0, 0.0, 2026.0),
            Err(GeomagError::BadLatitude(_))
        ));
    }

    /// Secular drift behaves: 2.5 years move D at Victoria by ~0.1–0.5°.
    #[test]
    fn secular_variation_moves_the_field() {
        let a = field(48.556_06, -123.381_36, 0.0, 2025.1).expect("a");
        let b = field(48.556_06, -123.381_36, 0.0, 2029.9).expect("b");
        let dd = (b.declination_deg - a.declination_deg).abs();
        assert!(dd > 0.05 && dd < 2.0, "4.8-year D drift {dd}");
    }
}
