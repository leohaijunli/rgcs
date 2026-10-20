//! The drape specification and result model (plan §5B DR1, ADR-022).
//!
//! [`DrapeSpec`] is the operator-facing contract: every number the trajectory
//! solver (DR2), the waypoint compiler (DR3) and the monitor (DR5) need.
//! [`DrapeResult`] is the solver's output — and the **single source** the
//! upload read-back (`itemsHash`), the preflight check (DR4), the monitor
//! baseline (DR5) and the archive all hash against (§5B principle 3).
//!
//! Rules baked into this model:
//! - `min_clearance_dsm_m` has **no default** (§5B principle 5): an unset
//!   clearance is a validation error, never a silently assumed number.
//! - `vertical_offset` is an explicit operator input carrying its origin and
//!   a note (M9 later fills it automatically from the DEM-bias estimate).
//! - Derived quantities (`max_grade_*`, the heights themselves) are **not**
//!   stored in the spec — they are functions of stored inputs, so the hash
//!   covers the inputs only.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::plan::PlannedWaypoint;
use crate::survey::pattern::LineKind;

/// Bump when a `DrapeSpec`/`DrapeResult` field changes meaning.
pub const DRAPE_SPEC_VERSION: u32 = 1;

/// How the solver may shape the turns between lines (§5B DR2 step 5: the
/// inter-line transition carries the height change; the line segment itself
/// only follows terrain). One policy this WP; DR2 extends as needed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum TurnPolicy {
    /// Standard boustrophedon turns: lead-in/out + inter-line transition.
    #[default]
    Standard,
}

/// Where a `vertical_offset` came from (§5B DR1: recorded with the value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum VerticalOffsetOrigin {
    /// The operator typed it (the only source until M9 unfreezes).
    Operator,
    /// The DEM-bias estimate (DR9) after the operator accepts it.
    DemBiasEstimate,
}

/// An explicit height offset applied on top of the target AGL, with its
/// provenance (§5B principle 6: the offset's origin and note travel with the
/// result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct VerticalOffset {
    /// Metres added to the planned height (positive = higher).
    pub meters: f64,
    pub origin: VerticalOffsetOrigin,
    /// Free-text note (why the offset exists — datum check, known bias…).
    pub note: String,
}

impl Default for VerticalOffset {
    fn default() -> Self {
        Self {
            meters: 0.0,
            origin: VerticalOffsetOrigin::Operator,
            note: String::new(),
        }
    }
}

/// Everything the drape pipeline needs, in metres/seconds (ADR-022).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DrapeSpec {
    pub version: u32,
    /// Target height above the **DTM** (the survey AGL).
    pub target_agl_m: f64,
    /// Monitoring/acceptance band around the target AGL (± metres).
    pub agl_band_m: f64,
    /// Minimum clearance above the **DSM**. **Required — no default**
    /// (§5B principle 5): `None` refuses generation with an explanation.
    pub min_clearance_dsm_m: Option<f64>,
    /// Horizontal margin for the clearance check: the DSM is read as the
    /// maximum within `dsm_footprint_m + pos_margin_m` (spikes survive, H3).
    pub pos_margin_m: f64,
    /// Body/sensor footprint radius, metres.
    pub dsm_footprint_m: f64,
    /// Climb-rate limit (m/s) — the smaller of the FC parameter and the
    /// airframe limit (parameter names *verify*, plan issue 13).
    pub max_climb_rate_mps: f64,
    /// Descent-rate limit (m/s), same rule.
    pub max_descent_rate_mps: f64,
    /// Cruise ground speed (m/s); the grade limits derive from the rates
    /// divided by this (`max_grade = rate / ground_speed`, up/down separately).
    pub ground_speed_mps: f64,
    /// Along-line sample step, metres — by default no larger than the DEM
    /// cell (`validate` enforces that when the resolution is supplied).
    pub sample_step_m: f64,
    /// Smoothing weight/length for the solver (DR2).
    pub smoothing_m: f64,
    /// Lead-in length before each line, metres (enter the line settled).
    pub runin_length_m: f64,
    pub turn_policy: TurnPolicy,
    /// Maximum deviation of the compiled waypoint polyline from the designed
    /// curve (DR3's Douglas–Peucker tolerance), metres.
    pub wp_tol_m: f64,
    pub vertical_offset: VerticalOffset,
}

