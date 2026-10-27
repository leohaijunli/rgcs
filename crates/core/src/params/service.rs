//! Parameter service (parameters plan §4.1/§4.2): a tokio task driving the
//! pure [`ParamSession`] state machine over the MAVLink link — same shape
//! as the mission and motor-test services. High-rate `PARAM_VALUE` streams
//! never touch the single-slot `CommandService`; the service reads the
//! routed frames directly and sends via `ConnectionHandle::send`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::sync::Mutex as StdMutex;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::codec::{decode_param_id, encode_param_id};
use super::model::{ParamEntry, ParamType, ParamValue};
use super::session::{IncomingParam, ParamSession, SetOutcome};

use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::MessageRoute;
use crate::mavlink::MavMessage;
use crate::runtime::{OperationId, TransportGeneration};
use ::mavlink::MessageData;

/// Routed message ids the parameter service consumes.
pub const PARAM_MESSAGE_IDS: &[u32] = &[
    ::mavlink::common::PARAM_VALUE_DATA::ID,        // 22
    ::mavlink::common::PARAM_REQUEST_READ_DATA::ID, // 20
    ::mavlink::common::PARAM_SET_DATA::ID,          // 23
    ::mavlink::common::AUTOPILOT_VERSION_DATA::ID,  // 148
];

/// Seconds of stream silence before a fill round (plan §4.2: 默认 2 s).
const PULL_QUIET: Duration = Duration::from_secs(2);
/// Batch flush size for the channel pushes (plan: ~100 per batch).
const BATCH_LEN: usize = 100;
/// Per-send echo timeout (retried by the session budget).
const SET_TIMEOUT: Duration = Duration::from_secs(1);
pub const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll period while waiting for `AUTOPILOT_VERSION` to land.
const FC_VERSION_POLL: Duration = Duration::from_millis(50);
const COMMAND_CAPACITY: usize = 8;
const EVENT_CAPACITY: usize = 256;
const SERVICE_TICK: Duration = Duration::from_millis(50);
const OWNED_CANCEL_TIMEOUT: Duration = Duration::from_secs(1);
const PARAM_ENCODE_BYTEWISE_CAPABILITY: u64 = 16;
const PARAM_ENCODE_CAST_CAPABILITY: u64 = 131_072;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ParamObservation {
    pub entry: ParamEntry,
    pub age_ms: u64,
}

/// Identity and admission boundary of one existing parameter-worker transaction.
/// Native composition issues its OperationId and verifies the target. This does
/// not create a second protocol/cache or grant device authority by itself.
#[derive(Clone)]
pub struct ParamTransaction {
    id: OperationId,
    generation: TransportGeneration,
    deadline: tokio::time::Instant,
    cancellation: crate::devices::SerialCancellation,
}

impl ParamTransaction {
    pub fn new(
        id: OperationId,
        generation: TransportGeneration,
        timeout: Duration,
    ) -> Result<Self, ParamError> {
        if timeout.is_zero() {
            return Err(ParamError::Timeout);
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or(ParamError::Timeout)?;
        Ok(Self {
            id,
            generation,
            deadline,
            cancellation: crate::devices::SerialCancellation::new(),
        })
    }

    pub fn id(&self) -> &OperationId {
        &self.id
    }

    pub fn generation(&self) -> TransportGeneration {
        self.generation
    }

    fn check(&self, handle: &ConnectionHandle) -> Result<(), ParamError> {
        if self.cancellation.is_cancelled() {
            return Err(ParamError::Cancelled);
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err(ParamError::Timeout);
        }
        if handle.generation() != self.generation.get() {
            return Err(ParamError::TargetChanged);
        }
        if handle.is_shutdown_requested() {
            return Err(ParamError::NotConnected);
        }
        Ok(())
    }
}

/// The shared protocol stream retains identity for scoped native publication.
/// Legacy consumers can still subscribe to ParamEvent on the same worker.
#[derive(Debug, Clone)]
pub struct ScopedParamEvent {
    pub transaction_id: Option<OperationId>,
    pub generation: u64,
    pub event: ParamEvent,
}

/// Terminal set outcome surfaced to the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum SetResult {
    Confirmed(ParamValue),
    Clamped {
        requested: ParamValue,
        actual: ParamValue,
    },
    Timeout,
}

/// Events streamed to the adapter/UI (§4.1 `ParamEvent`).
#[derive(Debug, Clone)]
pub enum ParamEvent {
    PullStarted {
        total: u16,
    },
    Batch(Vec<ParamEntry>),
    Progress {
        received: u16,
        total: u16,
    },
    PullDone {
        received: u16,
        total: u16,
    },
    /// A param changed that this GCS did not initiate (QGC or the FC).
    ExternalChange(ParamEntry),
    /// The FC's AUTOPILOT_VERSION arrived (e.g. "1.17.0").
    FcVersion(String),
}

