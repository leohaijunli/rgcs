//! Command service (issue #5).
//!
//! A single spawned task owns a [`CommandSession`] on a live MAVLink
//! connection: it accepts [`CommandCommand::Send`] enqueuements, feeds inbound
//! frames to the session, retransmits unacked commands on a fixed tick, and
//! emits [`CommandEvent`]s to its subscriber. It lives in `core` so the
//! headless server can reuse it; the desktop app only adapts the events onto
//! its UI bus.

use std::time::{Duration, Instant};

use ::mavlink::common::{MavCmd, COMMAND_ACK_DATA};
use ::mavlink::MessageData;
use tokio::sync::mpsc;

use super::{CommandError, CommandEvent, CommandSession};
use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::MessageRoute;

/// Retransmission tick period of the service loop.
pub const RETRANSMIT_TICK: Duration = Duration::from_millis(200);

/// Commands accepted by the service task.
#[derive(Debug)]
pub enum CommandCommand {
    /// Send a `COMMAND_LONG` and track it until its ack (or failure).
    Send { command: MavCmd, params: [f32; 7] },
}

/// Handle to the running command service.
#[derive(Clone)]
pub struct CommandService {
    cmd_tx: mpsc::Sender<CommandCommand>,
}

impl CommandService {
    /// Spawn the command service on the given link.
    ///
    /// Returns the handle and the event stream. The task stops when the
    /// handle is dropped, the connection worker shuts down, or the event
    /// subscriber goes away.
    pub fn spawn(
        handle: ConnectionHandle,
        target_sys: u8,
        target_comp: u8,
    ) -> (Self, mpsc::Receiver<CommandEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (evt_tx, evt_rx) = mpsc::channel(16);
        tokio::spawn(run(handle, cmd_rx, evt_tx, target_sys, target_comp));
        (Self { cmd_tx }, evt_rx)
    }

    /// Enqueue a command; returns `Err` if the service stopped.
    pub async fn send(&self, command: MavCmd, params: [f32; 7]) -> Result<(), CommandError> {
        self.cmd_tx
            .send(CommandCommand::Send { command, params })
            .await
            .map_err(|_| CommandError::ServiceStopped)
    }
}

/// Service task: drives the session from commands, inbound frames, and the
/// retransmission ticker.
async fn run(
    handle: ConnectionHandle,
    mut cmd_rx: mpsc::Receiver<CommandCommand>,
    evt_tx: mpsc::Sender<CommandEvent>,
    target_sys: u8,
    target_comp: u8,
) {
    let mut session = CommandSession::new(target_sys, target_comp);
    // Only COMMAND_ACK frames wake the service (issue #20); heartbeat/link
    // lifecycle events still pass through the router.
    let mut events_rx = handle.subscribe_route(MessageRoute::messages(&[COMMAND_ACK_DATA::ID]));
    let mut ticker = tokio::time::interval(RETRANSMIT_TICK);

    loop {
        let (events, frames) = tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return; };
                match cmd {
                    CommandCommand::Send { command, params } => {
                        match session.begin(command, params) {
                            Ok(out) => out,
                            Err(e) => (vec![CommandEvent::Failed { command, error: e }], Vec::new()),
                        }
                    }
                }
            }
            ev = events_rx.recv() => {
                match ev {
                    Ok(ConnectionEvent::Message(m)) => session.handle(&m.header, &m.message),
                    Ok(ConnectionEvent::HeartbeatLost { .. }) | Ok(ConnectionEvent::Failed(_)) => {
                        (session.cancel(), Vec::new())
                    }
                    Ok(_) => (Vec::new(), Vec::new()),
                    // A lagged bus is not a shutdown; keep serving.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        (Vec::new(), Vec::new())
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
            _ = ticker.tick() => {
                let (mut events, frames) = session.retransmit_due(Instant::now());
                if let Some((command, error)) = session.take_timeout_failure() {
                    events.push(CommandEvent::Failed { command, error });
                }
                (events, frames)
            }
        };

        for e in events {
            if evt_tx.send(e).await.is_err() {
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
