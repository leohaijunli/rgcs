//! Mission service (issue #18).
//!
//! A single spawned task owns a [`MissionProtocol`] on a live MAVLink
//! connection: it accepts [`MissionCommand`]s, feeds inbound frames to the
//! protocol, drives the retransmission ticker, and emits
//! [`MissionServiceEvent`]s to its subscriber. It lives in `core` so the
//! headless server can reuse it (ADR-001); the desktop app only adapts the
//! events onto its UI bus.

use std::time::Instant;

use thiserror::Error;
use tokio::sync::mpsc;

use super::protocol::{
    MissionEvent, MissionOperation, MissionProtocol, MISSION_MESSAGE_IDS, RETRY_TICK,
};
use super::types::MissionItem;
use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::MessageRoute;

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
    cmd_tx: mpsc::Sender<MissionCommand>,
}

impl MissionService {
    /// Spawn the mission service on the given link.
    ///
    /// Returns the handle and the event stream. The task stops when the
    /// handle is dropped, the connection worker fails, or the event
    /// subscriber goes away.
    pub fn spawn(
        handle: ConnectionHandle,
        ids: MissionIds,
    ) -> (Self, mpsc::Receiver<MissionServiceEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (evt_tx, evt_rx) = mpsc::channel(16);
        tokio::spawn(run(handle, cmd_rx, evt_tx, ids));
        (Self { cmd_tx }, evt_rx)
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
            .send(cmd)
            .await
            .map_err(|_| MissionServiceError::Stopped)
    }
}

/// Service task: drives the protocol from commands, inbound frames, and the
/// retransmission ticker.
async fn run(
    handle: ConnectionHandle,
    mut cmd_rx: mpsc::Receiver<MissionCommand>,
    evt_tx: mpsc::Sender<MissionServiceEvent>,
    ids: MissionIds,
) {
    let mut proto = MissionProtocol::new(
        ids.self_system,
        ids.self_component,
        ids.target_system,
        ids.target_component,
    );
    // Only mission-protocol frames wake the service (issue #20); the protocol
    // still validates the source and target (issues.md #9).
    let mut events_rx = handle.subscribe_route(MessageRoute::messages(MISSION_MESSAGE_IDS));
    let mut tick = tokio::time::interval(RETRY_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let (events, frames) = tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return; };
                match cmd {
                    MissionCommand::Upload(items) => match proto.begin_upload(items) {
                        Ok(frames) => (Vec::new(), frames),
                        Err(e) => (vec![MissionEvent::Failed(e)], Vec::new()),
                    },
                    MissionCommand::Download => (Vec::new(), proto.begin_download()),
                    MissionCommand::Clear => (Vec::new(), proto.begin_clear()),
                    MissionCommand::SetCurrent(seq) => (Vec::new(), proto.begin_set_current(seq)),
                }
            }
            ev = events_rx.recv() => {
                match ev {
                    Ok(ConnectionEvent::Message(m)) => proto.handle(&m.header, &m.message),
                    Ok(ConnectionEvent::Failed(_)) => return,
                    Ok(_) => (Vec::new(), Vec::new()),
                    // A lagged bus is not a shutdown; keep serving.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        (Vec::new(), Vec::new())
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
            _ = tick.tick() => proto.on_tick(Instant::now()),
        };

        let mut completed_download = false;
        for e in &events {
            if let MissionEvent::Completed(MissionOperation::Download) = e {
                completed_download = true;
            }
            if evt_tx
                .send(MissionServiceEvent::Protocol(e.clone()))
                .await
                .is_err()
            {
                return; // subscriber gone
            }
        }
        // A completed download carries the assembled plan; publish the items
        // after the terminal event so the UI has already matched the operation.
        if completed_download {
            let items = proto.take_downloaded();
            if evt_tx
                .send(MissionServiceEvent::PlanDownloaded(items))
                .await
                .is_err()
            {
                return; // subscriber gone
            }
        }
        for f in frames {
            if handle.send(f).await.is_err() {
                return; // link gone; the service stops with the connection
            }
        }
    }
}
