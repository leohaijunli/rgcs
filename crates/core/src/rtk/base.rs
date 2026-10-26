//! Application-owned RTK receiver sessions. A survey does not need a vehicle link.
//!
//! Configuration writes are opt-in (`BaseMode::SurveyIn`), F9P-identified and
//! RAM-only. See the module README for the device-write and cancellation contract.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::devices::{BaudSelection, SerialCancellation, SerialLease, SerialOwner};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch, Mutex};

use super::rtcm::RtcmFrame;
use super::source::{RtcmSource, SerialSource, SerialSourceConfig, SourceError};
use super::ubx::{self, Channel, ReceiverIo, WireFrame};

pub const DEFAULT_ACCURACY_M: f64 = 2.0;
pub const DEFAULT_MIN_DURATION_S: u32 = 180;
pub const MIN_ACCURACY_M: f64 = 0.01;
pub const MAX_ACCURACY_M: f64 = 5.0;
pub const MAX_MIN_DURATION_S: u32 = 360;
pub(crate) const ACCURACY_UNITS_PER_M: f64 = 10_000.0;
pub const BASE_STOP_TIMEOUT: Duration = Duration::from_millis(800);
pub const BASE_IDENTIFICATION_TIMEOUT: Duration = Duration::from_secs(16);
pub const CORRECTION_CAPACITY: usize = 64;
pub const MAX_CORRECTION_AGE: Duration = Duration::from_secs(2);
const STATUS_PERIOD: Duration = Duration::from_millis(250);
const READER_IDLE: Duration = Duration::from_millis(5);
const JOIN_POLL: Duration = Duration::from_millis(5);
const SURVEY_REPORT_TIMEOUT: Duration = Duration::from_secs(5);
const SURVEY_STATUS_STAGE: &str = "waiting for Survey-In status";
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SurveyInConfig {
    pub accuracy_limit_m: f64,
    pub min_duration_s: u32,
}

impl Default for SurveyInConfig {
    fn default() -> Self {
        Self {
            accuracy_limit_m: DEFAULT_ACCURACY_M,
            min_duration_s: DEFAULT_MIN_DURATION_S,
        }
    }
}

impl SurveyInConfig {
    pub(crate) fn wire_accuracy_limit(&self) -> Result<u32, BaseError> {
        self.validate()?;
        Ok((self.accuracy_limit_m * ACCURACY_UNITS_PER_M).round() as u32)
    }

