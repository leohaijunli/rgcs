//! PX4 SD-file capture control over the target's MAVLink shell.
//!
//! The pure state machine here owns the command sequence and the response
//! parsing; the app shell sends each returned line through `SERIAL_CONTROL`
//! (`SERIAL_CONTROL_DEV_SHELL`) and feeds reply chunks back in. Commands are
//! correlated with a unique `echo` marker, so an asynchronous shell echo can
//! never be mistaken for command completion.
//!
//! PX4 v1.17 facts: `logger on` / `logger off` override the arming-based file
//! logger; `logger status` reports `Running in mode: ...`, `Full File Logging
//! Running:` and the active `Log file:` path. `MAV_CMD_LOGGING_START/STOP`
//! controls MAVLink ULog streaming, not the SD file, so it is not used here.

use std::time::{Duration, Instant};

use thiserror::Error;

use ts_rs::TS;

/// Bounded shell output kept for diagnostics and parsing.
const MAX_OUTPUT: usize = 16 * 1024;

/// What the capture controller is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CapturePhase {
    Idle,
    Verifying,
    Ready,
    Starting,
    Recording,
    /// A file is recording, but it was not started/confirmed for this app's test.
    ExistingRecording,
    Stopping,
    Finalizing,
    Complete,
    Failed,
}

/// Logger backend reported by `logger status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LoggerBackend {
    File,
    Mavlink,
    FileAndMavlink,
    Other,
    Unknown,
}

/// One `logger status` observation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct LoggerStatus {
    pub running: bool,
    pub backend: LoggerBackend,
    /// Full or mission file logging is running.
    pub file_logging: bool,
    /// Remote ULog path, when PX4 reports one.
    pub remote_path: Option<String>,
}

/// Serializable capture state for the window. `last_error` is cleared on the
/// next verify/start attempt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct SdCaptureStatus {
    pub phase: CapturePhase,
    pub test_id: Option<String>,
    pub remote_path: Option<String>,
    pub logger: Option<LoggerStatus>,
    pub sdlog_mode: Option<i32>,
    pub sdlog_backend: Option<i32>,
    pub last_error: Option<SdCaptureError>,
}

impl Default for SdCaptureStatus {
    fn default() -> Self {
        Self {
            phase: CapturePhase::Idle,
            test_id: None,
            remote_path: None,
            logger: None,
            sdlog_mode: None,
            sdlog_backend: None,
            last_error: None,
        }
    }
}

/// Explicit failures; none of these are retried silently.
#[derive(Debug, Clone, PartialEq, Eq, Error, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SdCaptureError {
    #[error("no target shell connection")]
    TargetUnavailable,
    #[error("capture target changed; check the logger on the selected PX4 again")]
    TargetChanged,
    #[error("target shell transport failed: {message}")]
    Transport { message: String },
    #[error("logger module is not running")]
    LoggerNotRunning,
    #[error("logger is not running with the file backend")]
    LoggerFileBackendUnavailable,
    #[error("a file capture is already running")]
    CaptureAlreadyActive { path: Option<String> },
    #[error("shell command `{command}` timed out")]
    ShellTimeout { command: String },
    #[error("unexpected shell response: {output}")]
    UnexpectedResponse { output: String },
    #[error("capture start was not confirmed")]
    StartNotConfirmed,
    #[error("SD storage is unavailable: {detail}")]
    StorageUnavailable { detail: String },
    #[error("logger off did not stop SD recording within the finalization window; check the running logger mode (SDLOG_MODE=0 is the disarmed bench default, and mode changes require reboot)")]
    StopNotConfirmed,
    #[error("log path did not change (still {path})")]
    PathNotUnique { path: String },
    #[error("test id must not be empty")]
    InvalidTestId,
    #[error("capture controller is busy")]
    Busy,
}

/// Commands accepted by the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdCaptureCommand {
    /// Observe actual logger state. Parameter observations are read-only and
    /// may be unknown; neither an existing capture nor a nonzero mode rejects Check.
    Check {
        sdlog_mode: Option<i32>,
        sdlog_backend: Option<i32>,
    },
    Start {
        test_id: String,
        sdlog_mode: Option<i32>,
        sdlog_backend: Option<i32>,
    },
    Stop,
}

/// State/terminal notification for the UI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SdCaptureEvent {
    Status { status: SdCaptureStatus },
}

/// One controller step: events to publish and shell lines to send.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SdCaptureStep {
    pub events: Vec<SdCaptureEvent>,
    pub outbound: Vec<String>,
}

/// Timeouts, kept explicit so the state machine is deterministic in tests.
#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    pub command_timeout: Duration,
    pub start_timeout: Duration,
    pub finalize_timeout: Duration,
    pub poll_interval: Duration,
    pub finalize_settle: Duration,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            command_timeout: Duration::from_secs(2),
            start_timeout: Duration::from_secs(5),
            finalize_timeout: Duration::from_secs(10),
            poll_interval: Duration::from_millis(400),
            finalize_settle: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingCommand {
    Status,
    On,
    Off,
}

