//! Mission service (issue #18).
//!
//! A single spawned task owns a [`MissionProtocol`] on a live MAVLink
//! connection: it accepts [`MissionCommand`]s, feeds inbound frames to the
//! protocol, drives the retransmission ticker, and emits
//! [`MissionServiceEvent`]s to its subscriber. It lives in `core` so the
//! headless server can reuse it (ADR-001); the desktop app only adapts the
//! events onto its UI bus.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use super::operation_types::{MissionProtocolError, MissionResult};
use super::protocol::{
    MissionEvent, MissionOperation, MissionProtocol, MISSION_MESSAGE_IDS, RETRY_TICK,
};
use super::transaction::OwnedWait;
use super::types::MissionItem;
use super::MissionTransaction;
use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::message::{MavMessage, MessageEnvelope};
use crate::mavlink::router::MessageRoute;
use crate::runtime::{OperationId, TransportGeneration};

const COMMAND_CAPACITY: usize = 8;
const EVENT_CAPACITY: usize = 16;
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);

/// MAVLink identities used for the mission exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionIds {
    /// GCS system id.
    pub self_system: u8,
    /// GCS component id.
    pub self_component: u8,
    /// Flight-controller system id.
    pub target_system: u8,
    /// Flight-controller component id.
    pub target_component: u8,
}

/// Commands accepted by the service task.
#[derive(Debug, Clone)]
pub enum MissionCommand {
    /// Start uploading the given mission.
    Upload(Vec<MissionItem>),
    /// Start downloading the FC mission.
    Download,
    /// Clear the FC mission.
    Clear,
    /// Set the active waypoint.
    SetCurrent(u16),
}

/// Events published by the service to its adapter.
#[derive(Debug, Clone, PartialEq)]
pub enum MissionServiceEvent {
    /// Protocol-level progress or terminal event.
    Protocol(MissionEvent),
    /// A full plan was fetched from the FC (emitted after a completed
    /// download, once the protocol has assembled every item).
    PlanDownloaded(Vec<MissionItem>),
    /// Owned progress does not use the legacy unscoped terminal event bus.
    OwnedProgress {
        operation_id: OperationId,
        generation: TransportGeneration,
        sent: u16,
        total: u16,
    },
    /// A verified source/current-generation observation, separate from command completion.
    CurrentObserved {
        seq: u16,
        generation: TransportGeneration,
        received_at: Instant,
    },
}

/// Errors returned when enqueuing a command.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MissionServiceError {
    /// The service task has stopped (link dropped or handle gone).
    #[error("mission service stopped")]
    Stopped,
}

/// Handle to the running mission service.
#[derive(Clone)]
pub struct MissionService {
    cmd_tx: mpsc::Sender<QueuedCommand>,
    dropped_events: Arc<AtomicU64>,
}

enum QueuedCommand {
    Legacy(MissionCommand),
    Owned(Box<OwnedMission>),
}

struct OwnedMission {
    action: Option<MissionCommand>,
    transaction: MissionTransaction,
    reply: Option<oneshot::Sender<Result<MissionResult, MissionProtocolError>>>,
}

impl Drop for OwnedMission {
    fn drop(&mut self) {
        self.transaction.finish();
    }
}