/// Errors surfaced through the command channel.
#[derive(
    Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, thiserror::Error, ts_rs::TS,
)]
#[ts(export, rename = "ParamProtocolError")]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ParamError {
    #[error("another parameter transaction is running")]
    Busy,
    #[error("parameter target is disconnected")]
    NotConnected,
    #[error("parameter transaction timed out")]
    Timeout,
    #[error("parameter transaction cancelled")]
    Cancelled,
    #[error("parameter transaction belongs to an earlier target generation")]
    TargetChanged,
    #[error("parameter name must contain 1–16 non-NUL ASCII characters")]
    InvalidName,
    #[error("parameter value does not match {expected:?}")]
    TypeMismatch { expected: ParamType },
    #[error("parameter value is outside its declared type range")]
    OutOfRange,
    #[error("parameter {name} read-back differs: requested {requested:?}, observed {actual:?}")]
    VerificationMismatch {
        name: String,
        requested: ParamValue,
        actual: ParamValue,
    },
}

/// A set reply held until the FC's echo resolves it.
struct PendingReply(Option<oneshot::Sender<Result<SetResult, ParamError>>>);

impl PendingReply {
    fn new() -> Self {
        Self(None)
    }

    fn resolve(&mut self, outcome: SetOutcome) {
        let Some(reply) = self.0.take() else { return };
        let result = match outcome {
            SetOutcome::Confirmed(v) => Ok(SetResult::Confirmed(v)),
            SetOutcome::Clamped { requested, actual } => {
                Ok(SetResult::Clamped { requested, actual })
            }
            SetOutcome::Timeout => Err(ParamError::Timeout),
            SetOutcome::TypeMismatch { expected } => Err(ParamError::TypeMismatch { expected }),
        };
        let _ = reply.send(result);
    }

    fn reject(&mut self, error: ParamError) {
        if let Some(reply) = self.0.take() {
            let _ = reply.send(Err(error));
        }
    }
}

struct SetRequest {
    name: String,
    value: ParamValue,
    ptype: ParamType,
    reply: oneshot::Sender<Result<SetResult, ParamError>>,
    transaction: Option<ParamTransaction>,
}

enum Command {
    Pull(oneshot::Sender<Result<(), ParamError>>),
    PullOwned {
        transaction: ParamTransaction,
        reply: oneshot::Sender<Result<super::session::PullReport, ParamError>>,
    },
    Cancel,
    CancelOwned {
        id: OperationId,
        generation: TransportGeneration,
        reply: oneshot::Sender<bool>,
    },
    Shutdown,
    Set(Box<SetRequest>),
    Read(Box<ReadRequest>),
    RequestFcVersion,
}

struct ReadRequest {
    name: String,
    reply: oneshot::Sender<Result<ParamEntry, ParamError>>,
    transaction: Option<ParamTransaction>,
}

/// Handle to the running service.
#[derive(Clone)]
pub struct ParamService {
    cmd_tx: mpsc::Sender<Command>,
    cache: ParamCache,
    fc_version: FcVersion,
    events: broadcast::Sender<ParamEvent>,
    scoped_events: broadcast::Sender<ScopedParamEvent>,
}

/// The FC's reported AUTOPILOT_VERSION, invalidated when the link is lost.
#[derive(Clone, Default)]
pub struct FcVersion {
    inner: Arc<StdMutex<Option<String>>>,
}

impl FcVersion {
    fn set(&self, v: String) {
        if let Ok(mut g) = self.inner.lock() {
            *g = Some(v);
        }
    }
    fn clear(&self) {
        if let Ok(mut version) = self.inner.lock() {
            *version = None;
        }
    }
    pub fn get(&self) -> Option<String> {
        self.inner.lock().ok().and_then(|g| g.clone())
    }
}

impl ParamService {
    /// Spawn on a link. The task stops when the handle drops, the link
    /// closes, or the event subscriber goes away.
    pub fn spawn(
        handle: ConnectionHandle,
        target_sys: u8,
        target_comp: u8,
    ) -> (Self, broadcast::Receiver<ParamEvent>) {
        let (service, events, _task) = Self::spawn_with_task(handle, target_sys, target_comp);
        (service, events)
    }