    /// The same operator bounds as QGC. Validate before opening the port.
    pub fn validate(&self) -> Result<(), BaseError> {
        if !self.accuracy_limit_m.is_finite()
            || !(MIN_ACCURACY_M..=MAX_ACCURACY_M).contains(&self.accuracy_limit_m)
        {
            return Err(BaseError::InvalidConfig {
                field: "target accuracy must be between 0.01 and 5.00 m",
            });
        }
        if !(1..=MAX_MIN_DURATION_S).contains(&self.min_duration_s) {
            return Err(BaseError::InvalidConfig {
                field: "minimum observation time must be an integer between 1 and 360 s",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, rename = "RtkBaseMode")]
#[serde(tag = "kind", content = "config", rename_all = "snake_case")]
pub enum BaseMode {
    Preconfigured,
    SurveyIn(SurveyInConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BaseSerialRequest {
    pub baud: BaudSelection,
    pub mode: BaseMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SurveyInStatus {
    pub duration_s: u32,
    pub mean_accuracy_m: f64,
    pub observations: u32,
    pub active: bool,
    pub valid: bool,
}

impl SurveyInStatus {
    pub(crate) fn complete(&self, config: SurveyInConfig) -> bool {
        self.valid
            && !self.active
            && self.duration_s >= config.min_duration_s
            && config
                .wire_accuracy_limit()
                .is_ok_and(|limit| self.mean_accuracy_m <= f64::from(limit) / ACCURACY_UNITS_PER_M)
    }
}

/// An opaque, process-monotonic identity. Strings preserve all 64 bits over IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ts_rs::TS)]
#[ts(export, type = "string")]
pub struct BaseSessionId(u64);

impl Serialize for BaseSessionId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for BaseSessionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let id = value.parse::<u64>().map_err(serde::de::Error::custom)?;
        if id == 0 {
            return Err(serde::de::Error::custom("base session ID must be nonzero"));
        }
        Ok(Self(id))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum BasePhase {
    #[default]
    Disconnected,
    Identifying,
    Configuring,
    Surveying,
    Ready,
    Stopping,
    Stopped,
    Failed,
}

impl BasePhase {
    pub fn running(self) -> bool {
        matches!(
            self,
            Self::Identifying | Self::Configuring | Self::Surveying | Self::Ready | Self::Stopping
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BaseStatus {
    pub session_id: Option<BaseSessionId>,
    /// Increases within one session, including its final stop/failure report.
    pub revision: u32,
    pub phase: BasePhase,
    pub source: Option<String>,
    pub serial: Option<SerialSourceConfig>,
    pub mode: Option<BaseMode>,
    /// MON-VER identity, only present after a successful F9P probe.
    pub receiver: Option<String>,
    pub survey_in: Option<SurveyInStatus>,
    pub seconds_since_data: Option<f64>,
    pub seconds_since_rtcm: Option<f64>,
    pub error: Option<String>,
    #[serde(default)]
    pub failure_kind: Option<BaseFailureKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum BaseFailureKind {
    Transport,
    Permission,
    Identity,
    Configuration,
    Worker,
}

fn failure_kind(error: &BaseError, phase: BasePhase) -> BaseFailureKind {
    match error {
        BaseError::Source(SourceError::Serial { source, .. })
            if source.kind() == serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) =>
        {
            BaseFailureKind::Permission
        }
        BaseError::Source(SourceError::Io(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            BaseFailureKind::Permission
        }
        BaseError::Discovery(crate::devices::SerialDiscoveryError::PermissionDenied { .. }) => {
            BaseFailureKind::Permission
        }
        BaseError::Source(_) | BaseError::Discovery(_) => BaseFailureKind::Transport,
        BaseError::Timeout { .. } if phase == BasePhase::Identifying => BaseFailureKind::Transport,
        BaseError::Timeout { stage } if *stage == SURVEY_STATUS_STAGE => BaseFailureKind::Transport,
        BaseError::UnsupportedReceiver => BaseFailureKind::Identity,
        BaseError::Rejected { .. }
        | BaseError::ReadbackMismatch { .. }
        | BaseError::InvalidConfig { .. }
        | BaseError::Timeout { .. } => BaseFailureKind::Configuration,
        _ => BaseFailureKind::Worker,
    }
}

/// Device inventory for the base selector. A USB role is a hint, not proof that
/// the receiver supports survey-in or is already a functioning base.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BasePort {
    pub device: crate::devices::SerialDeviceInfo,
    pub port_ref: Option<crate::devices::SerialPortRef>,
    pub suggested_role: crate::devices::DeviceRole,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BaseError {
    #[error(transparent)]
    Discovery(#[from] crate::devices::SerialDiscoveryError),
    #[error("invalid base configuration: {field}")]
    InvalidConfig { field: &'static str },
    #[error("serial port is in use; disconnect its current session first")]
    PortInUse,
    #[error("receiver is not a supported F9P with UBX protocol 27 or later")]
    UnsupportedReceiver,
    #[error(
        "receiver rejected UBX {class:02X}/{id:02X}; earlier configuration may still be applied"
    )]
    Rejected { class: u8, id: u8 },
    #[error("base timeout during {stage}; earlier configuration may still be applied")]
    Timeout { stage: &'static str },
    #[error("receiver configuration readback differs for key {key:#010x}")]
    ReadbackMismatch { key: u32 },
    #[error("base operation cancelled")]
    Cancelled,
    #[error("base session has changed")]
    StaleSession,
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("base worker failed: {0}")]
    Worker(String),
}

/// One cancellation signal shared by the serial worker and in-flight MAVLink
/// sends. A receiver created after cancellation also observes it immediately.
#[derive(Debug, Clone)]
pub(crate) struct Cancellation(SerialCancellation);

impl Cancellation {
    pub(crate) fn new() -> Self {
        Self(SerialCancellation::new())
    }

    pub(crate) fn cancel(&self) {
        self.0.cancel();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    pub(crate) fn check(&self) -> Result<(), BaseError> {
        if self.is_cancelled() {
            Err(BaseError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(crate) async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Correction {
    pub session_id: BaseSessionId,
    pub frame: RtcmFrame,
    pub received_at: Instant,
    pub cancel: Cancellation,
}

struct Session {
    id: BaseSessionId,
    cancel: Cancellation,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct BaseInner {
    session: Mutex<Option<Session>>,
    status: watch::Sender<BaseStatus>,
    corrections: broadcast::Sender<Correction>,
    activity: watch::Sender<Option<(BaseSessionId, Instant)>>,
}

#[derive(Clone)]
pub struct RtkBaseServiceHandle {
    inner: Arc<BaseInner>,
}

impl Default for RtkBaseServiceHandle {
    fn default() -> Self {
        Self {
            inner: Arc::new(BaseInner {
                session: Mutex::new(None),
                status: watch::channel(BaseStatus::default()).0,
                corrections: broadcast::channel(CORRECTION_CAPACITY).0,
                activity: watch::channel(None).0,
            }),
        }
    }
}

impl RtkBaseServiceHandle {
    /// Launch a receiver whose actual serial source owns the shared lease.
    pub async fn start_leased_serial(
        &self,
        lease: SerialLease,
        request: BaseSerialRequest,
        cancel: SerialCancellation,
    ) -> Result<BaseSessionId, BaseError> {
        if lease.owner() != SerialOwner::RtkBase {
            return Err(BaseError::PortInUse);
        }
        request.baud.validate()?;
        let serial = SerialSourceConfig {
            port_name: lease.port().device.port_name.clone(),
            baud_rate: request.baud.rate_or_auto(),
        };
        validate_serial(&serial, request.mode)?;
        cancel.check()?;
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|e| BaseError::Worker(e.to_string()))?;
        let opening = serial.clone();
        self.start_worker_cancelled(
            Some(serial.clone()),
            serial.port_name,
            request.mode,
            Cancellation(cancel),
            move |ctx| {
                ctx.cancel.check()?;
                runtime.block_on(async {
                    tokio::select! {
                        biased;
                        _ = ctx.cancel.cancelled() => Err(BaseError::Cancelled),
                        result = lease.revalidate() => result.map_err(BaseError::from),
                    }
                })?;
                ctx.cancel.check()?;
                let mut config = opening;
                if config.baud_rate == 0 {
                    config.baud_rate = ubx::BAUD_RATES[0];
                }
                let port = SerialSource::open_leased(config, lease)?;
                runtime.block_on(async {
                    tokio::select! {
                        biased;
                        _ = ctx.cancel.cancelled() => Err(BaseError::Cancelled),
                        result = port.validate_attachment() => result.map_err(BaseError::from),
                    }
                })?;
                run_receiver(Box::new(port), request.mode, opening_baud(ctx), ctx)
            },
        )
        .await
    }

    pub fn snapshot(&self) -> BaseStatus {
        self.inner.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<BaseStatus> {
        self.inner.status.subscribe()
    }

    pub(crate) fn corrections(&self) -> broadcast::Receiver<Correction> {
        self.inner.corrections.subscribe()
    }

    pub(crate) fn last_data(&self, session: BaseSessionId) -> Option<Instant> {
        self.inner
            .activity
            .borrow()
            .filter(|(id, _)| *id == session)
            .map(|(_, at)| at)
    }

    /// Return after reserving the session and launching its worker, not after
    /// the survey. Configuration failures are reported in `BaseStatus`.
    pub async fn start_serial(
        &self,
        serial: SerialSourceConfig,
        mode: BaseMode,
    ) -> Result<BaseSessionId, BaseError> {
        validate_serial(&serial, mode)?;
        let opening = serial.clone();
        self.start_worker(Some(serial.clone()), serial.port_name, mode, move |ctx| {
            let mut config = opening;
            if config.baud_rate == 0 {
                config.baud_rate = ubx::BAUD_RATES[0];
            }
            ctx.cancel.check()?;
            let port = SerialSource::open(config)?;
            run_receiver(Box::new(port), mode, opening_baud(ctx), ctx)
        })
        .await
    }

    /// Adapter for the preconfigured byte-stream API and integration fixtures.
    pub(crate) async fn start_source(
        &self,
        mut source: Box<dyn RtcmSource>,
        label: String,
    ) -> Result<BaseSessionId, BaseError> {
        self.start_worker(None, label, BaseMode::Preconfigured, move |ctx| {
            ctx.update(|status| status.phase = BasePhase::Ready);
            let mut framer = ubx::ReceiverFramer::default();
            loop {
                ctx.cancel.check()?;
                if let Some(bytes) = source.read()? {
                    if !bytes.is_empty() {
                        ctx.activity.send_replace(Some((ctx.id, Instant::now())));
                    }
                    for frame in framer.push(&bytes) {
                        if let WireFrame::Rtcm(frame) = frame {
                            ctx.correction(frame);
                        }
                    }
                }
                ctx.refresh();
                std::thread::sleep(READER_IDLE);
            }
        })
        .await
    }

    async fn start_worker<F>(
        &self,
        serial: Option<SerialSourceConfig>,
        label: String,
        mode: BaseMode,
        worker: F,
    ) -> Result<BaseSessionId, BaseError>
    where
        F: FnOnce(&mut WorkerContext) -> Result<(), BaseError> + Send + 'static,
    {
        self.start_worker_cancelled(serial, label, mode, Cancellation::new(), worker)
            .await
    }

    async fn start_worker_cancelled<F>(
        &self,
        serial: Option<SerialSourceConfig>,
        label: String,
        mode: BaseMode,
        cancel: Cancellation,
        worker: F,
    ) -> Result<BaseSessionId, BaseError>
    where
        F: FnOnce(&mut WorkerContext) -> Result<(), BaseError> + Send + 'static,
    {
        let mut slot = self.inner.session.lock().await;
        if let Some(session) = slot.as_ref() {
            if session
                .reader
                .as_ref()
                .is_some_and(|reader| !reader.is_finished())
            {
                return Err(BaseError::PortInUse);
            }
        }
        // A finished worker has already closed its port. Keep a timed-out
        // worker reserved until it really exits; never detach and reuse it.
        if let Some(session) = slot.take() {
            join_session(session)?;
        }
        let id = BaseSessionId(
            NEXT_SESSION
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .map_err(|_| BaseError::Worker("session IDs exhausted".into()))?,
        );
        self.inner.activity.send_replace(None);
        self.inner.status.send_replace(BaseStatus {
            session_id: Some(id),
            revision: 1,
            phase: BasePhase::Identifying,
            source: Some(label),
            serial,
            mode: Some(mode),
            ..Default::default()
        });
        let mut context = WorkerContext {
            id,
            cancel: cancel.clone(),
            status: self.inner.status.clone(),
            corrections: self.inner.corrections.clone(),
            activity: self.inner.activity.clone(),
            last_rtcm: None,
            published_at: Instant::now(),
            identify_by: Instant::now() + BASE_IDENTIFICATION_TIMEOUT,
        };
        let reader = std::thread::Builder::new()
            .name(format!("rtk-base-{}", id.0))
            .spawn(move || {
                let result = worker(&mut context);
                // Cancels any correction already queued or currently sending.
                context.cancel.cancel();
                context.status.send_modify(|status| {
                    if status.session_id != Some(id) {
                        return;
                    }
                    status.revision = status.revision.saturating_add(1);
                    match result {
                        Ok(()) | Err(BaseError::Cancelled) => status.phase = BasePhase::Stopped,
                        Err(error) => {
                            status.failure_kind = Some(failure_kind(&error, status.phase));
                            status.phase = BasePhase::Failed;
                            status.error = Some(error.to_string());
                        }
                    }
                });
            })
            .map_err(|error| {
                let message = error.to_string();
                self.inner.status.send_modify(|status| {
                    status.phase = BasePhase::Failed;
                    status.failure_kind = Some(BaseFailureKind::Worker);
                    status.error = Some(message.clone());
                    status.revision = status.revision.saturating_add(1);
                });
                BaseError::Worker(message)
            })?;
        *slot = Some(Session {
            id,
            cancel,
            reader: Some(reader),
        });
        Ok(id)
    }

    pub async fn stop(&self, session_id: BaseSessionId) -> Result<(), BaseError> {
        let deadline = tokio::time::Instant::now() + BASE_STOP_TIMEOUT;
        let mut slot = tokio::time::timeout_at(deadline, self.inner.session.lock())
            .await
            .map_err(|_| BaseError::Timeout { stage: "base stop" })?;
        let Some(session) = slot.as_ref() else {
            return if self.snapshot().session_id == Some(session_id) {
                Ok(())
            } else {
                Err(BaseError::StaleSession)
            };
        };
        if session.id != session_id {
            return Err(BaseError::StaleSession);
        }
        session.cancel.cancel();
        self.inner.status.send_modify(|status| {
            status.phase = BasePhase::Stopping;
            status.revision = status.revision.saturating_add(1);
        });
        while session
            .reader
            .as_ref()
            .is_some_and(|reader| !reader.is_finished())
        {
            if tokio::time::Instant::now() >= deadline {
                return Err(BaseError::Timeout { stage: "base stop" });
            }
            tokio::time::sleep(JOIN_POLL).await;
        }
        if let Some(session) = slot.take() {
            join_session(session)?;
        }
        self.inner.status.send_modify(|status| {
            status.phase = BasePhase::Stopped;
            status.revision = status.revision.saturating_add(1);
        });
        Ok(())
    }
}

fn join_session(mut session: Session) -> Result<(), BaseError> {
    session.cancel.cancel();
    if let Some(reader) = session.reader.take() {
        reader
            .join()
            .map_err(|_| BaseError::Worker("reader thread panicked".into()))?;
    }
    Ok(())
}

fn validate_serial(serial: &SerialSourceConfig, mode: BaseMode) -> Result<(), BaseError> {
    if !crate::devices::manager::is_operator_port(&serial.port_name) {
        return Err(SourceError::NotAnOperatorPort {
            port: serial.port_name.clone(),
        }
        .into());
    }
    if let BaseMode::SurveyIn(config) = mode {
        config.validate()?;
    }
    if !(ubx::BAUD_RATES.contains(&serial.baud_rate)
        || serial.baud_rate == 0 && matches!(mode, BaseMode::SurveyIn(_)))
    {
        return Err(BaseError::InvalidConfig {
            field: "select a supported baud rate; auto detection requires Survey-In mode",
        });
    }
    Ok(())
}

fn opening_baud(context: &WorkerContext) -> u32 {
    context
        .status
        .borrow()
        .serial
        .as_ref()
        .map(|s| s.baud_rate)
        .unwrap_or(0)
}

struct WorkerContext {
    id: BaseSessionId,
    cancel: Cancellation,
    status: watch::Sender<BaseStatus>,
    corrections: broadcast::Sender<Correction>,
    activity: watch::Sender<Option<(BaseSessionId, Instant)>>,
    last_rtcm: Option<Instant>,
    published_at: Instant,
    identify_by: Instant,
}

impl WorkerContext {
    fn update(&self, apply: impl FnOnce(&mut BaseStatus)) {
        self.status.send_if_modified(|status| {
            if status.session_id != Some(self.id) || self.cancel.is_cancelled() {
                return false;
            }
            apply(status);
            status.revision = status.revision.saturating_add(1);
            true
        });
    }

    fn correction(&mut self, frame: RtcmFrame) {
        if self.cancel.is_cancelled() {
            return;
        }
        let received_at = Instant::now();
        self.last_rtcm = Some(received_at);
        let _ = self.corrections.send(Correction {
            session_id: self.id,
            frame,
            received_at,
            cancel: self.cancel.clone(),
        });
    }

    fn refresh(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.published_at) < STATUS_PERIOD {
            return;
        }
        let last_data = *self.activity.borrow();
        self.update(|status| {
            status.seconds_since_data =
                last_data.map(|(_, at)| now.duration_since(at).as_secs_f64());
            status.seconds_since_rtcm = self
                .last_rtcm
                .map(|at| now.duration_since(at).as_secs_f64());
        });
        self.published_at = now;
    }
}

fn run_receiver(
    io: Box<dyn ReceiverIo>,
    mode: BaseMode,
    baud: u32,
    context: &mut WorkerContext,
) -> Result<(), BaseError> {
    let mut channel = Channel::new(io, context.cancel.clone());
    if let BaseMode::SurveyIn(config) = mode {
        let identity = channel.identify_until(baud, context.identify_by)?;
        context.update(|status| {
            status.receiver = Some(identity);
            status.phase = BasePhase::Configuring;
        });
        channel.configure(config, baud)?;
        context.update(|status| status.phase = BasePhase::Surveying);
    } else {
        context.update(|status| status.phase = BasePhase::Ready);
    }
    let mut ready = matches!(mode, BaseMode::Preconfigured);
    let mut last_survey = Instant::now();
    loop {
        context.cancel.check()?;
        let was_ready = ready;
        let frames = channel.receive()?;
        if let Some(at) = channel.last_data() {
            context.activity.send_replace(Some((context.id, at)));
        }
        for frame in frames {
            match frame {
                WireFrame::Rtcm(frame) if was_ready && ready => context.correction(frame),
                WireFrame::Ubx(message) if message.is(ubx::NAV_SVIN) => {
                    let Some(survey) = ubx::survey_status(&message.payload) else {
                        continue;
                    };
                    if let BaseMode::SurveyIn(config) = mode {
                        last_survey = Instant::now();
                        context.update(|status| status.survey_in = Some(survey));
                        if !ready && survey.complete(config) {
                            channel.activate_rtcm()?;
                            ready = true;
                            context.update(|status| status.phase = BasePhase::Ready);
                        } else if ready && (!survey.valid || survey.active) {
                            return Err(BaseError::Worker(
                                "receiver left the surveyed base mode; reconnect the base".into(),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        if matches!(mode, BaseMode::SurveyIn(_)) && last_survey.elapsed() > SURVEY_REPORT_TIMEOUT {
            return Err(BaseError::Timeout {
                stage: SURVEY_STATUS_STAGE,
            });
        }
        context.refresh();
    }
}

#[cfg(test)]
mod tests;