impl Default for DrapeSpec {
    fn default() -> Self {
        Self {
            version: DRAPE_SPEC_VERSION,
            target_agl_m: 40.0,
            agl_band_m: 3.0,
            min_clearance_dsm_m: None,
            pos_margin_m: 1.0,
            dsm_footprint_m: 1.0,
            max_climb_rate_mps: 3.0,
            max_descent_rate_mps: 3.0,
            ground_speed_mps: 5.0,
            sample_step_m: 1.0,
            smoothing_m: 0.0,
            runin_length_m: 10.0,
            turn_policy: TurnPolicy::Standard,
            wp_tol_m: 1.0,
            vertical_offset: VerticalOffset::default(),
        }
    }
}

/// Why a [`DrapeSpec`] was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DrapeSpecError {
    /// The clearance was never set — the operator must enter it (§5B
    /// principle 5: no silent default).
    #[error("min_clearance_dsm is required — enter the minimum clearance above the DSM before generating")]
    MissingClearance,
    /// A number is not usable (non-finite, negative where positive is meant).
    #[error("{field}: {reason}")]
    InvalidValue { field: &'static str, reason: String },
    /// The sample step is coarser than the DEM cell it will be sampled on.
    #[error("sample_step {step} m exceeds the DEM cell {cell} m — refine the step")]
    SampleStepExceedsCell { step: f64, cell: f64 },
}

impl DrapeSpec {
    /// The derived grade limits (`rate / ground_speed`, per direction).
    #[must_use]
    pub fn max_grade_up(&self) -> f64 {
        self.max_climb_rate_mps / self.ground_speed_mps.max(f64::EPSILON)
    }

    #[must_use]
    pub fn max_grade_down(&self) -> f64 {
        self.max_descent_rate_mps / self.ground_speed_mps.max(f64::EPSILON)
    }

    /// Reject a spec that cannot drive the solver. `dem_resolution_m` (the
    /// DEM cell the lines will be sampled on, when known) additionally caps
    /// `sample_step_m` at the cell size.
    ///
    /// # Errors
    ///
    /// [`DrapeSpecError`] naming the first offending field.
    pub fn validate(&self, dem_resolution_m: Option<f64>) -> Result<(), DrapeSpecError> {
        let clearance = self
            .min_clearance_dsm_m
            .ok_or(DrapeSpecError::MissingClearance)?;
        if !clearance.is_finite() || clearance < 0.0 {
            return Err(DrapeSpecError::InvalidValue {
                field: "min_clearance_dsm_m",
                reason: format!("{clearance} is not a non-negative number"),
            });
        }
        for (field, value, allow_zero) in [
            ("target_agl_m", self.target_agl_m, false),
            ("agl_band_m", self.agl_band_m, true),
            ("pos_margin_m", self.pos_margin_m, true),
            ("dsm_footprint_m", self.dsm_footprint_m, true),
            ("max_climb_rate_mps", self.max_climb_rate_mps, false),
            ("max_descent_rate_mps", self.max_descent_rate_mps, false),
            ("ground_speed_mps", self.ground_speed_mps, false),
            ("sample_step_m", self.sample_step_m, false),
            ("smoothing_m", self.smoothing_m, true),
            ("runin_length_m", self.runin_length_m, true),
            ("wp_tol_m", self.wp_tol_m, false),
            ("vertical_offset.meters", self.vertical_offset.meters, true),
        ] {
            let ok = value.is_finite() && (allow_zero || value > 0.0);
            if !ok {
                return Err(DrapeSpecError::InvalidValue {
                    field,
                    reason: format!("{value} must be finite and positive"),
                });
            }
        }
        if let Some(cell) = dem_resolution_m {
            if self.sample_step_m > cell {
                return Err(DrapeSpecError::SampleStepExceedsCell {
                    step: self.sample_step_m,
                    cell,
                });
            }
        }
        Ok(())
    }

