//! Command service (issue #5).
//!
//! A single spawned task owns a [`CommandSession`] on a live MAVLink
//! connection: it accepts [`CommandCommand::Send`] enqueuements, feeds inbound
//! frames to the session, retransmits unacked commands on a fixed tick, and
//! emits bounded [`CommandEvent`] notifications to its legacy subscriber. Owned
//! commands have their own reply/retirement identity. It lives in `core` so the
//! headless server can reuse it; the desktop app only adapts the events onto
//! its UI bus.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::mavlink::common::{MavCmd, MavMessage, COMMAND_ACK_DATA};
use ::mavlink::MessageData;
use tokio::sync::{mpsc, oneshot};

use super::transaction::OwnedWait;
use super::{
    command_priority, CommandError, CommandEvent, CommandResult, CommandSession,
    CommandTransaction, Priority,
};
use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::message::MessageEnvelope;
use crate::mavlink::router::MessageRoute;

/// Retransmission tick period of the service loop.
pub const RETRANSMIT_TICK: Duration = Duration::from_millis(200);
const COMMAND_CAPACITY: usize = 8;
const EVENT_CAPACITY: usize = 16;
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_UNRESOLVED_COMMANDS: usize = 32;

/// Commands accepted by the service task.
#[derive(Debug)]
pub enum CommandCommand {
    /// Send a `COMMAND_LONG` and track it until its ack (or failure).
    Send {
        command: MavCmd,
        params: [f32; 7],
        priority: Priority,
    },
    /// Like [`Send`](CommandCommand::Send), but the task answers the
    /// terminal outcome through `reply` (see [`CommandService::send_await`]).
    SendAwait {
        command: MavCmd,
        params: [f32; 7],
        priority: Priority,
        reply: oneshot::Sender<Result<CommandResult, CommandError>>,
    },
    /// An owned request is also a retirement guard while queued or in flight.
    SendOwned(OwnedCommand),
}

/// Created only by `send_owned`; dropping queued/inflight work confirms that
/// this worker no longer owns a retry. It does not confirm the FC's outcome.
#[derive(Debug)]
pub struct OwnedCommand {
    command: MavCmd,
    params: [f32; 7],
    transaction: CommandTransaction,
    reply: Option<oneshot::Sender<Result<CommandResult, CommandError>>>,
}

impl Drop for OwnedCommand {
    fn drop(&mut self) {
        self.transaction.finish();
    }
}

/// Handle to the running command service.
#[derive(Clone)]
pub struct CommandService {
    cmd_tx: mpsc::Sender<CommandCommand>,
    dropped_events: Arc<AtomicU64>,
}

impl CommandService {
    /// Spawn the command service on the given link.
    ///
    /// Returns the handle and the event stream. The task stops when the
    /// handle is dropped or the connection worker shuts down. Observers do not
    /// own protocol work; a slow or closed event receiver cannot stall cleanup.
    pub fn spawn(
        handle: ConnectionHandle,
        target_sys: u8,
        target_comp: u8,
    ) -> (Self, mpsc::Receiver<CommandEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (evt_tx, evt_rx) = mpsc::channel(EVENT_CAPACITY);
        let dropped_events = Arc::new(AtomicU64::new(0));
        tokio::spawn(run(
            handle,
            cmd_rx,
            evt_tx,
            dropped_events.clone(),
            target_sys,
            target_comp,
        ));
        (
            Self {
                cmd_tx,
                dropped_events,
            },
            evt_rx,
        )
    }

    /// Legacy notifications dropped at the bounded observer boundary. Awaited
    /// replies and owned-operation outcomes do not travel through that queue.
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    /// Enqueue a command; returns `Err` if the service stopped.
    ///
    /// The urgency comes from [`command_priority`] — the one table — so an RTL
    /// preempts whatever is in flight from whichever call site sends it (A7),
    /// and a caller cannot accidentally send an emergency command as normal.
    pub async fn send(&self, command: MavCmd, params: [f32; 7]) -> Result<(), CommandError> {
        self.send_with_priority(command, params, command_priority(command))
            .await
    }

    /// Enqueue a command with an explicit urgency. Used by tests and by any
    /// future call site whose urgency is not a property of the `MAV_CMD`.
    pub async fn send_with_priority(
        &self,
        command: MavCmd,
        params: [f32; 7],
        priority: Priority,
    ) -> Result<(), CommandError> {
        self.cmd_tx
            .send(CommandCommand::Send {
                command,
                params,
                priority,
            })
            .await
            .map_err(|_| CommandError::ServiceStopped)
    }

