//! Parameter service (parameters plan §4.1/§4.2): a tokio task driving the
//! pure [`ParamSession`] state machine over the MAVLink link — same shape
//! as the mission and motor-test services. High-rate `PARAM_VALUE` streams
//! never touch the single-slot `CommandService`; the service reads the
//! routed frames directly and sends via `ConnectionHandle::send`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use std::sync::Mutex as StdMutex;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::codec::{decode_param_id, encode_param_id};
use super::model::{ParamEntry, ParamType, ParamValue};
use super::session::{IncomingParam, ParamSession, SetOutcome};

use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::MessageRoute;
use crate::mavlink::MavMessage;
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
#[derive(Debug, Clone, PartialEq)]
pub enum ParamError {
    Busy,
    NotConnected,
    Timeout,
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
        };
        let _ = reply.send(result);
    }
}

struct SetRequest {
    name: String,
    value: ParamValue,
    ptype: ParamType,
    reply: oneshot::Sender<Result<SetResult, ParamError>>,
}

enum Command {
    Pull(oneshot::Sender<Result<(), ParamError>>),
    Cancel,
    Shutdown,
    Set(Box<SetRequest>),
    Read(Box<ReadRequest>),
    RequestFcVersion,
}

struct ReadRequest {
    name: String,
    reply: oneshot::Sender<Result<ParamEntry, ParamError>>,
}

/// Handle to the running service.
#[derive(Clone)]
pub struct ParamService {
    cmd_tx: mpsc::Sender<Command>,
    cache: ParamCache,
    fc_version: FcVersion,
}

/// The FC's reported AUTOPILOT_VERSION (shared slot, set once on request).
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
        let (tx, cmd_rx) = mpsc::channel(8);
        let (evt_tx, evt_rx) = broadcast::channel(256);
        let cache = ParamCache::default();
        let fc_version = FcVersion::default();
        let task = tokio::spawn(run(
            handle,
            cmd_rx,
            evt_tx,
            cache.clone(),
            fc_version.clone(),
            target_sys,
            target_comp,
        ));
        (
            Self {
                cmd_tx: tx,
                cache,
                fc_version,
            },
            evt_rx,
            task,
        )
    }

    /// The FC's firmware version ("" until AUTOPILOT_VERSION arrives).
    pub fn fc_version(&self) -> Option<String> {
        self.fc_version.get()
    }

    /// Read-only cache view (other services use this, PM9).
    pub fn cache(&self) -> ParamCache {
        self.cache.clone()
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

    /// Read one parameter by name (lazy load); resolves with the FC's value.
    pub async fn read(&self, name: &str) -> Result<ParamEntry, ParamError> {
        let (tx, rx) = oneshot::channel();
        let req = ReadRequest {
            name: name.to_string(),
            reply: tx,
        };
        if self
            .cmd_tx
            .send(Command::Read(Box::new(req)))
            .await
            .is_err()
        {
            return Err(ParamError::NotConnected);
        }
        rx.await.unwrap_or(Err(ParamError::NotConnected))
    }

    /// Set one parameter; resolves on the FC's echo (or timeout).
    pub async fn set(
        &self,
        name: &str,
        value: ParamValue,
        ptype: ParamType,
    ) -> Result<SetResult, ParamError> {
        let (tx, rx) = oneshot::channel();
        let req = SetRequest {
            name: name.to_string(),
            value,
            ptype,
            reply: tx,
        };
        if self.cmd_tx.send(Command::Set(Box::new(req))).await.is_err() {
            return Err(ParamError::NotConnected);
        }
        rx.await.unwrap_or(Err(ParamError::NotConnected))
    }
}

/// Shared, read-mostly cache of the latest known values (plan §4.1 `cache`).
/// Other services (e.g. the motor-test window, PM9) read it without a
/// round-trip to the protocol task.
#[derive(Clone, Default)]
pub struct ParamCache {
    entries: Arc<StdMutex<HashMap<String, ParamEntry>>>,
}

impl ParamCache {
    pub fn get(&self, name: &str) -> Option<ParamEntry> {
        self.entries.lock().ok()?.get(name).cloned()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn update(&self, entry: ParamEntry) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.insert(entry.name.clone(), entry);
        }
    }
}

