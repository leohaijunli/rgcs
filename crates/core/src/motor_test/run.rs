//! Per-test run coordinator (Task 8): capture → pre-roll → motor → post-roll.
//!
//! The SD capture and the motor session are independent services. This pure
//! state machine binds them to one test id: it owns the 1 s pre-roll before
//! the operator's START is allowed, the 1 s post-roll after motor release, and
//! the final `logger off` hand-off. It never touches I/O; the app shell sends
//! the returned actions and writes the recovery manifest.
//!
//! Safety rule: capture is only stopped after a transport-confirmed motor
//! release. The coordinator never emits `StopCapture` while `MotorRunning`.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::logs::capture::{CapturePhase, SdCaptureError, SdCaptureStatus};

use super::service::StopReason;

/// Delay between a confirmed recording and the first allowed motor START.
pub const PRE_ROLL: Duration = Duration::from_secs(1);
/// Delay between a confirmed motor release and `logger off`.
pub const POST_ROLL: Duration = Duration::from_secs(1);
/// How long the capture may spend confirming `logger off`.
pub const RUN_FINALIZE_TIMEOUT: Duration = Duration::from_secs(12);

/// Phase of one planned test run. Serialized for the window and manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    StartingCapture,
    Preroll,
    MotorReady,
    MotorRunning,
    Postroll,
    StoppingCapture,
    Complete,
    Aborted,
    Interrupted,
    Failed,
}

impl RunPhase {
    /// The run will not advance without an explicit new start.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Complete | Self::Aborted | Self::Interrupted | Self::Failed
        )
    }

    /// The operator may start motors only in this phase.
    pub fn is_motor_ready(self) -> bool {
        self == Self::MotorReady
    }
}

/// Action the app shell must perform for this step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunAction {
    StartCapture,
    StopCapture,
}

/// Coordinator failure, mirrored into the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RunError {
    #[error("motor start requires a confirmed recording and the pre-roll to finish")]
    MotorNotReady,
    #[error("capture failed: {error}")]
    CaptureFailed { error: SdCaptureError },
    #[error("capture stop was not confirmed before the finalization deadline")]
    CaptureStopTimeout,
    #[error("run interrupted: {reason}")]
    Interrupted { reason: String },
}

/// Serializable current run state for the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RunStatus {
    pub test_id: String,
    pub phase: RunPhase,
    pub remote_path: Option<String>,
    pub stop_reason: Option<StopReason>,
    pub error: Option<RunError>,
}

/// One coordinator step, including whether the visible state changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunStep {
    pub status: Option<RunStatus>,
    pub actions: Vec<RunAction>,
    pub changed: bool,
}

/// Run timings. The test id is part of the configuration so every step can
/// report it without a second lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    pub test_id: String,
    pub pre_roll: Duration,
    pub post_roll: Duration,
    pub finalize_timeout: Duration,
}

impl RunConfig {
    pub fn new(test_id: impl Into<String>) -> Self {
        Self {
            test_id: test_id.into(),
            pre_roll: PRE_ROLL,
            post_roll: POST_ROLL,
            finalize_timeout: RUN_FINALIZE_TIMEOUT,
        }
    }

    /// Short timings for deterministic tests.
    pub fn with_timings(mut self, pre_roll: Duration, post_roll: Duration) -> Self {
        self.pre_roll = pre_roll;
        self.post_roll = post_roll;
        self
    }
}

/// Pure per-test coordinator. All transitions are monotonic; duplicate events
/// are ignored and never restart a timer.
pub struct RunCoordinator {
    config: RunConfig,
    phase: RunPhase,
    remote_path: Option<String>,
    stop_reason: Option<StopReason>,
    error: Option<RunError>,
    pre_roll_until: Option<Instant>,
    post_roll_until: Option<Instant>,
    finalize_deadline: Option<Instant>,
    aborted: bool,
}

impl RunCoordinator {
    /// Begin a run. The first action starts the SD capture; the app feeds the
    /// confirmed recording back through [`Self::observe_capture`].
    pub fn begin(config: RunConfig, _now: Instant) -> (Self, RunStep) {
        let coordinator = Self {
            config,
            phase: RunPhase::StartingCapture,
            remote_path: None,
            stop_reason: None,
            error: None,
            pre_roll_until: None,
            post_roll_until: None,
            finalize_deadline: None,
            aborted: false,
        };
        let step = RunStep {
            status: Some(coordinator.status()),
            actions: vec![RunAction::StartCapture],
            changed: true,
        };
        (coordinator, step)
    }

    pub fn status(&self) -> RunStatus {
        RunStatus {
            test_id: self.config.test_id.clone(),
            phase: self.phase,
            remote_path: self.remote_path.clone(),
            stop_reason: self.stop_reason,
            error: self.error.clone(),
        }
    }