impl MissionService {
    /// Spawn the mission service on the given link.
    ///
    /// Returns the handle and the event stream. The task stops when the
    /// handle is dropped or the connection shuts down. Event consumers cannot
    /// stall the protocol, its owned cancellation or its deadline.
    pub fn spawn(
        handle: ConnectionHandle,
        ids: MissionIds,
    ) -> (Self, mpsc::Receiver<MissionServiceEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (evt_tx, evt_rx) = mpsc::channel(EVENT_CAPACITY);
        let dropped_events = Arc::new(AtomicU64::new(0));
        tokio::spawn(run(handle, cmd_rx, evt_tx, dropped_events.clone(), ids));
        (
            Self {
                cmd_tx,
                dropped_events,
            },
            evt_rx,
        )
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    pub async fn upload_owned(
        &self,
        transaction: MissionTransaction,
        items: Vec<MissionItem>,
    ) -> Result<u16, MissionProtocolError> {
        super::types::validate_upload(&items)?;
        match self
            .send_owned(transaction, MissionCommand::Upload(items))
            .await?
        {
            MissionResult::Uploaded { count } => Ok(count),
            _ => Err(unexpected_result()),
        }
    }

    pub async fn download_owned(
        &self,
        transaction: MissionTransaction,
    ) -> Result<Vec<MissionItem>, MissionProtocolError> {
        match self
            .send_owned(transaction, MissionCommand::Download)
            .await?
        {
            MissionResult::Downloaded { items } => Ok(items),
            _ => Err(unexpected_result()),
        }
    }

    pub async fn clear_owned(
        &self,
        transaction: MissionTransaction,
    ) -> Result<(), MissionProtocolError> {
        match self.send_owned(transaction, MissionCommand::Clear).await? {
            MissionResult::Cleared => Ok(()),
            _ => Err(unexpected_result()),
        }
    }

    pub async fn set_current_owned(
        &self,
        transaction: MissionTransaction,
        seq: u16,
    ) -> Result<u16, MissionProtocolError> {
        match self
            .send_owned(transaction, MissionCommand::SetCurrent(seq))
            .await?
        {
            MissionResult::Current { seq: actual } if actual == seq => Ok(actual),
            _ => Err(unexpected_result()),
        }
    }

    pub async fn cancel_owned(
        &self,
        transaction: &MissionTransaction,
    ) -> Result<bool, MissionProtocolError> {
        transaction.cancel_and_wait().await
    }

    async fn send_owned(
        &self,
        transaction: MissionTransaction,
        action: MissionCommand,
    ) -> Result<MissionResult, MissionProtocolError> {
        transaction.submit()?;
        let _wait = OwnedWait(transaction.clone());
        let (reply, response) = oneshot::channel();
        let request = QueuedCommand::Owned(Box::new(OwnedMission {
            action: Some(action),
            transaction: transaction.clone(),
            reply: Some(reply),
        }));
        tokio::select! {
            biased;
            error = transaction.stopped() => Err(error),
            result = async {
                self.cmd_tx.send(request).await.map_err(|_| MissionProtocolError::Stopped)?;
                response.await.map_err(|_| MissionProtocolError::Stopped)?
            } => result,
        }
    }

    /// Start uploading the given mission.
    pub async fn upload(&self, items: Vec<MissionItem>) -> Result<(), MissionServiceError> {
        self.send(MissionCommand::Upload(items)).await
    }

    /// Start downloading the FC mission.
    pub async fn download(&self) -> Result<(), MissionServiceError> {
        self.send(MissionCommand::Download).await
    }

    /// Clear the FC mission.
    pub async fn clear(&self) -> Result<(), MissionServiceError> {
        self.send(MissionCommand::Clear).await
    }

    /// Set the active waypoint.
    pub async fn set_current(&self, seq: u16) -> Result<(), MissionServiceError> {
        self.send(MissionCommand::SetCurrent(seq)).await
    }

    async fn send(&self, cmd: MissionCommand) -> Result<(), MissionServiceError> {
        self.cmd_tx
            .send(QueuedCommand::Legacy(cmd))
            .await
            .map_err(|_| MissionServiceError::Stopped)
    }
}

fn unexpected_result() -> MissionProtocolError {
    MissionProtocolError::Unexpected {
        message: "reply kind did not match the submitted mission action".into(),
    }
}

struct Pending {
    operation: MissionOperation,
    count: u16,
    generation: u64,
    started_at: Instant,
    dispatched: bool,
    owned: Option<Box<OwnedMission>>,
}

impl Pending {
    fn check(
        &self,
        handle: &ConnectionHandle,
        ids: MissionIds,
    ) -> Result<(), MissionProtocolError> {
        if let Some(owned) = &self.owned {
            owned.transaction.check(handle)?;
        }
        if self.generation == 0
            || self.generation != handle.generation()
            || handle.target_identity().is_some_and(|identity| {
                identity.system_id != ids.target_system
                    || identity.component_id != ids.target_component
            })
        {
            return Err(MissionProtocolError::TargetChanged);
        }
        if handle.is_shutdown_requested() {
            return Err(MissionProtocolError::Stopped);
        }
        Ok(())
    }
}

struct Driver {
    protocol: MissionProtocol,
    ids: MissionIds,
    pending: Option<Pending>,
    generation: u64,
    quarantined: bool,
    events: mpsc::Sender<MissionServiceEvent>,
    dropped_events: Arc<AtomicU64>,
}

impl Driver {
    fn publish(&self, event: MissionServiceEvent) {
        if self.events.try_send(event).is_err() {
            let _ = self
                .dropped_events
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    Some(count.saturating_add(1))
                });
        }
    }

    fn observe_generation(&mut self, generation: u64) {
        if generation != self.generation {
            self.finish(Err(MissionProtocolError::TargetChanged));
            self.generation = generation;
            self.quarantined = false;
        }
    }

    fn complete(&mut self, operation: MissionOperation) {
        let Some(pending) = &self.pending else {
            return;
        };
        if operation != pending.operation {
            self.finish(Err(unexpected_result()));
            return;
        }
        let result = match operation {
            MissionOperation::Upload => MissionResult::Uploaded {
                count: pending.count,
            },
            MissionOperation::Download => MissionResult::Downloaded {
                items: self.protocol.take_downloaded(),
            },
            MissionOperation::ClearAll => MissionResult::Cleared,
            MissionOperation::SetCurrent(seq) => MissionResult::Current { seq },
        };
        self.finish(Ok(result));
    }

    fn finish(&mut self, result: Result<MissionResult, MissionProtocolError>) {
        self.protocol.abort();
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        if result.is_err() && pending.dispatched && pending.generation == self.generation {
            // ACKs do not identify upload vs clear or a Runtime operation. A
            // possibly dispatched abandoned exchange cannot authorize reuse.
            self.quarantined = true;
        }
        if result.is_err() {
            let _ = self.protocol.take_downloaded();
        }
        if let Some(mut owned) = pending.owned.take() {
            owned.transaction.finish();
            if let Some(reply) = owned.reply.take() {
                let _ = reply.send(result);
            }
            return;
        }
        match result {
            Ok(MissionResult::Downloaded { items }) => {
                self.publish(MissionServiceEvent::Protocol(MissionEvent::Completed(
                    MissionOperation::Download,
                )));
                self.publish(MissionServiceEvent::PlanDownloaded(items));
            }
            Ok(_) => self.publish(MissionServiceEvent::Protocol(MissionEvent::Completed(
                pending.operation,
            ))),
            Err(error) => self.publish(MissionServiceEvent::Protocol(MissionEvent::Failed(
                legacy_error(error),
            ))),
        }
    }

    fn begin(&mut self, request: QueuedCommand, handle: &ConnectionHandle) -> Vec<MavMessage> {
        let (action, owned) = match request {
            QueuedCommand::Legacy(action) => (action, None),
            QueuedCommand::Owned(mut request) => {
                let Some(action) = request.action.take() else {
                    return Vec::new();
                };
                (action, Some(request))
            }
        };
        let operation = match &action {
            MissionCommand::Upload(_) => MissionOperation::Upload,
            MissionCommand::Download => MissionOperation::Download,
            MissionCommand::Clear => MissionOperation::ClearAll,
            MissionCommand::SetCurrent(seq) => MissionOperation::SetCurrent(*seq),
        };
        let count = match &action {
            MissionCommand::Upload(items) => u16::try_from(items.len()).unwrap_or(0),
            _ => 0,
        };
        let mut pending = Pending {
            operation,
            count,
            generation: owned.as_ref().map_or(handle.generation(), |request| {
                request.transaction.generation().get()
            }),
            started_at: Instant::now(),
            dispatched: false,
            owned,
        };
        let admission = pending.check(handle, self.ids).and_then(|_| {
            if self.pending.is_some() {
                return Err(MissionProtocolError::Busy);
            }
            if self.quarantined {
                return Err(MissionProtocolError::AmbiguousReply);
            }
            if let MissionCommand::Upload(items) = &action {
                super::types::validate_upload(items)?;
            }
            Ok(())
        });
        if let Err(error) = admission {
            if let Some(mut owned) = pending.owned.take() {
                owned.transaction.finish();
                if let Some(reply) = owned.reply.take() {
                    let _ = reply.send(Err(error));
                }
            } else {
                self.publish(MissionServiceEvent::Protocol(MissionEvent::Failed(
                    legacy_error(error),
                )));
            }
            return Vec::new();
        }
        let frames = match action {
            MissionCommand::Upload(items) => match self.protocol.begin_upload(items) {
                Ok(frames) => frames,
                Err(error) => {
                    self.pending = Some(pending);
                    self.finish(Err(error.into()));
                    return Vec::new();
                }
            },
            MissionCommand::Download => self.protocol.begin_download(),
            MissionCommand::Clear => self.protocol.begin_clear(),
            MissionCommand::SetCurrent(seq) => self.protocol.begin_set_current(seq),
        };
        self.pending = Some(pending);
        frames
    }

    fn receive(
        &mut self,
        message: &MessageEnvelope,
        handle: &ConnectionHandle,
    ) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        if !self.protocol.accepts(&message.header, &message.message)
            || handle
                .connected_since_snapshot()
                .is_none_or(|opened| message.received_at < opened)
        {
            return (Vec::new(), Vec::new());
        }
        if let MavMessage::MISSION_CURRENT(current) = &message.message {
            if let Ok(generation) = TransportGeneration::new(self.generation) {
                self.publish(MissionServiceEvent::CurrentObserved {
                    seq: current.seq,
                    generation,
                    received_at: message.received_at,
                });
            }
            if self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.owned.is_none())
            {
                self.publish(MissionServiceEvent::Protocol(
                    MissionEvent::CurrentChanged { seq: current.seq },
                ));
            }
        }
        let Some(pending) = &self.pending else {
            return (Vec::new(), Vec::new());
        };
        if message.received_at < pending.started_at || self.quarantined {
            return (Vec::new(), Vec::new());
        }
        self.protocol.handle(&message.header, &message.message)
    }

    fn process(&mut self, events: Vec<MissionEvent>) {
        for event in events {
            match event {
                MissionEvent::Progress {
                    operation,
                    sent,
                    total,
                } => {
                    if let Some(pending) = &self.pending {
                        if let Some(owned) = &pending.owned {
                            self.publish(MissionServiceEvent::OwnedProgress {
                                operation_id: owned.transaction.id().clone(),
                                generation: owned.transaction.generation(),
                                sent,
                                total,
                            });
                        } else {
                            self.publish(MissionServiceEvent::Protocol(MissionEvent::Progress {
                                operation,
                                sent,
                                total,
                            }));
                        }
                    }
                }
                MissionEvent::Completed(operation) => self.complete(operation),
                MissionEvent::Failed(error) => self.finish(Err(error.into())),
                MissionEvent::CurrentChanged { .. } => {} // Published once, with source timing, on receive.
            }
        }
    }
}

