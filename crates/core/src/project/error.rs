//! Project storage errors (ADR-019 §5): a corrupt file surfaces as
//! [`ProjectError::Corrupt`], a newer schema as [`ProjectError::UnsupportedVersion`],
//! a second process on the same project as [`ProjectError::Locked`] — the
//! caller keeps its last good state and raises a Warn; a corrupt file must
//! never stop the app from starting.

use std::path::PathBuf;

/// Errors from the project store.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// Filesystem failure on `path` (missing permission, disk full, …).
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The file exists but is not the schema this build understands.
    #[error("project file {path} is corrupt: {reason}")]
    Corrupt { path: PathBuf, reason: String },
    /// The expected file does not exist (e.g. `project.json` on `open`).
    #[error("not a project directory: {path} is missing")]
    Missing { path: PathBuf },
    /// The file was written by a newer MagGCS.
    #[error("schema version {found} is newer than supported version {supported}")]
    UnsupportedVersion { found: u32, supported: u32 },
    /// Another process holds the project's advisory lock.
    #[error("project {0} is already open in another process")]
    Locked(PathBuf),
    /// The project directory already holds that survey/session.
    #[error("{0} already exists")]
    Exists(PathBuf),
    /// An id is not a filesystem-safe slug (see `is_valid_id`).
    #[error("invalid id {0:?} (use A-Za-z0-9._-)")]
    InvalidId(String),
}
