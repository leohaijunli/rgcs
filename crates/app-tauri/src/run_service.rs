//! Per-test run coordinator + recovery manifest (Task 8).
//!
//! The core coordinator decides the capture/motor/pre-roll sequence; this
//! adapter owns the clock, the manifest file and the Tauri surface. Every
//! boundary is persisted atomically so a crash or reconnect can reconcile a
//! run that never finished. Manifests live under
//! `<app_data>/motor-tests/<test_id>/manifest.json`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use maggcs_core::logs::capture::{CapturePhase, SdCaptureStatus};
use maggcs_core::motor_test::mapping::OutputMap;
use maggcs_core::motor_test::pwm::PwmModel;
use maggcs_core::motor_test::run::{
    RunAction, RunConfig, RunCoordinator, RunPhase, RunStatus, RunStep,
};
use maggcs_core::motor_test::service::StopReason;

use crate::motor_test_state::{MotorTestTargetRef, TARGET_COMPONENT_ID};
use crate::state::AppState;

/// How often the run ticker advances pre-roll/post-roll timers.
pub const RUN_TICK: Duration = Duration::from_millis(100);
const MANIFEST_SCHEMA_VERSION: u32 = 1;
const MANIFEST_DIR: &str = "motor-tests";
const MANIFEST_FILE: &str = "manifest.json";

/// Lifecycle of a persisted test run. `Pending` is the only state that needs
/// reconciliation after a crash/restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunManifestStatus {
    Pending,
    Complete,
    Aborted,
    Interrupted,
    Reconciled,
    Failed,
}

/// Everything needed to correlate a recorded ULog with one motor test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRunManifest {
    pub schema_version: u32,
    pub test_id: String,
    pub status: RunManifestStatus,
    pub created_utc_us: u64,
    pub updated_utc_us: u64,
    pub finished_utc_us: Option<u64>,
    pub fc: FcIdentity,
    pub capture: CaptureRecord,
    pub mapping: Option<OutputMap>,
    pub pwm_models: Vec<PwmModel>,
    pub boundaries: Vec<RunBoundary>,
    pub stop_reason: Option<StopReason>,
    pub error: Option<String>,
    pub recovery: Option<RunRecovery>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcIdentity {
    pub endpoint: String,
    pub system_id: u8,
    pub component_id: u8,
    pub firmware_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub remote_path: Option<String>,
    pub sdlog_mode: Option<i32>,
    pub sdlog_backend: Option<i32>,
    pub started_utc_us: Option<u64>,
    pub stopped_utc_us: Option<u64>,
    pub finalization_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunBoundary {
    pub at_utc_us: u64,
    pub phase: RunPhase,
    pub detail: Option<String>,
}

/// Evidence collected when a pending manifest is reconciled after reconnect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecovery {
    pub reconciled_utc_us: u64,
    pub logger_was_running: bool,
    pub logger_path: Option<String>,
    pub note: String,
}

/// State of the current/last run, kept with the motor-test process state.
#[derive(Default)]
pub struct ActuatorRun {
    pub active: Option<ActiveRun>,
    pub last_status: Option<RunStatus>,
    pub last_manifest_path: Option<PathBuf>,
}

pub struct ActiveRun {
    pub target: MotorTestTargetRef,
    pub coordinator: RunCoordinator,
    pub manifest: TestRunManifest,
    pub manifest_path: PathBuf,
    pub ticker: Option<tauri::async_runtime::JoinHandle<()>>,
}

/// Serializable snapshot for the window's status poll.
#[derive(Debug, Clone, Serialize)]
pub struct RunSnapshot {
    pub status: Option<RunStatus>,
    pub manifest_path: Option<String>,
}

