//! Target/session-bound transient ESC tones. The capture worker serializes
//! shell exchanges and retains the playback fence even if START is cancelled.

use std::sync::Arc;
use std::time::Duration;

use maggcs_core::mavlink::router::RoutedEvents;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::motor_test::tone::{commands_for_status, tone_outputs};
pub use maggcs_core::motor_test::tone::{
    MotorTone, MotorToneError, MotorToneSettings, MotorToneTrigger,
};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::State;
use tokio::sync::{oneshot, watch};

use crate::motor_test_state::{CaptureRequest, MotorTestTargetRef, OutputConfigSnapshot};
use crate::state::AppState;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
/// PX4's mixer backup schedule is 50 ms. Leave extra dispatch/ESC margin.
const PLAYBACK_MARGIN: Duration = Duration::from_millis(250);
/// Allow the released rotor to coast before requesting a beacon. This is not
/// an RPM/stationary measurement; acoustic acceptance remains hardware-based.
const STOP_COAST: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotorToneRequest {
    pub target: MotorTestTargetRef,
    pub config_revision: u64,
    pub session_generation: u64,
    pub trigger: MotorToneTrigger,
    pub tone: MotorTone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TonePhase {
    #[default]
    Idle,
    Queued,
    Checking,
    Playing,
    Complete,
    Failed,
    Cancelled,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MotorToneStatus {
    pub settings: MotorToneSettings,
    pub starting: bool,
    pub busy: bool,
    pub phase: TonePhase,
    pub trigger: Option<MotorToneTrigger>,
    pub error: Option<String>,
}

#[derive(Default)]
struct Inner {
    generation: u64,
    queued: usize,
    status: MotorToneStatus,
    running: Option<MotorToneRequest>,
}

pub struct MotorToneState {
    inner: Mutex<Inner>,
    changed: watch::Sender<u64>,
}

pub struct ToneJob {
    pub request: MotorToneRequest,
    pub reply: Option<oneshot::Sender<Result<(), MotorToneError>>>,
    state: Arc<MotorToneState>,
}

impl Drop for ToneJob {
    fn drop(&mut self) {
        let mut inner = self.state.inner.lock();
        inner.queued = inner.queued.saturating_sub(1);
        inner.status.busy = inner.queued > 0;
    }
}

impl Default for MotorToneState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            changed: watch::channel(0).0,
        }
    }
}

impl MotorToneState {
    pub fn snapshot(&self) -> MotorToneStatus {
        self.inner.lock().status.clone()
    }
    pub fn generation(&self) -> u64 {
        self.inner.lock().generation
    }

    fn bump(&self, inner: &mut Inner) -> u64 {
        inner.generation = inner.generation.wrapping_add(1);
        self.changed.send_replace(inner.generation);
        inner.generation
    }

    /// Called synchronously before awaiting a motor release or lifecycle lock.
    pub fn cancel(&self) {
        let mut inner = self.inner.lock();
        self.bump(&mut inner);
        inner.status.starting = false;
        inner.status.phase = TonePhase::Cancelled;
        inner.status.error = None;
    }

    pub fn begin(
        self: &Arc<Self>,
        config: &OutputConfigSnapshot,
        expected: u64,
    ) -> Result<StartLease, MotorToneError> {
        let supported = match tone_outputs(&config.map, &config.models) {
            Ok(_) => true,
            Err(MotorToneError::UnsupportedOutput { .. }) => false,
            Err(error) => return Err(error),
        };
        let mut inner = self.inner.lock();
        if inner.generation != expected {
            return Err(MotorToneError::Cancelled);
        }
        let generation = self.bump(&mut inner);
        inner.running = None;
        inner.status.starting = true;
        inner.status.phase = if supported {
            TonePhase::Queued
        } else {
            TonePhase::Unavailable
        };
        inner.status.error = None;
        let request = MotorToneRequest {
            target: config.target,
            config_revision: config.revision,
            session_generation: generation,
            trigger: MotorToneTrigger::Starting,
            tone: if supported {
                inner.status.settings.start
            } else {
                MotorTone::Off
            },
        };
        let stop = MotorToneRequest {
            trigger: MotorToneTrigger::Stopped,
            tone: if supported {
                inner.status.settings.stop
            } else {
                MotorTone::Off
            },
            ..request
        };
        Ok(StartLease {
            state: self.clone(),
            request,
            stop,
            committed: false,
        })
    }

    async fn cancelled(&self, generation: u64) {
        let mut changed = self.changed.subscribe();
        while self.generation() == generation {
            if changed.changed().await.is_err() {
                break;
            }
        }
    }