    /// Enqueue a command and await its terminal outcome: the `COMMAND_ACK`
    /// result when the FC answers, or the session failure (busy / no-ack
    /// after the retry budget / link lost). `await_timeout` is a caller-side
    /// cap on top of the session's own budget; on expiry the command keeps
    /// running and this returns [`CommandError::TimedOut`].
    pub async fn send_await(
        &self,
        command: MavCmd,
        params: [f32; 7],
        await_timeout: Duration,
    ) -> Result<CommandResult, CommandError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(CommandCommand::SendAwait {
                command,
                params,
                priority: command_priority(command),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CommandError::ServiceStopped)?;
        match tokio::time::timeout(await_timeout, reply_rx).await {
            Ok(Ok(result)) => result,
            // The service dropped the reply without answering: it stopped.
            Ok(Err(_)) => Err(CommandError::ServiceStopped),
            Err(_) => Err(CommandError::TimedOut),
        }
    }

    /// Send through the existing protocol slot, bound to this transaction's
    /// generation, deadline and cancellation. The budget includes queueing.
    pub async fn send_owned(
        &self,
        transaction: CommandTransaction,
        command: MavCmd,
        params: [f32; 7],
    ) -> Result<CommandResult, CommandError> {
        transaction.submit()?;
        let _wait = OwnedWait(transaction.clone());
        let (reply, response) = oneshot::channel();
        let request = CommandCommand::SendOwned(OwnedCommand {
            command,
            params,
            transaction: transaction.clone(),
            reply: Some(reply),
        });
        tokio::select! {
            biased;
            error = transaction.stopped() => Err(error),
            result = async {
                self.cmd_tx.send(request).await.map_err(|_| CommandError::ServiceStopped)?;
                response.await.map_err(|_| CommandError::ServiceStopped)?
            } => result,
        }
    }

    /// Revoke only this transaction, without queuing behind other clients or
    /// event consumers. `true` confirms worker retirement, not physical undo.
    pub async fn cancel_owned(
        &self,
        transaction: &CommandTransaction,
    ) -> Result<bool, CommandError> {
        transaction.cancel_and_wait().await
    }
}

struct Pending {
    command: MavCmd,
    generation: u64,
    started_at: Instant,
    reply: Option<oneshot::Sender<Result<CommandResult, CommandError>>>,
    owned: Option<OwnedCommand>,
    dispatches: u8,
}

impl Pending {
    fn check(&self, handle: &ConnectionHandle) -> Result<(), CommandError> {
        if let Some(owned) = &self.owned {
            owned.transaction.check(handle)?;
        }
        if handle.generation() != self.generation {
            return Err(CommandError::TargetChanged);
        }
        if handle.is_shutdown_requested() {
            return Err(CommandError::LinkLost);
        }
        Ok(())
    }

    fn finish(mut self, result: Result<CommandResult, CommandError>) -> Option<CommandEvent> {
        if let Some(mut owned) = self.owned.take() {
            // Retire the exact old owner before waking either its waiter or a
            // new command. Owned effects never enter the unscoped legacy bus.
            owned.transaction.finish();
            if let Some(reply) = owned.reply.take() {
                let _ = reply.send(result);
            }
            return None;
        }
        if let Some(reply) = self.reply {
            let _ = reply.send(result.clone());
        }
        Some(match result {
            Ok(result) => CommandEvent::Completed {
                command: self.command,
                result,
            },
            Err(error) => CommandEvent::Failed {
                command: self.command,
                error,
            },
        })
    }
}

struct Driver {
    session: CommandSession,
    pending: Option<Pending>,
    generation: u64,
    quarantined_acks: BTreeSet<u32>,
    ambiguous_all: bool,
    events: mpsc::Sender<CommandEvent>,
    dropped_events: Arc<AtomicU64>,
}

