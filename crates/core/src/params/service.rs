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
use super::session::{IncomingParam, ParamSession, PullUpdate, SetOutcome};

use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::MessageRoute;
use crate::mavlink::MavMessage;
use ::mavlink::MessageData;

/// Routed message ids the parameter service consumes.
pub const PARAM_MESSAGE_IDS: &[u32] = &[
    ::mavlink::common::PARAM_VALUE_DATA::ID,        // 22
    ::mavlink::common::PARAM_REQUEST_READ_DATA::ID, // 20
    ::mavlink::common::PARAM_SET_DATA::ID,          // 23
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
    Set(Box<SetRequest>),
}

/// Handle to the running service.
#[derive(Clone)]
pub struct ParamService {
    cmd_tx: mpsc::Sender<Command>,
    cache: ParamCache,
}

impl ParamService {
    /// Spawn on a link. The task stops when the handle drops, the link
    /// closes, or the event subscriber goes away.
    pub fn spawn(
        handle: ConnectionHandle,
        target_sys: u8,
        target_comp: u8,
    ) -> (Self, broadcast::Receiver<ParamEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (evt_tx, evt_rx) = broadcast::channel(256);
        let cache = ParamCache::default();
        tokio::spawn(run(
            handle,
            cmd_rx,
            evt_tx,
            cache.clone(),
            target_sys,
            target_comp,
        ));
        (Self { cmd_tx, cache }, evt_rx)
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
    target_sys: u8,
    target_comp: u8,
) {
    use super::session::PullUpdate;

    let mut events = handle.subscribe_route(MessageRoute::messages(PARAM_MESSAGE_IDS));
    let mut session = ParamSession::default();
    let mut batch: Vec<ParamEntry> = Vec::new();
    let mut sent_names: HashMap<String, ()> = HashMap::new(); // batched so far
    let mut quiet_deadline: Option<tokio::time::Instant> = None;
    let mut set_deadline: Option<tokio::time::Instant> = None;
    let mut pending_set_reply = PendingReply::new();
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
                    }
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
                            let (update, _) = session.on_param_value(incoming);
                            if let Some((_, outcome)) = session.take_set_outcome() {
                                pending_set_reply.resolve(outcome);
                                set_deadline = None;
                            }
                            if let Some(update) = update {
                                let is_progress = matches!(update, PullUpdate::Progress { .. });
                                handle_pull_update(
                                    update,
                                    &mut session,
                                    &mut batch,
                                    &mut sent_names,
                                    &evt_tx,
                                    &cache,
                                );
                                if is_progress {
                                    quiet_deadline = Some(tokio::time::Instant::now() + PULL_QUIET);
                                }
                            } else {
                                // No update: an external change or a set echo —
                                // keep the cache current quietly.
                                flush_entries(&mut session, &mut batch, &mut sent_names, &evt_tx, &cache);
                            }
                        }
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
            _ = fill_tick.tick() => {
                if session.pull_active() {
                    if let Some(deadline) = quiet_deadline {
                        if tokio::time::Instant::now() >= deadline {
                            if let Some(update) = session.on_pull_timeout() {
                                handle_pull_update(
                                    update,
                                    &mut session,
                                    &mut batch,
                                    &mut sent_names,
                                    &evt_tx,
                                    &cache,
                                );
                                quiet_deadline = None;
                            } else {
                                quiet_deadline = Some(tokio::time::Instant::now() + PULL_QUIET);
                            }
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

/// Snapshot session entries into the batch + cache, flushing when full.
fn flush_entries(
    session: &mut ParamSession,
    batch: &mut Vec<ParamEntry>,
    sent: &mut HashMap<String, ()>,
    evt_tx: &broadcast::Sender<ParamEvent>,
    cache: &ParamCache,
) {
    for entry in session.entries() {
        cache.update(entry.clone());
        if sent.insert(entry.name.clone(), ()).is_none() {
            batch.push(entry);
        }
    }
    if batch.len() >= BATCH_LEN {
        let _ = evt_tx.send(ParamEvent::Batch(std::mem::take(batch)));
        sent.clear();
    }
}

/// Turn a session pull update into events + cache writes (shared by the
/// frame arm and the quiet-deadline arm of the tick).
fn handle_pull_update(
    update: PullUpdate,
    session: &mut ParamSession,
    batch: &mut Vec<ParamEntry>,
    sent: &mut HashMap<String, ()>,
    evt_tx: &broadcast::Sender<ParamEvent>,
    cache: &ParamCache,
) {
    match update {
        PullUpdate::Batch(_) => {
            // The session never emits Batch itself (the shell batches in
            // flush_entries).
        }
        PullUpdate::Progress { received, total } => {
            flush_entries(session, batch, sent, evt_tx, cache);
            let _ = evt_tx.send(ParamEvent::Progress { received, total });
        }
        PullUpdate::Done { received, total } => {
            flush_entries(session, batch, sent, evt_tx, cache);
            let _ = evt_tx.send(ParamEvent::PullDone { received, total });
        }
    }
}