/// Begin a run once the capture service has confirmed a new recording.
pub(crate) fn begin_run(
    app: &AppHandle,
    state: &AppState,
    capture: &SdCaptureStatus,
) -> Result<RunSnapshot, String> {
    let test_id = capture.test_id.clone().ok_or("capture has no test id")?;
    validate_test_id(&test_id)?;
    if capture.phase != CapturePhase::Recording {
        return Err("confirm a new SD recording before starting a test run".into());
    }

    let manifest = build_manifest(app, state, &test_id, capture)?;
    let manifest_path = manifest_path(app, &test_id)?;

    let now = Instant::now();
    let (mut coordinator, _) = RunCoordinator::begin(RunConfig::new(test_id.clone()), now);
    let step = coordinator.observe_capture(capture, now);

    let mut run = state.motor_test.run.lock();
    if let Some(active) = &run.active {
        if !active.coordinator.is_terminal() {
            return Err("a test run is already active; stop or finish it first".into());
        }
    }
    if let Some(mut old) = run.active.take() {
        if let Some(ticker) = old.ticker.take() {
            ticker.abort();
        }
    }
    run.last_status = None;
    run.last_manifest_path = Some(manifest_path.clone());
    run.active = Some(ActiveRun {
        target: state
            .motor_test
            .target
            .lock()
            .as_ref()
            .map(|target| target.target.target_ref())
            .ok_or("no serial PX4 target — connect it first")?,
        coordinator,
        manifest,
        manifest_path,
        ticker: None,
    });

    if let Some(active) = run.active.as_ref() {
        if let Err(error) = write_manifest(&active.manifest_path, &active.manifest) {
            run.active = None;
            return Err(error);
        }
    }
    let actions = apply_step(app, &mut run, step);
    debug_assert!(actions.is_empty());
    let snapshot = snapshot_locked(&run);

    if run.active.is_some() {
        let ticker = spawn_ticker(app.clone());
        if let Some(active) = run.active.as_mut() {
            active.ticker = Some(ticker);
        }
    }
    Ok(snapshot)
}

/// Advance the run. Returns `false` once there is no active run.
fn tick(app: &AppHandle, state: &AppState) -> bool {
    let capture = crate::capture_service::capture_status_blocking(state);
    let target = {
        let mut run = state.motor_test.run.lock();
        let Some(active) = run.active.as_mut() else {
            return false;
        };
        let target = active.target;
        let now = Instant::now();
        let mut step = active.coordinator.observe_capture(&capture, now);
        let timer = active.coordinator.tick(now);
        if timer.changed || !timer.actions.is_empty() {
            step.changed |= timer.changed;
            if timer.status.is_some() {
                step.status = timer.status;
            }
            step.actions.extend(timer.actions);
        }
        let actions = apply_step(app, &mut run, step);
        if actions.contains(&RunAction::StopCapture) {
            Some(target)
        } else {
            None
        }
    };

    if let Some(target) = target {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            let _ = crate::capture_service::request_stop_for_target(&state, target).await;
        });
    }

    state.motor_test.run.lock().active.is_some()
}

fn spawn_ticker(app: AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(RUN_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let state = app.state::<AppState>();
            if !tick(&app, &state) {
                break;
            }
        }
    })
}

/// Apply one coordinator step: persist boundaries, emit status and schedule
/// the stop action. Returns actions for the async caller.
fn apply_step(app: &AppHandle, run: &mut ActuatorRun, step: RunStep) -> Vec<RunAction> {
    if step.changed {
        if let Some(status) = &step.status {
            run.last_status = Some(status.clone());
            let now = maggcs_core::recorder::now_utc_us();
            if let Some(active) = run.active.as_mut() {
                update_manifest(active, status, now);
                if let Err(error) = write_manifest(&active.manifest_path, &active.manifest) {
                    eprintln!(
                        "warning: cannot write motor-test manifest {}: {error}",
                        active.manifest_path.display()
                    );
                }
            }
            let _ = app.emit("actuator_run", status);
        }
    }

    if run
        .active
        .as_ref()
        .is_some_and(|active| active.coordinator.is_terminal())
    {
        if let Some(mut active) = run.active.take() {
            run.last_manifest_path = Some(active.manifest_path.clone());
            if let Some(ticker) = active.ticker.take() {
                ticker.abort();
            }
        }
    }
    step.actions
}