impl Driver {
    fn publish(&self, event: CommandEvent) {
        if self.events.try_send(event).is_err() {
            let _ = self
                .dropped_events
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    Some(count.saturating_add(1))
                });
        }
    }

    fn observe_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.quarantined_acks.clear();
            self.ambiguous_all = false;
        }
    }

    fn ack_is_quarantined(&self, command: MavCmd) -> bool {
        self.ambiguous_all || self.quarantined_acks.contains(&(command as u32))
    }

    fn finish(&mut self, result: Result<CommandResult, CommandError>) {
        self.session.cancel();
        let Some(pending) = self.pending.take() else {
            return;
        };
        if result.is_err() && pending.generation == self.generation && pending.dispatches > 0 {
            // The wire protocol has no operation ID and IN_PROGRESS may produce
            // later terminal updates. Quarantine this MAV_CMD for the whole
            // generation: normal reuse is refused; an emergency is still sent,
            // but none of its ACKs are trusted.
            let key = pending.command as u32;
            if self.quarantined_acks.contains(&key) {
                // Already quarantined for this generation.
            } else if self.quarantined_acks.len() < MAX_UNRESOLVED_COMMANDS {
                self.quarantined_acks.insert(key);
            } else {
                self.ambiguous_all = true;
                self.quarantined_acks.clear();
            }
        }
        if let Some(event) = pending.finish(result) {
            self.publish(event);
        }
    }

    fn begin(&mut self, request: CommandCommand, handle: &ConnectionHandle) -> Vec<MavMessage> {
        let (command, params, priority, reply, owned) = match request {
            CommandCommand::Send {
                command,
                params,
                priority,
            } => (command, params, priority, None, None),
            CommandCommand::SendAwait {
                command,
                params,
                priority,
                reply,
            } => (command, params, priority, Some(reply), None),
            CommandCommand::SendOwned(request) => (
                request.command,
                request.params,
                command_priority(request.command),
                None,
                Some(request),
            ),
        };
        let pending = Pending {
            command,
            generation: owned.as_ref().map_or(handle.generation(), |owned| {
                owned.transaction.generation().get()
            }),
            started_at: Instant::now(),
            reply,
            owned,
            dispatches: 0,
        };
        let admission = pending.check(handle).and_then(|_| {
            if self.ack_is_quarantined(command) && !priority.is_emergency() {
                return Err(CommandError::AmbiguousAck);
            }
            match self.session.in_flight_priority() {
                Some(Priority::Normal) if priority.is_emergency() => Ok(()),
                Some(_) => Err(CommandError::Busy),
                None => Ok(()),
            }
        });
        if let Err(error) = admission {
            if let Some(event) = pending.finish(Err(error)) {
                self.publish(event);
            }
            return Vec::new();
        }
        if self.pending.is_some() {
            self.finish(Err(CommandError::Superseded));
        }
        match self.session.begin_with_priority(command, params, priority) {
            Ok((events, frames)) => {
                let legacy = pending.owned.is_none();
                self.pending = Some(pending);
                if legacy {
                    for event in events {
                        self.publish(event);
                    }
                }
                frames
            }
            Err(error) => {
                if let Some(event) = pending.finish(Err(error)) {
                    self.publish(event);
                }
                Vec::new()
            }
        }
    }

    fn receive(&mut self, message: &MessageEnvelope, handle: &ConnectionHandle) {
        let MavMessage::COMMAND_ACK(ack) = &message.message else {
            return;
        };
        // MAVLink 2 ACK extensions address the GCS; zero remains the supported
        // MAVLink 1/broadcast value. Source routing is checked by the session.
        if (ack.target_system != 0 && ack.target_system != handle.config().system_id)
            || (ack.target_component != 0 && ack.target_component != handle.config().component_id)
        {
            return;
        }
        if message.header.system_id != self.session.target_system()
            || message.header.component_id != self.session.target_component()
        {
            return;
        }
        if self.ack_is_quarantined(ack.command) {
            return;
        }
        let Some(pending) = &self.pending else {
            return;
        };
        if ack.command != pending.command
            || message.received_at < pending.started_at
            || handle
                .connected_since_snapshot()
                .is_none_or(|opened| message.received_at < opened)
        {
            return;
        }
        let (events, _) = self.session.handle(&message.header, &message.message);
        for event in events {
            if let CommandEvent::Completed { result, .. } = event {
                self.finish(Ok(result));
            }
        }
    }

    fn tick(&mut self) -> Vec<MavMessage> {
        let (events, frames) = self.session.retransmit_due(Instant::now());
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.owned.is_none())
        {
            for event in events {
                self.publish(event);
            }
        }
        if let Some((_, error)) = self.session.take_timeout_failure() {
            self.finish(Err(error));
        }
        frames
    }
}

async fn dispatch(
    handle: &ConnectionHandle,
    pending: &mut Pending,
    frames: Vec<MavMessage>,
) -> Result<(), CommandError> {
    for frame in frames {
        pending.check(handle)?;
        pending.dispatches = pending.dispatches.saturating_add(1);
        let deadline = tokio::time::Instant::now() + WRITE_TIMEOUT;
        let sent = if let Some(owned) = &pending.owned {
            let transaction = &owned.transaction;
            tokio::select! {
                biased;
                error = transaction.stopped() => return Err(error),
                result = async {
                    transaction.check(handle)?;
                    transaction.mark_dispatched();
                    handle.send_in_generation(frame, pending.generation, deadline.min(transaction.deadline))
                        .await.map_err(|_| CommandError::LinkLost)
                } => result,
            }
        } else {
            handle
                .send_in_generation(frame, pending.generation, deadline)
                .await
                .map_err(|_| CommandError::LinkLost)
        };
        if sent.is_err() {
            pending.check(handle)?;
        }
        sent?;
    }
    Ok(())
}

