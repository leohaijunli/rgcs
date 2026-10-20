//! Survey-type profiles and the registry (plan §5A PJ2; ADR-019).
//!
//! A [`SurveyProfile`] declares everything that varies by measurement type —
//! required devices, the phase navigation (U1), the preflight checklist
//! template (A5), QC thresholds (M4 ⏸), line following (DR) and output
//! formats — so **new types are new profiles, not conditionals**: the
//! generic code paths (`safety::evaluate`, U1's navigation, the recorder)
//! consume the profile they are given.
//!
//! This WP registers only [`SurveyKind::UavMag`]. The `TestWalk` variant is a
//! `#[doc(hidden)]` fixture kind for the acceptance test "a second type
//! registered in tests yields its own template and phases without changing
//! generic code"; nothing registers it in production. The full "generic state
//! machine + platform driver" split stays deferred to M1 (⏸) — when it lands,
//! the driver is bound per profile (UAV: `MISSION_CURRENT`), not refactored
//! from a UAV-only version.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::devices::DeviceRole;
use crate::safety::{ChecklistItemId, ChecklistItemSpec, ChecklistTemplate};

/// The measurement types this build knows. Serialized kebab-case so the
/// on-disk value matches the string PJ1 wrote (`"uav-mag"`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum SurveyKind {
    /// UAV-borne magnetometer (the only production type this WP registers).
    #[default]
    UavMag,
    /// Test-only fixture kind (ADR-020's walk survey is not implemented yet):
    /// exercises the registry in tests, never registered in production.
    #[doc(hidden)]
    TestWalk,
}

/// One stage of the job navigation (U1). Labels render via i18n; the profile
/// carries the ordered ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PhaseId {
    /// Connect, calibration files, DEM preparation (VIC ⏸ M7).
    Prepare,
    /// Area, pattern, waypoints, project data.
    Plan,
    /// Preflight checklist (U5) and the arm gate.
    Preflight,
    /// HUD, command status bar, drape monitoring (DR6).
    Fly,
    /// Flight record summary; QC/export ⏸ M3/M6.
    Review,
}

/// How the flown path follows the planned lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LineFollowing {
    /// A waypoint mission on the FC; progress read from `MISSION_CURRENT`
    /// (the future M1 UAV driver binds exactly this message).
    WaypointMission,
}

/// Archive/output formats the type produces (consumed by A4/W2 WPs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// Raw MAVLink frames (the recorder's lossless stream).
    Tlog,
    /// The structured event log (`events.jsonl`).
    EventsJsonl,
    /// Delimited sample table (W2's interchange format).
    Csv,
}

/// QC thresholds and gates (placeholder: lands with M4's `qc.json`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QcTemplate {}

/// Everything that varies by measurement type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SurveyProfile {
    pub kind: SurveyKind,
    /// Human-readable type name (i18n stays with the UI).
    pub name: String,
    /// Devices that must be present/healthy for this survey.
    pub required_devices: Vec<DeviceRole>,
    /// Ordered stage navigation (U1 renders the labels and badges).
    pub phases: Vec<PhaseId>,
    /// Which preflight items apply and how they block (A5 consumes this).
    pub safety_template: ChecklistTemplate,
    pub qc_template: QcTemplate,
    pub line_following: LineFollowing,
    pub outputs: Vec<OutputFormat>,
}

/// Error from [`Registry::register`].
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The registry is poisoned by a panicked writer (recoverable: the guard
    /// still carries the map; [`Registry`] recovers transparently).
    #[error("registry lock poisoned")]
    Poisoned,
}

/// The survey-type registry: kind → profile. Lazily seeded with the UavMag
/// profile on first access; tests may register more kinds.
pub struct Registry {
    profiles: RwLock<HashMap<SurveyKind, Arc<SurveyProfile>>>,
}

impl Registry {
    /// Look up a registered profile.
    #[must_use]
    pub fn profile(&self, kind: SurveyKind) -> Option<Arc<SurveyProfile>> {
        self.read().get(&kind).cloned()
    }

