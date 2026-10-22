//! Motor-test service shell: owns the link, the clock and the session.
//!
//! Same shape as `commands::service` — a task driven by a command channel,
//! an inbound frame stream and a ticker — but the high-rate path sends
//! directly (`ConnectionHandle::send`) and only *counts* acks, never waits
//! for them (plan D4: this traffic must not go through the single-slot
//! `CommandService`).

use std::future::Future;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use ::mavlink::MessageData;

use super::interlock::SafetyLimits;
use super::source::AnySource;
use super::{MotorTestSession, SessionState};
use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::{MessageRoute, RoutedEvents};
use crate::mavlink::{MavMessage, MavlinkError};
use crate::telemetry::BaseMode;

/// Only these wake the shell: our acks and heartbeats (the armed interlock).
const WATCHED_MESSAGES: &[u32] = &[
    ::mavlink::common::COMMAND_ACK_DATA::ID,
    ::mavlink::common::HEARTBEAT_DATA::ID,
];

/// Overall stop budget, including command queueing and release writes.
pub const MOTOR_TEST_STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Transport evidence for a completed release pass. This is not a PX4 ACK
/// or a physical-output measurement. Zero releases means already stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopReceipt {
    pub released_motors: usize,
    pub state: SessionState,
}

/// A stop must never turn a failed or unconfirmed write into success.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MotorTestStopError {
    #[error("motor-test service stopped before release confirmation")]
    ServiceStopped,
    #[error("timed out waiting for motor-test release confirmation")]
    Timeout,
    /// `motor` is the 1-based motor function whose release write failed.
    #[error("motor {motor} release write failed: {message}")]
    ReleaseWrite { motor: usize, message: String },
}

/// Private transport boundary lets regression tests hold/fail the actual
/// confirmation step rather than infer write ordering from UDP arrival times.
trait MotorTestTransport: Send + Sync + 'static {
    fn subscribe_route(&self, route: MessageRoute) -> RoutedEvents;
    fn send(&self, frame: MavMessage) -> impl Future<Output = Result<(), MavlinkError>> + Send;
    fn send_confirmed(
        &self,
        frame: MavMessage,
    ) -> impl Future<Output = Result<(), MavlinkError>> + Send;
}

impl MotorTestTransport for ConnectionHandle {
    fn subscribe_route(&self, route: MessageRoute) -> RoutedEvents {
        ConnectionHandle::subscribe_route(self, route)
    }

    fn send(&self, frame: MavMessage) -> impl Future<Output = Result<(), MavlinkError>> + Send {
        ConnectionHandle::send(self, frame)
    }

    fn send_confirmed(
        &self,
        frame: MavMessage,
    ) -> impl Future<Output = Result<(), MavlinkError>> + Send {
        ConnectionHandle::send_confirmed(self, frame)
    }
}

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
    /// Stop and reply only after the full release pass has been written.
    StopAndWait {
        deadline: tokio::time::Instant,
        reply: tokio::sync::oneshot::Sender<Result<StopReceipt, MotorTestStopError>>,
    },
    Emergency,
    EmergencyAndWait {
        deadline: tokio::time::Instant,
        reply: tokio::sync::oneshot::Sender<Result<StopReceipt, MotorTestStopError>>,
    },
    ResetEmergency,
}

