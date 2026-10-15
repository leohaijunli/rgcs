//! `session.json`: the manifest that makes a recording directory
//! self-describing (what link, when, how much — plan §3 A4).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{RecorderConfig, RecorderError, EVENTS_FILE, SESSION_FILE, TLOG_FILE};

/// Bumped when the manifest schema changes incompatibly.
pub const SESSION_MANIFEST_VERSION: u32 = 1;

/// One recording session's metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionManifest {
    pub version: u32,
    /// Microseconds since the Unix epoch when the session opened.
    pub started_utc_us: u64,
    /// Set when the session closed cleanly; `None` after a crash.
    pub closed_utc_us: Option<u64>,
    /// The MAVLink endpoint the session recorded.
    pub endpoint: String,
    pub system_id: u8,
    pub component_id: u8,
    pub app_version: String,
    /// Frames written to the tlog.
    pub frames: u64,
    /// Events written to `events.jsonl`.
    pub events: u64,
    /// Frames/events dropped because the writer fell behind.
    pub dropped: u64,
    /// Relative paths, so a session directory stays movable.
    pub tlog: String,
    pub event_log: String,
}

impl SessionManifest {
    /// The manifest for a freshly opened session.
    pub fn new(config: &RecorderConfig, started_utc_us: u64) -> Self {
        Self {
            version: SESSION_MANIFEST_VERSION,
            started_utc_us,
            closed_utc_us: None,
            endpoint: config.endpoint.clone(),
            system_id: config.target_system_id,
            component_id: config.target_component_id,
            app_version: config.app_version.clone(),
            frames: 0,
            events: 0,
            dropped: 0,
            tlog: TLOG_FILE.to_string(),
            event_log: EVENTS_FILE.to_string(),
        }
    }
}

/// Write `session.json` atomically (temp file + rename), so a crash during the
/// final update cannot truncate a previously valid manifest.
pub(crate) fn write_manifest(dir: &Path, manifest: &SessionManifest) -> Result<(), RecorderError> {
    let path = dir.join(SESSION_FILE);
    let text = serde_json::to_string_pretty(manifest)?;
    let tmp = dir.join(format!("{SESSION_FILE}.tmp"));
    fs::write(&tmp, text.as_bytes())?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read a `session.json` (used by tests and the archive/QC tools).
pub fn read_manifest(path: &Path) -> Result<SessionManifest, RecorderError> {
    let text = fs::read_to_string(path).map_err(|e| RecorderError::Open {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(serde_json::from_str(&text)?)
}