    /// The production default profile — always present (the registry seeds it
    /// on first access and re-seeds if somehow missing).
    #[must_use]
    pub fn default_profile(&self) -> Arc<SurveyProfile> {
        if let Some(profile) = self.profile(SurveyKind::UavMag) {
            return profile;
        }
        let profile = Arc::new(uav_mag_profile());
        self.profiles
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(SurveyKind::UavMag, profile.clone());
        profile
    }

    /// Register (or replace) a profile. Tests use this to add fixture kinds.
    pub fn register(&self, profile: SurveyProfile) {
        self.profiles
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(profile.kind, Arc::new(profile));
    }

    /// Every registered kind, sorted for stable display.
    #[must_use]
    pub fn kinds(&self) -> Vec<SurveyKind> {
        let mut kinds: Vec<SurveyKind> = self.read().keys().copied().collect();
        kinds.sort_by_key(|k| match k {
            SurveyKind::UavMag => 0,
            SurveyKind::TestWalk => 1,
        });
        kinds
    }

    /// Recover-from-poison read guard: the map is only mutated under the write
    /// guard, which never panics, so poisoning cannot lose data.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<SurveyKind, Arc<SurveyProfile>>> {
        self.profiles
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The process-wide registry.
#[must_use]
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let registry = Registry {
            profiles: RwLock::new(HashMap::new()),
        };
        registry.register(uav_mag_profile());
        registry
    })
}