/// Events out to the adapter (low-rate; the curve data has its own channel).
#[derive(Debug, Clone, PartialEq)]
pub enum MotorTestEvent {
    StateChanged(SessionState),
    /// Published only after all releases have confirmed transport writes.
    Stopped(StopReason),
    /// Active refreshes remain stopped; the full release pass can be retried.
    ReleaseFailed(MotorTestStopError),
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
        spawn_on_transport(handle, limits)
    }

    /// Stop and await all release transport writes within the overall budget.
    pub async fn stop_and_wait(&self) -> Result<StopReceipt, MotorTestStopError> {
        self.request_stop(false).await
    }

    /// Release every motor and keep Emergency latched until explicitly reset.
    pub async fn emergency_stop_and_wait(&self) -> Result<StopReceipt, MotorTestStopError> {
        self.request_stop(true).await
    }

    async fn request_stop(&self, emergency: bool) -> Result<StopReceipt, MotorTestStopError> {
        let deadline = tokio::time::Instant::now() + MOTOR_TEST_STOP_TIMEOUT;
        let (reply, ack) = tokio::sync::oneshot::channel();
        let command = if emergency {
            MotorTestCommand::EmergencyAndWait { deadline, reply }
        } else {
            MotorTestCommand::StopAndWait { deadline, reply }
        };
        tokio::time::timeout_at(deadline, async {
            self.cmd_tx
                .send(command)
                .await
                .map_err(|_| MotorTestStopError::ServiceStopped)?;
            ack.await.map_err(|_| MotorTestStopError::ServiceStopped)?
        })
        .await
        .map_err(|_| MotorTestStopError::Timeout)?
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

fn spawn_on_transport(
    handle: impl MotorTestTransport,
    limits: SafetyLimits,
) -> (
    MotorTestService,
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
    (MotorTestService { cmd_tx }, evt_rx, output_rx)
}

/// The shell loop. The session is the pure core; this only moves frames.
async fn run(
    handle: impl MotorTestTransport,
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
        let mut stop_reply = None;
        let mut exit_after_release = false;

        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => {
                        session.stop(StopReason::LinkLost);
                        exit_after_release = true;
                    }
                    Some(cmd) => match cmd {
                    MotorTestCommand::StartManual => session.start_manual(Instant::now()),
                    MotorTestCommand::StartPreset { preset } => {
                        session.start_preset(preset, Instant::now())
                    }
                    MotorTestCommand::StartForward { slot } => {
                        session.start_forward(slot, Instant::now())
                    }
                    MotorTestCommand::SetValues(values) => session.set_any_values(values),
                    MotorTestCommand::Stop => session.stop(StopReason::Command),
                    MotorTestCommand::StopAndWait { deadline, reply } => {
                        session.stop(StopReason::Command);
                        stop_reply = Some((deadline, reply));
                    }
                    MotorTestCommand::Emergency => session.emergency_stop(),
                    MotorTestCommand::EmergencyAndWait { deadline, reply } => {
                        session.emergency_stop();
                        stop_reply = Some((deadline, reply));
                    }
                    MotorTestCommand::ResetEmergency => session.reset_emergency(),
                    },
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
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        session.stop(StopReason::LinkLost);
                        exit_after_release = true;
                    }
                }
            }
            _ = ticker.tick() => {
                outgoing = session.tick(Instant::now());
            }
        }

        // Release is an explicit delivery kind, independent of whether the
        // request came from a tick, command, emergency, ACK, or link event.
        // Generating frames never settles the session ahead of write evidence.
        let result = if session.release_pending() {
            let deadline = stop_reply
                .as_ref()
                .map(|(deadline, _)| *deadline)
                .unwrap_or_else(|| tokio::time::Instant::now() + MOTOR_TEST_STOP_TIMEOUT);
            match write_releases(&handle, session.release_frames(), deadline).await {
                Ok(released_motors) => {
                    session.complete_release();
                    Ok(StopReceipt {
                        released_motors,
                        state: session.state(),
                    })
                }
                Err(error) => Err(error),
            }
        } else {
            // A control tick is queued in order, not applied atomically by PX4.
            for frame in outgoing {
                if handle.send(frame).await.is_err() {
                    session.stop(StopReason::LinkLost);
                    break;
                }
            }
            Ok(StopReceipt {
                released_motors: 0,
                state: session.state(),
            })
        };

        // Never clear running demands or publish Stopped after a failed pass.
        // The readout is calculated/queued demand for controls, and zero only
        // once release transport writes are confirmed.
        let _ = output_tx.send(session.output_values());

        // Resolve the transport receipt before any slow event consumer can
        // block notification. Errors leave release pending for a later retry.
        if let Some((_, reply)) = stop_reply {
            let _ = reply.send(result.clone());
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
        match result {
            Ok(_) if !session.release_pending() => {
                if let Some(reason) = session.take_stop_reason() {
                    if evt_tx.send(MotorTestEvent::Stopped(reason)).await.is_err() {
                        return;
                    }
                }
            }
            Err(error) => {
                // Repeated write failures must not fill the event queue and
                // block retries; a waiting stop caller receives the same error.
                let _ = evt_tx.try_send(MotorTestEvent::ReleaseFailed(error));
            }
            _ => {}
        }
        if exit_after_release {
            return;
        }
    }
}

