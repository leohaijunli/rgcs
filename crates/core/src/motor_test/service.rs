//! Motor-test service shell: owns the link, the clock and the session.
//!
//! Same shape as `commands::service` — a task driven by a command channel,
//! an inbound frame stream and a ticker — but the high-rate path sends
//! directly (`ConnectionHandle::send`) and only *counts* acks, never waits
//! for them (plan D4: this traffic must not go through the single-slot
//! `CommandService`).

use std::time::Instant;

use tokio::sync::mpsc;

use ::mavlink::MessageData;

use super::interlock::SafetyLimits;
use super::source::AnySource;
use super::{MotorTestSession, SessionState};
use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::MessageRoute;
use crate::mavlink::MavMessage;
use crate::telemetry::BaseMode;

/// Only these wake the shell: our acks and heartbeats (the armed interlock).
const WATCHED_MESSAGES: &[u32] = &[
    ::mavlink::common::COMMAND_ACK_DATA::ID,
    ::mavlink::common::HEARTBEAT_DATA::ID,
];

/// What ended a running session (surfaced to the UI, plan §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Operator pressed stop.
    Command,
    /// Operator emergency-stopped (the event carries the latched state too).
    Emergency,
    /// The vehicle armed mid-test.
    Armed,
    /// No (fresh) heartbeat while sending.
    HeartbeatStale,
    /// The FC denied/failed our commands.
    AcksDenied,
    /// The MAVLink link dropped.
    LinkLost,
    /// The source ran to its end (a preset finished its duration).
    Completed,
}

/// Commands into the service (from the Tauri adapter).
#[derive(Debug)]
pub enum MotorTestCommand {
    /// Start manual sending with the current slider values.
    StartManual,
    /// Start a preset waveform (owns all motors for its duration).
    StartPreset {
        preset: super::source::WaveformPreset,
    },
    /// Start realtime forwarding from the shared slot (revised D3; the
    /// app-side SITL tap feeds it with normalized actuator outputs).
    StartForward {
        slot: super::source::ForwardSlot,
    },
    /// Latest-value-wins manual update (UI throttles to ~30 Hz).
    SetValues(Vec<f32>),
    Stop,
    /// Stop and acknowledge after the release frames have been written.
    StopAndWait {
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    Emergency,
    ResetEmergency,
}

/// Events out to the adapter (low-rate; the curve data has its own channel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MotorTestEvent {
    StateChanged(SessionState),
    Stopped(StopReason),
}

/// Handle to the running motor-test service.
#[derive(Clone)]
pub struct MotorTestService {
    cmd_tx: mpsc::Sender<MotorTestCommand>,
}

impl MotorTestService {
    /// Spawn on a link. The task stops when the handle drops, the link
    /// closes, or the event subscriber goes away. The session starts Idle:
    /// arming observations flow before any Start.
    ///
    /// Returns the service handle, the event stream specialist, and a watch of
    /// the last values actually sent to the FC (updated every tick while
    /// sending, `0.0` otherwise) for the window's output readout.
    pub fn spawn(
        handle: ConnectionHandle,
        limits: SafetyLimits,
    ) -> (
        Self,
        mpsc::Receiver<MotorTestEvent>,
        tokio::sync::watch::Receiver<Vec<f32>>,
    ) {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (evt_tx, evt_rx) = mpsc::channel(32);
        let session = MotorTestSession::new(
            AnySource::Manual(super::source::ManualSource::new(vec![0.0; 4])),
            limits.clone(),
        );
        let (output_tx, output_rx) = tokio::sync::watch::channel(session.output_values());
        let tick = std::time::Duration::from_secs_f64(1.0 / super::DEFAULT_TICK_HZ);
        tokio::spawn(run(handle, session, cmd_rx, evt_tx, output_tx, tick));
        (Self { cmd_tx }, evt_rx, output_rx)
    }

    /// Send a command; `Err` = the service is gone (link dropped).
    pub async fn send(
        &self,
        cmd: MotorTestCommand,
    ) -> Result<(), mpsc::error::SendError<MotorTestCommand>> {
        self.cmd_tx.send(cmd).await
    }

    /// Non-blocking stop-path send (window close): the buffered channel takes
    /// it without awaiting from a sync context.
    pub fn try_send(
        &self,
        cmd: MotorTestCommand,
    ) -> Result<(), mpsc::error::TrySendError<MotorTestCommand>> {
        self.cmd_tx.try_send(cmd)
    }

    /// Stop and wait until the release pass has been sent on the link.
    pub async fn stop_and_wait(&self) -> Result<bool, String> {
        let (reply, ack) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(MotorTestCommand::StopAndWait { reply })
            .await
            .map_err(|error| error.to_string())?;
        ack.await.map_err(|error| error.to_string())
    }
}