#[derive(Debug)]
struct Pending {
    marker: String,
    command: PendingCommand,
    deadline: Instant,
}

/// The capture controller for one target shell.
#[derive(Debug)]
pub struct SdCapture {
    config: CaptureConfig,
    phase: CapturePhase,
    test_id: Option<String>,
    remote_path: Option<String>,
    previous_path: Option<String>,
    logger: Option<LoggerStatus>,
    sdlog_mode: Option<i32>,
    sdlog_backend: Option<i32>,
    last_error: Option<SdCaptureError>,
    pending: Option<Pending>,
    /// Complete lines since the last command was sent.
    output: String,
    /// Bytes not yet terminated by `\n`.
    line_buffer: String,
    /// Diagnostics from `logger on` and intervening status polls.
    start_diagnostics: String,
    /// True when the current status check was requested by Start.
    start_after_verify: bool,
    deadline: Option<Instant>,
    next_poll: Option<Instant>,
    not_logging_since: Option<Instant>,
    nonce: u32,
    sequence: u64,
}

impl Default for SdCapture {
    fn default() -> Self {
        Self::new(CaptureConfig::default())
    }
}

impl SdCapture {
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            config,
            phase: CapturePhase::Idle,
            test_id: None,
            remote_path: None,
            previous_path: None,
            logger: None,
            sdlog_mode: None,
            sdlog_backend: None,
            last_error: None,
            pending: None,
            output: String::new(),
            line_buffer: String::new(),
            start_diagnostics: String::new(),
            start_after_verify: false,
            deadline: None,
            next_poll: None,
            not_logging_since: None,
            nonce: 0x4d47_4353,
            sequence: 0,
        }
    }

    pub fn status(&self) -> SdCaptureStatus {
        SdCaptureStatus {
            phase: self.phase,
            test_id: self.test_id.clone(),
            remote_path: self.remote_path.clone(),
            logger: self.logger.clone(),
            sdlog_mode: self.sdlog_mode,
            sdlog_backend: self.sdlog_backend,
            last_error: self.last_error.clone(),
        }
    }

    /// Start a check/start/stop sequence. The returned lines must be sent to
    /// the target shell in order.
    ///
    /// # Errors
    ///
    /// Configuration errors and busy/invalid-state requests. A failed shell
    /// exchange is reported through [`Self::status`] after [`Self::tick`].
    pub fn command(
        &mut self,
        command: SdCaptureCommand,
        now: Instant,
    ) -> Result<SdCaptureStep, SdCaptureError> {
        if self.pending.is_some()
            || matches!(
                self.phase,
                CapturePhase::Starting | CapturePhase::Stopping | CapturePhase::Finalizing
            )
        {
            return Err(SdCaptureError::Busy);
        }
        match command {
            SdCaptureCommand::Check {
                sdlog_mode,
                sdlog_backend,
            } => {
                self.sdlog_mode = sdlog_mode;
                self.sdlog_backend = sdlog_backend;
                self.phase = CapturePhase::Verifying;
                self.reset_exchange();
                let lines = self.send_command("logger status", PendingCommand::Status, now);
                Ok(self.step(lines))
            }
            SdCaptureCommand::Start {
                test_id,
                sdlog_mode,
                sdlog_backend,
            } => {
                if matches!(
                    self.phase,
                    CapturePhase::Recording | CapturePhase::ExistingRecording
                ) {
                    return Err(SdCaptureError::CaptureAlreadyActive {
                        path: self.remote_path.clone(),
                    });
                }
                if test_id.trim().is_empty() {
                    return Err(self.reject(SdCaptureError::InvalidTestId));
                }
                self.begin_start(sdlog_mode, sdlog_backend, test_id, now)
            }
            SdCaptureCommand::Stop => {
                // Capture is independent of manual motor control. Coordinated
                // callers separately order this after release and post-roll.
                // Never rely on our cached phase: a timeout or reconnect may
                // have left a real file recording while the local state failed.
                self.phase = CapturePhase::Stopping;
                self.reset_exchange();
                self.deadline = Some(now + self.config.finalize_timeout);
                let lines = self.send_command("logger status", PendingCommand::Status, now);
                Ok(self.step(lines))
            }
        }
    }

    /// Feed one raw shell reply chunk.
    pub fn ingest_shell(&mut self, chunk: &str, now: Instant) -> SdCaptureStep {
        self.line_buffer.push_str(chunk);
        if self.line_buffer.len() > MAX_OUTPUT {
            let keep = self
                .line_buffer
                .char_indices()
                .rev()
                .nth(MAX_OUTPUT)
                .map(|(idx, _)| idx)
                .unwrap_or(0);
            self.line_buffer.drain(..keep);
        }
        let mut step = SdCaptureStep::default();
        while let Some(pos) = self.line_buffer.find('\n') {
            let line: String = self.line_buffer.drain(..=pos).collect();
            self.handle_line(&line, now, &mut step);
        }
        step
    }

    /// Advance timers and schedule the next status poll.
    pub fn tick(&mut self, now: Instant) -> SdCaptureStep {
        if matches!(
            self.phase,
            CapturePhase::Stopping | CapturePhase::Finalizing
        ) && self.deadline.is_some_and(|deadline| now >= deadline)
        {
            self.fail(SdCaptureError::StopNotConfirmed);
            return self.step(Vec::new());
        }
        if let Some(pending) = &self.pending {
            if now >= pending.deadline {
                let command = pending.command;
                let error = SdCaptureError::ShellTimeout {
                    command: command_label(command).into(),
                };
                self.fail(error);
                return self.step(Vec::new());
            }
            return SdCaptureStep::default();
        }
        if matches!(self.phase, CapturePhase::Starting)
            && self.deadline.is_some_and(|deadline| now >= deadline)
        {
            let error = self.classify_start_failure();
            self.fail(error);
            return self.step(Vec::new());
        }
        if self.next_poll.is_some_and(|at| now >= at) {
            self.next_poll = None;
            let lines = self.send_command("logger status", PendingCommand::Status, now);
            return self.step(lines);
        }
        SdCaptureStep::default()
    }

    /// Invalidate trusted recording state after a link/transport failure.
    /// Keep the test/path evidence so a later explicit Check or Stop can reconcile it.
    pub fn invalidate(&mut self, error: SdCaptureError) -> SdCaptureStep {
        self.fail(error);
        self.step(Vec::new())
    }

    fn reset_exchange(&mut self) {
        self.last_error = None;
        self.output.clear();
        self.line_buffer.clear();
        self.start_diagnostics.clear();
        self.start_after_verify = false;
        self.deadline = None;
        self.next_poll = None;
        self.not_logging_since = None;
    }

    fn begin_start(
        &mut self,
        sdlog_mode: Option<i32>,
        sdlog_backend: Option<i32>,
        test_id: String,
        now: Instant,
    ) -> Result<SdCaptureStep, SdCaptureError> {
        self.phase = CapturePhase::Verifying;
        self.test_id = Some(test_id);
        self.previous_path = self.remote_path.take().or(self.previous_path.take());
        self.remote_path = None;
        self.logger = None;
        self.sdlog_mode = sdlog_mode;
        self.sdlog_backend = sdlog_backend;
        self.reset_exchange();
        self.start_after_verify = true;
        let lines = self.send_command("logger status", PendingCommand::Status, now);
        Ok(self.step(lines))
    }

    fn send_command(&mut self, line: &str, command: PendingCommand, now: Instant) -> Vec<String> {
        self.sequence = self.sequence.wrapping_add(1);
        let marker = format!("__MAGCAP_{:08x}_{}__", self.nonce, self.sequence);
        self.pending = Some(Pending {
            marker: marker.clone(),
            command,
            deadline: now + self.config.command_timeout,
        });
        self.output.clear();
        vec![line.to_string(), format!("echo {marker}")]
    }

    fn handle_line(&mut self, raw: &str, now: Instant, step: &mut SdCaptureStep) {
        let line = raw.trim();
        if line.is_empty() {
            return;
        }
        let Some(pending) = &self.pending else {
            return;
        };
        // NuttShell echoes `nsh> echo <marker>` before executing echo. Only
        // the marker's output line proves completion, not that command echo.
        if line == pending.marker {
            let command = pending.command;
            self.pending = None;
            let output = std::mem::take(&mut self.output);
            self.on_reply(command, &output, now, step);
        } else {
            self.output.push_str(line);
            self.output.push('\n');
            if self.output.len() > MAX_OUTPUT {
                let keep = self.output.len() - MAX_OUTPUT;
                let keep = self
                    .output
                    .char_indices()
                    .find(|(idx, _)| *idx >= keep)
                    .map(|(idx, _)| idx)
                    .unwrap_or(0);
                self.output.drain(..keep);
            }
        }
    }

    fn on_reply(
        &mut self,
        command: PendingCommand,
        output: &str,
        now: Instant,
        step: &mut SdCaptureStep,
    ) {
        match command {
            PendingCommand::Status => {
                let logger = match parse_logger_status(output) {
                    Ok(logger) => logger,
                    Err(error) => {
                        self.fail(error);
                        step.events.push(self.status_event());
                        return;
                    }
                };
                self.logger = Some(logger.clone());
                match self.phase {
                    CapturePhase::Verifying => {
                        if !self.start_after_verify {
                            self.observe_logger(&logger);
                        } else if let Err(error) = self.validate_logger(&logger) {
                            if logger.file_logging {
                                self.observe_logger(&logger);
                                self.last_error = Some(error);
                            } else {
                                self.fail(error);
                            }
                        } else {
                            self.phase = CapturePhase::Starting;
                            self.deadline = Some(now + self.config.start_timeout);
                            self.start_diagnostics.clear();
                            step.outbound = self.send_command("logger on", PendingCommand::On, now);
                        }
                        step.events.push(self.status_event());
                    }
                    CapturePhase::Starting => {
                        self.start_diagnostics.push_str(output);
                        if logger.file_logging {
                            let Some(path) = logger.remote_path.clone() else {
                                self.fail(SdCaptureError::StartNotConfirmed);
                                step.events.push(self.status_event());
                                return;
                            };
                            if self.previous_path.as_deref() == Some(path.as_str()) {
                                self.fail(SdCaptureError::PathNotUnique { path });
                            } else {
                                self.remote_path = Some(path);
                                self.phase = CapturePhase::Recording;
                                self.deadline = None;
                                self.next_poll = None;
                            }
                            step.events.push(self.status_event());
                        } else if self.deadline.is_some_and(|deadline| now >= deadline) {
                            let error = self.classify_start_failure();
                            self.fail(error);
                            step.events.push(self.status_event());
                        } else {
                            self.next_poll = Some(now + self.config.poll_interval);
                        }
                    }
                    CapturePhase::Stopping => {
                        // Explicit Stop can take control of an observed
                        // existing capture, but must not relabel it as our test.
                        if logger.file_logging {
                            self.observe_logger(&logger);
                            self.phase = CapturePhase::Stopping;
                            step.outbound =
                                self.send_command("logger off", PendingCommand::Off, now);
                        } else {
                            // Idempotent stop: already inactive still needs
                            // stable observations, and never sends logger on/off.
                            self.phase = CapturePhase::Finalizing;
                            self.not_logging_since = Some(now);
                            self.next_poll = Some(now + self.config.finalize_settle);
                        }
                        step.events.push(self.status_event());
                    }
                    CapturePhase::Finalizing => {
                        if self.deadline.is_some_and(|deadline| now >= deadline) {
                            self.fail(SdCaptureError::StopNotConfirmed);
                            step.events.push(self.status_event());
                            return;
                        }
                        if logger.file_logging {
                            self.not_logging_since = None;
                            if self.deadline.is_some_and(|deadline| now >= deadline) {
                                self.fail(SdCaptureError::StopNotConfirmed);
                                step.events.push(self.status_event());
                            } else {
                                self.next_poll = Some(now + self.config.poll_interval);
                            }
                        } else if let Some(since) = self.not_logging_since {
                            if now.duration_since(since) >= self.config.finalize_settle {
                                self.phase = CapturePhase::Complete;
                                self.deadline = None;
                                self.next_poll = None;
                                step.events.push(self.status_event());
                            } else {
                                self.next_poll = Some(since + self.config.finalize_settle);
                            }
                        } else {
                            self.not_logging_since = Some(now);
                            self.next_poll = Some(now + self.config.finalize_settle);
                        }
                    }
                    _ => step.events.push(self.status_event()),
                }
            }
            PendingCommand::On => {
                self.start_diagnostics.push_str(output);
                if command_failed(output) || self.deadline.is_some_and(|deadline| now >= deadline) {
                    let error = self.classify_start_failure();
                    self.fail(error);
                } else {
                    self.next_poll = Some(now + self.config.poll_interval);
                }
                step.events.push(self.status_event());
            }
            PendingCommand::Off => {
                if command_failed(output) {
                    self.fail(SdCaptureError::StopNotConfirmed);
                } else {
                    self.phase = CapturePhase::Finalizing;
                    self.not_logging_since = None;
                    self.next_poll = Some(now + self.config.poll_interval);
                }
                step.events.push(self.status_event());
            }
        }
    }

    fn observe_logger(&mut self, logger: &LoggerStatus) {
        self.last_error = None;
        if logger.file_logging {
            let owned = self.test_id.is_some()
                && self.remote_path.is_some()
                && self.remote_path == logger.remote_path;
            if !owned {
                self.test_id = None;
            }
            self.remote_path = logger.remote_path.clone();
            self.phase = if owned {
                CapturePhase::Recording
            } else {
                CapturePhase::ExistingRecording
            };
        } else if let Err(error) = self.validate_logger(logger) {
            self.fail(error);
        } else {
            self.phase = CapturePhase::Ready;
        }
    }

    fn validate_logger(&self, logger: &LoggerStatus) -> Result<(), SdCaptureError> {
        if !logger.running {
            return Err(SdCaptureError::LoggerNotRunning);
        }
        if !matches!(
            logger.backend,
            LoggerBackend::File | LoggerBackend::FileAndMavlink
        ) {
            return Err(SdCaptureError::LoggerFileBackendUnavailable);
        }
        if logger.file_logging {
            return Err(SdCaptureError::CaptureAlreadyActive {
                path: logger.remote_path.clone(),
            });
        }
        Ok(())
    }

    fn classify_start_failure(&self) -> SdCaptureError {
        let lower = self.start_diagnostics.to_ascii_lowercase();
        for marker in [
            "no space",
            "enospc",
            "no such file",
            "failed to create",
            "failed creating log root",
            "mount",
        ] {
            if lower.contains(marker) {
                let detail = self
                    .start_diagnostics
                    .lines()
                    .find(|line| line.to_ascii_lowercase().contains(marker))
                    .unwrap_or(marker)
                    .trim()
                    .to_string();
                return SdCaptureError::StorageUnavailable { detail };
            }
        }
        SdCaptureError::StartNotConfirmed
    }

    fn reject(&mut self, error: SdCaptureError) -> SdCaptureError {
        self.fail(error.clone());
        error
    }

    fn fail(&mut self, error: SdCaptureError) {
        self.phase = CapturePhase::Failed;
        self.last_error = Some(error);
        self.pending = None;
        self.deadline = None;
        self.next_poll = None;
        self.not_logging_since = None;
    }

    fn step(&mut self, outbound: Vec<String>) -> SdCaptureStep {
        SdCaptureStep {
            events: vec![self.status_event()],
            outbound,
        }
    }

    fn status_event(&self) -> SdCaptureEvent {
        SdCaptureEvent::Status {
            status: self.status(),
        }
    }
}

