//! Project service (ADR-019, plan PJ1): thin Tauri commands over
//! [`maggcs_core::project::ProjectStore`].
//!
//! The store lives in `AppState.project` — one project per process, held for
//! the advisory-lock lifetime (a second app on the same tree fails with
//! `ProjectError::Locked`). The commands are pure plumbing: every rule
//! (schema version, atomic writes, id slugs) lives in `core::project`.
//!
//! `project_create` seeds the project-level settings from the current app
//! config (A6): the planning presets move into the project; the app config
//! keeps its copy until the frontend reads project settings.

use parking_lot::MutexGuard;
use tauri::State;

use maggcs_core::project::{Project, ProjectError, ProjectSettings, ProjectStore, Session, Survey};

use crate::state::AppState;

fn take_open<'a>(
    state: &'a State<'a, AppState>,
) -> Result<MutexGuard<'a, Option<ProjectStore>>, String> {
    Ok(state.project.lock())
}

fn err(e: ProjectError) -> String {
    e.to_string()
}

/// Create a project at `path` (must not already be one) and open it.
/// Project-level settings are seeded from the app config's planning presets
/// (ADR-019 §6).
#[tauri::command]
pub fn project_create(
    state: State<'_, AppState>,
    path: String,
    name: String,
    crs: String,
    vertical_datum: String,
) -> Result<Project, String> {
    let presets = state.config().plan.presets;
    let project = Project::new(
        &name,
        &crs,
        &vertical_datum,
        ProjectSettings {
            dem_dir: None,
            calibration_dir: None,
            presets,
        },
    );
    let mut guard = take_open(&state)?;
    let store = ProjectStore::create(std::path::Path::new(&path), project).map_err(err)?;
    let manifest = store.project().clone();
    *guard = Some(store);
    Ok(manifest)
}

/// Open an existing project (takes the advisory lock; a second process on the
/// same tree is rejected).
#[tauri::command]
pub fn project_open(state: State<'_, AppState>, path: String) -> Result<Project, String> {
    let mut guard = take_open(&state)?;
    let store = ProjectStore::open(std::path::Path::new(&path)).map_err(err)?;
    let manifest = store.project().clone();
    *guard = Some(store);
    Ok(manifest)
}

/// Close the open project, releasing its lock. `true` when one was open.
#[tauri::command]
pub fn project_close(state: State<'_, AppState>) -> Result<bool, String> {
    let mut guard = take_open(&state)?;
    Ok(guard.take().is_some())
}

/// The open project's manifest, or `None` when no project is open.
#[tauri::command]
pub fn project_status(state: State<'_, AppState>) -> Result<Option<Project>, String> {
    let guard = take_open(&state)?;
    Ok(guard.as_ref().map(|store| store.project().clone()))
}

/// Persist the manifest of the open project.
#[tauri::command]
pub fn project_save(state: State<'_, AppState>, project: Project) -> Result<(), String> {
    let mut guard = take_open(&state)?;
    let store = guard
        .as_mut()
        .ok_or_else(|| "no project is open".to_string())?;
    store.save_project(project).map_err(err)
}

/// Every survey of the open project, ordered by id.
#[tauri::command]
pub fn project_list_surveys(state: State<'_, AppState>) -> Result<Vec<Survey>, String> {
    let guard = take_open(&state)?;
    let store = guard
        .as_ref()
        .ok_or_else(|| "no project is open".to_string())?;
    store.list_surveys().map_err(err)
}

/// Load one survey of the open project.
#[tauri::command]
pub fn project_load_survey(state: State<'_, AppState>, id: String) -> Result<Survey, String> {
    let guard = take_open(&state)?;
    let store = guard
        .as_ref()
        .ok_or_else(|| "no project is open".to_string())?;
    store.load_survey(&id).map_err(err)
}

/// Persist a survey of the open project (creating its directory when new).
#[tauri::command]
pub fn project_save_survey(state: State<'_, AppState>, survey: Survey) -> Result<(), String> {
    let guard = take_open(&state)?;
    let store = guard
        .as_ref()
        .ok_or_else(|| "no project is open".to_string())?;
    store.save_survey(&survey).map_err(err)
}

/// Every session of a survey, ordered by id.
#[tauri::command]
pub fn project_list_sessions(
    state: State<'_, AppState>,
    survey_id: String,
) -> Result<Vec<Session>, String> {
    let guard = take_open(&state)?;
    let store = guard
        .as_ref()
        .ok_or_else(|| "no project is open".to_string())?;
    store.list_sessions(&survey_id).map_err(err)
}

/// Persist a session of the open project (creating its directory when new).
#[tauri::command]
pub fn project_save_session(
    state: State<'_, AppState>,
    survey_id: String,
    session: Session,
) -> Result<(), String> {
    let guard = take_open(&state)?;
    let store = guard
        .as_ref()
        .ok_or_else(|| "no project is open".to_string())?;
    store.save_session(&survey_id, &session).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maggcs_core::config::AppConfig;
    use maggcs_core::project::{ProjectStore, SESSION_FILE_NAME, SURVEY_FILE_NAME};
    use std::fs;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "maggcs-project-service-{}-{}",
            tag,
            maggcs_core::recorder::now_utc_us()
        ))
    }

    /// The service is plumbing over the store; the rules are tested in core.
    /// This pins the one piece the service owns: a created store stays open in
    /// the state and its manifest is returned.
    #[test]
    fn create_holds_the_store_and_returns_the_manifest() {
        let root = temp_root("create");
        let state = AppState::default();
        let presets = vec![maggcs_core::config::PlanPreset {
            name: "p".into(),
            kind: "sweep".into(),
            agl_m: 40.0,
            speed_mps: 5.0,
            line_spacing_m: 5.0,
            tie_spacing_m: None,
            width_m: 100.0,
            height_m: 100.0,
            heading_deg: 0.0,
            min_clearance_dsm_m: Some(5.0),
            vertical_offset_m: None,
        }];
        state.set_config(AppConfig {
            plan: maggcs_core::config::PlanPrefs {
                last_center: None,
                presets: presets.clone(),
            },
            ..AppConfig::default()
        });
        // The command body, inline (a Tauri `State` cannot be built in tests):
        // seed from the config, create, hold.
        let project = Project::new(
            "proj",
            "EPSG:4326",
            "cgvd2013",
            ProjectSettings {
                dem_dir: None,
                calibration_dir: None,
                presets: state.config().plan.presets,
            },
        );
        let store = ProjectStore::create(&root, project).expect("create");
        let manifest = store.project().clone();
        assert_eq!(manifest.settings.presets, presets, "seeded from the config");
        *state.project.lock() = Some(store);

        let guard = state.project.lock();
        assert!(guard.as_ref().expect("held").project().name == "proj");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_and_survey_file_names_are_stable() {
        assert_eq!(SURVEY_FILE_NAME, "survey.json");
        assert_eq!(SESSION_FILE_NAME, "session.json");
    }
}