/// The service shell: command channel + routed frames + a ticker feeding
/// the pure session.
async fn run(
    handle: ConnectionHandle,
    mut cmd_rx: mpsc::Receiver<Command>,
    evt_tx: broadcast::Sender<ParamEvent>,
    cache: ParamCache,
    fc_version: FcVersion,
    target_sys: u8,
    target_comp: u8,
) {
    use super::session::PullUpdate;

    let mut events = handle.subscribe_route(MessageRoute::messages(PARAM_MESSAGE_IDS));
    let mut session = ParamSession::default();
    let mut batch: Vec<ParamEntry> = Vec::new();
    let mut batch_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut last_progress: Option<(u16, u16)> = None;
    let mut quiet_deadline: Option<tokio::time::Instant> = None;
    let mut set_deadline: Option<tokio::time::Instant> = None;
    let mut pending_set_reply = PendingReply::new();
    let mut pending_read_reply: Option<(String, oneshot::Sender<Result<ParamEntry, ParamError>>)> =
        None;
    let mut read_deadline: Option<tokio::time::Instant> = None;
    let mut fill_tick = tokio::time::interval(Duration::from_millis(50));
    fill_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Drain one fill request per tick (rate-limit friendly).
        if session.pull_active() {
            if let Some(request) = session.next_fill_request() {
                if let Some(frame) = outbound_frame(&request, target_sys, target_comp) {
                    if handle.send(frame).await.is_err() {
                        return;
                    }
                }
            }
        }

        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    Command::Pull(reply) => match session.start_pull() {
                        Ok(out) => {
                            quiet_deadline = Some(tokio::time::Instant::now() + PULL_QUIET);
                            let _ = reply.send(Ok(()));
                            for req in out {
                                if let Some(frame) = outbound_frame(&req, target_sys, target_comp) {
                                    if handle.send(frame).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            let _ = reply.send(Err(ParamError::Busy));
                        }
                    },
                    Command::Cancel => {
                        session.cancel();
                        quiet_deadline = None;
                        set_deadline = None;
                        read_deadline = None;
                        if let Some((_, reply)) = pending_read_reply.take() {
                            let _ = reply.send(Err(ParamError::Timeout));
                        }
                    }
                    Command::Shutdown => {
                        session.cancel();
                        pending_set_reply.resolve(SetOutcome::Timeout);
                        if let Some((_, reply)) = pending_read_reply.take() {
                            let _ = reply.send(Err(ParamError::Timeout));
                        }
                        return;
                    }
                    Command::RequestFcVersion => {
                        // MAV_CMD_REQUEST_MESSAGE param1 = 148 (AUTOPILOT_VERSION).
                        let frame = MavMessage::COMMAND_LONG(
                            ::mavlink::common::COMMAND_LONG_DATA {
                                param1: 148.0,
                                param2: 0.0,
                                param3: 0.0,
                                param4: 0.0,
                                param5: 0.0,
                                param6: 0.0,
                                param7: 0.0,
                                command: ::mavlink::common::MavCmd::MAV_CMD_REQUEST_MESSAGE,
                                target_system: target_sys,
                                target_component: target_comp,
                                confirmation: 0,
                            },
                        );
                        if handle.send(frame).await.is_err() {
                            return;
                        }
                    }
                    Command::Read(req) => match session.start_read(&req.name) {
                        Ok(out) => {
                            read_deadline = Some(tokio::time::Instant::now() + SET_TIMEOUT);
                            for r in out {
                                if let Some(frame) = outbound_frame(&r, target_sys, target_comp) {
                                    if handle.send(frame).await.is_err() {
                                        return;
                                    }
                                }
                            }
                            pending_read_reply = Some((req.name, req.reply));
                        }
                        Err(_) => {
                            let _ = req.reply.send(Err(ParamError::Busy));
                        }
                    },
                    Command::Set(req) => match session.start_set(&req.name, req.value, req.ptype)
                    {
                        Ok(out) => {
                            set_deadline = Some(tokio::time::Instant::now() + SET_TIMEOUT);
                            for r in out {
                                if let Some(frame) = outbound_frame(&r, target_sys, target_comp) {
                                    if handle.send(frame).await.is_err() {
                                        return;
                                    }
                                }
                            }
                            pending_set_reply.0 = Some(req.reply);
                        }
                        Err(_) => {
                            let _ = req.reply.send(Err(ParamError::Busy));
                        }
                    },
                }
            }
            ev = events.recv() => {
                match ev {
                    Ok(ConnectionEvent::Message(env)) => {
                        if let MavMessage::AUTOPILOT_VERSION(v) = &env.message {
                            // 4 bytes MSB→LSB: major.minor.patch.type.
                            let f = v.flight_sw_version;
                            let ver = format!(
                                "{}.{}.{}",
                                (f >> 24) & 0xFF,
                                (f >> 16) & 0xFF,
                                (f >> 8) & 0xFF
                            );
                            fc_version.set(ver.clone());
                            let _ = evt_tx.send(ParamEvent::FcVersion(ver));
                        }
                        if let MavMessage::PARAM_VALUE(v) = &env.message {
                            let Some(ptype) = ParamType::from_mav(v.param_type) else {
                                continue;
                            };
                            let incoming = IncomingParam {
                                index: v.param_index,
                                count: v.param_count,
                                name: decode_param_id(&v.param_id),
                                raw_value: v.param_value,
                                ptype,
                            };
                            // Incremental: the session hands back the single new
                            // entry (never a full snapshot — a 1794-param pull
                            // made the old O(n) flush per frame starve the task).
                            let (entry, update, _) = session.on_param_value(incoming);
                            if let Some((_, outcome)) = session.take_set_outcome() {
                                pending_set_reply.resolve(outcome);
                                set_deadline = None;
                            }
                            // A lazy read resolves on its first matching entry.
                            if let (Some(entry), Some((name, reply))) = (&entry, pending_read_reply.take()) {
                                if entry.name == name {
                                    cache.update(entry.clone());
                                    let _ = reply.send(Ok(entry.clone()));
                                    read_deadline = None;
                                    let _ = evt_tx.send(ParamEvent::ExternalChange(entry.clone()));
                                } else {
                                    // Not our read (name mismatch): restore.
                                    pending_read_reply = Some((name, reply));
                                }
                            }
                            if let Some(entry) = entry {
                                cache.update(entry.clone());
                                if !batch_names.contains(&entry.name) {
                                    batch_names.insert(entry.name.clone());
                                    batch.push(entry);
                                }
                                if batch.len() >= BATCH_LEN {
                                    let _ = evt_tx.send(ParamEvent::Batch(std::mem::take(&mut batch)));
                                    batch_names.clear();
                                }
                            }
                            match update {
                                Some(PullUpdate::Done { received, total }) => {
                                    flush_batch(&mut batch, &mut batch_names, &evt_tx);
                                    let _ = evt_tx.send(ParamEvent::PullDone { received, total });
                                    quiet_deadline = None;
                                }
                                Some(PullUpdate::Progress { received, total }) => {
                                    // Throttle: the tick emits progress; the
                                    // per-frame 1794-progress storm drowned the
                                    // event channel in practice.
                                    quiet_deadline = Some(tokio::time::Instant::now() + PULL_QUIET);
                                    last_progress = Some((received, total));
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
            _ = fill_tick.tick() => {
                if let Some((received, total)) = last_progress.take() {
                    let _ = evt_tx.send(ParamEvent::Progress { received, total });
                }
                if session.pull_active() {
                    if let Some(deadline) = quiet_deadline {
                        if tokio::time::Instant::now() >= deadline {
                            let (update, resend) = session.on_pull_timeout();
                            if let Some(super::session::PullUpdate::Done { received, total }) = update {
                                flush_batch(&mut batch, &mut batch_names, &evt_tx);
                                let _ = evt_tx.send(ParamEvent::PullDone { received, total });
                                quiet_deadline = None;
                            } else {
                                // A fill round started, or the lost list
                                // request is being re-issued: re-arm the
                                // quiet window either way.
                                if let Some(req) = resend {
                                    if let Some(frame) = outbound_frame(&req, target_sys, target_comp) {
                                        if handle.send(frame).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                quiet_deadline = Some(tokio::time::Instant::now() + PULL_QUIET);
                            }
                        }
                    }
                }
                if let Some(deadline) = read_deadline {
                    if tokio::time::Instant::now() >= deadline {
                        if session.on_read_timeout() {
                            if let Some(name) = session.reading_name() {
                                let req = super::session::Outbound::RequestReadByName { name };
                                if let Some(frame) = outbound_frame(&req, target_sys, target_comp) {
                                    if handle.send(frame).await.is_err() {
                                        return;
                                    }
                                }
                            }
                            read_deadline = Some(tokio::time::Instant::now() + SET_TIMEOUT);
                        } else if let Some((_, reply)) = pending_read_reply.take() {
                            let _ = reply.send(Err(ParamError::Timeout));
                            read_deadline = None;
                        }
                    }
                }
                if let Some(deadline) = set_deadline {
                    if tokio::time::Instant::now() >= deadline {
                        if let Some(retry) = session.on_set_timeout() {
                            if let Some(frame) = outbound_frame(&retry, target_sys, target_comp) {
                                if handle.send(frame).await.is_err() {
                                    return;
                                }
                            }
                            set_deadline = Some(tokio::time::Instant::now() + SET_TIMEOUT);
                        } else {
                            pending_set_reply.resolve(SetOutcome::Timeout);
                            set_deadline = None;
                        }
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

/// Flush the pending batch (if any) as one event.
fn flush_batch(
    batch: &mut Vec<ParamEntry>,
    names: &mut std::collections::HashSet<String>,
    evt_tx: &broadcast::Sender<ParamEvent>,
) {
    if !batch.is_empty() {
        let _ = evt_tx.send(ParamEvent::Batch(std::mem::take(batch)));
    }
    names.clear();
}
