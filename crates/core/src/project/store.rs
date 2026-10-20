//! The project store (ADR-019): the single writer of a project tree.
//!
//! - **Atomic writes**: every file goes to a sibling `<name>.tmp`, is flushed,
//!   then renamed over the target — a crash mid-write cannot leave a truncated
//!   file, and a leftover temp is ignored on load.
//! - **Single open**: the store holds an advisory exclusive lock on
//!   `<root>/.lock` for its whole lifetime; a second `open`/`create` on the
//!   same tree fails with [`ProjectError::Locked`]. The OS releases the lock
//!   when the process dies, so there is no stale-lock cleanup.
//! - **Migration**: files older than [`PROJECT_SCHEMA_VERSION`] are stamped
//!   forward on load; newer files are rejected.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::error::ProjectError;
use super::model::{
    is_valid_id, Project, Session, Survey, LOCK_FILE_NAME, PROJECT_FILE_NAME, SESSION_FILE_NAME,
    SURVEY_FILE_NAME,
};

const RESOURCES_DIR: &str = "resources";
const SURVEYS_DIR: &str = "surveys";
const SESSIONS_DIR: &str = "sessions";
const DEM_DIR: &str = "dem";
const CALIBRATION_DIR: &str = "calibration";
const PRESETS_DIR: &str = "presets";

/// The three persisted file types, unified for the store's read/write
/// helpers: parse migrates (and rejects newer schemas), serialize stamps the
/// current version so the on-disk file is always up to date.
trait ProjectFile: Sized {
    fn parse(text: &str) -> Result<Self, ProjectError>;
    fn serialize_pretty(&self) -> Result<String, serde_json::Error>;
}

impl ProjectFile for Project {
    fn parse(text: &str) -> Result<Self, ProjectError> {
        Project::from_json(text)
    }
    fn serialize_pretty(&self) -> Result<String, serde_json::Error> {
        self.to_json_pretty()
    }
}

impl ProjectFile for Survey {
    fn parse(text: &str) -> Result<Self, ProjectError> {
        Survey::from_json(text)
    }
    fn serialize_pretty(&self) -> Result<String, serde_json::Error> {
        self.to_json_pretty()
    }
}

impl ProjectFile for Session {
    fn parse(text: &str) -> Result<Self, ProjectError> {
        Session::from_json(text)
    }
    fn serialize_pretty(&self) -> Result<String, serde_json::Error> {
        self.to_json_pretty()
    }
}

/// An open project: the loaded manifest plus the advisory lock guard.
pub struct ProjectStore {
    root: PathBuf,
    project: Project,
    /// Held for its Drop: closing the file releases the OS lock.
    _lock: File,
}

impl ProjectStore {
    /// Create a new project directory at `root` (which must not already be a
    /// project) and open it. `resources/` and `surveys/` are created empty.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Exists`] when `root` already holds a `project.json`;
    /// [`ProjectError::Locked`] when another process has the tree open; I/O
    /// errors pass through.
    pub fn create(root: &Path, project: Project) -> Result<Self, ProjectError> {
        let manifest = root.join(PROJECT_FILE_NAME);
        if manifest.is_file() {
            return Err(ProjectError::Exists(root.to_path_buf()));
        }
        fs::create_dir_all(root).map_err(|e| ProjectError::Io {
            path: root.to_path_buf(),
            source: e,
        })?;
        let store = Self::acquire(root, project)?;
        for dir in [
            store.resources_dir(),
            store.dem_dir(),
            store.calibration_dir(),
            store.presets_dir(),
            store.surveys_dir(),
        ] {
            fs::create_dir_all(&dir).map_err(|e| ProjectError::Io {
                path: dir,
                source: e,
            })?;
        }
        store.write_json(&store.manifest_path(), &store.project)?;
        Ok(store)
    }

