//! Magnetometer calibration (plan §9, A4a).
//!
//! The motor-test window **applies** calibration parameters loaded from a
//! JSON file — it never calibrates (operator decision 2026-10-10). The model
//! per plan §9: `m_cal = A · (m_raw − b)`, with an optional per-axis current
//! compensation `m_comp = m_cal − K·I`.
//!
//! The JSON also declares which domain the parameters were fitted against
//! (`px4_raw` = PX4's uncalibrated sensor readout, `px4_calibrated` = PX4's
//! `CAL_MAG*`-corrected output) — the arithmetic is identical, the domain
//! only documents what the values mean and is surfaced in the UI/metadata.

use serde::Deserialize;

/// Which sensor readout the calibration parameters were fitted against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CalibrationDomain {
    /// PX4's uncalibrated raw sensor values.
    #[default]
    Px4Raw,
    /// PX4's `CAL_MAG*`-corrected output.
    Px4Calibrated,
}

/// `m_cal = A · (m_raw − b)`, optional `m_comp = m_cal − K·I`.
#[derive(Debug, Clone, PartialEq)]
pub struct MagCalibration {
    /// 3×3 correction matrix (row-major).
    pub a: [[f64; 3]; 3],
    /// Hard-iron offset per axis.
    pub b: [f64; 3],
    /// Optional per-axis current-compensation coefficients (nT per A).
    pub k: Option<[f64; 3]>,
    pub domain: CalibrationDomain,
}

#[derive(Debug, Deserialize)]
struct MagCalibrationJson {
    /// `[[a11,a12,a13],[a21,…],[…]]` (3×3) or `[ax,ay,az]` (diagonal).
    a: serde_json::Value,
    b: [f64; 3],
    k: Option<[f64; 3]>,
    #[serde(default)]
    domain: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MagCalibrationError(pub String);

impl std::fmt::Display for MagCalibrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mag calibration: {}", self.0)
    }
}
impl std::error::Error for MagCalibrationError {}

impl MagCalibration {
    /// Parse the calibration from a JSON string. Tolerant shapes: `a` may be
    /// a 3×3 matrix or a 3-vector (diagonal); `domain` is
    /// `"px4_raw"`/`"px4_calibrated"`.
    pub fn from_json(json: &str) -> Result<Self, MagCalibrationError> {
        let parsed: MagCalibrationJson =
            serde_json::from_str(json).map_err(|e| MagCalibrationError(e.to_string()))?;
        let a = parse_matrix(&parsed.a)?;
        if !parsed.b.iter().all(|v| v.is_finite()) {
            return Err(MagCalibrationError("b must be finite".into()));
        }
        let domain = match parsed.domain.as_deref() {
            None | Some("px4_raw") => CalibrationDomain::Px4Raw,
            Some("px4_calibrated") => CalibrationDomain::Px4Calibrated,
            Some(other) => return Err(MagCalibrationError(format!("unknown domain: {other}"))),
        };
        Ok(Self {
            a,
            b: parsed.b,
            k: parsed.k,
            domain,
        })
    }

    /// Apply to one raw sample: `m_cal = A · (m_raw − b)`, then subtract
    /// `K·I` when the calibration carries current compensation and a current
    /// is known.
    pub fn apply(&self, raw: [f64; 3], motor_current_a: Option<f64>) -> [f64; 3] {
        let mut out = [0.0f64; 3];
        for (r, row) in self.a.iter().enumerate() {
            let mut acc = 0.0;
            for (c, &coef) in row.iter().enumerate() {
                acc += coef * (raw[c] - self.b[c]);
            }
            out[r] = acc;
        }
        if let (Some(k), Some(current)) = (self.k, motor_current_a) {
            for (r, &coef) in k.iter().enumerate() {
                out[r] -= coef * current;
            }
        }
        out
    }
}

/// Accept a 3×3 matrix or a 3-vector (diagonal); reject anything else.
fn parse_matrix(v: &serde_json::Value) -> Result<[[f64; 3]; 3], MagCalibrationError> {
    // Diagonal shorthand: [ax, ay, az].
    if let Ok(diag) = serde_json::from_value::<[f64; 3]>(v.clone()) {
        return Ok([
            [diag[0], 0.0, 0.0],
            [0.0, diag[1], 0.0],
            [0.0, 0.0, diag[2]],
        ]);
    }
    let m: Vec<Vec<f64>> =
        serde_json::from_value(v.clone()).map_err(|e| MagCalibrationError(format!("a: {e}")))?;
    if m.len() != 3 || m.iter().any(|row| row.len() != 3) {
        return Err(MagCalibrationError("a must be 3×3 or a 3-vector".into()));
    }
    let mut out = [[0.0f64; 3]; 3];
    for (r, row) in m.iter().enumerate() {
        out[r].copy_from_slice(&row[..3]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: &str = r#"{"a": [[1,0,0],[0,1,0],[0,0,1]], "b": [0,0,0], "domain": "px4_raw"}"#;

    #[test]
    fn identity_calibration_is_an_identity() {
        let cal = MagCalibration::from_json(IDENTITY).unwrap();
        assert_eq!(cal.domain, CalibrationDomain::Px4Raw);
        let out = cal.apply([100.0, -50.0, 42000.0], None);
        assert_eq!(out, [100.0, -50.0, 42000.0]);
    }

    #[test]
    fn offset_and_scale_apply_per_plan_formula() {
        // A = 2·I (diagonal shorthand), b = [10, 20, 30]:
        // m_cal = 2·(raw − b).
        let json = r#"{"a": [2, 2, 2], "b": [10, 20, 30]}"#;
        let cal = MagCalibration::from_json(json).unwrap();
        let out = cal.apply([20.0, 30.0, 40.0], None);
        assert_eq!(out, [20.0, 20.0, 20.0]);
    }

    #[test]
    fn current_compensation_subtracts_k_times_current() {
        let json = r#"{"a": [[1,0,0],[0,1,0],[0,0,1]], "b": [0,0,0], "k": [10, 0, -5]}"#;
        let cal = MagCalibration::from_json(json).unwrap();
        let out = cal.apply([100.0, 100.0, 100.0], Some(2.0));
        assert_eq!(out, [80.0, 100.0, 110.0]);
        // No current known: the K term is skipped.
        let out = cal.apply([100.0, 100.0, 100.0], None);
        assert_eq!(out, [100.0, 100.0, 100.0]);
    }

    #[test]
    fn rejects_bad_shapes_and_domains() {
        assert!(MagCalibration::from_json(r#"{"a": 3, "b": [0,0,0]}"#).is_err());
        assert!(MagCalibration::from_json(r#"{"a": [[1,0],[0,1]], "b": [0,0,0]}"#).is_err());
        assert!(MagCalibration::from_json(
            r#"{"a": [[1,0,0],[0,1,0],[0,0,1]], "b": [0,0,0], "domain": "magic"}"#
        )
        .is_err());
        assert!(MagCalibration::from_json("not json").is_err());
    }

    #[test]
    fn px4_calibrated_domain_is_documented_not_transformed() {
        let json = r#"{"a": [[1,0,0],[0,1,0],[0,0,1]], "b": [0,0,0], "domain": "px4_calibrated"}"#;
        let cal = MagCalibration::from_json(json).unwrap();
        assert_eq!(cal.domain, CalibrationDomain::Px4Calibrated);
        assert_eq!(cal.apply([1.0, 2.0, 3.0], None), [1.0, 2.0, 3.0]);
    }
}
