//! Project archive export/import (plan §5A PJ4): a project folder —
//! `project.json`, `surveys/` (incl. `survey.json` route tables), `sessions/`
//! (A4 recorder files + event logs), `resources/` — packs into one zip with a
//! sha256 manifest. Import verifies **every** hash before writing anything
//! and refuses on mismatch (the acceptance), on an existing project id, and
//! on unsupported manifest versions.
//!
//! The M4 mission manifest and the "exclude NRCan raw diurnal data" clause
//! join when M4/M7 unfreeze (plan §5A note).

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::project::{is_valid_id, Project};

pub const ARCHIVE_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "manifest.json";
const LOCK_FILE: &str = ".lock";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct ArchiveEntry {
    /// Zip path, `/`-separated, relative to the project root.
    pub name: String,
    /// Lowercase hex sha256 of the file contents.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct ArchiveManifest {
    pub schema_version: u32,
    pub project_id: String,
    pub project_name: String,
    /// Export wall clock, epoch microseconds (the recorder's convention).
    pub exported_utc_us: u64,
    /// The exporting build (`CARGO_PKG_VERSION`).
    pub app_version: String,
    pub files: Vec<ArchiveEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("project: {0}")]
    Project(#[from] crate::project::ProjectError),
    #[error("archive schema version {0} is not supported")]
    UnsupportedVersion(u32),
    #[error("the archive carries no manifest")]
    ManifestMissing,
    #[error("the archive is not a project export: {0}")]
    Invalid(String),
    #[error("hash mismatch for {name}: manifest {expected}, archive {found}")]
    HashMismatch {
        name: String,
        expected: String,
        found: String,
    },
    #[error("a project with id {0} already exists at the import root")]
    Exists(String),
}

pub type ArchiveResult<T> = Result<T, ArchiveError>;

/// Export the project under `root` into `out_zip`. Every file except the
/// advisory `.lock` is included; `manifest.json` lists each file's sha256.
///
/// # Errors
///
/// [`ArchiveError::Io`]/[`ArchiveError::Zip`] when reading or packing fails,
/// [`ArchiveError::Project`] when `project.json` cannot be re-read for the
/// manifest's project identity.
pub fn export_project(root: &Path, out_zip: &Path) -> ArchiveResult<ArchiveManifest> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.name.cmp(&b.name));

    // The manifest identity comes from the project file on disk, not from a
    // caller-supplied string (the archive must describe what is in it).
    let project_json = fs::read_to_string(root.join("project.json"))?;
    let project = Project::from_json(&project_json)?;
    // The archive identity is the project **folder** name — that is what
    // `ProjectStore::open` addresses and what the import root joins below.
    let project_id = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| ArchiveError::Invalid("the project root has no folder name".into()))?;
    if !is_valid_id(&project_id) {
        return Err(ArchiveError::Invalid(format!(
            "project folder `{project_id}` is not a valid slug"
        )));
    }

    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    let manifest = ArchiveManifest {
        schema_version: ARCHIVE_SCHEMA_VERSION,
        project_id,
        project_name: project.name.clone(),
        exported_utc_us: now_us,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        files,
    };

    if let Some(parent) = out_zip.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(out_zip)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for entry in &manifest.files {
        let bytes = fs::read(root.join(&entry.name))?;
        zip.start_file(entry.name.as_str(), options)
            .map_err(ArchiveError::from)?;
        zip.write_all(&bytes)?;
    }
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    zip.start_file(MANIFEST_FILE, options)
        .map_err(ArchiveError::from)?;
    zip.write_all(manifest_json.as_bytes())?;
    zip.finish().map_err(ArchiveError::from)?;
    Ok(manifest)
}

/// Recursively collect `name` + sha256 for every file under `root`, skipping
/// the advisory `.lock` (a stale lock is process state, not project data).
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<ArchiveEntry>) -> ArchiveResult<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.file_name().is_some_and(|n| n == LOCK_FILE) {
            continue;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| ArchiveError::Invalid(format!("path walk: {e}")))?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path)?;
            out.push(ArchiveEntry {
                name: rel,
                sha256: hash_hex(&bytes),
            });
        }
    }
    Ok(())
}