/// The shell loop. The session is the pure core; this only moves frames.
async fn run(
    handle: ConnectionHandle,
    mut session: MotorTestSession<AnySource>,
    mut cmd_rx: mpsc::Receiver<MotorTestCommand>,
    evt_tx: mpsc::Sender<MotorTestEvent>,
    output_tx: tokio::sync::watch::Sender<Vec<f32>>,
    tick: std::time::Duration,
) {
    // A shared MAVLink bus can carry multiple vehicles. Only the aircraft
    // addressed by ACTUATOR_TEST may supply this session's armed state or
    // acknowledgements; a separate armed SITL must not trip the bench target.
    let mut events_rx = handle.subscribe_route(MessageRoute::messages(WATCHED_MESSAGES).from(
        session.interlock().limits().target_sys,
        session.interlock().limits().target_comp,
    ));
    let mut ticker = tokio::time::interval(ticker_period(tick));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_state = session.state();

    loop {
        let mut outgoing: Vec<MavMessage> = Vec::new();
        let mut stop_event: Option<StopReason> = None;
        let mut stop_reply = None;

        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    MotorTestCommand::StartManual => session.start_manual(Instant::now()),
                    MotorTestCommand::StartPreset { preset } => {
                        session.start_preset(preset, Instant::now())
                    }
                    MotorTestCommand::StartForward { slot } => {
                        session.start_forward(slot, Instant::now())
                    }
                    MotorTestCommand::SetValues(values) => session.set_any_values(values),
                    MotorTestCommand::Stop => session.stop(StopReason::Command),
                    MotorTestCommand::StopAndWait { reply } => {
                        let was_sending = session.state().is_sending();
                        session.stop(StopReason::Command);
                        stop_reply = Some((reply, was_sending));
                    }
                    MotorTestCommand::Emergency => session.emergency_stop(),
                    MotorTestCommand::ResetEmergency => session.reset_emergency(),
                }
            }
            ev = events_rx.recv() => {
                match ev {
                    Ok(ConnectionEvent::Message(env)) => match &env.message {
                        ::mavlink::common::MavMessage::HEARTBEAT(hb) => {
                            let armed = BaseMode::from_raw(hb.base_mode.bits()).safety_armed;
                            session.interlock_mut().observe_heartbeat(armed, Instant::now());
                        }
                        ::mavlink::common::MavMessage::COMMAND_ACK(ack)
                            if ack.command
                                == ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST =>
                        {
                            session.handle_ack(ack.result);
                        }
                        _ => {}
                    },
                    Ok(ConnectionEvent::Failed(_)) | Ok(ConnectionEvent::HeartbeatLost { .. }) => {
                        session.interlock_mut().link_lost();
                        session.stop(StopReason::LinkLost);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
            _ = ticker.tick() => {
                outgoing = session.tick(Instant::now());
            }
        }

        // A stop outside the tick (command, ack denial, link loss) must also
        // put release frames on the wire this pass.
        if session.state() == SessionState::Stopping {
            outgoing.extend(session.release_frames());
        }
        if let Some(reason) = session.take_stop_reason() {
            stop_event = Some(reason);
        }

        // Enqueue a tick's frames back-to-back so PX4 applies all four
        // `ACTUATOR_TEST` functions in one consumer pass (the motors must move
        // together). Only the stop/release pass waits for the transport write,
        // so `stop_and_wait` still guarantees the release frames hit the wire.
        let stopping = session.state() == SessionState::Stopping;
        let mut send_succeeded = true;
        for frame in outgoing {
            let result = if stopping {
                handle.send_confirmed(frame).await
            } else {
                handle.send(frame).await
            };
            if result.is_err() {
                send_succeeded = false;
                break;
            }
        }
        if !send_succeeded {
            return; // link gone: PX4's 3 s timeout is the backstop
        }

        // Publish the latest values that went onto the wire (the window's
        // output readout; a dropped receiver — window closed — is fine).
        let _ = output_tx.send(session.output_values());

        if session.state() != last_state {
            last_state = session.state();
            if evt_tx
                .send(MotorTestEvent::StateChanged(last_state))
                .await
                .is_err()
            {
                return;
            }
        }
        if let Some(reason) = stop_event {
            if evt_tx.send(MotorTestEvent::Stopped(reason)).await.is_err() {
                return;
            }
        }
        if let Some((reply, was_sending)) = stop_reply {
            let _ = reply.send(was_sending);
        }
    }
}

/// Clamp the tick into `interval`'s expectations (never zero).
fn ticker_period(tick: std::time::Duration) -> std::time::Duration {
    tick.max(std::time::Duration::from_millis(1))
}