fn legacy_error(error: MissionProtocolError) -> super::MissionError {
    use super::MissionError;
    match error {
        MissionProtocolError::AckDenied { ack_type } => MissionError::AckDenied(ack_type),
        MissionProtocolError::SeqMismatch { expected, got } => {
            MissionError::SeqMismatch { expected, got }
        }
        MissionProtocolError::CountMismatch { expected, got } => {
            MissionError::CountMismatch { expected, got }
        }
        MissionProtocolError::UnsupportedFrame { frame } => MissionError::UnsupportedFrame(frame),
        MissionProtocolError::NoItems => MissionError::NoItems,
        MissionProtocolError::RetriesExhausted => MissionError::RetriesExhausted,
        MissionProtocolError::Timeout { .. } | MissionProtocolError::TimedOut => {
            MissionError::Timeout("mission")
        }
        other => MissionError::InvalidRequest {
            message: other.to_string(),
        },
    }
}

async fn dispatch(
    handle: &ConnectionHandle,
    pending: &mut Pending,
    ids: MissionIds,
    frames: Vec<MavMessage>,
) -> Result<(), MissionProtocolError> {
    for frame in frames {
        pending.check(handle, ids)?;
        let deadline = tokio::time::Instant::now() + WRITE_TIMEOUT;
        pending.dispatched = true;
        if let Some(owned) = &pending.owned {
            let transaction = &owned.transaction;
            tokio::select! {
                biased;
                error = transaction.stopped() => return Err(error),
                result = async {
                    transaction.check(handle)?;
                    transaction.mark_dispatched();
                    handle.send_in_generation(frame, pending.generation, deadline.min(transaction.deadline)).await
                        .map_err(|_| MissionProtocolError::Stopped)
                } => result?,
            }
        } else {
            handle
                .send_in_generation(frame, pending.generation, deadline)
                .await
                .map_err(|_| MissionProtocolError::Stopped)?;
        }
    }
    Ok(())
}