/// The UAV-magnetometer profile (the only production registration this WP):
/// its checklist template covers exactly the items the A5 baseline evaluated
/// before profiles existed, all [`ChecklistItemMode::Required`], so behaviour
/// is unchanged.
#[must_use]
pub fn uav_mag_profile() -> SurveyProfile {
    use crate::safety::ChecklistItemMode;
    use ChecklistItemId::{
        Battery, DemAge, DemCoverage, DemHasDtm, DrapeBatteryEta, DrapeClimbRate, DrapeInfeasible,
        DrapeItemCount, DrapeThresholds, FlightModeReady, GpsFix, HomePosition, LandedOnGround,
        LinkFresh, MissionReady, MotorTestIdle, Rtk, SensorHealth,
    };
    SurveyProfile {
        kind: SurveyKind::UavMag,
        name: "UAV magnetometer".into(),
        required_devices: vec![DeviceRole::FlightController],
        phases: vec![
            PhaseId::Prepare,
            PhaseId::Plan,
            PhaseId::Preflight,
            PhaseId::Fly,
            PhaseId::Review,
        ],
        safety_template: ChecklistTemplate {
            items: vec![
                ChecklistItemSpec::required(LinkFresh),
                ChecklistItemSpec::required(GpsFix),
                ChecklistItemSpec::required(Rtk),
                ChecklistItemSpec::required(HomePosition),
                ChecklistItemSpec::required(Battery),
                ChecklistItemSpec::required(SensorHealth),
                ChecklistItemSpec::required(LandedOnGround),
                ChecklistItemSpec::required(MissionReady),
                ChecklistItemSpec::required(FlightModeReady),
                ChecklistItemSpec::required(MotorTestIdle),
                ChecklistItemSpec::required(DemCoverage),
                ChecklistItemSpec {
                    id: DemHasDtm,
                    mode: ChecklistItemMode::WarnOnly,
                },
                ChecklistItemSpec {
                    id: DemAge,
                    mode: ChecklistItemMode::WarnOnly,
                },
                ChecklistItemSpec::required(DrapeInfeasible),
                ChecklistItemSpec::required(DrapeItemCount),
                ChecklistItemSpec::required(DrapeClimbRate),
                ChecklistItemSpec::required(DrapeThresholds),
                ChecklistItemSpec {
                    id: DrapeBatteryEta,
                    mode: ChecklistItemMode::WarnOnly,
                },
            ],
        },
        qc_template: QcTemplate {},
        line_following: LineFollowing::WaypointMission,
        outputs: vec![OutputFormat::Tlog, OutputFormat::EventsJsonl],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::{ChecklistItemMode, ChecklistStatus};

    #[test]
    fn the_default_profile_is_registered_and_complete() {
        let profile = registry().default_profile();
        assert_eq!(profile.kind, SurveyKind::UavMag);
        assert_eq!(profile.phases.len(), 5);
        assert_eq!(profile.phases[0], PhaseId::Prepare);
        assert_eq!(profile.safety_template.items.len(), 18);
        assert!(profile.outputs.contains(&OutputFormat::Tlog));
    }

    #[test]
    fn survey_kind_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&SurveyKind::UavMag).expect("serialize"),
            "\"uav-mag\""
        );
        let kind: SurveyKind = serde_json::from_str("\"uav-mag\"").expect("deserialize");
        assert_eq!(kind, SurveyKind::UavMag);
    }

    /// The PJ1 acceptance, inverted: a second type registered in tests yields
    /// its own checklist template and phase labels through the same generic
    /// registry/evaluation paths — no `if kind == ...` anywhere.
    #[test]
    fn a_registered_test_kind_serves_its_own_profile() {
        registry().register(SurveyProfile {
            kind: SurveyKind::TestWalk,
            name: "Walk (fixture)".into(),
            required_devices: vec![DeviceRole::RtkBaseStation],
            // The walk survey never arms: no Preflight stage, no motor test.
            phases: vec![
                PhaseId::Prepare,
                PhaseId::Plan,
                PhaseId::Fly,
                PhaseId::Review,
            ],
            safety_template: ChecklistTemplate {
                items: vec![
                    ChecklistItemSpec {
                        id: ChecklistItemId::LinkFresh,
                        mode: ChecklistItemMode::Required,
                    },
                    ChecklistItemSpec {
                        id: ChecklistItemId::GpsFix,
                        mode: ChecklistItemMode::WarnOnly,
                    },
                ],
            },
            qc_template: QcTemplate {},
            line_following: LineFollowing::WaypointMission,
            outputs: vec![OutputFormat::Csv],
        });

        let profile = registry()
            .profile(SurveyKind::TestWalk)
            .expect("registered fixture kind");
        assert_eq!(profile.phases.len(), 4);
        assert!(!profile.phases.contains(&PhaseId::Preflight));
        assert_eq!(profile.safety_template.items.len(), 2);
        assert_eq!(profile.outputs, vec![OutputFormat::Csv]);
        // The default profile is untouched by the registration.
        assert_eq!(registry().default_profile().phases.len(), 5);
        // Every kind the registry now serves, in stable order.
        assert_eq!(
            registry().kinds(),
            vec![SurveyKind::UavMag, SurveyKind::TestWalk]
        );
    }

    #[test]
    fn warn_only_items_never_block() {
        // A running motor test fails the motor-test item deterministically:
        // WarnOnly must downgrade that to a Warn (ack, not a block), while a
        // Required template entry keeps the Fail.
        let plan = crate::safety::PlanState {
            motor_test_active: true,
            ..crate::safety::PlanState::default()
        };
        let mode = |mode: ChecklistItemMode| {
            let template = ChecklistTemplate {
                items: vec![ChecklistItemSpec {
                    id: ChecklistItemId::MotorTestIdle,
                    mode,
                }],
            };
            crate::safety::evaluate_with_template(
                &crate::telemetry::TelemetrySnapshot::new(),
                &plan,
                &crate::safety::SafetyConfig::default(),
                1_000,
                &template,
            )
        };
        let required = mode(ChecklistItemMode::Required);
        assert_eq!(required.items[0].status, ChecklistStatus::Fail);
        let warn_only = mode(ChecklistItemMode::WarnOnly);
        assert_eq!(warn_only.items[0].status, ChecklistStatus::Warn);
    }
}
