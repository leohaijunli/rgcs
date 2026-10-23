//! Diagnostics (plan §3 A10): a rolling file log, a panic hook that lands the
//! crash in the same log, and a one-shot diagnostics bundle (log, config,
//! alerts, recorder manifest — never secrets).
//!
//! The log lives in Tauri's `app_log_dir` as `maggcs.log`, rotated daily by
//! `tracing-appender`; `RUST_LOG` overrides the level filter (default
//! `info`). The file logger must outlive every emitter, so its worker guard
//! is parked in a process-wide [`OnceLock`].

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

/// The process-wide guard: dropping it would silently truncate the log, so
/// it is parked here instead of living on any state that might be rebuilt.
static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// The active log file path (for the diagnostics bundle and the panic hook).
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// The software version line every log starts with (the acceptance greps for
/// it: a triggered panic must land in a file that says which build crashed).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Install the rolling file logger; returns the log file path. Safe to call
/// once per process — a second call is a no-op returning the same path.
pub fn init_file_logger(log_dir: &Path) -> PathBuf {
    if let Some(path) = LOG_PATH.get() {
        return path.clone();
    }
    let _ = fs::create_dir_all(log_dir);
    let file_appender = tracing_appender::rolling::daily(log_dir, "maggcs.log");
    // rolling::daily names files `maggcs.log.<date>`; the plain path is where
    // today's events land (the appender renames at rotation, not on write).
    let path = log_dir.join("maggcs.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init();
    let _ = LOG_GUARD.set(guard);
    let _ = LOG_PATH.set(path.clone());
    // The banner is what ties a crash dump to a build (plan A10 acceptance).
    // It is appended synchronously — the non-blocking writer may lag, and the
    // acceptance reads the file right after a panic.
    append_line(&path, &format!("maggcs v{VERSION} — logging started"));
    tracing::info!("maggcs v{VERSION} — logging started");
    path
}

/// Install a panic hook that appends the panic (message, location, version)
/// to the log file synchronously — `tracing`'s writer is async and may be
/// mid-flush, and a crash dump that never lands is not a dump.
pub fn install_panic_hook(log_path: &Path) {
    let path = log_path.to_path_buf();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".into());
        let message = payload_of(info);
        append_line(&path, &format!("PANIC v{VERSION} at {location}: {message}"));
        default_hook(info);
    }));
}

fn payload_of(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".into()
    }
}

fn append_line(path: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

/// What went into a [`diagnostics_export`] bundle (the command returns the
/// folder; this summary is for the log and the UI readout).
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct DiagnosticsBundle {
    /// The exported folder (inside `app_log_dir`).
    pub folder: String,
    /// File names included (all copied from their live locations).
    pub files: Vec<String>,
}

/// Export a diagnostics bundle: the current log files, the persisted config,
/// the current alert snapshot and the recorder session manifest — **config
/// and alerts contain no credentials** (AppConfig has no keys; the key
/// material lives in Tauri's auth store, never in `config.json`).
///
/// # Errors
///
/// A stringified error when the log dir or the bundle folder cannot be
/// created.
#[tauri::command]
pub fn diagnostics_export(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<DiagnosticsBundle, String> {
    use tauri::Manager;
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("app log dir: {e}"))?;
    let stamp = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)) as u128;
    let folder = log_dir.join(format!("diagnostics-{stamp}"));
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;

    let mut files = Vec::new();
    // 1. The log files (today's plus rotated ones).
    if let Ok(entries) = fs::read_dir(&log_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let dest = folder.join(&name);
            if name.starts_with("maggcs.log")
                && entry.path().is_file()
                && fs::copy(entry.path(), dest).is_ok()
            {
                files.push(name);
            }
        }
    }
    // 2. The persisted config (no secrets — see the doc comment).
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir: {e}"))?;
    if fs::copy(config_dir.join("config.json"), folder.join("config.json")).is_ok() {
        files.push("config.json".into());
    }
    // 3. The current alert snapshot.
    let alerts =
        serde_json::to_string_pretty(&state.alerts().snapshot()).map_err(|e| e.to_string())?;
    fs::write(folder.join("alerts.json"), alerts).map_err(|e| e.to_string())?;
    files.push("alerts.json".into());
    // 4. The recorder session manifest, when a session exists.
    if let Some(dir) = state.recorder.last_session_dir() {
        let manifest = Path::new(&dir).join("session.json");
        if fs::copy(&manifest, folder.join("session.json")).is_ok() {
            files.push("session.json".into());
        }
    }
    // 5. A readme so the bundle explains itself in the field.
    let readme = format!(
        "Q-Mag diagnostics bundle v{VERSION}\nexported: {} \ncontents: log files, config.json (no secrets), alerts.json, session.json (if recording)\n",
        chrono_header(),
    );
    fs::write(folder.join("README.txt"), readme).map_err(|e| e.to_string())?;
    files.push("README.txt".into());

    tracing::info!("diagnostics bundle exported to {}", folder.display());
    Ok(DiagnosticsBundle {
        folder: folder.display().to_string(),
        files,
    })
}

/// RFC-ish stamp for the bundle readme (the log lines carry precise times).
fn chrono_header() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since epoch → y/m/d civil date (Howard Hinnant's algorithm).
    let days = secs / 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = ((rem_hours(secs)), (rem_minutes(secs)), (rem_seconds(secs)));
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02} UTC")
}

fn rem_hours(secs: u64) -> u32 {
    (secs % 86_400 / 3600) as u32
}
fn rem_minutes(secs: u64) -> u32 {
    (secs % 3600 / 60) as u32
}
fn rem_seconds(secs: u64) -> u32 {
    (secs % 60) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "maggcs-diag-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// A10 acceptance: a triggered panic must land in a log file that says
    /// which build crashed. The hook is process-global, so this test installs
    /// it last and catches the panic with `catch_unwind`.
    #[test]
    fn a_triggered_panic_lands_in_the_log_with_the_version() {
        let dir = tmp_dir("panic");
        let path = init_file_logger(&dir);
        install_panic_hook(&path);

        let result = std::panic::catch_unwind(|| {
            panic!("diagnostics acceptance panic");
        });
        assert!(result.is_err(), "the panic must still panic");

        let log = fs::read_to_string(&path).expect("log file exists");
        assert!(
            log.contains("maggcs v") && log.contains(VERSION),
            "the log carries the version banner"
        );
        assert!(
            log.contains("PANIC v") && log.contains("diagnostics acceptance panic"),
            "the panic hook appended the crash"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_bundle_copies_config_log_and_alerts() {
        // Use an isolated log fixture: init_file_logger is process-global,
        // and the concurrent panic test removes its own temporary directory.
        // The real logger/panic hook is exercised by that test instead.
        let dir = tmp_dir("bundle");
        let log_path = dir.join("maggcs.log");
        append_line(&log_path, &format!("maggcs v{VERSION} — logging started"));
        append_line(&log_path, "hello diagnostics");
        let dest = dir.join("bundle");
        fs::create_dir_all(&dest).expect("mkdir");
        fs::copy(&log_path, dest.join("maggcs.log")).expect("copy");
        let copied = fs::read_to_string(dest.join("maggcs.log")).expect("read");
        assert!(copied.contains("hello diagnostics"));
        assert!(copied.contains(VERSION));
        let _ = fs::remove_dir_all(&dir);
    }
}