fn update_manifest(active: &mut ActiveRun, status: &RunStatus, now_utc_us: u64) {
    active.manifest.updated_utc_us = now_utc_us;
    active.manifest.status = manifest_status(status.phase);
    active.manifest.stop_reason = status.stop_reason;
    active.manifest.error = status.error.as_ref().map(ToString::to_string);
    if let Some(path) = &status.remote_path {
        active.manifest.capture.remote_path = Some(path.clone());
        active
            .manifest
            .capture
            .started_utc_us
            .get_or_insert(now_utc_us);
    }
    if status.phase.is_terminal() {
        active.manifest.finished_utc_us.get_or_insert(now_utc_us);
        active
            .manifest
            .capture
            .stopped_utc_us
            .get_or_insert(now_utc_us);
        active.manifest.capture.finalization_confirmed =
            matches!(status.phase, RunPhase::Complete | RunPhase::Aborted);
    }
    if active
        .manifest
        .boundaries
        .last()
        .is_none_or(|last| last.phase != status.phase)
    {
        active.manifest.boundaries.push(RunBoundary {
            at_utc_us: now_utc_us,
            phase: status.phase,
            detail: status.error.as_ref().map(ToString::to_string),
        });
    }
}

fn manifest_status(phase: RunPhase) -> RunManifestStatus {
    match phase {
        RunPhase::Complete => RunManifestStatus::Complete,
        RunPhase::Aborted => RunManifestStatus::Aborted,
        RunPhase::Interrupted => RunManifestStatus::Interrupted,
        RunPhase::Failed => RunManifestStatus::Failed,
        RunPhase::StartingCapture
        | RunPhase::Preroll
        | RunPhase::MotorReady
        | RunPhase::MotorRunning
        | RunPhase::Postroll
        | RunPhase::StoppingCapture => RunManifestStatus::Pending,
    }
}

pub(crate) fn snapshot(run: &parking_lot::Mutex<ActuatorRun>) -> RunSnapshot {
    snapshot_locked(&run.lock())
}

fn snapshot_locked(run: &ActuatorRun) -> RunSnapshot {
    RunSnapshot {
        status: run
            .active
            .as_ref()
            .map(|active| active.coordinator.status())
            .or_else(|| run.last_status.clone()),
        manifest_path: run
            .last_manifest_path
            .as_ref()
            .map(|path| path.display().to_string()),
    }
}

/// Current/last run status for the window poll.
#[tauri::command]
pub fn actuator_run_status(state: State<'_, AppState>) -> RunSnapshot {
    snapshot_locked(&state.motor_test.run.lock())
}

/// Start a new run: begin the capture, then arm the coordinator's pre-roll.
/// Kept separate from the low-level capture command so the UI has one action.
#[tauri::command]
pub async fn actuator_run_start(
    app: AppHandle,
    state: State<'_, AppState>,
    test_id: String,
) -> Result<RunSnapshot, String> {
    validate_test_id(&test_id)?;
    let _ops = state.ops().lock().await;
    ensure_no_active_run(&state)?;
    let capture = crate::capture_service::start_capture_locked(&state, test_id).await?;
    if capture.phase != CapturePhase::Recording {
        return Err("the target logger is already recording another file".into());
    }
    match begin_run(&app, &state, &capture) {
        Ok(snapshot) => Ok(snapshot),
        Err(error) => {
            let _ = crate::capture_service::stop_capture_locked(&state).await;
            Err(error)
        }
    }
}