async fn owned_stop(transaction: Option<CommandTransaction>) -> CommandError {
    match transaction {
        Some(transaction) => transaction.stopped().await,
        None => std::future::pending().await,
    }
}

/// One command session/transport owner, with independent operation replies.
async fn run(
    handle: ConnectionHandle,
    mut cmd_rx: mpsc::Receiver<CommandCommand>,
    evt_tx: mpsc::Sender<CommandEvent>,
    dropped_events: Arc<AtomicU64>,
    target_sys: u8,
    target_comp: u8,
) {
    let mut driver = Driver {
        session: CommandSession::new(target_sys, target_comp),
        pending: None,
        generation: handle.generation(),
        quarantined_acks: BTreeSet::new(),
        ambiguous_all: false,
        events: evt_tx,
        dropped_events,
    };
    // Only COMMAND_ACK frames wake the service (issue #20); heartbeat/link
    // lifecycle events still pass through the router.
    let mut events_rx = handle.subscribe_route(MessageRoute::messages(&[COMMAND_ACK_DATA::ID]));
    let mut ticker = tokio::time::interval(RETRANSMIT_TICK);
    loop {
        driver.observe_generation(handle.generation());
        if let Some(error) = driver
            .pending
            .as_ref()
            .and_then(|pending| pending.check(&handle).err())
        {
            driver.finish(Err(error));
        }
        if handle.is_shutdown_requested() {
            driver.finish(Err(CommandError::LinkLost));
            return;
        }
        let transaction = driver.pending.as_ref().and_then(|pending| {
            pending
                .owned
                .as_ref()
                .map(|owned| owned.transaction.clone())
        });
        let frames = tokio::select! {
            error = owned_stop(transaction) => {
                driver.finish(Err(error));
                Vec::new()
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else {
                    driver.finish(Err(CommandError::ServiceStopped));
                    return;
                };
                driver.begin(cmd, &handle)
            }
            ev = events_rx.recv() => {
                match ev {
                    Ok(ConnectionEvent::Message(m)) => {
                        if let Some(error) = driver.pending.as_ref().and_then(|pending| pending.check(&handle).err()) {
                            driver.finish(Err(error));
                        } else {
                            driver.receive(&m, &handle);
                        }
                    }
                    Ok(ConnectionEvent::HeartbeatLost { .. } | ConnectionEvent::LinkError(_) | ConnectionEvent::Failed(_)) => {
                        driver.finish(Err(CommandError::LinkLost));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        driver.finish(Err(CommandError::ServiceStopped));
                        return;
                    }
                    _ => {}
                }
                Vec::new()
            }
            _ = ticker.tick() => driver.tick(),
        };
        if !frames.is_empty() {
            if let Some(pending) = &mut driver.pending {
                if let Err(error) = dispatch(&handle, pending, frames).await {
                    driver.finish(Err(error));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{OperationId, TransportGeneration};

    #[tokio::test]
    async fn owned_budget_includes_waiting_for_a_full_command_queue() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(1);
        let service = CommandService {
            cmd_tx,
            dropped_events: Arc::new(AtomicU64::new(0)),
        };
        service
            .send(MavCmd::MAV_CMD_DO_REPOSITION, [0.0; 7])
            .await
            .expect("fill queue");
        let transaction = CommandTransaction::new(
            OperationId::fresh().expect("ID"),
            TransportGeneration::new(1).expect("generation"),
            Duration::from_millis(50),
        )
        .expect("transaction");
        let started = Instant::now();
        assert_eq!(
            service
                .send_owned(transaction.clone(), MavCmd::MAV_CMD_DO_SET_HOME, [0.0; 7])
                .await,
            Err(CommandError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!transaction.may_have_dispatched());
        assert!(service
            .cancel_owned(&transaction)
            .await
            .expect("queued request retired"));
        assert!(matches!(
            cmd_rx.recv().await,
            Some(CommandCommand::Send { .. })
        ));
        assert!(
            cmd_rx.try_recv().is_err(),
            "expired owned intent never entered the queue"
        );
    }
}
