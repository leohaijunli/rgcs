//! GDAL/PROJ discovery (ADR-018 addendum).
//!
//! dem-prep shells out to the GDAL/PROJ command-line tools and finds them on
//! `PATH` (ADR-018: never linked, so no libgdal headers at build time). Desktop
//! installs, however, usually place GDAL in a user-writable prefix — a conda
//! env, `~/.local/gdal-env`, Homebrew — that is not on the `PATH` the app
//! inherits (a `.desktop` launch gives dem-prep a minimal environment). This
//! module finds such a prefix and prepends its `bin` to the process `PATH`,
//! plus points `PROJ_DATA`/`PROJ_LIB` at its `share/proj`, so an already
//! installed GDAL is used without the operator editing their shell profile.
//!
//! It is deliberately conservative: it does nothing when a probe tool is
//! already on `PATH` (the operator/CI setup keeps precedence), it only
//! prepends a `bin` that actually contains the probe, and it never overwrites
//! an operator-set `PROJ_DATA`/`PROJ_LIB`. Nothing is installed or downloaded.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Explicit override: a GDAL prefix (the directory holding `bin/` and
/// `share/proj`). Checked before any of the common locations.
pub const PREFIX_ENV: &str = "MAGGCS_GDAL_PREFIX";

/// The tool used to probe a candidate prefix — the first one dem-prep needs.
const PROBE_TOOL: &str = "gdaltransform";

/// Prepend an autodetected GDAL prefix to `PATH` when the tools are missing.
///
/// Returns the prefix that was used, or `None` when GDAL is already on `PATH`
/// or no candidate prefix holds the probe tool (the caller then fails later
/// with the actionable [`crate::hrdem::HrdemError::GdalMissing`]).
pub fn augment_path() -> Option<PathBuf> {
    if is_on_path(PROBE_TOOL) {
        return None;
    }
    let prefix = first_prefix_with_tool(&candidate_prefixes(), PROBE_TOOL)?;
    let bin = prefix.join("bin");
    std::env::set_var(
        "PATH",
        path_with_prefix(std::env::var_os("PATH").as_deref(), &bin),
    );

    // Point PROJ at the matching data dir, but never clobber the operator's.
    let proj = prefix.join("share").join("proj");
    if proj.is_dir() {
        if std::env::var_os("PROJ_DATA").is_none() {
            std::env::set_var("PROJ_DATA", &proj);
        }
        if std::env::var_os("PROJ_LIB").is_none() {
            std::env::set_var("PROJ_LIB", &proj);
        }
    }
    Some(prefix)
}

/// Prefixes to probe, in priority order: the explicit override, the active
/// conda env, then the well-known per-user / system install locations.
fn candidate_prefixes() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };

    for var in [PREFIX_ENV, "CONDA_PREFIX"] {
        if let Some(value) = std::env::var_os(var) {
            if !value.is_empty() {
                push(PathBuf::from(value));
            }
        }
    }
    if let Some(home) = home_dir() {
        for rel in [
            ".local/gdal-env",
            ".local/gdal",
            "miniconda3",
            "anaconda3",
            "miniforge3",
            "mambaforge",
        ] {
            push(home.join(rel));
        }
    }
    for abs in ["/opt/homebrew", "/usr/local", "/opt/local", "/usr"] {
        push(PathBuf::from(abs));
    }
    #[cfg(windows)]
    for abs in [r"C:\Program Files\GDAL", r"C:\OSGeo4W", r"C:\OSGeo4W64"] {
        push(PathBuf::from(abs));
    }
    out
}

/// The first prefix whose `bin` holds the probe tool, or `None`. Pure over the
/// candidate list so it can be unit-tested against a scratch directory tree.
fn first_prefix_with_tool(candidates: &[PathBuf], tool: &str) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|prefix| tool_in_prefix(prefix, tool))
        .cloned()
}

/// Whether `prefix/bin/<tool>` (or `.exe` on Windows) is an executable file.
fn tool_in_prefix(prefix: &Path, tool: &str) -> bool {
    let bin = prefix.join("bin");
    is_executable_file(&bin.join(tool)) || is_executable_file(&bin.join(format!("{tool}.exe")))
}

/// Whether a tool resolves on the current `PATH` (any lookup would find it).
fn is_on_path(tool: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        is_executable_file(&dir.join(tool)) || is_executable_file(&dir.join(format!("{tool}.exe")))
    })
}

/// `PATH` with `dir` prepended, preserving the rest; unchanged if `dir` is
/// already present (so a duplicate entry cannot reorder precedence). Pure.
fn path_with_prefix(existing: Option<&std::ffi::OsStr>, dir: &Path) -> OsString {
    let Some(existing) = existing else {
        return dir.as_os_str().to_owned();
    };
    if existing.is_empty() {
        return dir.as_os_str().to_owned();
    }
    if std::env::split_paths(existing).any(|p| p == dir) {
        return existing.to_owned();
    }
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut out = dir.as_os_str().to_owned();
    out.push(sep);
    out.push(existing);
    out
}

/// `$HOME` (Unix) / `%USERPROFILE%` (Windows).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// A regular file with an executable bit set (on Unix; plain file elsewhere).
fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_first_candidate_that_holds_the_probe() {
        let root = std::env::temp_dir().join(format!("maggcs-gdal-{}", std::process::id()));
        let empty = root.join("empty");
        let good = root.join("good");
        std::fs::create_dir_all(empty.join("bin")).unwrap();
        std::fs::create_dir_all(good.join("bin")).unwrap();
        std::fs::write(empty.join("bin/other"), b"x").unwrap();
        let probe = good.join("bin").join(PROBE_TOOL);
        std::fs::write(&probe, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let found = first_prefix_with_tool(&[empty.clone(), good.clone()], PROBE_TOOL);
        assert_eq!(found.as_deref(), Some(good.as_path()));
        let none = first_prefix_with_tool(&[empty], PROBE_TOOL);
        assert_eq!(none, None);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_non_executable_probe_is_not_accepted_on_unix() {
        #[cfg(unix)]
        {
            let root =
                std::env::temp_dir().join(format!("maggcs-gdal-noexec-{}", std::process::id()));
            let prefix = root.join("prefix");
            std::fs::create_dir_all(prefix.join("bin")).unwrap();
            // 0o644: readable but not executable — must not count as a tool.
            std::fs::write(prefix.join("bin").join(PROBE_TOOL), b"x").unwrap();
            assert!(!tool_in_prefix(&prefix, PROBE_TOOL));
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn path_prefix_prepends_and_preserves_the_rest() {
        let dir = Path::new("/opt/gdal/bin");
        let got = path_with_prefix(Some(std::ffi::OsStr::new("/usr/bin:/bin")), dir);
        let parts: Vec<PathBuf> = std::env::split_paths(&got).collect();
        assert_eq!(
            parts,
            vec![
                PathBuf::from("/opt/gdal/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin")
            ]
        );
    }

    #[test]
    fn path_prefix_is_idempotent() {
        let dir = Path::new("/opt/gdal/bin");
        let existing = std::ffi::OsStr::new("/opt/gdal/bin:/usr/bin");
        let got = path_with_prefix(Some(existing), dir);
        assert_eq!(got, std::ffi::OsStr::new("/opt/gdal/bin:/usr/bin"));
    }

    #[test]
    fn path_prefix_handles_an_absent_or_empty_path() {
        let dir = Path::new("/opt/gdal/bin");
        assert_eq!(path_with_prefix(None, dir), dir.as_os_str());
        assert_eq!(
            path_with_prefix(Some(std::ffi::OsStr::new("")), dir),
            dir.as_os_str()
        );
    }
}