    /// Spawn on a link and return the task handle for an owning adapter that
    /// must await teardown before closing the connection.
    pub fn spawn_with_task(
        handle: ConnectionHandle,
        target_sys: u8,
        target_comp: u8,
    ) -> (
        Self,
        broadcast::Receiver<ParamEvent>,
        tokio::task::JoinHandle<()>,
    ) {
        let (tx, cmd_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (evt_tx, evt_rx) = broadcast::channel(EVENT_CAPACITY);
        let (scoped_events, _) = broadcast::channel(EVENT_CAPACITY);
        let cache = ParamCache::default();
        let fc_version = FcVersion::default();
        let service_events = evt_tx.clone();
        let worker = ParamWorker::new(
            handle,
            EventChannels {
                legacy: evt_tx,
                scoped: scoped_events.clone(),
            },
            cache.clone(),
            fc_version.clone(),
            (target_sys, target_comp),
        );
        let task = tokio::spawn(worker.run(cmd_rx));
        (
            Self {
                cmd_tx: tx,
                cache,
                fc_version,
                events: service_events,
                scoped_events,
            },
            evt_rx,
            task,
        )
    }

    /// The FC's firmware version (None until AUTOPILOT_VERSION arrives).
    pub fn fc_version(&self) -> Option<String> {
        self.fc_version.get()
    }

    /// Read-only cache view (other services use this, PM9).
    pub fn cache(&self) -> ParamCache {
        self.cache.clone()
    }

    /// Subscribe to this service's protocol events (pull completion etc.).
    pub fn subscribe(&self) -> broadcast::Receiver<ParamEvent> {
        self.events.subscribe()
    }

    pub fn subscribe_scoped(&self) -> broadcast::Receiver<ScopedParamEvent> {
        self.scoped_events.subscribe()
    }

    pub async fn pull_owned(
        &self,
        transaction: ParamTransaction,
    ) -> Result<super::session::PullReport, ParamError> {
        let (reply, response) = oneshot::channel();
        self.await_owned(
            transaction.clone(),
            Command::PullOwned { transaction, reply },
            response,
        )
        .await
    }

    pub async fn read_owned(
        &self,
        transaction: ParamTransaction,
        name: &str,
    ) -> Result<ParamEntry, ParamError> {
        validate_name(name)?;
        let (reply, response) = oneshot::channel();
        let request = ReadRequest {
            name: name.into(),
            reply,
            transaction: Some(transaction.clone()),
        };
        self.await_owned(transaction, Command::Read(Box::new(request)), response)
            .await
    }

    pub async fn set_owned(
        &self,
        transaction: ParamTransaction,
        name: &str,
        value: ParamValue,
        ptype: ParamType,
    ) -> Result<SetResult, ParamError> {
        validate_name(name)?;
        validate_value(value, ptype)?;
        let (reply, response) = oneshot::channel();
        let request = SetRequest {
            name: name.into(),
            value,
            ptype,
            reply,
            transaction: Some(transaction.clone()),
        };
        self.await_owned(transaction, Command::Set(Box::new(request)), response)
            .await
    }

    /// Revoke queued/inflight intent synchronously, then wait for this worker
    /// to confirm only the matching transaction stopped. An idle/obsolete cancel
    /// never cancels a different owned or legacy transaction.
    pub async fn cancel_owned(&self, transaction: &ParamTransaction) -> Result<bool, ParamError> {
        transaction.cancellation.cancel();
        let (reply, response) = oneshot::channel();
        tokio::time::timeout(OWNED_CANCEL_TIMEOUT, async {
            self.cmd_tx
                .send(Command::CancelOwned {
                    id: transaction.id.clone(),
                    generation: transaction.generation,
                    reply,
                })
                .await
                .map_err(|_| ParamError::NotConnected)?;
            response.await.map_err(|_| ParamError::NotConnected)
        })
        .await
        .map_err(|_| ParamError::Timeout)?
    }

    async fn await_owned<T>(
        &self,
        transaction: ParamTransaction,
        command: Command,
        response: oneshot::Receiver<Result<T, ParamError>>,
    ) -> Result<T, ParamError> {
        let mut guard = OwnedWait {
            transaction: transaction.clone(),
            finished: false,
        };
        let result = tokio::select! {
            biased;
            _ = transaction.cancellation.cancelled() => Err(ParamError::Cancelled),
            result = tokio::time::timeout_at(transaction.deadline, async {
                self.cmd_tx.send(command).await.map_err(|_| ParamError::NotConnected)?;
                response.await.map_err(|_| ParamError::NotConnected)?
            }) => result.map_err(|_| ParamError::Timeout)?,
        };
        guard.finished = true;
        if result.is_err() {
            transaction.cancellation.cancel();
        }
        result
    }

    /// Run a full parameter pull and wait for its completion report.
    ///
    /// `pull()` only acknowledges that the request was started; callers that
    /// need the FC's parameter inventory must wait for `PullDone`.
    pub async fn pull_and_wait(
        &self,
        timeout: Duration,
    ) -> Result<super::session::PullReport, ParamError> {
        let mut events = self.subscribe();
        self.pull().await?;
        tokio::time::timeout(timeout, async {
            loop {
                match events.recv().await {
                    Ok(ParamEvent::PullDone { received, total }) => {
                        return Ok(super::session::PullReport { received, total });
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(ParamError::NotConnected);
                    }
                }
            }
        })
        .await
        .map_err(|_| ParamError::Timeout)?
    }

    /// Start a full pull; `Err` when a transaction is already running.
    pub async fn pull(&self) -> Result<(), ParamError> {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(Command::Pull(tx)).await.is_err() {
            return Err(ParamError::NotConnected);
        }
        rx.await.unwrap_or(Err(ParamError::NotConnected))
    }

    /// Cancel the running transaction (idempotent).
    pub async fn cancel(&self) {
        let _ = self.cmd_tx.send(Command::Cancel).await;
    }

    /// Stop the protocol task even when other clones of this handle are still
    /// alive. Pending read/set requests resolve as timeouts; later commands
    /// observe a closed channel.
    pub async fn shutdown(&self) {
        let _ = self.cmd_tx.send(Command::Shutdown).await;
    }

    /// Ask the FC for AUTOPILOT_VERSION (fire-and-forget: the version arrives
    /// as a routed frame and lands in `fc_version()`/`FcVersion` events).
    pub async fn request_fc_version(&self) -> Result<(), ParamError> {
        if self.cmd_tx.send(Command::RequestFcVersion).await.is_err() {
            return Err(ParamError::NotConnected);
        }
        Ok(())
    }

    /// Request `AUTOPILOT_VERSION` and wait for the answer to land.
    ///
    /// Replies arrive asynchronously, including after the caller has queued
    /// its request. Each attempt waits up to `settle` for the slot to fill and
    /// re-requests between attempts; a cached version returns instantly.
    pub async fn wait_fc_version(&self, attempts: u32, settle: Duration) -> Option<String> {
        for _ in 0..attempts {
            if let Some(version) = self.fc_version() {
                return Some(version);
            }
            if self.request_fc_version().await.is_err() {
                return self.fc_version();
            }
            let deadline = Instant::now() + settle;
            while Instant::now() < deadline {
                tokio::time::sleep(FC_VERSION_POLL).await;
                if let Some(version) = self.fc_version() {
                    return Some(version);
                }
            }
        }
        self.fc_version()
    }

    /// Read one parameter by name (lazy load); resolves with the FC's value.
    pub async fn read(&self, name: &str) -> Result<ParamEntry, ParamError> {
        self.read_with_timeout(name, TRANSACTION_TIMEOUT).await
    }

    /// Read one parameter with a caller-chosen deadline. Used for capability
    /// probes where a missing name must not stall for the full transaction
    /// timeout.
    pub async fn read_with_timeout(
        &self,
        name: &str,
        timeout: Duration,
    ) -> Result<ParamEntry, ParamError> {
        validate_name(name)?;
        let (tx, rx) = oneshot::channel();
        let req = ReadRequest {
            name: name.to_string(),
            reply: tx,
            transaction: None,
        };
        tokio::time::timeout(timeout, async {
            if self
                .cmd_tx
                .send(Command::Read(Box::new(req)))
                .await
                .is_err()
            {
                return Err(ParamError::NotConnected);
            }
            rx.await.map_err(|_| ParamError::NotConnected)?
        })
        .await
        .map_err(|_| ParamError::Timeout)?
    }

    /// Set one parameter; resolves on the FC's echo (or timeout).
    pub async fn set(
        &self,
        name: &str,
        value: ParamValue,
        ptype: ParamType,
    ) -> Result<SetResult, ParamError> {
        validate_name(name)?;
        validate_value(value, ptype)?;
        let (tx, rx) = oneshot::channel();
        let req = SetRequest {
            name: name.to_string(),
            value,
            ptype,
            reply: tx,
            transaction: None,
        };
        tokio::time::timeout(TRANSACTION_TIMEOUT, async {
            if self.cmd_tx.send(Command::Set(Box::new(req))).await.is_err() {
                return Err(ParamError::NotConnected);
            }
            rx.await.map_err(|_| ParamError::NotConnected)?
        })
        .await
        .map_err(|_| ParamError::Timeout)?
    }

    /// Capture the set echo and require an explicit fresh read-back. A
    /// clamped/rejected value is retained in the cache but never succeeds.
    pub async fn set_confirmed(
        &self,
        name: &str,
        value: ParamValue,
        ptype: ParamType,
    ) -> Result<ParamEntry, ParamError> {
        tokio::time::timeout(TRANSACTION_TIMEOUT, async {
            match self.set(name, value, ptype).await? {
                SetResult::Confirmed(_) => {}
                SetResult::Clamped { actual, .. } => {
                    return Err(ParamError::VerificationMismatch {
                        name: name.into(),
                        requested: value,
                        actual,
                    })
                }
                SetResult::Timeout => return Err(ParamError::Timeout),
            }
            let entry = self.read(name).await?;
            if entry.ptype != ptype {
                return Err(ParamError::TypeMismatch { expected: ptype });
            }
            if !entry.value.same_as(value) {
                return Err(ParamError::VerificationMismatch {
                    name: name.into(),
                    requested: value,
                    actual: entry.value,
                });
            }
            Ok(entry)
        })
        .await
        .map_err(|_| ParamError::Timeout)?
    }
}

struct OwnedWait {
    transaction: ParamTransaction,
    finished: bool,
}

impl Drop for OwnedWait {
    fn drop(&mut self) {
        if !self.finished {
            self.transaction.cancellation.cancel();
        }
    }
}

pub fn validate_name(name: &str) -> Result<(), ParamError> {
    if name.is_empty()
        || name.len() > super::codec::PARAM_ID_LEN
        || !name.is_ascii()
        || name.as_bytes().contains(&0)
    {
        Err(ParamError::InvalidName)
    } else {
        Ok(())
    }
}

pub fn validate_value(value: ParamValue, ptype: ParamType) -> Result<(), ParamError> {
    let range = match ptype {
        ParamType::U8 => (0, u8::MAX as i64),
        ParamType::I8 => (i8::MIN as i64, i8::MAX as i64),
        ParamType::U16 => (0, u16::MAX as i64),
        ParamType::I16 => (i16::MIN as i64, i16::MAX as i64),
        ParamType::U32 => (0, u32::MAX as i64),
        ParamType::I32 => (i32::MIN as i64, i32::MAX as i64),
        ParamType::Real32 => {
            return match value {
                ParamValue::Real(v) if v.is_finite() => Ok(()),
                ParamValue::Real(_) => Err(ParamError::OutOfRange),
                _ => Err(ParamError::TypeMismatch { expected: ptype }),
            }
        }
    };
    match value {
        ParamValue::Int(v) if v >= range.0 && v <= range.1 => Ok(()),
        ParamValue::Int(_) => Err(ParamError::OutOfRange),
        _ => Err(ParamError::TypeMismatch { expected: ptype }),
    }
}

/// Shared, read-mostly cache of the latest known values (plan §4.1 `cache`).
/// Other services (e.g. the motor-test window, PM9) read it without a
/// round-trip to the protocol task.
#[derive(Clone, Default)]
pub struct ParamCache {
    entries: Arc<StdMutex<HashMap<String, (ParamEntry, Instant)>>>,
}

impl ParamCache {
    pub fn get(&self, name: &str) -> Option<ParamEntry> {
        self.entries
            .lock()
            .ok()?
            .get(name)
            .map(|(entry, _)| entry.clone())
    }