    /// Canonical JSON of the spec (version included) — the hash input. Same
    /// spec ⇒ same bytes; any field change ⇒ different bytes.
    fn canonical_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| String::new())
    }

    /// SHA-256 of the canonical JSON (hex). Enters the upload read-back, the
    /// preflight check, the monitor baseline and the archive (§5B
    /// principle 3: design and monitoring share one source of truth).
    ///
    /// Serialization of a plain struct cannot fail, so the impossible encode
    /// error hashes the empty string — caught by the round-trip test.
    #[must_use]
    pub fn spec_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.canonical_json().as_bytes());
        hex(&hasher.finalize())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One along-line sample of the designed profile (DR2 fills this).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DrapeSample {
    /// Along-line distance from the line start, metres.
    pub s: f64,
    /// WGS84 degrees.
    pub x: f64,
    pub y: f64,
    /// Planned (draped) height, DEM datum.
    pub z_plan: f64,
    pub dtm: f64,
    /// `None` where the DSM is absent (DTM-only tiles skip the check).
    pub dsm: Option<f64>,
    /// Planned height above the DTM at this sample.
    pub agl_plan: f64,
    /// Planned clearance above the footprint-max DSM; `None` without a DSM.
    pub clearance: Option<f64>,
}

impl Default for DrapeSample {
    fn default() -> Self {
        Self {
            s: 0.0,
            x: 0.0,
            y: 0.0,
            z_plan: 0.0,
            dtm: 0.0,
            dsm: None,
            agl_plan: 0.0,
            clearance: None,
        }
    }
}

/// Why a span of a line could not satisfy every constraint (§5B principle 2:
/// report, never silently truncate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum InfeasibilityReason {
    /// The DSM clearance floor cannot be held within the grade limits.
    Clearance,
    /// The climb-rate limit cannot follow the terrain.
    ClimbRate,
    /// The descent-rate limit cannot follow the terrain.
    DescentRate,
    /// The DEM has no data here — nothing may be extrapolated.
    NoData,
}

/// An interval of a line where the constraints conflict, with the worst gap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct InfeasibleSpan {
    pub line_id: String,
    pub s_start: f64,
    pub s_end: f64,
    pub reason: InfeasibilityReason,
    /// How much the binding constraint is violated at the worst point, metres.
    pub max_deficit_m: f64,
}

impl Default for InfeasibleSpan {
    fn default() -> Self {
        Self {
            line_id: String::new(),
            s_start: 0.0,
            s_end: 0.0,
            reason: InfeasibilityReason::Clearance,
            max_deficit_m: 0.0,
        }
    }
}

/// One survey/tie crossing (DR2 step 6): how consistent the plan is where the
/// lines meet — the magnetic cross-over levelling depends on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct Crossing {
    pub line_a: String,
    pub line_b: String,
    /// Planned AGL difference at the crossing, metres.
    pub agl_diff_m: f64,
    /// Planned AMSL difference at the crossing, metres.
    pub z_diff_m: f64,
}

impl Default for Crossing {
    fn default() -> Self {
        Self {
            line_a: String::new(),
            line_b: String::new(),
            agl_diff_m: 0.0,
            z_diff_m: 0.0,
        }
    }
}

/// Aggregate numbers for the whole plan (the planning panel's AGL table).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DrapeStats {
    pub agl_mean_m: f64,
    pub agl_std_m: f64,
    pub agl_min_m: f64,
    pub agl_max_m: f64,
    /// Tightest planned clearance above the DSM; `None` without a DSM.
    pub min_clearance_m: Option<f64>,
    /// Largest |grade| in the designed profile.
    pub max_grade: f64,
    /// Largest climb the profile asks of the vehicle, m/s.
    pub max_climb_needed_mps: f64,
    /// Worst vertical deviation of the compiled waypoint polyline from the
    /// designed curve (DR3); `None` before compilation.
    pub compile_deviation_max_m: Option<f64>,
}

/// Which DEM produced a result (ADR-004 archives the provenance).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DemRef {
    /// HRDEM STAC item id, when the source was an HRDEM session.
    pub item_id: Option<String>,
    pub project_name: Option<String>,
    pub acquisition: Option<String>,
    /// The DEM's provenance hash (`dem.json` → `provenance_sha256`).
    pub hash: String,
    /// The DEM's vertical datum — the datum compiled waypoints carry (DR3).
    pub datum: crate::height::HeightDatum,
}

/// The solver's output — and the shared baseline for upload, preflight,
/// monitoring and archive (§5B principle 3).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct DrapeResult {
    pub version: u32,
    /// Per-line designed profiles, in line order.
    pub lines: Vec<LineProfile>,
    /// The compiled waypoints (AMSL-style heights in the DEM datum).
    pub waypoints: Vec<PlannedWaypoint>,
    /// The compiled waypoints with their plan metadata (DR3): `s`, line id
    /// and kind per waypoint, plus one speed item per line.
    pub compiled: Vec<super::drape_compile::CompiledWaypoint>,
    pub infeasible: Vec<InfeasibleSpan>,
    pub stats: DrapeStats,
    pub crossings: Vec<Crossing>,
    /// Hash of the [`DrapeSpec`] this result was computed from.
    pub spec_hash: String,
    pub dem_ref: DemRef,
}