/// Import `zip_path` into `projects_root/<project_id>/`. Every manifest entry
/// is hash-verified from the zip's bytes **before** anything is written; a
/// single mismatch rejects the whole import (plan PJ4 acceptance) and leaves
/// no partial project behind. An existing project id at the root refuses.
///
/// # Errors
///
/// [`ArchiveError::HashMismatch`] on any hash mismatch,
/// [`ArchiveError::Exists`] when the target project folder already exists,
/// [`ArchiveError::UnsupportedVersion`] for a future schema,
/// [`ArchiveError::ManifestMissing`] / [`ArchiveError::Invalid`] when the zip
/// is not a project export, plus I/O and zip errors.
pub fn import_project(zip_path: &Path, projects_root: &Path) -> ArchiveResult<PathBuf> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    // The manifest first: it names the target and the files to verify.
    let manifest: ArchiveManifest = {
        let mut manifest_file = archive
            .by_name(MANIFEST_FILE)
            .map_err(|_| ArchiveError::ManifestMissing)?;
        let mut text = String::new();
        manifest_file.read_to_string(&mut text)?;
        serde_json::from_str(&text)?
    };
    if manifest.schema_version != ARCHIVE_SCHEMA_VERSION {
        return Err(ArchiveError::UnsupportedVersion(manifest.schema_version));
    }
    if !is_valid_id(&manifest.project_id) {
        return Err(ArchiveError::Invalid(format!(
            "project id `{}` is not a valid slug",
            manifest.project_id
        )));
    }

    // Read everything into memory (project archives are JSON + logs, tens of
    // MB at worst), then verify — a mismatch must reject before any write.
    let mut contents: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();
        if name == MANIFEST_FILE {
            continue;
        }
        if name.ends_with('/') {
            continue; // directory entries the writer may have added
        }
        if !manifest.files.iter().any(|e| e.name == name) {
            return Err(ArchiveError::Invalid(format!(
                "the archive contains {name}, which the manifest does not list"
            )));
        }
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes)?;
        contents.push((name, bytes));
    }
    for entry in &manifest.files {
        let Some((_, bytes)) = contents.iter().find(|(name, _)| *name == entry.name) else {
            return Err(ArchiveError::Invalid(format!(
                "manifest lists {}, which the archive does not contain",
                entry.name
            )));
        };
        let found = hash_hex(bytes);
        if found != entry.sha256 {
            return Err(ArchiveError::HashMismatch {
                name: entry.name.clone(),
                expected: entry.sha256.clone(),
                found,
            });
        }
    }

    let target = projects_root.join(&manifest.project_id);
    if target.exists() {
        return Err(ArchiveError::Exists(manifest.project_id));
    }
    fs::create_dir_all(&target)?;
    for (name, bytes) in &contents {
        let path = target.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
    }
    // The import is only good if the project file parses (the route tables
    // come from `survey.json` — the acceptance rebuilds from it).
    let project_json = fs::read_to_string(target.join("project.json"))?;
    Project::from_json(&project_json)?;
    Ok(target)
}

