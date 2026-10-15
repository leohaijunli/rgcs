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
use tokio::sync::{mpsc, oneshot};

use super::{
    command_priority, CommandError, CommandEvent, CommandResult, CommandSession, Priority,
};
use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::MessageRoute;

/// Retransmission tick period of the service loop.
pub const RETRANSMIT_TICK: Duration = Duration::from_millis(200);

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
    // The pending send_await reply and the command it belongs to. One at
    // most: a send_await arriving while another command is in flight is
    // answered with Busy immediately.
    let mut pending: Option<(MavCmd, oneshot::Sender<Result<CommandResult, CommandError>>)> = None;

    loop {
        let (events, frames) = tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return; };
                match cmd {
                    CommandCommand::Send { command, params, priority } => {
                        match session.begin_with_priority(command, params, priority) {
                            Ok(out) => out,
                            Err(e) => (vec![CommandEvent::Failed { command, error: e }], Vec::new()),
                        }
                    }
                    CommandCommand::SendAwait { command, params, priority, reply } => {
                        match session.begin_with_priority(command, params, priority) {
                            Ok(out) => {
                                pending = Some((command, reply));
                                out
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e.clone()));
                                (vec![CommandEvent::Failed { command, error: e }], Vec::new())
                            }
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

        // A terminal event answers the pending send_await. A `Busy` failure
        // is a concurrent command's rejection, never the awaited command's
        // outcome, so it must not consume the reply.
        if let Some((pending_cmd, _)) = &pending {
            let terminal = events.iter().find_map(|e| match e {
                CommandEvent::Completed { command, result } if *command == *pending_cmd => {
                    Some(Ok(*result))
                }
                CommandEvent::Failed { command, error }
                    if *command == *pending_cmd && *error != CommandError::Busy =>
                {
                    Some(Err(error.clone()))
                }
                _ => None,
            });
            if let Some(outcome) = terminal {
                let (_, reply) = pending.take().expect("pending was just checked");
                // A dropped receiver (caller timed out first) is fine.
                let _ = reply.send(outcome);
            }
        }

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
