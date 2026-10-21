//! Mag-calibration persistence (L8): `CalibrationStore` was RAM-only, so a
//! restart lost the calibration and the preflight row showed "off" until the
//! inspector window happened to re-apply its localStorage copy. The
//! calibration JSON now lives in a sidecar file next to `config.json`, is
//! written by the same `set_mag_calibration` command every window uses, and
//! is loaded back into the shared store at startup — the status (hash prefix)
//! the startup card shows comes from this module.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tauri::Manager;

use crate::state::{AppState, CalibrationStore};

const MAG_CAL_FILE: &str = "mag-calibration.json";

/// The sidecar path (next to `config.json` in `app_config_dir`).
///
/// # Errors
///
/// A stringified error when the config directory cannot be resolved.
pub fn mag_cal_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir: {e}"))?;
    Ok(dir.join(MAG_CAL_FILE))
}

/// Load a persisted calibration into the store; a missing file is the normal
/// first-launch state, a corrupt one is dropped with a warning (the operator
/// re-loads the file — same fail-soft rule as `config.json`).
pub fn load_from_path(path: &Path, store: &CalibrationStore) {
    let Ok(json) = std::fs::read_to_string(path) else {
        return;
    };
    match maggcs_core::calib::mag::MagCalibration::from_json(&json) {
        Ok(cal) => {
            store.set(Some(cal));
        }
        Err(e) => {
            eprintln!("mag-calibration.json is corrupt, ignoring: {e}");
        }
    }
}

/// Startup hook: hydrate the shared store from the sidecar file.
pub fn load_at_startup(app: &tauri::AppHandle, state: &AppState) {
    if let Ok(path) = mag_cal_path(app) {
        load_from_path(&path, &state.calibration);
    }
}

/// Persist (or clear) the calibration sidecar; returns the sha256 prefix the
/// status row shows. Called by `set_mag_calibration` — every window that
/// loads a calibration persists it through here, so the file is the single
/// source of truth (the inspector's localStorage mirror stays as a fallback).
///
/// # Errors
///
/// A stringified error when the file cannot be written or removed.
pub fn persist_to_path(path: &Path, json: Option<&str>) -> Result<Option<String>, String> {
    match json {
        Some(json) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(path, json).map_err(|e| e.to_string())?;
            Ok(Some(hash_prefix(json.as_bytes())))
        }
        None => {
            match std::fs::remove_file(path) {
                Ok(()) | Err(_) => {}
            }
            Ok(None)
        }
    }
}

/// The 8-hex sha256 prefix of the calibration JSON (L8 acceptance).
#[must_use]
pub fn hash_prefix(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// The status the startup card shows: loaded + the hash prefix of the
/// persisted file (not of the in-memory copy — the file is the truth).
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct MagCalStatus {
    pub loaded: bool,
    /// First 8 hex chars of the persisted calibration's sha256; `None` when
    /// nothing is persisted.
    pub hash8: Option<String>,
}

/// Read the persisted calibration status (the shared store mirrors it).
///
/// # Errors
///
/// A stringified error when the config directory cannot be resolved.
#[tauri::command]
pub fn mag_calibration_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<MagCalStatus, String> {
    let path = mag_cal_path(&app)?;
    let hash8 = std::fs::read(&path).ok().map(|bytes| hash_prefix(&bytes));
    Ok(MagCalStatus {
        loaded: state.calibration.get().is_some(),
        hash8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const CAL_JSON: &str =
        r#"{"a":[[1.0,0,0],[0,1.0,0],[0,0,1.0]],"b":[0,0,0],"k":null,"domain":"px4_raw"}"#;

    fn tmp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "maggcs-cal-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.join(MAG_CAL_FILE)
    }

    #[test]
    fn persist_writes_the_file_and_hashes_it() {
        let path = tmp_path("persist");
        let hash = persist_to_path(&path, Some(CAL_JSON)).expect("write");
        assert_eq!(hash.as_deref().map(str::len), Some(8));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), CAL_JSON);
        // Deterministic: the same bytes hash to the same prefix.
        assert_eq!(
            hash_prefix(CAL_JSON.as_bytes()),
            persist_to_path(&path, Some(CAL_JSON))
                .expect("again")
                .expect("hash")
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn clear_removes_the_file_and_the_hash() {
        let path = tmp_path("clear");
        persist_to_path(&path, Some(CAL_JSON)).expect("write");
        let hash = persist_to_path(&path, None).expect("clear");
        assert_eq!(hash, None);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn load_hydrates_the_store_and_drops_corrupt_files() {
        let path = tmp_path("load");
        persist_to_path(&path, Some(CAL_JSON)).expect("write");
        let store = CalibrationStore::default();
        load_from_path(&path, &store);
        assert!(store.get().is_some(), "valid calibration hydrates");

        std::fs::write(&path, "{ not json").expect("corrupt");
        let store = CalibrationStore::default();
        load_from_path(&path, &store);
        assert!(store.get().is_none(), "corrupt calibration is dropped");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_missing_file_is_the_normal_first_launch() {
        let store = CalibrationStore::default();
        load_from_path(Path::new("/nonexistent/mag-calibration.json"), &store);
        assert!(store.get().is_none());
    }
}