    pub fn observation(&self, name: &str) -> Option<ParamObservation> {
        self.entries
            .lock()
            .ok()?
            .get(name)
            .map(|(entry, at)| ParamObservation {
                entry: entry.clone(),
                age_ms: at.elapsed().as_millis().min(u64::MAX as u128) as u64,
            })
    }

    /// A target-bound reader must not relabel a value retained from before a
    /// transport reopen as an observation of the new generation. The original
    /// cache remains shared and unchanged; callers may request fresh read-back.
    pub fn observation_since(&self, name: &str, not_before: Instant) -> Option<ParamObservation> {
        self.entries
            .lock()
            .ok()?
            .get(name)
            .filter(|(_, at)| *at >= not_before)
            .map(|(entry, at)| ParamObservation {
                entry: entry.clone(),
                age_ms: at.elapsed().as_millis().min(u64::MAX as u128) as u64,
            })
    }

    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot of every cached parameter name (param UI & diagnostics).
    pub fn names(&self) -> Vec<String> {
        self.entries
            .lock()
            .map(|guard| guard.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn update(&self, entry: ParamEntry, received_at: Instant) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.insert(entry.name.clone(), (entry, received_at));
        }
    }
}

struct EventChannels {
    legacy: broadcast::Sender<ParamEvent>,
    scoped: broadcast::Sender<ScopedParamEvent>,
}

type ReadReply = (String, oneshot::Sender<Result<ParamEntry, ParamError>>);
type PullReply = oneshot::Sender<Result<super::session::PullReport, ParamError>>;

/// One service shell around the unchanged pure ParamSession. Ownership identity
/// belongs to its accepted transaction, never the current UI/global selection.
struct ParamWorker {
    handle: ConnectionHandle,
    events: crate::mavlink::router::RoutedEvents,
    output: EventChannels,
    cache: ParamCache,
    fc_version: FcVersion,
    target: (u8, u8),
    generation: u64,
    session: ParamSession,
    owned: Option<ParamTransaction>,
    started_at: Option<Instant>,
    pull_reply: Option<PullReply>,
    read_reply: Option<ReadReply>,
    set_reply: PendingReply,
    batch: Vec<ParamEntry>,
    batch_names: std::collections::HashSet<String>,
    progress: Option<(u16, u16)>,
    quiet_deadline: Option<tokio::time::Instant>,
    read_deadline: Option<tokio::time::Instant>,
    set_deadline: Option<tokio::time::Instant>,
}

impl ParamWorker {
    fn new(
        handle: ConnectionHandle,
        output: EventChannels,
        cache: ParamCache,
        fc_version: FcVersion,
        target: (u8, u8),
    ) -> Self {
        let events = handle
            .subscribe_route(MessageRoute::messages(PARAM_MESSAGE_IDS).from(target.0, target.1));
        let generation = handle.generation();
        Self {
            handle,
            events,
            output,
            cache,
            fc_version,
            target,
            generation,
            session: ParamSession::default(),
            owned: None,
            started_at: None,
            pull_reply: None,
            read_reply: None,
            set_reply: PendingReply::new(),
            batch: Vec::new(),
            batch_names: Default::default(),
            progress: None,
            quiet_deadline: None,
            read_deadline: None,
            set_deadline: None,
        }
    }

