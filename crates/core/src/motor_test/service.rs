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
    /// Latest-value-wins manual update (UI throttles to ~30 Hz).
    SetValues(Vec<f32>),
    Stop,
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
    pub fn spawn(
        handle: ConnectionHandle,
        limits: SafetyLimits,
    ) -> (Self, mpsc::Receiver<MotorTestEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (evt_tx, evt_rx) = mpsc::channel(32);
        let session = MotorTestSession::new(
            AnySource::Manual(super::source::ManualSource::new(vec![0.0; 4])),
            limits.clone(),
        );
        let tick = std::time::Duration::from_secs_f64(1.0 / super::DEFAULT_TICK_HZ);
        tokio::spawn(run(handle, session, cmd_rx, evt_tx, tick));
        (Self { cmd_tx }, evt_rx)
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
}

/// The shell loop. The session is the pure core; this only moves frames.
async fn run(
    handle: ConnectionHandle,
    mut session: MotorTestSession<AnySource>,
    mut cmd_rx: mpsc::Receiver<MotorTestCommand>,
    evt_tx: mpsc::Sender<MotorTestEvent>,
    tick: std::time::Duration,
) {
    let mut events_rx = handle.subscribe_route(MessageRoute::messages(WATCHED_MESSAGES));
    let mut ticker = tokio::time::interval(ticker_period(tick));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_state = session.state();

    loop {
        let mut outgoing: Vec<MavMessage> = Vec::new();
        let mut stop_event: Option<StopReason> = None;

        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    MotorTestCommand::StartManual => session.start_manual(Instant::now()),
                    MotorTestCommand::StartPreset { preset } => {
                        session.start_preset(preset, Instant::now())
                    }
                    MotorTestCommand::SetValues(values) => session.set_any_values(values),
                    MotorTestCommand::Stop => session.stop(StopReason::Command),
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

        for frame in outgoing {
            if handle.send(frame).await.is_err() {
                return; // link gone: PX4's 3 s timeout is the backstop
            }
        }

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
    }
}

/// Clamp the tick into `interval`'s expectations (never zero).
fn ticker_period(tick: std::time::Duration) -> std::time::Duration {
    tick.max(std::time::Duration::from_millis(1))
}