    pub fn finish(&self, request: MotorToneRequest, result: &Result<(), MotorToneError>) {
        let mut inner = self.inner.lock();
        if inner.generation != request.session_generation {
            return;
        }
        inner.status.trigger = Some(request.trigger);
        inner.status.phase = match result {
            Ok(()) => TonePhase::Complete,
            Err(MotorToneError::Cancelled | MotorToneError::SessionChanged) => TonePhase::Cancelled,
            Err(_) => TonePhase::Failed,
        };
        inner.status.error = match result {
            Err(MotorToneError::Cancelled | MotorToneError::SessionChanged) => None,
            Err(error) => Some(error.to_string()),
            Ok(()) => None,
        };
    }
}

/// A dropped/failed START cancels any queued tone; committing the actual motor
/// command and cancelling START are linearized by the same short mutex.
pub struct StartLease {
    state: Arc<MotorToneState>,
    pub request: MotorToneRequest,
    stop: MotorToneRequest,
    committed: bool,
}

impl StartLease {
    pub fn commit(&mut self, send: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let mut inner = self.state.inner.lock();
        if inner.generation != self.request.session_generation || !inner.status.starting {
            return Err(MotorToneError::Cancelled.to_string());
        }
        send()?;
        inner.running = Some(self.stop);
        inner.status.starting = false;
        self.committed = true;
        Ok(())
    }
}

impl Drop for StartLease {
    fn drop(&mut self) {
        if !self.committed {
            let mut inner = self.state.inner.lock();
            if inner.generation == self.request.session_generation {
                self.state.bump(&mut inner);
                inner.status.starting = false;
            }
        }
    }
}

#[tauri::command]
pub async fn actuator_set_tones(
    state: State<'_, AppState>,
    settings: MotorToneSettings,
) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    if state.motor_test.session().is_some_and(|session| {
        session.is_sending() || session == maggcs_core::motor_test::SessionState::Stopping
    }) {
        return Err(MotorToneError::Busy.to_string());
    }
    let mut inner = state.motor_test.tones.inner.lock();
    if inner.status.starting || inner.status.busy {
        return Err(MotorToneError::Busy.to_string());
    }
    inner.status.settings = settings;
    Ok(())
}

fn validate(
    state: &AppState,
    request: MotorToneRequest,
) -> Result<OutputConfigSnapshot, MotorToneError> {
    crate::motor_control_service::ensure_enabled(state).map_err(|_| MotorToneError::Cancelled)?;
    if state.motor_test.tones.generation() != request.session_generation {
        return Err(MotorToneError::Cancelled);
    }
    let config = {
        let guard = state.motor_test.target.lock();
        let runtime = guard.as_ref().ok_or(MotorToneError::TargetChanged)?;
        if runtime.target.target_ref() != request.target
            || runtime
                .target
                .config_stale
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(MotorToneError::TargetChanged);
        }
        let config = runtime
            .target
            .config
            .lock()
            .clone()
            .ok_or(MotorToneError::TargetChanged)?;
        config
    };
    if config.target != request.target
        || config.revision != request.config_revision
        || !config.is_startable()
    {
        return Err(MotorToneError::TargetChanged);
    }
    crate::actuator_service::ensure_target_disarmed(state, request.target)
        .map_err(|message| MotorToneError::Shell { message })?;
    if state
        .motor_test
        .session()
        .is_some_and(|session| session.is_sending())
    {
        return Err(MotorToneError::Busy);
    }
    Ok(config)
}

fn queue(
    state: &AppState,
    request: MotorToneRequest,
) -> Result<oneshot::Receiver<Result<(), MotorToneError>>, MotorToneError> {
    validate(state, request)?;
    let sender = crate::capture_service::capture_sender(state, request.target)
        .map_err(|message| MotorToneError::Shell { message })?;
    let (reply, response) = oneshot::channel();
    let permit = sender.try_reserve().map_err(|_| MotorToneError::Busy)?;
    let mut inner = state.motor_test.tones.inner.lock();
    if inner.generation != request.session_generation {
        return Err(MotorToneError::Cancelled);
    }
    inner.queued += 1;
    inner.status.busy = true;
    inner.status.phase = TonePhase::Queued;
    inner.status.trigger = Some(request.trigger);
    permit.send(CaptureRequest::Tone(ToneJob {
        request,
        reply: Some(reply),
        state: state.motor_test.tones.clone(),
    }));
    Ok(response)
}

pub async fn play_motor_tone(
    state: &AppState,
    request: MotorToneRequest,
) -> Result<(), MotorToneError> {
    let result = async {
      let response = queue(state, request)?;
      tokio::select! {
        biased;
        _ = state.motor_test.tones.cancelled(request.session_generation) => Err(MotorToneError::Cancelled),
        result = tokio::time::timeout(REQUEST_TIMEOUT, response) => {
            result.map_err(|_| MotorToneError::Timeout)?.map_err(|_| MotorToneError::TargetChanged)?
        }
      }
    }.await;
    state.motor_test.tones.finish(request, &result);
    result
}