    fn emit(&self, event: ParamEvent) {
        if self.output.legacy.receiver_count() > 0 {
            let _ = self.output.legacy.send(event.clone());
        }
        let _ = self.output.scoped.send(ScopedParamEvent {
            transaction_id: self
                .owned
                .as_ref()
                .map(|transaction| transaction.id.clone()),
            generation: self
                .owned
                .as_ref()
                .map_or(self.generation, |transaction| transaction.generation.get()),
            event,
        });
    }

    fn flush(&mut self) {
        if !self.batch.is_empty() {
            let batch = std::mem::take(&mut self.batch);
            self.emit(ParamEvent::Batch(batch));
        }
        self.batch_names.clear();
    }

    fn abort(&mut self, error: ParamError) {
        self.session.cancel();
        self.flush();
        self.progress = None;
        self.quiet_deadline = None;
        self.read_deadline = None;
        self.set_deadline = None;
        if let Some(reply) = self.pull_reply.take() {
            let _ = reply.send(Err(error.clone()));
        }
        if let Some((_, reply)) = self.read_reply.take() {
            let _ = reply.send(Err(error.clone()));
        }
        self.set_reply.reject(error);
        self.owned = None;
        self.started_at = None;
    }

    fn reset_transport(&mut self, error: ParamError) {
        let error = if self.owned.is_none() && error == ParamError::TargetChanged {
            ParamError::NotConnected
        } else {
            error
        };
        self.batch.clear();
        self.batch_names.clear();
        self.abort(error);
        self.session = ParamSession::default();
        self.cache.clear();
        self.fc_version.clear();
        self.generation = self.handle.generation();
    }