/// Abort the run. Motors are released first; the coordinator then stops the
/// capture after its post-roll and records the run as aborted.
#[tauri::command]
pub async fn actuator_run_abort(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RunSnapshot, String> {
    let _ops = state.ops().lock().await;
    if state.motor_test.run.lock().active.is_none() {
        return Ok(snapshot_locked(&state.motor_test.run.lock()));
    }
    crate::actuator_service::stop_active_session(&state).await?;
    let target = {
        let mut run = state.motor_test.run.lock();
        let Some(active) = run.active.as_mut() else {
            return Ok(snapshot_locked(&run));
        };
        let target = active.target;
        let step = active.coordinator.abort(Instant::now());
        let actions = apply_step(&app, &mut run, step);
        if actions.contains(&RunAction::StopCapture) {
            Some(target)
        } else {
            None
        }
    };
    if let Some(target) = target {
        let _ = crate::capture_service::request_stop_for_target(&state, target).await;
    }
    Ok(snapshot_locked(&state.motor_test.run.lock()))
}

pub(crate) fn ensure_no_active_run(state: &AppState) -> Result<(), String> {
    if state
        .motor_test
        .run
        .lock()
        .active
        .as_ref()
        .is_some_and(|active| !active.coordinator.is_terminal())
    {
        Err("a test run is already active; stop or finish it first".into())
    } else {
        Ok(())
    }
}

/// Gate the operator's START: only after the confirmed recording has finished
/// its pre-roll. Advances the timer first so a start at the boundary works.
pub(crate) fn authorize_motor_start(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let mut run = state.motor_test.run.lock();
    let Some(active) = run.active.as_mut() else {
        // SD capture is optional: with no run, the motor test is standalone.
        return Ok(());
    };
    let step = active.coordinator.tick(Instant::now());
    let actions = apply_step(app, &mut run, step);
    debug_assert!(actions.is_empty());
    let Some(active) = run.active.as_ref() else {
        // The run ended on this tick; the motor test may proceed standalone.
        return Ok(());
    };
    match active.coordinator.phase() {
        RunPhase::MotorReady => Ok(()),
        RunPhase::Preroll | RunPhase::StartingCapture => {
            Err("wait for the 1 s pre-roll after the confirmed capture".into())
        }
        RunPhase::MotorRunning => Err("the motor test is already running".into()),
        phase if phase.is_terminal() => Ok(()),
        phase => Err(format!("motor start is not allowed in the {phase:?} phase")),
    }
}

/// Record the transport-confirmed motor start.
pub(crate) fn confirm_motor_started(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let mut run = state.motor_test.run.lock();
    let Some(active) = run.active.as_mut() else {
        // No SD capture/run: the motor test is standalone, nothing to record.
        return Ok(());
    };
    let step = active
        .coordinator
        .note_motor_started(Instant::now())
        .map_err(|error| error.to_string())?;
    let actions = apply_step(app, &mut run, step);
    debug_assert!(actions.is_empty());
    Ok(())
}

/// Record a transport-confirmed release; the ticker owns the post-roll.
pub(crate) fn note_motor_released(app: &AppHandle, state: &AppState, reason: StopReason) {
    let mut run = state.motor_test.run.lock();
    let Some(active) = run.active.as_mut() else {
        return;
    };
    let step = active
        .coordinator
        .note_motor_released(reason, Instant::now());
    let _ = apply_step(app, &mut run, step);
}

/// Mark the active run interrupted (target/link teardown) and persist it.
pub(crate) fn interrupt_active(app: &AppHandle, state: &AppState, reason: &str) {
    let mut run = state.motor_test.run.lock();
    let Some(active) = run.active.as_mut() else {
        return;
    };
    let step = active.coordinator.interrupt(reason, Instant::now());
    let _ = apply_step(app, &mut run, step);
}

pub(crate) fn validate_test_id(test_id: &str) -> Result<(), String> {
    let valid = !test_id.is_empty()
        && test_id.len() <= 96
        && test_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && test_id != "."
        && test_id != "..";
    if valid {
        Ok(())
    } else {
        Err("test id must be 1-96 characters of ASCII letters, digits, '.', '-' or '_'".into())
    }
}

fn manifests_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(MANIFEST_DIR))
        .map_err(|error| format!("cannot resolve app data dir: {error}"))
}

fn manifest_path(app: &AppHandle, test_id: &str) -> Result<PathBuf, String> {
    validate_test_id(test_id)?;
    Ok(manifests_dir(app)?.join(test_id).join(MANIFEST_FILE))
}

