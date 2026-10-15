//! Versioned config file service (plan §3 A6).
//!
//! Owns the read/write of `config.json` under Tauri's `app_config_dir`; the
//! schema and its migration live in `core::config` (ADR-002). The file I/O is
//! split into free functions taking a path so the format is unit-testable
//! without a Tauri runtime (same pattern as `write_sitl_home_file`).
//!
//! Failure policy: a missing or unusable file returns `None` and prints a
//! warning — the frontend then keeps its compiled-in defaults and rewrites a
//! fresh file on the next change, so a corrupt file can never stop the app
//! from starting.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use maggcs_core::config::{AppConfig, CONFIG_FILE_NAME};
use tauri::{AppHandle, Manager};

/// `app_config_dir/config.json`.
fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("cannot resolve app config dir: {e}"))?;
    Ok(dir.join(CONFIG_FILE_NAME))
}

/// Read and parse a config file. Returns `None` (and warns) when the file is
/// absent or unusable, so the caller falls back to defaults.
fn load_from_path(path: &Path) -> Option<AppConfig> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("warning: cannot read config {}: {e}", path.display());
            return None;
        }
    };
    match AppConfig::from_json(&text) {
        Ok(config) => Some(config),
        Err(e) => {
            eprintln!(
                "warning: ignoring unreadable config {}: {e}",
                path.display()
            );
            None
        }
    }
}

/// Write `config` atomically: serialize to a sibling temp file, then rename
/// over the target so a crash mid-write cannot leave a truncated file.
fn write_to_path(path: &Path, config: &AppConfig) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = config
        .to_json_pretty()
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e.to_string()))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, json.as_bytes())?;
    fs::rename(&tmp, path)
}

/// Load the operator preferences (or `None` when none are stored yet).
#[tauri::command]
pub fn config_load(app: AppHandle) -> Result<Option<AppConfig>, String> {
    Ok(load_from_path(&config_path(&app)?))
}

/// Persist the operator preferences. Values are normalized by the core schema
/// before writing, so a buggy caller cannot store an out-of-range setting.
#[tauri::command]
pub fn config_save(app: AppHandle, config: AppConfig) -> Result<(), String> {
    write_to_path(&config_path(&app)?, &config.normalized()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maggcs_core::config::{DEFAULT_ALERT_VOLUME, DEFAULT_ENDPOINT};
    use maggcs_core::plan::GeoPoint;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("maggcs-config-{}-{}", tag, std::process::id()))
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = temp_dir("roundtrip");
        let path = dir.join(CONFIG_FILE_NAME);
        let config = AppConfig {
            endpoint: "tcpout:10.0.0.2:5760".to_string(),
            auto_connect: false,
            initial_position: GeoPoint::new(51.0, -114.0),
            alert_volume: 0.3,
            ..AppConfig::default()
        };
        write_to_path(&path, &config).expect("write");
        let loaded = load_from_path(&path).expect("load");
        assert_eq!(loaded, config.normalized());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_none() {
        let path = temp_dir("missing").join(CONFIG_FILE_NAME);
        assert!(load_from_path(&path).is_none());
    }

    #[test]
    fn corrupt_file_is_none_and_keeps_defaults_available() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, b"{ this is not json").expect("write");
        // No panic; caller falls back to `AppConfig::default()`.
        assert!(load_from_path(&path).is_none());
        let fallback = AppConfig::default();
        assert_eq!(fallback.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(fallback.alert_volume, DEFAULT_ALERT_VOLUME);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_normalizes_out_of_range_values() {
        let dir = temp_dir("normalize");
        let path = dir.join(CONFIG_FILE_NAME);
        let config = AppConfig {
            alert_volume: 5.0,
            ..AppConfig::default()
        };
        write_to_path(&path, &config.normalized()).expect("write");
        let loaded = load_from_path(&path).expect("load");
        assert_eq!(loaded.alert_volume, 1.0);
        let _ = fs::remove_dir_all(&dir);
    }
}