    pub fn phase(&self) -> RunPhase {
        self.phase
    }

    pub fn is_terminal(&self) -> bool {
        self.phase.is_terminal()
    }

    pub fn is_motor_ready(&self) -> bool {
        self.phase.is_motor_ready()
    }

    /// Observe the capture service. `Recording` completes the pre-roll timer;
    /// `Complete` finalizes a normal stop; `Failed` mirrors the capture error.
    pub fn observe_capture(&mut self, status: &SdCaptureStatus, now: Instant) -> RunStep {
        match status.phase {
            CapturePhase::Recording => {
                let path_changed =
                    status.remote_path.is_some() && status.remote_path != self.remote_path;
                if status.remote_path.is_some() {
                    self.remote_path = status.remote_path.clone();
                }
                match self.phase {
                    RunPhase::StartingCapture => {
                        if self.aborted {
                            self.begin_stopping(now);
                            return self.changed_step(vec![RunAction::StopCapture]);
                        }
                        self.phase = RunPhase::Preroll;
                        self.pre_roll_until = Some(now + self.config.pre_roll);
                        self.changed_step(Vec::new())
                    }
                    RunPhase::Preroll
                    | RunPhase::MotorReady
                    | RunPhase::MotorRunning
                    | RunPhase::Postroll => {
                        if path_changed {
                            self.changed_step(Vec::new())
                        } else {
                            RunStep::default()
                        }
                    }
                    RunPhase::StoppingCapture => {
                        if path_changed {
                            self.changed_step(Vec::new())
                        } else {
                            RunStep::default()
                        }
                    }
                    RunPhase::Complete
                    | RunPhase::Aborted
                    | RunPhase::Interrupted
                    | RunPhase::Failed => RunStep::default(),
                }
            }
            CapturePhase::ExistingRecording => {
                if self.phase.is_terminal() {
                    return RunStep::default();
                }
                self.phase = RunPhase::Interrupted;
                self.error = Some(RunError::Interrupted {
                    reason: "the target logger was already recording another file".into(),
                });
                self.clear_deadlines();
                self.changed_step(Vec::new())
            }
            CapturePhase::Complete => match self.phase {
                RunPhase::StoppingCapture => {
                    self.phase = if self.aborted {
                        RunPhase::Aborted
                    } else {
                        RunPhase::Complete
                    };
                    self.clear_deadlines();
                    self.changed_step(Vec::new())
                }
                RunPhase::Complete
                | RunPhase::Aborted
                | RunPhase::Interrupted
                | RunPhase::Failed => RunStep::default(),
                _ => {
                    self.phase = RunPhase::Interrupted;
                    self.error = Some(RunError::Interrupted {
                        reason: "capture stopped before the motor-test run ended".into(),
                    });
                    self.clear_deadlines();
                    self.changed_step(Vec::new())
                }
            },
            CapturePhase::Failed => {
                if self.phase.is_terminal() {
                    return RunStep::default();
                }
                let error = status
                    .last_error
                    .clone()
                    .unwrap_or(SdCaptureError::StartNotConfirmed);
                self.phase = RunPhase::Failed;
                self.error = Some(RunError::CaptureFailed { error });
                self.clear_deadlines();
                self.changed_step(Vec::new())
            }
            _ => RunStep::default(),
        }
    }

    /// Confirm that the motor start command was sent. The app calls this only
    /// after transport confirmation, so a rejected start leaves the run in
    /// `MotorReady` for a retry.
    pub fn note_motor_started(&mut self, now: Instant) -> Result<RunStep, RunError> {
        self.advance(now);
        if self.phase != RunPhase::MotorReady {
            return Err(RunError::MotorNotReady);
        }
        self.phase = RunPhase::MotorRunning;
        Ok(self.changed_step(Vec::new()))
    }

    /// The session confirmed a full release. Only this starts the post-roll;
    /// duplicate release events are ignored.
    pub fn note_motor_released(&mut self, reason: StopReason, now: Instant) -> RunStep {
        if self.phase != RunPhase::MotorRunning {
            return RunStep::default();
        }
        self.stop_reason = Some(reason);
        self.phase = RunPhase::Postroll;
        self.post_roll_until = Some(now + self.config.post_roll);
        self.changed_step(Vec::new())
    }