fn build_manifest(
    app: &AppHandle,
    state: &AppState,
    test_id: &str,
    capture: &SdCaptureStatus,
) -> Result<TestRunManifest, String> {
    let now = maggcs_core::recorder::now_utc_us();
    let target = state.motor_test.target.lock();
    let target = target
        .as_ref()
        .ok_or("no serial PX4 target — connect it first")?;
    let mapping = target.target.output_map.lock().clone();
    let pwm_models = target.target.pwm_models.lock().clone();
    let _ = app;
    Ok(TestRunManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        test_id: test_id.to_string(),
        status: RunManifestStatus::Pending,
        created_utc_us: now,
        updated_utc_us: now,
        finished_utc_us: None,
        fc: FcIdentity {
            endpoint: target.target.endpoint.clone(),
            system_id: target.target.target_system_id,
            component_id: TARGET_COMPONENT_ID,
            firmware_version: target.target.params.fc_version(),
        },
        capture: CaptureRecord {
            remote_path: capture.remote_path.clone(),
            sdlog_mode: capture.sdlog_mode,
            sdlog_backend: capture.sdlog_backend,
            started_utc_us: Some(now),
            stopped_utc_us: None,
            finalization_confirmed: false,
        },
        mapping: Some(mapping),
        pwm_models,
        boundaries: vec![RunBoundary {
            at_utc_us: now,
            phase: RunPhase::Preroll,
            detail: Some("capture recording confirmed".into()),
        }],
        stop_reason: None,
        error: None,
        recovery: None,
    })
}

/// Atomic manifest write: sibling temp file then rename.
pub(crate) fn write_manifest(path: &Path, manifest: &TestRunManifest) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create manifest dir {}: {error}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|error| format!("cannot serialize manifest: {error}"))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, json.as_bytes())
        .map_err(|error| format!("cannot write manifest {}: {error}", tmp.display()))?;
    fs::rename(&tmp, path)
        .map_err(|error| format!("cannot replace manifest {}: {error}", path.display()))
}

pub(crate) fn read_manifest(path: &Path) -> Option<TestRunManifest> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("warning: cannot read manifest {}: {error}", path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            eprintln!(
                "warning: ignoring unreadable manifest {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn pending_manifests(app: &AppHandle) -> Vec<(PathBuf, TestRunManifest)> {
    let Ok(dir) = manifests_dir(app) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut pending = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join(MANIFEST_FILE);
        if let Some(manifest) = read_manifest(&path) {
            if manifest.status == RunManifestStatus::Pending {
                pending.push((path, manifest));
            }
        }
    }
    pending
}

/// Reconcile a pending manifest against the live logger observation.
pub(crate) fn reconcile_manifest(
    mut manifest: TestRunManifest,
    live: &SdCaptureStatus,
    now_utc_us: u64,
) -> TestRunManifest {
    if manifest.status != RunManifestStatus::Pending {
        return manifest;
    }
    let logger_path = live.remote_path.clone();
    let still_recording = live.phase == CapturePhase::Recording
        && manifest.capture.remote_path.is_some()
        && logger_path == manifest.capture.remote_path;
    manifest.status = if still_recording {
        RunManifestStatus::Interrupted
    } else {
        RunManifestStatus::Reconciled
    };
    manifest.updated_utc_us = now_utc_us;
    manifest.finished_utc_us = Some(now_utc_us);
    manifest.capture.stopped_utc_us.get_or_insert(now_utc_us);
    manifest.capture.finalization_confirmed = !still_recording;
    manifest.recovery = Some(RunRecovery {
        reconciled_utc_us: now_utc_us,
        logger_was_running: live.logger.as_ref().is_some_and(|logger| logger.running),
        logger_path,
        note: if still_recording {
            "the logger is still recording this run's file; stop it before starting a new run"
                .into()
        } else {
            "no matching logger recording is active; the run was closed after restart".into()
        },
    });
    manifest
}