    async fn send(&self, frame: MavMessage) -> Result<(), ParamError> {
        let Some(transaction) = self.owned.as_ref() else {
            return self
                .handle
                .send(frame)
                .await
                .map_err(|_| ParamError::NotConnected);
        };
        transaction.check(&self.handle)?;
        let result = tokio::select! {
            biased;
            _ = transaction.cancellation.cancelled() => return Err(ParamError::Cancelled),
            result = self.handle.send_in_generation(frame, transaction.generation.get(), transaction.deadline) => result,
        };
        if result.is_err() {
            transaction.check(&self.handle)?;
            return Err(ParamError::NotConnected);
        }
        transaction.check(&self.handle)
    }

    async fn send_requests(
        &self,
        requests: Vec<super::session::Outbound>,
    ) -> Result<(), ParamError> {
        for request in requests {
            if let Some(frame) = outbound_frame(&request, self.target.0, self.target.1) {
                self.send(frame).await?;
            }
        }
        Ok(())
    }

    fn accept_owned(&self, transaction: Option<&ParamTransaction>) -> Result<(), ParamError> {
        if let Some(transaction) = transaction {
            transaction.check(&self.handle)?;
        }
        Ok(())
    }

    fn begin(&mut self, transaction: Option<ParamTransaction>) {
        self.flush();
        self.owned = transaction;
        self.started_at = Some(Instant::now());
    }

    async fn command(&mut self, command: Command) -> Result<bool, ParamError> {
        match command {
            Command::Cancel => self.abort(ParamError::Cancelled),
            Command::CancelOwned {
                id,
                generation,
                reply,
            } => {
                let matches = self.owned.as_ref().is_some_and(|transaction| {
                    transaction.id == id && transaction.generation == generation
                });
                if matches {
                    self.abort(ParamError::Cancelled);
                }
                let _ = reply.send(matches);
            }
            Command::Shutdown => {
                self.abort(ParamError::NotConnected);
                return Ok(false);
            }
            Command::Pull(reply) => match self.session.start_pull() {
                Ok(requests) => {
                    self.begin(None);
                    self.quiet_deadline = Some(tokio::time::Instant::now() + PULL_QUIET);
                    self.emit(ParamEvent::PullStarted { total: 0 });
                    let result = self.send_requests(requests).await;
                    let _ = reply.send(result.clone());
                    result?;
                }
                Err(_) => {
                    let _ = reply.send(Err(ParamError::Busy));
                }
            },
            Command::PullOwned { transaction, reply } => {
                if let Err(error) = self.accept_owned(Some(&transaction)) {
                    let _ = reply.send(Err(error));
                    return Ok(true);
                }
                match self.session.start_pull() {
                    Ok(requests) => {
                        self.begin(Some(transaction));
                        self.pull_reply = Some(reply);
                        self.quiet_deadline = Some(tokio::time::Instant::now() + PULL_QUIET);
                        self.emit(ParamEvent::PullStarted { total: 0 });
                        self.send_requests(requests).await?;
                    }
                    Err(_) => {
                        let _ = reply.send(Err(ParamError::Busy));
                    }
                }
            }
            Command::Read(request) => {
                if let Err(error) = self.accept_owned(request.transaction.as_ref()) {
                    let _ = request.reply.send(Err(error));
                    return Ok(true);
                }
                match self.session.start_read(&request.name) {
                    Ok(requests) => {
                        self.begin(request.transaction);
                        self.read_reply = Some((request.name, request.reply));
                        self.read_deadline = Some(tokio::time::Instant::now() + SET_TIMEOUT);
                        self.send_requests(requests).await?;
                    }
                    Err(_) => {
                        let _ = request.reply.send(Err(ParamError::Busy));
                    }
                }
            }
            Command::Set(request) => {
                if let Err(error) = self.accept_owned(request.transaction.as_ref()) {
                    let _ = request.reply.send(Err(error));
                    return Ok(true);
                }
                match self
                    .session
                    .start_set(&request.name, request.value, request.ptype)
                {
                    Ok(requests) => {
                        self.begin(request.transaction);
                        self.set_reply.0 = Some(request.reply);
                        self.set_deadline = Some(tokio::time::Instant::now() + SET_TIMEOUT);
                        self.send_requests(requests).await?;
                    }
                    Err(_) => {
                        let _ = request.reply.send(Err(ParamError::Busy));
                    }
                }
            }
            Command::RequestFcVersion => {
                self.send(MavMessage::COMMAND_LONG(
                    ::mavlink::common::COMMAND_LONG_DATA {
                        param1: ::mavlink::common::AUTOPILOT_VERSION_DATA::ID as f32,
                        command: ::mavlink::common::MavCmd::MAV_CMD_REQUEST_MESSAGE,
                        target_system: self.target.0,
                        target_component: self.target.1,
                        ..Default::default()
                    },
                ))
                .await?;
            }
        }
        Ok(true)
    }