    /// Operator abort. While motors run the caller must release them first;
    /// the abort flag then makes the eventual stop an `Aborted` run.
    pub fn abort(&mut self, now: Instant) -> RunStep {
        if self.phase.is_terminal() {
            return RunStep::default();
        }
        self.aborted = true;
        match self.phase {
            RunPhase::StartingCapture => self.changed_step(Vec::new()),
            RunPhase::Preroll | RunPhase::MotorReady => {
                self.begin_stopping(now);
                self.changed_step(vec![RunAction::StopCapture])
            }
            RunPhase::MotorRunning | RunPhase::Postroll | RunPhase::StoppingCapture => {
                self.changed_step(Vec::new())
            }
            RunPhase::Complete | RunPhase::Aborted | RunPhase::Interrupted | RunPhase::Failed => {
                RunStep::default()
            }
        }
    }

    /// Link/target loss. The app owns best-effort motor release and link
    /// teardown; no further capture action is emitted here.
    pub fn interrupt(&mut self, reason: impl Into<String>, _now: Instant) -> RunStep {
        if self.phase.is_terminal() {
            return RunStep::default();
        }
        self.phase = RunPhase::Interrupted;
        self.error = Some(RunError::Interrupted {
            reason: reason.into(),
        });
        self.clear_deadlines();
        self.changed_step(Vec::new())
    }

    /// Advance timers. Called by the app's run ticker.
    pub fn tick(&mut self, now: Instant) -> RunStep {
        self.advance(now)
    }

    fn advance(&mut self, now: Instant) -> RunStep {
        match self.phase {
            RunPhase::Preroll if self.pre_roll_until.is_some_and(|at| now >= at) => {
                self.pre_roll_until = None;
                self.phase = RunPhase::MotorReady;
                self.changed_step(Vec::new())
            }
            RunPhase::Postroll if self.post_roll_until.is_some_and(|at| now >= at) => {
                self.post_roll_until = None;
                self.begin_stopping(now);
                self.changed_step(vec![RunAction::StopCapture])
            }
            RunPhase::StoppingCapture if self.finalize_deadline.is_some_and(|at| now >= at) => {
                self.phase = RunPhase::Failed;
                self.error = Some(RunError::CaptureStopTimeout);
                self.clear_deadlines();
                self.changed_step(Vec::new())
            }
            _ => RunStep::default(),
        }
    }

    fn begin_stopping(&mut self, now: Instant) {
        self.phase = RunPhase::StoppingCapture;
        self.pre_roll_until = None;
        self.post_roll_until = None;
        self.finalize_deadline = Some(now + self.config.finalize_timeout);
    }

    fn clear_deadlines(&mut self) {
        self.pre_roll_until = None;
        self.post_roll_until = None;
        self.finalize_deadline = None;
    }