    /// Open an existing project: take the lock, load and migrate the manifest.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Missing`] when `project.json` is absent; corrupt/newer
    /// files as [`ProjectError::Corrupt`] / [`ProjectError::UnsupportedVersion`].
    pub fn open(root: &Path) -> Result<Self, ProjectError> {
        let manifest = root.join(PROJECT_FILE_NAME);
        let text = fs::read_to_string(&manifest).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ProjectError::Missing { path: manifest },
            _ => ProjectError::Io {
                path: manifest,
                source: e,
            },
        })?;
        let project = Project::from_json(&text)?;
        Self::acquire(root, project)
    }

    /// The loaded (and migrated) manifest.
    #[must_use]
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Replace and persist the manifest.
    ///
    /// # Errors
    ///
    /// See [`atomic_write`].
    pub fn save_project(&mut self, project: Project) -> Result<(), ProjectError> {
        self.write_json(&self.manifest_path(), &project)?;
        self.project = project;
        Ok(())
    }

    /// Every survey in the project, ordered by id.
    ///
    /// # Errors
    ///
    /// A corrupt `survey.json` fails the whole list — the caller warns and
    /// keeps its previous state rather than silently dropping one survey.
    pub fn list_surveys(&self) -> Result<Vec<Survey>, ProjectError> {
        let mut out = Vec::new();
        for entry in self.read_dir(&self.surveys_dir())? {
            let path = entry.join(SURVEY_FILE_NAME);
            if !path.is_file() {
                continue;
            }
            let text = self.read_to_string(&path)?;
            out.push(<Survey as ProjectFile>::parse(&text)?);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Load one survey by id.
    ///
    /// # Errors
    ///
    /// [`ProjectError::InvalidId`] for a non-slug id; [`ProjectError::Missing`]
    /// when the survey directory does not exist.
    pub fn load_survey(&self, id: &str) -> Result<Survey, ProjectError> {
        if !is_valid_id(id) {
            return Err(ProjectError::InvalidId(id.to_string()));
        }
        let path = self.surveys_dir().join(id).join(SURVEY_FILE_NAME);
        let text = self.read_to_string(&path)?;
        <Survey as ProjectFile>::parse(&text)
    }

    /// Persist a survey (creating its directory when new).
    ///
    /// # Errors
    ///
    /// [`ProjectError::InvalidId`] for a non-slug id; see [`atomic_write`].
    pub fn save_survey(&self, survey: &Survey) -> Result<(), ProjectError> {
        if !is_valid_id(&survey.id) {
            return Err(ProjectError::InvalidId(survey.id.clone()));
        }
        let dir = self.surveys_dir().join(&survey.id);
        fs::create_dir_all(&dir).map_err(|e| ProjectError::Io {
            path: dir.clone(),
            source: e,
        })?;
        self.write_json(&dir.join(SURVEY_FILE_NAME), survey)
    }

    /// Every session of a survey, ordered by id.
    ///
    /// # Errors
    ///
    /// As [`ProjectStore::list_surveys`].
    pub fn list_sessions(&self, survey_id: &str) -> Result<Vec<Session>, ProjectError> {
        if !is_valid_id(survey_id) {
            return Err(ProjectError::InvalidId(survey_id.to_string()));
        }
        let sessions = self.surveys_dir().join(survey_id).join(SESSIONS_DIR);
        let mut out = Vec::new();
        for entry in self.read_dir(&sessions)? {
            let path = entry.join(SESSION_FILE_NAME);
            if !path.is_file() {
                continue;
            }
            let text = self.read_to_string(&path)?;
            out.push(<Session as ProjectFile>::parse(&text)?);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Load one session of a survey.
    ///
    /// # Errors
    ///
    /// As [`ProjectStore::load_survey`].
    pub fn load_session(&self, survey_id: &str, id: &str) -> Result<Session, ProjectError> {
        if !is_valid_id(survey_id) {
            return Err(ProjectError::InvalidId(survey_id.to_string()));
        }
        if !is_valid_id(id) {
            return Err(ProjectError::InvalidId(id.to_string()));
        }
        let path = self
            .surveys_dir()
            .join(survey_id)
            .join(SESSIONS_DIR)
            .join(id)
            .join(SESSION_FILE_NAME);
        let text = self.read_to_string(&path)?;
        <Session as ProjectFile>::parse(&text)
    }

    /// Persist a session (creating its directory when new). The session's
    /// `survey_id` must match the directory it lands in.
    ///
    /// # Errors
    ///
    /// [`ProjectError::InvalidId`] for a non-slug id; [`ProjectError::Corrupt`]
    /// when the ids disagree.
    pub fn save_session(&self, survey_id: &str, session: &Session) -> Result<(), ProjectError> {
        if !is_valid_id(survey_id) {
            return Err(ProjectError::InvalidId(survey_id.to_string()));
        }
        if !is_valid_id(&session.id) {
            return Err(ProjectError::InvalidId(session.id.clone()));
        }
        if session.survey_id != survey_id {
            return Err(ProjectError::Corrupt {
                path: PathBuf::from(SESSION_FILE_NAME),
                reason: format!(
                    "session.survey_id {:?} does not match its directory {:?}",
                    session.survey_id, survey_id
                ),
            });
        }
        let dir = self
            .surveys_dir()
            .join(survey_id)
            .join(SESSIONS_DIR)
            .join(&session.id);
        fs::create_dir_all(&dir).map_err(|e| ProjectError::Io {
            path: dir.clone(),
            source: e,
        })?;
        self.write_json(&dir.join(SESSION_FILE_NAME), session)
    }

    /// The project root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Shared resources (`<root>/resources`).
    #[must_use]
    pub fn resources_dir(&self) -> PathBuf {
        self.root.join(RESOURCES_DIR)
    }

    /// DEM cache (`<root>/resources/dem`).
    #[must_use]
    pub fn dem_dir(&self) -> PathBuf {
        self.resources_dir().join(DEM_DIR)
    }

    /// Calibration files (`<root>/resources/calibration`).
    #[must_use]
    pub fn calibration_dir(&self) -> PathBuf {
        self.resources_dir().join(CALIBRATION_DIR)
    }

    /// Planning presets (`<root>/resources/presets`).
    #[must_use]
    pub fn presets_dir(&self) -> PathBuf {
        self.resources_dir().join(PRESETS_DIR)
    }

    /// Survey directories (`<root>/surveys`).
    #[must_use]
    pub fn surveys_dir(&self) -> PathBuf {
        self.root.join(SURVEYS_DIR)
    }

    /// Acquire the exclusive lock, then build the store. Private: both
    /// constructors need exactly this order (a store without the lock must
    /// never exist).
    fn acquire(root: &Path, project: Project) -> Result<Self, ProjectError> {
        let lock_path = root.join(LOCK_FILE_NAME);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&lock_path)
            .map_err(|e| ProjectError::Io {
                path: lock_path,
                source: e,
            })?;
        lock.try_lock_exclusive().map_err(|e| match e.kind() {
            std::io::ErrorKind::WouldBlock => ProjectError::Locked(root.to_path_buf()),
            _ => ProjectError::Io {
                path: root.to_path_buf(),
                source: e,
            },
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            project,
            _lock: lock,
        })
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join(PROJECT_FILE_NAME)
    }

    /// Read a file, mapping NotFound to [`ProjectError::Missing`].
    fn read_to_string(&self, path: &Path) -> Result<String, ProjectError> {
        fs::read_to_string(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ProjectError::Missing {
                path: path.to_path_buf(),
            },
            _ => ProjectError::Io {
                path: path.to_path_buf(),
                source: e,
            },
        })
    }

    /// Directories under `dir` (sorted); a missing directory is empty.
    fn read_dir(&self, dir: &Path) -> Result<Vec<PathBuf>, ProjectError> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(ProjectError::Io {
                    path: dir.to_path_buf(),
                    source: e,
                })
            }
        };
        let mut out = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|e| ProjectError::Io {
                    path: dir.to_path_buf(),
                    source: e,
                })?
                .path();
            if path.is_dir() {
                out.push(path);
            }
        }
        out.sort();
        Ok(out)
    }

    /// Atomically write `value` as pretty JSON: sibling temp file, flushed,
    /// then renamed over the target (ADR-019 §4). The version stamp comes from
    /// [`ProjectFile::serialize_pretty`].
    fn write_json<T: ProjectFile>(&self, path: &Path, value: &T) -> Result<(), ProjectError> {
        let json = value
            .serialize_pretty()
            .map_err(|e| ProjectError::Corrupt {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?
            .into_bytes();
        atomic_write(path, &json)
    }
}

/// Atomically write `bytes` to `path`: serialize to a sibling `<file>.tmp`,
/// flush it to disk, then rename over the target so a crash mid-write cannot
/// leave a truncated file. A leftover temp is simply overwritten by the next
/// write and never read back.
///
/// # Errors
///
/// [`ProjectError::Io`] on any filesystem failure.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ProjectError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| ProjectError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = File::create(&tmp).map_err(|e| ProjectError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    file.write_all(bytes).map_err(|e| ProjectError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    file.sync_all().map_err(|e| ProjectError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    fs::rename(&tmp, path).map_err(|e| ProjectError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlanPreset;
    use crate::project::{ProjectSettings, SessionStatus, SurveyStatus, PROJECT_SCHEMA_VERSION};

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "maggcs-project-{}-{}-{}",
            tag,
            std::process::id(),
            crate::recorder::now_utc_us()
        ))
    }

    fn demo_project() -> Project {
        Project::new(
            "Saanich 2026-10",
            "EPSG:4326",
            "cgvd2013",
            ProjectSettings {
                dem_dir: None,
                calibration_dir: None,
                presets: vec![PlanPreset {
                    name: "40 m AGL · 5 m/s · 5 m".into(),
                    kind: "sweep".into(),
                    agl_m: 40.0,
                    speed_mps: 5.0,
                    line_spacing_m: 5.0,
                    tie_spacing_m: Some(250.0),
                    width_m: 100.0,
                    height_m: 100.0,
                    heading_deg: 0.0,
                    min_clearance_dsm_m: Some(5.0),
                    vertical_offset_m: None,
                }],
            },
        )
    }

    fn demo_survey() -> Survey {
        Survey {
            version: PROJECT_SCHEMA_VERSION,
            id: "mag-01".into(),
            kind: crate::survey_profile::SurveyKind::UavMag,
            name: "Line 1".into(),
            status: SurveyStatus::Ready,
            ..Survey::default()
        }
    }

    #[test]
    fn create_open_round_trips_all_three_levels() {
        let root = temp_root("roundtrip");
        let store = ProjectStore::create(&root, demo_project()).expect("create");
        // The resource scaffolding exists.
        for dir in [
            store.dem_dir(),
            store.calibration_dir(),
            store.presets_dir(),
        ] {
            assert!(dir.is_dir(), "{}", dir.display());
        }

        let survey = demo_survey();
        store.save_survey(&survey).expect("save survey");
        let session = Session {
            version: PROJECT_SCHEMA_VERSION,
            id: "2026-10-15T1432Z".into(),
            survey_id: survey.id.clone(),
            status: SessionStatus::Recording,
            operator: Some("op".into()),
            started_utc_us: Some(1_000),
            ended_utc_us: None,
        };
        store
            .save_session(&survey.id, &session)
            .expect("save session");
        drop(store);

        let store = ProjectStore::open(&root).expect("reopen");
        let project = store.project();
        assert_eq!(project.name, "Saanich 2026-10");
        assert_eq!(project.crs, "EPSG:4326");
        assert_eq!(project.settings.presets.len(), 1);
        assert_eq!(project.version, PROJECT_SCHEMA_VERSION);

        let surveys = store.list_surveys().expect("list");
        assert_eq!(surveys.len(), 1);
        assert_eq!(surveys[0], survey);

        let loaded = store.load_survey("mag-01").expect("load survey");
        assert_eq!(loaded, survey);

        let sessions = store.list_sessions("mag-01").expect("list sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            store
                .load_session("mag-01", "2026-10-15T1432Z")
                .expect("load"),
            session
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_process_on_the_same_project_is_rejected() {
        let root = temp_root("locked");
        let store = ProjectStore::create(&root, demo_project()).expect("create");
        assert!(matches!(
            ProjectStore::open(&root),
            Err(ProjectError::Locked(_))
        ));
        assert!(matches!(
            ProjectStore::create(&root, demo_project()),
            Err(ProjectError::Exists(_))
        ));
        // Released on drop: the tree opens again.
        drop(store);
        assert!(ProjectStore::open(&root).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_kill_mid_write_keeps_the_previous_manifest() {
        let root = temp_root("crash");
        let mut store = ProjectStore::create(&root, demo_project()).expect("create");
        let mut renamed = store.project().clone();
        renamed.name = "Renamed".into();
        store.save_project(renamed).expect("save");

        // Simulate a crash between writing the temp file and the rename: the
        // temp is stale garbage, the target still holds the last good state.
        let tmp = root.join(format!("{PROJECT_FILE_NAME}.tmp"));
        fs::write(&tmp, b"{ truncated").expect("write tmp");
        drop(store);

        let store = ProjectStore::open(&root).expect("reopen");
        assert_eq!(store.project().name, "Renamed", "target survived the crash");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_older_schema_migrates_forward() {
        let root = temp_root("migrate");
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join(PROJECT_FILE_NAME), b"{\n  \"version\": 0\n}").expect("write");
        let store = ProjectStore::open(&root).expect("open legacy");
        assert_eq!(store.project().version, PROJECT_SCHEMA_VERSION);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_newer_schema_is_rejected() {
        let root = temp_root("newer");
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join(PROJECT_FILE_NAME), b"{\n  \"version\": 99\n}").expect("write");
        assert!(matches!(
            ProjectStore::open(&root),
            Err(ProjectError::UnsupportedVersion { .. })
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_corrupt_manifest_is_corrupt_and_a_missing_one_is_missing() {
        let corrupt = temp_root("corrupt");
        fs::create_dir_all(&corrupt).expect("mkdir");
        fs::write(corrupt.join(PROJECT_FILE_NAME), b"{ not json").expect("write");
        assert!(matches!(
            ProjectStore::open(&corrupt),
            Err(ProjectError::Corrupt { .. })
        ));
        let _ = fs::remove_dir_all(&corrupt);

        let missing = temp_root("missing");
        fs::create_dir_all(&missing).expect("mkdir");
        assert!(matches!(
            ProjectStore::open(&missing),
            Err(ProjectError::Missing { .. })
        ));
        let _ = fs::remove_dir_all(&missing);
    }

    #[test]
    fn ids_must_be_slugs() {
        let root = temp_root("ids");
        let store = ProjectStore::create(&root, demo_project()).expect("create");
        for bad in ["", ".", "..", "a/b", "../evil", "a b"] {
            assert!(
                matches!(
                    store.save_survey(&Survey {
                        id: bad.into(),
                        ..demo_survey()
                    }),
                    Err(ProjectError::InvalidId(_))
                ),
                "id {bad:?} must be rejected"
            );
        }
        assert!(is_valid_id("2026-10-15T1432Z"));
        assert!(is_valid_id("mag_01.v2"));
        let _ = fs::remove_dir_all(&root);
    }
}