/// One line's designed profile.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(default)]
pub struct LineProfile {
    pub line_id: String,
    pub kind: LineKind,
    pub profile: Vec<DrapeSample>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_spec() -> DrapeSpec {
        DrapeSpec {
            min_clearance_dsm_m: Some(5.0),
            ..DrapeSpec::default()
        }
    }

    #[test]
    fn a_spec_without_the_clearance_is_refused_with_a_reason() {
        let spec = DrapeSpec::default();
        assert!(matches!(
            spec.validate(None),
            Err(DrapeSpecError::MissingClearance)
        ));
        // And the message says what to do.
        assert!(spec
            .validate(None)
            .unwrap_err()
            .to_string()
            .contains("required"));
    }

    #[test]
    fn validation_checks_each_field_and_the_sample_step_cap() {
        assert!(valid_spec().validate(None).is_ok());
        let mut spec = valid_spec();
        spec.target_agl_m = -1.0;
        assert!(matches!(
            spec.validate(None),
            Err(DrapeSpecError::InvalidValue {
                field: "target_agl_m",
                ..
            })
        ));
        let mut spec = valid_spec();
        spec.ground_speed_mps = 0.0;
        assert!(matches!(
            spec.validate(None),
            Err(DrapeSpecError::InvalidValue {
                field: "ground_speed_mps",
                ..
            })
        ));
        // A step coarser than the DEM cell is refused when the cell is known.
        let mut spec = valid_spec();
        spec.sample_step_m = 2.0;
        assert!(matches!(
            spec.validate(Some(1.0)),
            Err(DrapeSpecError::SampleStepExceedsCell { step, cell })
            if step == 2.0 && cell == 1.0
        ));
        // Same step passes when the cell is finer-unknown or equal.
        assert!(spec.validate(Some(2.0)).is_ok());
        assert!(spec.validate(None).is_ok());
    }

    #[test]
    fn the_hash_is_stable_and_field_sensitive() {
        let spec = valid_spec();
        let again = valid_spec();
        assert_eq!(spec.spec_hash(), again.spec_hash(), "same input, same hash");
        assert_eq!(spec.spec_hash().len(), 64, "sha256 hex");
        let mut other = valid_spec();
        other.vertical_offset.meters = 0.5;
        assert_ne!(
            spec.spec_hash(),
            other.spec_hash(),
            "any field change shows"
        );
        // The version participates.
        let mut other = valid_spec();
        other.version += 1;
        assert_ne!(spec.spec_hash(), other.spec_hash());
    }

    #[test]
    fn derived_grades_come_from_the_rates_and_speed() {
        let mut spec = valid_spec();
        spec.max_climb_rate_mps = 3.0;
        spec.max_descent_rate_mps = 2.0;
        spec.ground_speed_mps = 5.0;
        assert!((spec.max_grade_up() - 0.6).abs() < 1e-9);
        assert!((spec.max_grade_down() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn spec_and_result_round_trip_through_json() {
        let spec = valid_spec();
        let json = serde_json::to_string(&spec).expect("serialize spec");
        let back: DrapeSpec = serde_json::from_str(&json).expect("deserialize spec");
        assert_eq!(back, spec);
        assert_eq!(
            back.spec_hash(),
            spec.spec_hash(),
            "round trip keeps the hash"
        );

        let result = DrapeResult {
            version: DRAPE_SPEC_VERSION,
            spec_hash: spec.spec_hash(),
            dem_ref: DemRef {
                item_id: Some("BC-Vancouver_Island_Sunshine_Coast_2018-1m".into()),
                project_name: Some("BC Vancouver Island 2018".into()),
                acquisition: Some("2019-10-01".into()),
                hash: "abc123".into(),
                datum: crate::height::HeightDatum::OrthometricCgvd2013,
            },
            ..DrapeResult::default()
        };
        let json = serde_json::to_string(&result).expect("serialize result");
        let back: DrapeResult = serde_json::from_str(&json).expect("deserialize result");
        assert_eq!(back, result);
    }

    #[test]
    fn defaults_carry_no_clearance_so_nobody_inherits_one() {
        assert_eq!(DrapeSpec::default().min_clearance_dsm_m, None);
    }
}