    fn done_pull(&mut self, received: u16, total: u16) {
        self.flush();
        self.emit(ParamEvent::PullDone { received, total });
        if let Some(reply) = self.pull_reply.take() {
            let _ = reply.send(Ok(super::session::PullReport { received, total }));
        }
        self.progress = None;
        self.quiet_deadline = None;
        self.owned = None;
        self.started_at = None;
    }

    fn message(&mut self, envelope: &crate::mavlink::MessageEnvelope) {
        use super::session::PullUpdate;
        if self
            .handle
            .connected_since_snapshot()
            .is_none_or(|opened| envelope.received_at < opened)
            || (self.session.is_busy()
                && self
                    .started_at
                    .is_some_and(|started| envelope.received_at < started))
        {
            return;
        }
        if let MavMessage::AUTOPILOT_VERSION(version) = &envelope.message {
            let encoding = if version.capabilities.bits() & PARAM_ENCODE_BYTEWISE_CAPABILITY != 0 {
                super::codec::IntEncoding::ByteConcat
            } else if version.capabilities.bits() & PARAM_ENCODE_CAST_CAPABILITY != 0 {
                super::codec::IntEncoding::Cast
            } else {
                super::codec::IntEncoding::ByteConcat
            };
            self.session.set_encoding(encoding);
            let value = version.flight_sw_version;
            let version = format!(
                "{}.{}.{}",
                (value >> 24) & 0xff,
                (value >> 16) & 0xff,
                (value >> 8) & 0xff
            );
            self.fc_version.set(version.clone());
            self.emit(ParamEvent::FcVersion(version));
        }
        let MavMessage::PARAM_VALUE(value) = &envelope.message else {
            return;
        };
        let name = decode_param_id(&value.param_id);
        if name == "_HASH_CHECK" {
            return;
        }
        let Some(ptype) = ParamType::from_mav(value.param_type) else {
            return;
        };
        let was_pull = self.session.pull_active();
        let (entry, update, _) = self.session.on_param_value(IncomingParam {
            index: value.param_index,
            count: value.param_count,
            name,
            raw_value: value.param_value,
            ptype,
        });
        let set_outcome = self.session.take_set_outcome();
        if let Some(entry) = entry {
            self.cache.update(entry.clone(), envelope.received_at);
            if !was_pull {
                self.emit(ParamEvent::ExternalChange(entry.clone()));
            }
            if let Some((name, reply)) = self.read_reply.take() {
                if entry.name == name {
                    let _ = reply.send(Ok(entry.clone()));
                    self.read_deadline = None;
                } else {
                    self.read_reply = Some((name, reply));
                }
            }
            if self.batch_names.insert(entry.name.clone()) {
                self.batch.push(entry);
            }
            if self.batch.len() >= BATCH_LEN {
                self.flush();
            }
        }
        if let Some((_, outcome)) = set_outcome {
            self.set_reply.resolve(outcome);
            self.set_deadline = None;
        }
        match update {
            Some(PullUpdate::Done { received, total }) => self.done_pull(received, total),
            Some(PullUpdate::Progress { received, total }) => {
                self.progress = Some((received, total));
                self.quiet_deadline = Some(tokio::time::Instant::now() + PULL_QUIET);
            }
            _ => {}
        }
        if !self.session.is_busy() {
            self.flush();
            self.owned = None;
            self.started_at = None;
        }
    }