/// Reconcile after a target reconnect. Runs only for pending manifests, so a
/// healthy completed run is never rewritten.
pub(crate) async fn reconcile_target(app: AppHandle, target: MotorTestTargetRef) {
    let pending = pending_manifests(&app);
    if pending.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let live = match crate::capture_service::check_capture_for_target(&state, target).await {
        Ok(status) => status,
        Err(error) => {
            eprintln!("warning: cannot reconcile motor-test runs: {error}");
            return;
        }
    };
    let now = maggcs_core::recorder::now_utc_us();
    for (path, manifest) in pending {
        let updated = reconcile_manifest(manifest, &live, now);
        if let Err(error) = write_manifest(&path, &updated) {
            eprintln!(
                "warning: cannot reconcile motor-test manifest {}: {error}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "maggcs-run-manifest-{}-{}",
            tag,
            std::process::id()
        ))
    }

    fn manifest(test_id: &str) -> TestRunManifest {
        TestRunManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            test_id: test_id.into(),
            status: RunManifestStatus::Pending,
            created_utc_us: 10,
            updated_utc_us: 10,
            finished_utc_us: None,
            fc: FcIdentity {
                endpoint: "serial:/dev/ttyACM0:921600".into(),
                system_id: 1,
                component_id: 1,
                firmware_version: Some("1.17.0".into()),
            },
            capture: CaptureRecord {
                remote_path: Some("/fs/microsd/log/1.ulg".into()),
                sdlog_mode: Some(0),
                sdlog_backend: Some(1),
                started_utc_us: Some(10),
                stopped_utc_us: None,
                finalization_confirmed: false,
            },
            mapping: None,
            pwm_models: Vec::new(),
            boundaries: Vec::new(),
            stop_reason: None,
            error: None,
            recovery: None,
        }
    }

    #[test]
    fn test_ids_are_safe_path_components() {
        assert!(validate_test_id("test-20261022T120000").is_ok());
        assert!(validate_test_id("..").is_err());
        assert!(validate_test_id("a/b").is_err());
        assert!(validate_test_id("").is_err());
        assert!(validate_test_id(&"x".repeat(97)).is_err());
    }

    #[test]
    fn manifest_round_trips_atomically() {
        let dir = temp_dir("roundtrip");
        let path = dir.join(MANIFEST_FILE);
        let expected = manifest("test-1");
        write_manifest(&path, &expected).expect("write");
        assert!(!dir.join("manifest.json.tmp").exists());
        let loaded = read_manifest(&path).expect("read");
        assert_eq!(loaded.test_id, expected.test_id);
        assert_eq!(loaded.status, expected.status);
        assert_eq!(
            loaded.capture.remote_path.as_deref(),
            Some("/fs/microsd/log/1.ulg")
        );
    }

    fn live(phase: CapturePhase, path: Option<&str>) -> SdCaptureStatus {
        SdCaptureStatus {
            phase,
            remote_path: path.map(Into::into),
            logger: Some(maggcs_core::logs::capture::LoggerStatus {
                running: phase == CapturePhase::Recording,
                backend: maggcs_core::logs::capture::LoggerBackend::File,
                file_logging: phase == CapturePhase::Recording,
                remote_path: path.map(Into::into),
            }),
            ..SdCaptureStatus::default()
        }
    }

    #[test]
    fn reconcile_marks_a_still_recording_run_interrupted() {
        let updated = reconcile_manifest(
            manifest("test-1"),
            &live(CapturePhase::Recording, Some("/fs/microsd/log/1.ulg")),
            20,
        );
        assert_eq!(updated.status, RunManifestStatus::Interrupted);
        assert!(updated.recovery.expect("recovery").logger_was_running);
        assert!(!updated.capture.finalization_confirmed);
    }

    #[test]
    fn reconcile_closes_a_run_whose_logger_already_stopped() {
        let updated = reconcile_manifest(
            manifest("test-1"),
            &live(CapturePhase::Complete, Some("/fs/microsd/log/1.ulg")),
            20,
        );
        assert_eq!(updated.status, RunManifestStatus::Reconciled);
        assert!(updated.capture.finalization_confirmed);
        assert_eq!(updated.finished_utc_us, Some(20));
    }

    #[test]
    fn reconcile_never_rewrites_a_finished_manifest() {
        let mut finished = manifest("test-1");
        finished.status = RunManifestStatus::Complete;
        let updated =
            reconcile_manifest(finished.clone(), &live(CapturePhase::Recording, None), 20);
        assert_eq!(updated.status, RunManifestStatus::Complete);
        assert_eq!(updated.updated_utc_us, finished.updated_utc_us);
    }
}