pub fn begin_start(
    state: &AppState,
    target: MotorTestTargetRef,
    revision: u64,
    expected: u64,
) -> Result<StartLease, MotorToneError> {
    let config = validate(
        state,
        MotorToneRequest {
            target,
            config_revision: revision,
            session_generation: expected,
            trigger: MotorToneTrigger::Starting,
            tone: MotorTone::Off,
        },
    )?;
    state.motor_test.tones.begin(&config, expected)
}

/// Exactly one stop cue per successfully committed run, even if a transport
/// receipt and its asynchronous Stopped event arrive in either order.
pub fn after_release(state: &AppState, target: MotorTestTargetRef, started_generation: u64) {
    let request = {
        let tones = &state.motor_test.tones;
        let mut inner = tones.inner.lock();
        let Some(running) = inner.running else {
            return;
        };
        if running.target != target || running.session_generation != started_generation {
            return;
        }
        inner.running = None;
        let generation = tones.bump(&mut inner);
        MotorToneRequest {
            session_generation: generation,
            ..running
        }
    };
    if request.tone == MotorTone::Off {
        return;
    }
    if let Err(error) = queue(state, request) {
        state.motor_test.tones.finish(request, &Err(error));
    }
}

fn shell_error(output: &str) -> Result<(), MotorToneError> {
    let lower = output.to_ascii_lowercase();
    if lower.contains("timeout") {
        return Err(MotorToneError::Timeout);
    }
    if crate::px4_shell::command_error(output).is_some()
        || lower.contains("not running")
        || lower.contains("error")
    {
        return Err(MotorToneError::Shell {
            message: output.trim().into(),
        });
    }
    Ok(())
}

fn shell_failure(error: maggcs_core::logs::capture::SdCaptureError) -> MotorToneError {
    use maggcs_core::logs::capture::SdCaptureError;
    match error {
        SdCaptureError::ShellTimeout { .. } => MotorToneError::Timeout,
        SdCaptureError::TargetChanged | SdCaptureError::TargetUnavailable => {
            MotorToneError::TargetChanged
        }
        SdCaptureError::Busy => MotorToneError::Busy,
        error => MotorToneError::Shell {
            message: error.to_string(),
        },
    }
}

/// Runs only on the one target shell worker. Once a beep may have been sent,
/// drain its exchange and playback window even after cancellation. Subsequent
/// START requests (including Off) pass this worker as a quiet-output fence.
pub async fn execute(
    state: &AppState,
    handle: &ConnectionHandle,
    route: &mut RoutedEvents,
    request: MotorToneRequest,
) -> Result<(), MotorToneError> {
    let config = validate(state, request)?;
    if handle.handle_id() != request.target.connection_id {
        return Err(MotorToneError::TargetChanged);
    }
    if request.tone == MotorTone::Off {
        return Ok(());
    }
    if request.trigger == MotorToneTrigger::Stopped {
        tokio::select! {
            _ = state.motor_test.tones.cancelled(request.session_generation) => return Err(MotorToneError::Cancelled),
            _ = tokio::time::sleep(STOP_COAST) => {}
        }
    }
    validate(state, request)?;
    let status = crate::px4_shell::exchange(
        handle,
        request.target.target_system_id,
        request.target.target_component_id,
        route,
        "dshot status",
    )
    .await
    .map_err(shell_failure)?;
    shell_error(&status)?;
    validate(state, request)?;
    let outputs = tone_outputs(&config.map, &config.models)?;
    let commands = commands_for_status(&outputs, request.tone, &status)?;
    let mut quiet_until = tokio::time::Instant::now();
    let mut result = Ok(());
    for command in commands {
        if let Err(error) = validate(state, request) {
            result = Err(error);
            break;
        }
        {
            let mut inner = state.motor_test.tones.inner.lock();
            if inner.generation != request.session_generation {
                result = Err(MotorToneError::Cancelled);
                break;
            }
            if crate::motor_control_service::with_enabled(state, || {
                inner.status.phase = TonePhase::Playing;
                Ok(())
            })
            .is_err()
            {
                result = Err(MotorToneError::Cancelled);
                break;
            }
        }
        let response = crate::px4_shell::exchange(
            handle,
            request.target.target_system_id,
            request.target.target_component_id,
            route,
            &command,
        )
        .await;
        quiet_until = tokio::time::Instant::now() + request.tone.duration() + PLAYBACK_MARGIN;
        result = response
            .map_err(shell_failure)
            .and_then(|output| shell_error(&output));
        if result.is_err() {
            break;
        }
    }
    tokio::time::sleep_until(quiet_until).await;
    result?;
    validate(state, request)?;
    Ok(())
}

pub fn working(state: &AppState, request: MotorToneRequest) {
    let mut inner = state.motor_test.tones.inner.lock();
    if inner.generation == request.session_generation {
        inner.status.phase = TonePhase::Checking;
        inner.status.trigger = Some(request.trigger);
        inner.status.error = None;
    }
}