    async fn tick(&mut self) -> Result<(), ParamError> {
        use super::session::{Outbound, PullUpdate};
        if let Some((received, total)) = self.progress.take() {
            self.emit(ParamEvent::Progress { received, total });
        }
        self.flush();
        let now = tokio::time::Instant::now();
        if self.session.pull_active() {
            if self.quiet_deadline.is_some_and(|deadline| now >= deadline) {
                let (update, resend) = self.session.on_pull_timeout();
                if let Some(PullUpdate::Done { received, total }) = update {
                    self.done_pull(received, total);
                } else {
                    if let Some(request) = resend {
                        self.send_requests(vec![request]).await?;
                    }
                    self.quiet_deadline = Some(now + PULL_QUIET);
                }
            }
            if let Some(request) = self.session.next_fill_request() {
                self.send_requests(vec![request]).await?;
            }
        }
        if self.read_deadline.is_some_and(|deadline| now >= deadline) {
            if self.session.on_read_timeout() {
                if let Some(name) = self.session.reading_name() {
                    self.send_requests(vec![Outbound::RequestReadByName { name }])
                        .await?;
                }
                self.read_deadline = Some(now + SET_TIMEOUT);
            } else {
                self.abort(ParamError::Timeout);
            }
        }
        if self.set_deadline.is_some_and(|deadline| now >= deadline) {
            if let Some(request) = self.session.on_set_timeout() {
                self.send_requests(vec![request]).await?;
                self.set_deadline = Some(now + SET_TIMEOUT);
            } else {
                self.abort(ParamError::Timeout);
            }
        }
        Ok(())
    }

    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        let mut tick = tokio::time::interval(SERVICE_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if self.generation != self.handle.generation() {
                self.reset_transport(ParamError::TargetChanged);
            }
            if let Some(error) = self
                .owned
                .as_ref()
                .and_then(|transaction| transaction.check(&self.handle).err())
            {
                self.abort(error);
            }
            let result = tokio::select! {
                biased;
                command = commands.recv() => match command { Some(command) => self.command(command).await, None => { self.abort(ParamError::NotConnected); return; } },
                event = self.events.recv() => {
                    match event {
                        Ok(ConnectionEvent::Message(envelope)) => self.message(&envelope),
                        Ok(ConnectionEvent::Connected { generation, .. }) if generation == self.handle.generation() && generation != self.generation => self.reset_transport(ParamError::TargetChanged),
                        Ok(ConnectionEvent::LinkError(_)) | Ok(ConnectionEvent::Failed(_)) | Ok(ConnectionEvent::HeartbeatLost { .. }) => self.reset_transport(ParamError::NotConnected),
                        Err(broadcast::error::RecvError::Closed) => { self.abort(ParamError::NotConnected); return; },
                        _ => {},
                    }
                    Ok(true)
                },
                _ = tick.tick() => self.tick().await.map(|_| true),
            };
            match result {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    let owned = self.owned.is_some();
                    self.abort(error);
                    if !owned {
                        return;
                    }
                }
            }
        }
    }
}

/// Map a session outbound request onto the wire.
fn outbound_frame(
    request: &super::session::Outbound,
    target_sys: u8,
    target_comp: u8,
) -> Option<MavMessage> {
    use super::session::Outbound;
    Some(match request {
        Outbound::RequestList => {
            MavMessage::PARAM_REQUEST_LIST(::mavlink::common::PARAM_REQUEST_LIST_DATA {
                target_system: target_sys,
                target_component: target_comp,
            })
        }
        Outbound::RequestRead { index } => {
            MavMessage::PARAM_REQUEST_READ(::mavlink::common::PARAM_REQUEST_READ_DATA {
                target_system: target_sys,
                target_component: target_comp,
                param_index: *index as i16,
                param_id: [0u8; 16].into(),
            })
        }
        Outbound::RequestReadByName { name } => {
            MavMessage::PARAM_REQUEST_READ(::mavlink::common::PARAM_REQUEST_READ_DATA {
                target_system: target_sys,
                target_component: target_comp,
                // MAVLINK: param_index = 65535 means "by name".
                param_index: -1,
                param_id: encode_param_id(name).into(),
            })
        }
        Outbound::Set { name, value, ptype } => {
            MavMessage::PARAM_SET(::mavlink::common::PARAM_SET_DATA {
                target_system: target_sys,
                target_component: target_comp,
                param_value: *value,
                param_id: encode_param_id(name).into(),
                param_type: ptype.to_mav(),
            })
        }
    })
}

#[cfg(test)]
mod scoped_cache_tests {
    use super::*;

    fn entry(value: i64) -> ParamEntry {
        ParamEntry {
            name: "SCOPED".into(),
            index: 0,
            ptype: ParamType::I32,
            value: ParamValue::Int(value),
        }
    }

    #[test]
    fn queued_old_observation_is_not_relabelled_as_a_reopened_transport() {
        let cache = ParamCache::default();
        let reopened = Instant::now();
        let received_before_reopen = reopened - Duration::from_secs(1);
        cache.update(entry(17), received_before_reopen);
        assert!(cache.observation_since("SCOPED", reopened).is_none());
        assert_eq!(
            cache.get("SCOPED").expect("retained cache").value,
            ParamValue::Int(17)
        );
        assert!(cache.observation("SCOPED").expect("age").age_ms >= 1_000);
        cache.update(entry(18), Instant::now());
        assert_eq!(
            cache
                .observation_since("SCOPED", reopened)
                .expect("fresh generation")
                .entry
                .value,
            ParamValue::Int(18)
        );
    }
}
