//! Core project model (ADR-019, plan PJ1): the `Project` → `Survey` →
//! `Session` hierarchy with its per-file schema version and migration.
//!
//! Every file carries `version: u32`. A version **newer** than
//! [`PROJECT_SCHEMA_VERSION`] is rejected (never silently downgraded); older
//! versions migrate in place on load. `#[serde(default)]` fills missing
//! fields, so a partial file still loads and one bad field does not lose the
//! rest (same policy as `core::config`).
//!
//! Timestamps are epoch microseconds (`u64`), the same convention as
//! [`crate::recorder::now_utc_us`] — one time format across the repo.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::ProjectError;
use crate::config::PlanPreset;
use crate::plan::{GeoPoint, PlannedWaypoint};
use crate::survey_profile::SurveyKind;

/// Schema version written into every project file. Bump when a field changes
/// meaning; older files migrate in [`Project::migrate`] and friends.
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// The project manifest, at the project root.
pub const PROJECT_FILE_NAME: &str = "project.json";
/// One survey per directory under `surveys/<survey-id>/`.
pub const SURVEY_FILE_NAME: &str = "survey.json";
/// One flight per directory under `surveys/<id>/sessions/<session-id>/`.
pub const SESSION_FILE_NAME: &str = "session.json";
/// Advisory lock guarding the project tree against a second process.
pub const LOCK_FILE_NAME: &str = ".lock";

/// Validate a survey/session id: a non-empty filesystem-safe slug that is its
/// directory name. `.` and `..` match the charset but must never be accepted.
#[must_use]
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Project-level settings the whole project shares (ADR-019 §6): the DEM
/// cache, calibration files, planning presets. Application-level settings
/// (endpoint, safety thresholds, …) stay in `core::config`'s `config.json`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ProjectSettings {
    /// DEM folder the project's surveys drape against (`resources/dem` by
    /// default; an absolute path when the operator picked another folder).
    pub dem_dir: Option<String>,
    /// Calibration folder (`resources/calibration` by default).
    pub calibration_dir: Option<String>,
    /// Named survey-parameter presets (moved from the app config; VIC source
    /// settings join here when M7 unfreezes).
    pub presets: Vec<PlanPreset>,
}

/// A survey area rectangle inside the project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct SurveyArea {
    pub name: String,
    /// Centre in WGS84 degrees.
    pub center: GeoPoint,
    /// Rectangle size, metres.
    pub width_m: f64,
    pub height_m: f64,
    /// Heading of the rectangle, degrees from north.
    pub heading_deg: f64,
}

impl Default for SurveyArea {
    fn default() -> Self {
        Self {
            name: String::new(),
            center: GeoPoint::new(0.0, 0.0),
            width_m: 100.0,
            height_m: 100.0,
            heading_deg: 0.0,
        }
    }
}

/// Survey execution status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SurveyStatus {
    #[default]
    Draft,
    Ready,
    Flying,
    Complete,
    Archived,
}

/// Survey flight configuration (the numbers the pattern/drape pipeline reads).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct SurveyConfig {
    /// Survey AGL above the DEM ground, metres.
    pub agl_m: f64,
    /// Cruise speed, m/s.
    pub speed_mps: f64,
    /// Line spacing, metres.
    pub line_spacing_m: f64,
    /// Tie-line spacing, metres (`None` = no tie lines).
    pub tie_spacing_m: Option<f64>,
    /// Altitude mode the route was planned in (`"amsl"` | `"terrain"`).
    pub altitude_mode: String,
}

impl Default for SurveyConfig {
    fn default() -> Self {
        Self {
            agl_m: 40.0,
            speed_mps: 5.0,
            line_spacing_m: 50.0,
            tie_spacing_m: None,
            altitude_mode: "amsl".to_string(),
        }
    }
}

/// One survey inside a project: a typed measurement (kind string until PJ2
/// introduces the typed [`crate::survey_profile`] registry) over an area,
/// with its route and configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct Survey {
    /// Schema version; see [`PROJECT_SCHEMA_VERSION`].
    pub version: u32,
    /// Directory name under `surveys/` — see [`is_valid_id`].
    pub id: String,
    /// Survey type (PJ2: typed enum; the serialized value stays the
    /// `"uav-mag"` string PJ1 wrote).
    pub kind: SurveyKind,
    pub name: String,
    pub area: SurveyArea,
    /// The planned route in mission order (AMSL waypoints, as compiled).
    pub route: Vec<PlannedWaypoint>,
    pub config: SurveyConfig,
    pub status: SurveyStatus,
    /// Creation time, epoch microseconds (a JSON number over the IPC).
    #[ts(type = "number")]
    pub created_utc_us: u64,
}

/// Session execution status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionStatus {
    #[default]
    Planned,
    Recording,
    Complete,
    Aborted,
}

/// One actual flight of a survey: its directory carries `session.json` now and
/// the recorder's artifacts (raw tlog, `events.jsonl`, manifest, QC) as those
/// WPs land (M3/M4).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct Session {
    /// Schema version; see [`PROJECT_SCHEMA_VERSION`].
    pub version: u32,
    /// Directory name under `sessions/` — see [`is_valid_id`].
    pub id: String,
    /// The owning survey's id.
    pub survey_id: String,
    pub status: SessionStatus,
    pub operator: Option<String>,
    /// First arm / takeoff, epoch microseconds (a JSON number over the IPC).
    #[ts(type = "number")]
    pub started_utc_us: Option<u64>,
    /// Land / abort, epoch microseconds (a JSON number over the IPC).
    #[ts(type = "number")]
    pub ended_utc_us: Option<u64>,
}