fn hash_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectStore;
    use crate::project::{Project, Survey, SurveyStatus, PROJECT_SCHEMA_VERSION};

    fn sample_project(tag: &str) -> (PathBuf, String, String) {
        let root = std::env::temp_dir().join(format!(
            "maggcs-archive-src-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("mkdir");
        let id = format!("archive-{tag}");
        let store = ProjectStore::create(
            &root.join(&id),
            Project::new("Archive Test", "EPSG:4326", "cgvd2013", Default::default()),
        )
        .expect("create project");
        let survey = Survey {
            version: PROJECT_SCHEMA_VERSION,
            id: "s1".into(),
            kind: crate::survey_profile::SurveyKind::UavMag,
            name: "Archive line".into(),
            status: SurveyStatus::default(),
            ..Survey::default()
        };
        store.save_survey(&survey).expect("save survey");
        let survey_json = fs::read_to_string(root.join(&id).join("surveys/s1/survey.json"))
            .expect("survey on disk");
        drop(store);
        (root, survey_json, id)
    }

    /// PJ4 acceptance: import rebuilds the same project and the survey route
    /// table comes back from `survey.json` byte-for-byte.
    #[test]
    fn export_then_import_rebuilds_the_project_and_its_route_tables() {
        let (parent, survey_json, id) = sample_project("roundtrip");
        let root = parent.join(&id);
        let zip_path = parent.join("archive-test.zip");
        let manifest = export_project(&root, &zip_path).expect("export");
        assert_eq!(manifest.project_id, id);
        assert!(
            manifest
                .files
                .iter()
                .any(|e| e.name == "surveys/s1/survey.json"),
            "the route table is in the archive"
        );

        let import_root = std::env::temp_dir().join(format!(
            "maggcs-archive-dst-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&import_root).expect("mkdir");
        let target = import_project(&zip_path, &import_root).expect("import");

        let store = ProjectStore::open(&target).expect("reopen imported project");
        assert_eq!(store.project().name, "Archive Test");
        let survey = store.load_survey("s1").expect("survey");
        assert_eq!(survey.id, "s1");
        let imported_json = fs::read_to_string(target.join("surveys/s1/survey.json"))
            .expect("imported survey.json");
        assert_eq!(imported_json, survey_json, "route table rebuilt verbatim");
        drop(store);
        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_file(&zip_path);
        let _ = fs::remove_dir_all(&import_root);
    }

    /// A single flipped byte must reject the whole import, leaving no
    /// partial project on disk (the acceptance).
    #[test]
    fn a_hash_mismatch_rejects_the_import_and_writes_nothing() {
        let (parent, _, id) = sample_project("tamper");
        let root = parent.join(&id);
        let zip_path = parent.join("tamper.zip");
        export_project(&root, &zip_path).expect("export");

        // Tamper: rewrite the zip with one file's contents corrupted by
        // re-exporting with an extra line appended to project.json's copy
        // inside a fresh zip built by hand.
        let tampered = build_tampered_zip(&zip_path, &root);
        assert!(tampered, "the tampered zip was rebuilt");

        let import_root = std::env::temp_dir().join(format!(
            "maggcs-archive-tamper-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&import_root).expect("mkdir");
        let err = import_project(&zip_path, &import_root).expect_err("must reject");
        assert!(
            matches!(err, ArchiveError::HashMismatch { .. }),
            "got {err}"
        );
        assert!(
            !import_root.join(&id).exists(),
            "no partial project may survive a rejected import"
        );
        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_file(&zip_path);
        let _ = fs::remove_dir_all(&import_root);
    }

    /// Rebuild the zip with `project.json`'s contents altered but the
    /// manifest unchanged (the classic corrupted-transfer shape).
    fn build_tampered_zip(original: &Path, _root: &Path) -> bool {
        let file = File::open(original).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("read");
        let out = File::create(original.with_extension("tmp.zip")).expect("create");
        let mut writer = ZipWriter::new(out);
        let options = SimpleFileOptions::default();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).expect("entry");
            let name = entry.name().to_string();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).expect("read");
            if name == "project.json" {
                bytes.extend_from_slice(b"\n");
            }
            writer.start_file(name.as_str(), options).expect("start");
            writer.write_all(&bytes).expect("write");
        }
        writer.finish().expect("finish");
        fs::rename(original.with_extension("tmp.zip"), original).is_ok()
    }

    #[test]
    fn an_existing_project_id_refuses_the_import() {
        let (parent, _, id) = sample_project("exists");
        let root = parent.join(&id);
        let zip_path = parent.join("exists.zip");
        export_project(&root, &zip_path).expect("export");
        let import_root = parent.clone();
        // The source project itself sits at the import root under the same id.
        let err = import_project(&zip_path, &import_root).expect_err("must refuse");
        assert!(matches!(err, ArchiveError::Exists(_)), "got {err}");
        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_file(&zip_path);
    }

    #[test]
    fn a_future_manifest_version_is_rejected() {
        let (parent, _, id) = sample_project("future");
        let root = parent.join(&id);
        let zip_path = parent.join("future.zip");
        export_project(&root, &zip_path).expect("export");
        // Rewrite with a bumped schema version in the manifest.
        let file = File::open(&zip_path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("read");
        let out = File::create(zip_path.with_extension("tmp.zip")).expect("create");
        let mut writer = ZipWriter::new(out);
        let options = SimpleFileOptions::default();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).expect("entry");
            let name = entry.name().to_string();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).expect("read");
            if name == MANIFEST_FILE {
                let text = String::from_utf8(bytes).expect("manifest text");
                bytes = text
                    .replace("\"schema_version\": 1", "\"schema_version\": 99")
                    .into_bytes();
            }
            writer.start_file(name.as_str(), options).expect("start");
            writer.write_all(&bytes).expect("write");
        }
        writer.finish().expect("finish");
        fs::rename(zip_path.with_extension("tmp.zip"), &zip_path).expect("rename");

        let import_root = std::env::temp_dir().join("maggcs-archive-future-root");
        fs::create_dir_all(&import_root).expect("mkdir");
        let err = import_project(&zip_path, &import_root).expect_err("must reject");
        assert!(
            matches!(err, ArchiveError::UnsupportedVersion(99)),
            "got {err}"
        );
        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_file(&zip_path);
        let _ = fs::remove_dir_all(&import_root);
    }
}