async fn write_releases(
    handle: &impl MotorTestTransport,
    frames: Vec<MavMessage>,
    deadline: tokio::time::Instant,
) -> Result<usize, MotorTestStopError> {
    let motors = frames.len();
    let mut first_error = None;
    for (index, frame) in frames.into_iter().enumerate() {
        match tokio::time::timeout_at(deadline, handle.send_confirmed(frame)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                // Try the remaining motors even if one release write fails.
                first_error.get_or_insert_with(|| MotorTestStopError::ReleaseWrite {
                    motor: index + 1,
                    message: error.to_string(),
                });
            }
            Err(_) => return Err(MotorTestStopError::Timeout),
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(motors),
    }
}

/// Clamp the tick into `interval`'s expectations (never zero).
fn ticker_period(tick: std::time::Duration) -> std::time::Duration {
    tick.max(std::time::Duration::from_millis(1))
}

#[cfg(test)]
mod stop_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::sync::{broadcast, oneshot};

    type WriteResult = oneshot::Sender<Result<(), MavlinkError>>;

    #[derive(Clone)]
    struct ControlledTransport {
        events: broadcast::Sender<ConnectionEvent>,
        controls: Arc<Mutex<Vec<MavMessage>>>,
        writes: mpsc::UnboundedSender<(MavMessage, WriteResult)>,
    }

    impl MotorTestTransport for ControlledTransport {
        fn subscribe_route(&self, route: MessageRoute) -> RoutedEvents {
            RoutedEvents::new(self.events.subscribe(), route)
        }

        async fn send(&self, frame: MavMessage) -> Result<(), MavlinkError> {
            self.controls.lock().expect("controls lock").push(frame);
            Ok(())
        }

        async fn send_confirmed(&self, frame: MavMessage) -> Result<(), MavlinkError> {
            let (reply, written) = oneshot::channel();
            self.writes
                .send((frame, reply))
                .map_err(|_| MavlinkError::ChannelClosed)?;
            written.await.map_err(|_| MavlinkError::ChannelClosed)?
        }
    }

    fn transport() -> (
        ControlledTransport,
        mpsc::UnboundedReceiver<(MavMessage, WriteResult)>,
    ) {
        let (events, _) = broadcast::channel(16);
        let (writes, rx) = mpsc::unbounded_channel();
        (
            ControlledTransport {
                events,
                controls: Arc::new(Mutex::new(Vec::new())),
                writes,
            },
            rx,
        )
    }

    fn limits() -> SafetyLimits {
        SafetyLimits {
            max_value: 1.0,
            max_slew_per_s: 10.0,
            heartbeat_timeout: Duration::from_secs(30),
            target_sys: 42,
            target_comp: 1,
        }
    }

    async fn started(
        transport: ControlledTransport,
    ) -> (MotorTestService, mpsc::Receiver<MotorTestEvent>) {
        let (svc, mut events, _) = spawn_on_transport(transport, limits());
        svc.send(MotorTestCommand::SetValues(vec![0.4; 4]))
            .await
            .expect("values");
        svc.send(MotorTestCommand::StartManual)
            .await
            .expect("start");
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::StateChanged(SessionState::ManualRunning))
        );
        (svc, events)
    }

    async fn next_release(
        rx: &mut mpsc::UnboundedReceiver<(MavMessage, WriteResult)>,
        motor: usize,
    ) -> WriteResult {
        let (frame, reply) = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("release requested promptly")
            .expect("transport alive");
        let MavMessage::COMMAND_LONG(command) = frame else {
            panic!("actuator command")
        };
        assert_eq!(
            command.command,
            ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST
        );
        assert_eq!(command.target_system, 42);
        assert_eq!(command.param5, motor as f32);
        assert_eq!(command.param2, 0.0);
        reply
    }

    #[tokio::test]
    async fn stop_receipt_waits_for_every_transport_write() {
        let (transport, mut writes) = transport();
        let controls = transport.controls.clone();
        let (svc, mut events) = started(transport).await;
        let stop = tokio::spawn({
            let svc = svc.clone();
            async move { svc.stop_and_wait().await }
        });
        for motor in 1..=4 {
            let reply = next_release(&mut writes, motor).await;
            assert!(
                !stop.is_finished(),
                "no receipt before motor {motor} is written"
            );
            assert!(events.try_recv().is_err(), "no premature stopped event");
            reply.send(Ok(())).expect("write confirmation awaited");
        }
        assert_eq!(
            stop.await.expect("join").expect("stop"),
            StopReceipt {
                released_motors: 4,
                state: SessionState::Idle
            }
        );
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::StateChanged(SessionState::Idle))
        );
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::Stopped(StopReason::Command))
        );
        let count = controls.lock().expect("controls").len();
        tokio::time::sleep(Duration::from_millis(220)).await;
        assert_eq!(
            controls.lock().expect("controls").len(),
            count,
            "no refresh after stop"
        );
        assert_eq!(
            svc.stop_and_wait().await.expect("idempotent"),
            StopReceipt {
                released_motors: 0,
                state: SessionState::Idle
            }
        );
    }

    #[tokio::test]
    async fn emergency_releases_all_motors_and_preserves_latch() {
        let (transport, mut writes) = transport();
        let controls = transport.controls.clone();
        let (svc, mut events) = started(transport).await;
        let stop = tokio::spawn({
            let svc = svc.clone();
            async move { svc.emergency_stop_and_wait().await }
        });
        for motor in 1..=4 {
            next_release(&mut writes, motor)
                .await
                .send(Ok(()))
                .expect("confirmation");
        }
        assert_eq!(
            stop.await.expect("join").expect("emergency"),
            StopReceipt {
                released_motors: 4,
                state: SessionState::Emergency
            }
        );
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::StateChanged(SessionState::Emergency))
        );
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::Stopped(StopReason::Emergency))
        );
        let count = controls.lock().expect("controls").len();
        svc.send(MotorTestCommand::StartManual)
            .await
            .expect("request start");
        tokio::time::sleep(Duration::from_millis(220)).await;
        assert_eq!(controls.lock().expect("controls").len(), count);
        assert_eq!(
            svc.stop_and_wait().await.expect("latched stop").state,
            SessionState::Emergency
        );
        svc.send(MotorTestCommand::ResetEmergency)
            .await
            .expect("reset");
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::StateChanged(SessionState::Idle))
        );
    }

    #[tokio::test]
    async fn failed_release_never_succeeds_and_remaining_motors_are_attempted() {
        let (transport, mut writes) = transport();
        let (svc, mut events) = started(transport).await;
        let stop = tokio::spawn({
            let svc = svc.clone();
            async move { svc.stop_and_wait().await }
        });
        for motor in 1..=4 {
            let reply = next_release(&mut writes, motor).await;
            reply
                .send(if motor == 2 {
                    Err(MavlinkError::Protocol("injected failure".into()))
                } else {
                    Ok(())
                })
                .expect("confirmation");
        }
        assert!(matches!(
            stop.await.expect("join"),
            Err(MotorTestStopError::ReleaseWrite { motor: 2, .. })
        ));
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::StateChanged(SessionState::Stopping))
        );
        assert!(matches!(
            events.recv().await,
            Some(MotorTestEvent::ReleaseFailed(_))
        ));
        // The retained session retries a complete batch rather than declaring
        // an incomplete release successful or continuing active demands.
        for motor in 1..=4 {
            next_release(&mut writes, motor)
                .await
                .send(Ok(()))
                .expect("retry confirmation");
        }
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::StateChanged(SessionState::Idle))
        );
        assert_eq!(
            events.recv().await,
            Some(MotorTestEvent::Stopped(StopReason::Command))
        );
    }
}