fn command_label(command: PendingCommand) -> &'static str {
    match command {
        PendingCommand::Status => "logger status",
        PendingCommand::On => "logger on",
        PendingCommand::Off => "logger off",
    }
}

fn command_failed(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("logger not running")
        || lower.contains("unknown command")
        || lower.contains("error")
        || lower.contains("failed")
}

pub fn parse_logger_status(output: &str) -> Result<LoggerStatus, SdCaptureError> {
    let mut running = false;
    let mut backend = LoggerBackend::Unknown;
    let mut file_logging = false;
    let mut remote_path = None;
    let mut saw_status = false;
    let mut saw_logging_state = false;
    let mut full_file_section = false;
    let mut full_path = None;
    for line in output.lines() {
        let line = line.trim();
        if line.contains("logger not running") {
            saw_status = true;
            saw_logging_state = true;
            running = false;
            file_logging = false;
            remote_path = None;
            full_path = None;
        }
        if let Some((_, value)) = line.split_once("Running in mode:") {
            saw_status = true;
            running = true;
            backend = parse_backend(value.trim());
            file_logging = false;
            remote_path = None;
            full_path = None;
            full_file_section = false;
            saw_logging_state = false;
        }
        if line.contains("Full File Logging Running:") {
            file_logging = true;
            full_file_section = true;
            saw_logging_state = true;
        } else if line.contains("Mission File Logging Running:") {
            file_logging = true;
            full_file_section = false;
            saw_logging_state = true;
        } else if line.contains("Not logging") || line.contains("Mavlink Logging Running") {
            saw_logging_state = true;
        }
        if let Some((_, value)) = line.split_once("Log file:") {
            remote_path = value
                .split_whitespace()
                .next()
                .filter(|path| !path.is_empty())
                .map(str::to_string);
            if full_file_section {
                full_path = remote_path.clone();
            }
        }
    }
    if !saw_status || !saw_logging_state {
        return Err(SdCaptureError::UnexpectedResponse {
            output: output.trim().to_string(),
        });
    }
    Ok(LoggerStatus {
        running,
        backend,
        file_logging,
        remote_path: full_path.or(remote_path),
    })
}

