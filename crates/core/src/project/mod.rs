//! Project / survey / session model and storage (ADR-019, plan PJ1).
//!
//! A project is a **local directory** — no cloud dependency:
//!
//! ```text
//! <project>/
//!   project.json                 # Project: name, CRS, datum, areas, settings
//!   resources/{dem,calibration,presets}/
//!   surveys/<survey-id>/
//!     survey.json                # Survey: kind, area, route, config, status
//!     sessions/<session-id>/session.json   # Session: status, operator, times
//!   .lock                        # advisory single-open guard
//! ```
//!
//! [`ProjectStore`] is the **single writer**: every read and write of the tree
//! goes through it, never by ad-hoc path joins. Writes are atomic (temp +
//! rename); the tree is locked against a second process with an advisory
//! (`fs2`) lock that the OS releases on process death; files carry a schema
//! version and migrate forward on load, while newer versions are rejected.
//!
//! Corruption policy (ADR-019 §5): a bad file surfaces as
//! [`ProjectError::Corrupt`]/[`ProjectError::Io`] — the caller keeps its last
//! good state and raises a Warn. A corrupt file must never stop the app from
//! starting.

mod error;
mod model;
mod store;

pub use error::ProjectError;
pub use model::{
    is_valid_id, Project, ProjectSettings, Session, SessionStatus, Survey, SurveyArea,
    SurveyConfig, SurveyStatus, LOCK_FILE_NAME, PROJECT_FILE_NAME, PROJECT_SCHEMA_VERSION,
    SESSION_FILE_NAME, SURVEY_FILE_NAME,
};
pub use store::{atomic_write, ProjectStore};