/// The project manifest: name, datum decision, areas and shared settings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct Project {
    /// Schema version; see [`PROJECT_SCHEMA_VERSION`].
    pub version: u32,
    pub name: String,
    /// Horizontal CRS of the project, e.g. `"EPSG:4326"` (ADR-018 §D: the
    /// NAD83(CSRS) ≡ WGS84 null-transform decision is recorded per source).
    pub crs: String,
    /// Vertical datum of the project's heights, e.g. `"cgvd2013"`.
    pub vertical_datum: String,
    /// Survey areas defined at project level.
    pub areas: Vec<SurveyArea>,
    pub notes: String,
    pub settings: ProjectSettings,
    /// Creation time, epoch microseconds (a JSON number over the IPC).
    #[ts(type = "number")]
    pub created_utc_us: u64,
}

impl Project {
    /// A fresh project stamped with the current schema version and time.
    #[must_use]
    pub fn new(name: &str, crs: &str, vertical_datum: &str, settings: ProjectSettings) -> Self {
        Self {
            version: PROJECT_SCHEMA_VERSION,
            name: name.to_string(),
            crs: crs.to_string(),
            vertical_datum: vertical_datum.to_string(),
            areas: Vec::new(),
            notes: String::new(),
            settings,
            created_utc_us: crate::recorder::now_utc_us(),
        }
    }

    /// Parse a project manifest body (see the module docs for the policy).
    ///
    /// # Errors
    ///
    /// [`crate::project::ProjectError::UnsupportedVersion`] when the file is
    /// newer than this build; JSON errors pass through.
    pub fn from_json(text: &str) -> Result<Self, ProjectError> {
        let mut project: Project =
            serde_json::from_str(text).map_err(|e| ProjectError::Corrupt {
                path: PathBuf::from(PROJECT_FILE_NAME),
                reason: e.to_string(),
            })?;
        if project.version > PROJECT_SCHEMA_VERSION {
            return Err(ProjectError::UnsupportedVersion {
                found: project.version,
                supported: PROJECT_SCHEMA_VERSION,
            });
        }
        project.migrate();
        Ok(project)
    }

    /// Serialize to pretty JSON (with a trailing newline).
    ///
    /// # Errors
    ///
    /// Serialization failure (a `serde_json` invariant, practically never).
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        let mut project = self.clone();
        project.version = PROJECT_SCHEMA_VERSION;
        let mut text = serde_json::to_string_pretty(&project)?;
        text.push('\n');
        Ok(text)
    }

    /// Upgrade an older schema in place. v1 is the first version, so the only
    /// work is stamping the current version (newer files are rejected by the
    /// store before this runs).
    fn migrate(&mut self) {
        self.version = PROJECT_SCHEMA_VERSION;
    }
}

impl Survey {
    /// Parse a survey body (policy as [`Project::from_json`]).
    ///
    /// # Errors
    ///
    /// JSON errors pass through; the store rejects newer versions.
    pub fn from_json(text: &str) -> Result<Self, ProjectError> {
        let mut survey: Survey = serde_json::from_str(text).map_err(|e| ProjectError::Corrupt {
            path: PathBuf::from(SURVEY_FILE_NAME),
            reason: e.to_string(),
        })?;
        if survey.version > PROJECT_SCHEMA_VERSION {
            return Err(ProjectError::UnsupportedVersion {
                found: survey.version,
                supported: PROJECT_SCHEMA_VERSION,
            });
        }
        survey.version = PROJECT_SCHEMA_VERSION;
        Ok(survey)
    }

    /// Serialize to pretty JSON (with a trailing newline).
    ///
    /// # Errors
    ///
    /// Serialization failure.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        let mut survey = self.clone();
        survey.version = PROJECT_SCHEMA_VERSION;
        let mut text = serde_json::to_string_pretty(&survey)?;
        text.push('\n');
        Ok(text)
    }
}

impl Session {
    /// Parse a session body (policy as [`Project::from_json`]).
    ///
    /// # Errors
    ///
    /// JSON errors pass through; the store rejects newer versions.
    pub fn from_json(text: &str) -> Result<Self, ProjectError> {
        let mut session: Session =
            serde_json::from_str(text).map_err(|e| ProjectError::Corrupt {
                path: PathBuf::from(SESSION_FILE_NAME),
                reason: e.to_string(),
            })?;
        if session.version > PROJECT_SCHEMA_VERSION {
            return Err(ProjectError::UnsupportedVersion {
                found: session.version,
                supported: PROJECT_SCHEMA_VERSION,
            });
        }
        session.version = PROJECT_SCHEMA_VERSION;
        Ok(session)
    }

    /// Serialize to pretty JSON (with a trailing newline).
    ///
    /// # Errors
    ///
    /// Serialization failure.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        let mut session = self.clone();
        session.version = PROJECT_SCHEMA_VERSION;
        let mut text = serde_json::to_string_pretty(&session)?;
        text.push('\n');
        Ok(text)
    }
}