fn parse_backend(value: &str) -> LoggerBackend {
    match value.trim().to_ascii_lowercase().as_str() {
        "file" => LoggerBackend::File,
        "mavlink" => LoggerBackend::Mavlink,
        "file,mavlink" | "mavlink,file" | "all" => LoggerBackend::FileAndMavlink,
        "" => LoggerBackend::Unknown,
        _ => LoggerBackend::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CaptureConfig {
        CaptureConfig {
            command_timeout: Duration::from_millis(100),
            start_timeout: Duration::from_millis(500),
            finalize_timeout: Duration::from_millis(800),
            poll_interval: Duration::from_millis(10),
            finalize_settle: Duration::from_millis(50),
        }
    }

    fn finish(capture: &mut SdCapture, output: &str, now: Instant) -> SdCaptureStep {
        let marker = capture.pending.as_ref().expect("pending").marker.clone();
        capture.ingest_shell(&format!("{output}\n{marker}\n"), now)
    }

    fn not_logging() -> &'static str {
        "INFO  [logger] Running in mode: file\nINFO  [logger] Number of subscriptions: 10 (100 bytes)\nINFO  [logger] Not logging"
    }

    fn recording(path: &str) -> String {
        format!(
            "INFO  [logger] Running in mode: file\nINFO  [logger] Full File Logging Running:\nINFO  [logger] Log file: {path}\nINFO  [logger] Wrote 1.00 KiB"
        )
    }

    fn ready_capture(now: Instant) -> SdCapture {
        let mut capture = SdCapture::new(config());
        capture
            .command(
                SdCaptureCommand::Check {
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(3),
                },
                now,
            )
            .expect("verify");
        let _ = finish(&mut capture, not_logging(), now);
        assert_eq!(capture.status().phase, CapturePhase::Ready);
        capture
    }

    #[test]
    fn parses_not_logging_and_recording_status() {
        let status = parse_logger_status(not_logging()).expect("status");
        assert!(status.running);
        assert_eq!(status.backend, LoggerBackend::File);
        assert!(!status.file_logging);
        assert_eq!(status.remote_path, None);

        let status =
            parse_logger_status(&recording("/fs/microsd/log/2026-10-22/1.ulg")).expect("status");
        assert!(status.file_logging);
        assert_eq!(
            status.remote_path.as_deref(),
            Some("/fs/microsd/log/2026-10-22/1.ulg")
        );
    }

    #[test]
    fn status_marker_can_arrive_split_across_chunks() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture
            .command(
                SdCaptureCommand::Check {
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("verify");
        let marker = capture.pending.as_ref().expect("pending").marker.clone();
        let _ = capture.ingest_shell("INFO [logger] Running in mode: file\n", now);
        let (head, tail) = marker.split_at(9);
        let _ = capture.ingest_shell("INFO [logger] Not logging\n", now);
        let _ = capture.ingest_shell(head, now);
        let _ = capture.ingest_shell(&format!("{tail}\n"), now);
        assert_eq!(capture.status().phase, CapturePhase::Ready);
    }

    #[test]
    fn start_accepts_any_reported_mode_because_parameters_are_read_only() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        let step = capture
            .command(
                SdCaptureCommand::Start {
                    test_id: "test-mode".into(),
                    sdlog_mode: Some(1),
                    sdlog_backend: Some(3),
                },
                now,
            )
            .expect("start does not write SDLOG_MODE");
        assert_eq!(step.outbound[0], "logger status");
        assert_eq!(capture.status().sdlog_mode, Some(1));
        assert_eq!(capture.status().sdlog_backend, Some(3));
    }

    #[test]
    fn check_reports_an_existing_capture_without_failing() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture
            .command(
                SdCaptureCommand::Check {
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("verify");
        let _ = finish(
            &mut capture,
            "WARN [logger] logger not running\nUsage: logger <command> [arguments...]",
            now,
        );
        assert_eq!(
            capture.status().last_error,
            Some(SdCaptureError::LoggerNotRunning)
        );

        let mut capture = SdCapture::new(config());
        capture
            .command(
                SdCaptureCommand::Check {
                    sdlog_mode: Some(2),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("verify");
        let _ = finish(
            &mut capture,
            &recording("/fs/microsd/log/2026-10-22/1.ulg"),
            now,
        );
        assert_eq!(capture.status().phase, CapturePhase::ExistingRecording);
        assert_eq!(capture.status().last_error, None);
        assert_eq!(capture.status().test_id, None);
        assert_eq!(capture.status().sdlog_mode, Some(2));
        assert_eq!(
            capture.status().remote_path.as_deref(),
            Some("/fs/microsd/log/2026-10-22/1.ulg")
        );
    }

    #[test]
    fn start_confirms_a_new_recording_path() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        let step = capture
            .command(
                SdCaptureCommand::Start {
                    test_id: "test-1".into(),
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("start");
        assert_eq!(capture.status().phase, CapturePhase::Verifying);
        assert_eq!(step.outbound.len(), 2);

        let step = finish(&mut capture, not_logging(), now);
        assert_eq!(capture.status().phase, CapturePhase::Starting);
        assert_eq!(step.outbound[0], "logger on");

        let step = finish(&mut capture, "INFO [logger] started", now);
        assert!(step.outbound.is_empty());
        let step = capture.tick(now + Duration::from_millis(10));
        assert_eq!(step.outbound[0], "logger status");

        let _ = finish(
            &mut capture,
            &recording("/fs/microsd/log/2026-10-22/2.ulg"),
            now + Duration::from_millis(20),
        );
        let status = capture.status();
        assert_eq!(status.phase, CapturePhase::Recording);
        assert_eq!(
            status.remote_path.as_deref(),
            Some("/fs/microsd/log/2026-10-22/2.ulg")
        );
        assert_eq!(status.test_id.as_deref(), Some("test-1"));
    }

    #[test]
    fn start_rejects_an_unchanged_path() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture
            .command(
                SdCaptureCommand::Start {
                    test_id: "test-1".into(),
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("start");
        let _ = finish(&mut capture, not_logging(), now);
        let _ = finish(&mut capture, "INFO [logger] started", now);
        capture.previous_path = Some("/fs/microsd/log/2026-10-22/same.ulg".into());
        let _ = capture.tick(now + Duration::from_millis(10));
        let _ = finish(
            &mut capture,
            &recording("/fs/microsd/log/2026-10-22/same.ulg"),
            now + Duration::from_millis(20),
        );
        assert_eq!(capture.status().phase, CapturePhase::Failed);
        assert_eq!(
            capture.status().last_error,
            Some(SdCaptureError::PathNotUnique {
                path: "/fs/microsd/log/2026-10-22/same.ulg".into()
            })
        );
    }

    #[test]
    fn start_reports_absent_storage() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture
            .command(
                SdCaptureCommand::Start {
                    test_id: "test-1".into(),
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("start");
        let _ = finish(&mut capture, not_logging(), now);
        let _ = finish(
            &mut capture,
            "ERROR [logger] failed creating log root dir: /fs/microsd/log (ENOSPC)",
            now,
        );
        for delay in [10u64, 20, 30, 40, 50, 60] {
            let _ = capture.tick(now + Duration::from_millis(delay));
        }
        let _ = capture.tick(now + Duration::from_millis(600));
        assert_eq!(capture.status().phase, CapturePhase::Failed);
        assert!(matches!(
            capture.status().last_error,
            Some(SdCaptureError::StorageUnavailable { .. })
        ));
    }

    #[test]
    fn stop_waits_for_stable_not_logging() {
        let recording_now = Instant::now();
        let mut capture = ready_capture(recording_now);
        capture
            .command(
                SdCaptureCommand::Start {
                    test_id: "test-1".into(),
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                recording_now,
            )
            .expect("start");
        let _ = finish(&mut capture, not_logging(), recording_now);
        let _ = finish(&mut capture, "INFO [logger] started", recording_now);
        let _ = capture.tick(recording_now + Duration::from_millis(10));
        let _ = finish(
            &mut capture,
            &recording("/fs/microsd/log/2026-10-22/3.ulg"),
            recording_now + Duration::from_millis(20),
        );
        assert_eq!(capture.status().phase, CapturePhase::Recording);

        let stop_at = recording_now + Duration::from_millis(30);
        capture
            .command(SdCaptureCommand::Stop, stop_at)
            .expect("stop");
        let step = finish(
            &mut capture,
            &recording("/fs/microsd/log/2026-10-22/3.ulg"),
            stop_at,
        );
        assert_eq!(step.outbound[0], "logger off");
        let _ = finish(&mut capture, "INFO [logger] stopped", stop_at);
        assert_eq!(capture.status().phase, CapturePhase::Finalizing);

        let poll_at = stop_at + Duration::from_millis(10);
        assert_eq!(capture.tick(poll_at).outbound[0], "logger status");
        let _ = finish(&mut capture, not_logging(), poll_at);
        assert_eq!(capture.status().phase, CapturePhase::Finalizing);

        let final_at = poll_at + Duration::from_millis(50);
        assert_eq!(capture.tick(final_at).outbound[0], "logger status");
        let _ = finish(&mut capture, not_logging(), final_at);
        assert_eq!(capture.status().phase, CapturePhase::Complete);
        assert_eq!(
            capture.status().remote_path.as_deref(),
            Some("/fs/microsd/log/2026-10-22/3.ulg")
        );
        assert!(!capture.status().logger.expect("logger").file_logging);
    }

    #[test]
    fn shell_timeout_fails_the_capture() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture
            .command(
                SdCaptureCommand::Check {
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("verify");
        assert_eq!(
            capture.tick(now + Duration::from_millis(101)).events.len(),
            1
        );
        assert_eq!(capture.status().phase, CapturePhase::Failed);
        assert!(matches!(
            capture.status().last_error,
            Some(SdCaptureError::ShellTimeout { .. })
        ));
    }

    #[test]
    fn checking_an_owned_recording_keeps_its_test_and_clears_a_previous_error() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture.phase = CapturePhase::Recording;
        capture.test_id = Some("test-owned".into());
        capture.remote_path = Some("/fs/microsd/log/sess001/log100.ulg".into());
        capture.last_error = Some(SdCaptureError::StartNotConfirmed);

        let step = capture
            .command(
                SdCaptureCommand::Check {
                    sdlog_mode: None,
                    sdlog_backend: None,
                },
                now,
            )
            .expect("read-only check with unknown params");
        assert_eq!(step.outbound[0], "logger status");
        let _ = finish(
            &mut capture,
            &recording("/fs/microsd/log/sess001/log100.ulg"),
            now,
        );
        assert_eq!(capture.status().phase, CapturePhase::Recording);
        assert_eq!(capture.status().test_id.as_deref(), Some("test-owned"));
        assert_eq!(capture.status().last_error, None);
        assert_eq!(capture.status().sdlog_mode, None);
    }

    #[test]
    fn start_does_not_claim_or_fail_an_existing_recording() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture
            .command(
                SdCaptureCommand::Start {
                    test_id: "new-test".into(),
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("start check");
        let step = finish(
            &mut capture,
            &recording("/fs/microsd/log/sess001/log100.ulg"),
            now,
        );
        assert!(
            step.outbound.is_empty(),
            "no logger on/off for an existing capture"
        );
        assert_eq!(capture.status().phase, CapturePhase::ExistingRecording);
        assert_eq!(capture.status().test_id, None);
        assert!(matches!(
            capture.status().last_error,
            Some(SdCaptureError::CaptureAlreadyActive { .. })
        ));
    }

    #[test]
    fn shell_command_echo_never_completes_an_exchange() {
        let now = Instant::now();
        let mut capture = ready_capture(now);
        capture
            .command(
                SdCaptureCommand::Check {
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("check");
        let marker = capture.pending.as_ref().expect("pending").marker.clone();
        let _ = capture.ingest_shell(&format!("{}\nnsh> echo {marker}\n", not_logging()), now);
        assert_eq!(capture.status().phase, CapturePhase::Verifying);
        assert!(capture.pending.is_some());
        let _ = capture.ingest_shell(&format!("{marker}\r\n"), now);
        assert_eq!(capture.status().phase, CapturePhase::Ready);
    }

    #[test]
    fn parser_requires_recording_evidence_and_prefers_the_full_log_path() {
        assert!(matches!(
            parse_logger_status("INFO [logger] Running in mode: file\n"),
            Err(SdCaptureError::UnexpectedResponse { .. })
        ));
        let output = format!("{}\nINFO [logger] Mission File Logging Running:\nINFO [logger] Log file: /fs/microsd/mission_log/sess001/log100.ulg", recording("/fs/microsd/log/sess001/log100.ulg"));
        assert_eq!(
            parse_logger_status(&output)
                .expect("both files")
                .remote_path
                .as_deref(),
            Some("/fs/microsd/log/sess001/log100.ulg")
        );
        let latest = format!("{output}\n{}", not_logging());
        assert!(
            !parse_logger_status(&latest)
                .expect("latest status")
                .file_logging
        );
    }

    #[test]
    fn stop_recovers_from_failure_and_from_an_existing_recording() {
        for failed in [false, true] {
            let now = Instant::now();
            let mut capture = SdCapture::new(config());
            if failed {
                let _ = capture.invalidate(SdCaptureError::TargetUnavailable);
            } else {
                capture
                    .command(
                        SdCaptureCommand::Check {
                            sdlog_mode: Some(2),
                            sdlog_backend: Some(1),
                        },
                        now,
                    )
                    .expect("check");
                let _ = finish(
                    &mut capture,
                    &recording("/fs/microsd/log/sess001/log100.ulg"),
                    now,
                );
            }
            let step = capture
                .command(SdCaptureCommand::Stop, now)
                .expect("recovery stop");
            assert_eq!(
                step.outbound[0], "logger status",
                "refresh state before logger off"
            );
            let step = finish(
                &mut capture,
                &recording("/fs/microsd/log/sess001/log100.ulg"),
                now,
            );
            assert_eq!(step.outbound[0], "logger off");
            let _ = finish(&mut capture, "", now);
            let poll = now + config().poll_interval;
            let _ = capture.tick(poll);
            let _ = finish(&mut capture, not_logging(), poll);
            assert_eq!(capture.status().phase, CapturePhase::Finalizing);
            let settled = poll + config().finalize_settle;
            let _ = capture.tick(settled);
            let _ = finish(&mut capture, not_logging(), settled);
            assert_eq!(capture.status().phase, CapturePhase::Complete);
            assert_eq!(
                capture.status().test_id,
                None,
                "existing log is not an owned test"
            );
            assert_eq!(capture.status().last_error, None);
        }
    }

    #[test]
    fn repeated_stops_recheck_inactive_state_without_writing_logger_off() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        for index in 0..2 {
            let at = now + Duration::from_secs(index);
            let step = capture
                .command(SdCaptureCommand::Stop, at)
                .expect("idempotent stop");
            assert_eq!(step.outbound[0], "logger status");
            let step = finish(&mut capture, not_logging(), at);
            assert!(step.outbound.is_empty());
            assert_eq!(capture.status().phase, CapturePhase::Finalizing);
            let settled = at + config().finalize_settle;
            let _ = capture.tick(settled);
            let _ = finish(&mut capture, not_logging(), settled);
            assert_eq!(capture.status().phase, CapturePhase::Complete);
        }
    }

    #[test]
    fn logger_off_that_does_not_stop_recording_has_a_bounded_failure() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture.command(SdCaptureCommand::Stop, now).expect("stop");
        let _ = finish(
            &mut capture,
            &recording("/fs/microsd/log/sess001/log100.ulg"),
            now,
        );
        let _ = finish(&mut capture, "", now);
        let mut at = now + config().poll_interval;
        while at < now + config().finalize_timeout {
            let step = capture.tick(at);
            if !step.outbound.is_empty() {
                let _ = finish(
                    &mut capture,
                    &recording("/fs/microsd/log/sess001/log100.ulg"),
                    at,
                );
            }
            at += config().poll_interval;
        }
        let _ = capture.tick(at);
        assert_eq!(
            capture.status().last_error,
            Some(SdCaptureError::StopNotConfirmed)
        );
        assert!(
            capture.command(SdCaptureCommand::Stop, at).is_ok(),
            "failure must allow another recovery stop"
        );
    }

    #[test]
    fn a_checked_replacement_file_is_never_owned_by_the_previous_test() {
        let now = Instant::now();
        let mut capture = SdCapture::new(config());
        capture.phase = CapturePhase::Recording;
        capture.test_id = Some("old-test".into());
        capture.remote_path = Some("/fs/microsd/log/sess001/log100.ulg".into());
        capture
            .command(
                SdCaptureCommand::Check {
                    sdlog_mode: Some(0),
                    sdlog_backend: Some(1),
                },
                now,
            )
            .expect("check");
        let _ = finish(
            &mut capture,
            &recording("/fs/microsd/log/sess001/log101.ulg"),
            now,
        );
        assert_eq!(capture.status().phase, CapturePhase::ExistingRecording);
        assert_eq!(capture.status().test_id, None);
    }
}