    fn changed_step(&self, actions: Vec<RunAction>) -> RunStep {
        RunStep {
            status: Some(self.status()),
            actions,
            changed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(path: &str, test_id: &str) -> SdCaptureStatus {
        SdCaptureStatus {
            phase: CapturePhase::Recording,
            test_id: Some(test_id.into()),
            remote_path: Some(path.into()),
            ..SdCaptureStatus::default()
        }
    }

    fn capture_complete() -> SdCaptureStatus {
        SdCaptureStatus {
            phase: CapturePhase::Complete,
            ..SdCaptureStatus::default()
        }
    }

    fn started(path: &str) -> (RunCoordinator, Instant) {
        let now = Instant::now();
        let (mut run, step) = RunCoordinator::begin(RunConfig::new("t1"), now);
        assert_eq!(step.actions, vec![RunAction::StartCapture]);
        let step = run.observe_capture(&recording(path, "t1"), now);
        assert_eq!(step.status.unwrap().phase, RunPhase::Preroll);
        (run, now)
    }

    #[test]
    fn allows_motor_only_after_the_pre_roll() {
        let (mut run, now) = started("/fs/microsd/log/a.ulg");
        assert_eq!(run.note_motor_started(now), Err(RunError::MotorNotReady));
        let step = run.tick(now + PRE_ROLL / 2);
        assert!(!step.changed);
        let step = run.tick(now + PRE_ROLL);
        assert_eq!(step.status.unwrap().phase, RunPhase::MotorReady);
        let step = run.note_motor_started(now + PRE_ROLL).expect("ready");
        assert_eq!(step.status.unwrap().phase, RunPhase::MotorRunning);
    }

    #[test]
    fn motor_release_waits_for_postroll_then_stops_capture() {
        let (mut run, now) = started("/fs/microsd/log/a.ulg");
        run.tick(now + PRE_ROLL);
        run.note_motor_started(now + PRE_ROLL).expect("start");
        let step = run.note_motor_released(StopReason::Command, now + PRE_ROLL);
        assert_eq!(step.status.unwrap().phase, RunPhase::Postroll);

        // A duplicate release never restarts the post-roll.
        assert!(
            !run.note_motor_released(StopReason::Emergency, now + PRE_ROLL / 2)
                .changed
        );

        let step = run.tick(now + PRE_ROLL + POST_ROLL / 2);
        assert!(!step.changed);
        let step = run.tick(now + PRE_ROLL + POST_ROLL);
        assert_eq!(
            step.status.as_ref().unwrap().phase,
            RunPhase::StoppingCapture
        );
        assert_eq!(step.actions, vec![RunAction::StopCapture]);

        let step = run.observe_capture(&capture_complete(), now + PRE_ROLL + POST_ROLL);
        let status = step.status.unwrap();
        assert_eq!(status.phase, RunPhase::Complete);
        assert_eq!(status.stop_reason, Some(StopReason::Command));
        assert!(run.is_terminal());
    }

    #[test]
    fn abort_before_motor_start_stops_capture_and_reports_aborted() {
        let (mut run, now) = started("/fs/microsd/log/a.ulg");
        let step = run.abort(now);
        assert_eq!(step.status.unwrap().phase, RunPhase::StoppingCapture);
        assert_eq!(step.actions, vec![RunAction::StopCapture]);
        let step = run.observe_capture(&capture_complete(), now);
        assert_eq!(step.status.unwrap().phase, RunPhase::Aborted);
    }

    #[test]
    fn abort_while_recording_start_in_flight_stops_on_confirmation() {
        let now = Instant::now();
        let (mut run, _) = RunCoordinator::begin(RunConfig::new("t1"), now);
        assert!(run.abort(now).changed);
        let step = run.observe_capture(&recording("/fs/microsd/log/a.ulg", "t1"), now);
        assert_eq!(step.status.unwrap().phase, RunPhase::StoppingCapture);
        assert_eq!(step.actions, vec![RunAction::StopCapture]);
    }

    #[test]
    fn abort_while_running_never_stops_capture_before_release() {
        let (mut run, now) = started("/fs/microsd/log/a.ulg");
        run.tick(now + PRE_ROLL);
        run.note_motor_started(now + PRE_ROLL).expect("start");

        let step = run.abort(now + PRE_ROLL);
        assert_eq!(step.status.unwrap().phase, RunPhase::MotorRunning);
        assert!(step.actions.is_empty());

        // Time alone must not reach `logger off` while the motor is running.
        let step = run.tick(now + PRE_ROLL + POST_ROLL * 2);
        assert!(step.actions.is_empty());
        assert_eq!(run.phase(), RunPhase::MotorRunning);

        let step = run.note_motor_released(StopReason::Command, now + PRE_ROLL + POST_ROLL * 2);
        assert_eq!(step.status.unwrap().phase, RunPhase::Postroll);
        let stop_at = now + PRE_ROLL + POST_ROLL * 2 + POST_ROLL;
        let step = run.tick(stop_at);
        assert_eq!(step.actions, vec![RunAction::StopCapture]);
        let step = run.observe_capture(&capture_complete(), stop_at);
        assert_eq!(step.status.unwrap().phase, RunPhase::Aborted);
    }

    #[test]
    fn capture_stop_timeout_fails_the_run_instead_of_hanging() {
        let (mut run, now) = started("/fs/microsd/log/a.ulg");
        run.abort(now);
        let step = run.tick(now + RUN_FINALIZE_TIMEOUT);
        let status = step.status.unwrap();
        assert_eq!(status.phase, RunPhase::Failed);
        assert_eq!(status.error, Some(RunError::CaptureStopTimeout));
    }

    #[test]
    fn capture_failure_mirrors_the_capture_error() {
        let (mut run, now) = started("/fs/microsd/log/a.ulg");
        run.tick(now + PRE_ROLL);
        run.note_motor_started(now + PRE_ROLL).expect("start");
        let failed = SdCaptureStatus {
            phase: CapturePhase::Failed,
            last_error: Some(SdCaptureError::TargetUnavailable),
            ..SdCaptureStatus::default()
        };
        let status = run.observe_capture(&failed, now + PRE_ROLL).status.unwrap();
        assert_eq!(status.phase, RunPhase::Failed);
        assert_eq!(
            status.error,
            Some(RunError::CaptureFailed {
                error: SdCaptureError::TargetUnavailable
            })
        );
    }

    #[test]
    fn interrupt_is_terminal_and_keeps_the_path_for_recovery() {
        let (mut run, now) = started("/fs/microsd/log/a.ulg");
        let status = run.interrupt("target link closed", now).status.unwrap();
        assert_eq!(status.phase, RunPhase::Interrupted);
        assert_eq!(status.remote_path.as_deref(), Some("/fs/microsd/log/a.ulg"));
        assert!(!run.tick(now + POST_ROLL).changed);
    }
}