async fn owned_stop(transaction: Option<MissionTransaction>) -> MissionProtocolError {
    match transaction {
        Some(transaction) => transaction.stopped().await,
        None => std::future::pending().await,
    }
}

/// One existing protocol/transport worker, independent owned replies and bounded observation.
async fn run(
    handle: ConnectionHandle,
    mut cmd_rx: mpsc::Receiver<QueuedCommand>,
    evt_tx: mpsc::Sender<MissionServiceEvent>,
    dropped_events: Arc<AtomicU64>,
    ids: MissionIds,
) {
    let mut driver = Driver {
        protocol: MissionProtocol::new(
            ids.self_system,
            ids.self_component,
            ids.target_system,
            ids.target_component,
        ),
        ids,
        pending: None,
        generation: handle.generation(),
        quarantined: false,
        events: evt_tx,
        dropped_events,
    };
    let mut events_rx = handle.subscribe_route(MessageRoute::messages(MISSION_MESSAGE_IDS));
    let mut tick = tokio::time::interval(RETRY_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        driver.observe_generation(handle.generation());
        if let Some(error) = driver
            .pending
            .as_ref()
            .and_then(|pending| pending.check(&handle, ids).err())
        {
            driver.finish(Err(error));
        }
        if handle.is_shutdown_requested() {
            driver.finish(Err(MissionProtocolError::Stopped));
            return;
        }
        let transaction = driver.pending.as_ref().and_then(|pending| {
            pending
                .owned
                .as_ref()
                .map(|owned| owned.transaction.clone())
        });
        let (events, frames) = tokio::select! {
            error = owned_stop(transaction) => { driver.finish(Err(error)); (Vec::new(), Vec::new()) },
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { driver.finish(Err(MissionProtocolError::Stopped)); return; };
                (Vec::new(), driver.begin(cmd, &handle))
            }
            ev = events_rx.recv() => {
                match ev {
                    Ok(ConnectionEvent::Message(message)) => {
                        driver.observe_generation(handle.generation());
                        if let Some(error) = driver.pending.as_ref().and_then(|pending| pending.check(&handle, ids).err()) {
                            driver.finish(Err(error)); (Vec::new(), Vec::new())
                        } else { driver.receive(&message, &handle) }
                    }
                    Ok(ConnectionEvent::HeartbeatLost { .. } | ConnectionEvent::LinkError(_) | ConnectionEvent::Failed(_)) => {
                        driver.finish(Err(MissionProtocolError::Stopped)); (Vec::new(), Vec::new())
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => { driver.finish(Err(MissionProtocolError::Stopped)); return; }
                    _ => (Vec::new(), Vec::new()),
                }
            }
            _ = tick.tick() => driver.protocol.on_tick(Instant::now()),
        };
        if !frames.is_empty() {
            if let Some(pending) = &mut driver.pending {
                if let Err(error) = dispatch(&handle, pending, ids, frames).await {
                    driver.finish(Err(error));
                    continue;
                }
            }
        }
        driver.process(events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_owned_deadline_includes_a_full_queue() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(1);
        let service = MissionService {
            cmd_tx,
            dropped_events: Arc::new(AtomicU64::new(0)),
        };
        service.download().await.expect("fill queue");
        let transaction = MissionTransaction::new(
            OperationId::fresh().expect("id"),
            TransportGeneration::new(1).expect("generation"),
            Duration::from_millis(50),
        )
        .expect("transaction");
        assert_eq!(
            service.clear_owned(transaction.clone()).await,
            Err(MissionProtocolError::TimedOut)
        );
        assert!(!transaction.may_have_dispatched());
        assert!(service
            .cancel_owned(&transaction)
            .await
            .expect("local receipt"));
        assert!(matches!(
            cmd_rx.recv().await,
            Some(QueuedCommand::Legacy(MissionCommand::Download))
        ));
        assert!(cmd_rx.try_recv().is_err());
    }
}
