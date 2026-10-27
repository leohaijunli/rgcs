//! Motor-test service shell: owns the link, the clock and the session.
//!
//! Same shape as `commands::service` — a task driven by a command channel,
//! an inbound frame stream and a ticker — but the high-rate path sends
//! directly with bounded, generation-bound transport writes and only *counts* acks, never waits
//! for them (plan D4: this traffic must not go through the single-slot
//! `CommandService`).

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use ts_rs::TS;

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
const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_millis(200);
const COMMAND_CAPACITY: usize = 8;
const PRIORITY_CAPACITY: usize = 2;
const EVENT_CAPACITY: usize = 32;

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

struct BoundTransport {
    handle: ConnectionHandle,
    generation: u64,
}

impl MotorTestTransport for BoundTransport {
    fn subscribe_route(&self, route: MessageRoute) -> RoutedEvents {
        self.handle.subscribe_route(route)
    }

    fn send(&self, frame: MavMessage) -> impl Future<Output = Result<(), MavlinkError>> + Send {
        self.handle.send_in_generation(
            frame,
            self.generation,
            tokio::time::Instant::now() + CONTROL_WRITE_TIMEOUT,
        )
    }

    fn send_confirmed(
        &self,
        frame: MavMessage,
    ) -> impl Future<Output = Result<(), MavlinkError>> + Send {
        self.handle.send_in_generation(
            frame,
            self.generation,
            tokio::time::Instant::now() + MOTOR_TEST_STOP_TIMEOUT,
        )
    }
}

/// What ended a running session (surfaced to the UI, plan §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
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
    /// Resize the idle session to the configured M1..MN prefix before a
    /// start. Ignored while sending; the app sends this before every start.
    SetMotorCount {
        motors: usize,
    },
    /// Per-motor PWM models for the next start (sent after every mapping
    /// refresh and before every start).
    SetPwmModels {
        models: Vec<super::pwm::PwmModel>,
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
    cmd_tx: mpsc::Sender<QueuedCommand>,
    priority_tx: mpsc::Sender<MotorTestCommand>,
    pwm_rx: tokio::sync::watch::Receiver<Vec<Option<f32>>>,
    state_rx: tokio::sync::watch::Receiver<Option<SessionState>>,
    dropped_events: Arc<AtomicU64>,
    command_epoch: Arc<AtomicU64>,
}

struct QueuedCommand {
    command: MotorTestCommand,
    epoch: u64,
}

struct Receivers {
    commands: mpsc::Receiver<QueuedCommand>,
    priority: mpsc::Receiver<MotorTestCommand>,
    epoch: Arc<AtomicU64>,
}

fn is_priority(command: &MotorTestCommand) -> bool {
    matches!(
        command,
        MotorTestCommand::Stop
            | MotorTestCommand::Emergency
            | MotorTestCommand::StopAndWait { .. }
            | MotorTestCommand::EmergencyAndWait { .. }
    )
}

impl MotorTestService {
    /// Latest service transition, independent of event pressure. None preserves
    /// the native startup reservation until Start is actually processed.
    pub fn session_state(&self) -> Option<SessionState> {
        *self.state_rx.borrow()
    }
    /// Observer pressure is explicit and never delays control or release.
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }
    /// Converted command readback in microseconds, not measured FC feedback.
    /// None marks a released motor or an unavailable target PWM model.
    pub fn pwm_output(&self) -> Vec<Option<f32>> {
        self.pwm_rx.borrow().clone()
    }
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
        let generation = handle.generation();
        spawn_on_transport(BoundTransport { handle, generation }, limits)
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
            self.priority_tx
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
        if is_priority(&cmd) {
            self.priority_tx.send(cmd).await
        } else {
            let epoch = self.command_epoch.load(Ordering::Acquire);
            if epoch == 0 {
                return Err(mpsc::error::SendError(cmd));
            }
            self.cmd_tx
                .send(QueuedCommand {
                    command: cmd,
                    epoch,
                })
                .await
                .map_err(|error| mpsc::error::SendError(error.0.command))
        }
    }

    /// Non-blocking stop-path send (window close): the buffered channel takes
    /// it without awaiting from a sync context.
    pub fn try_send(
        &self,
        cmd: MotorTestCommand,
    ) -> Result<(), mpsc::error::TrySendError<MotorTestCommand>> {
        if is_priority(&cmd) {
            self.priority_tx.try_send(cmd)
        } else {
            let epoch = self.command_epoch.load(Ordering::Acquire);
            if epoch == 0 {
                return Err(mpsc::error::TrySendError::Closed(cmd));
            }
            self.cmd_tx
                .try_send(QueuedCommand {
                    command: cmd,
                    epoch,
                })
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(queued) => {
                        mpsc::error::TrySendError::Full(queued.command)
                    }
                    mpsc::error::TrySendError::Closed(queued) => {
                        mpsc::error::TrySendError::Closed(queued.command)
                    }
                })
        }
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
    let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_CAPACITY);
    let (priority_tx, priority_rx) = mpsc::channel(PRIORITY_CAPACITY);
    let (evt_tx, evt_rx) = mpsc::channel(EVENT_CAPACITY);
    let dropped_events = Arc::new(AtomicU64::new(0));
    let command_epoch = Arc::new(AtomicU64::new(1));
    let session = MotorTestSession::new(
        AnySource::Manual(super::source::ManualSource::new(vec![0.0; 4])),
        limits.clone(),
    );
    let (output_tx, output_rx) = tokio::sync::watch::channel(session.output_values());
    let (pwm_tx, pwm_rx) = tokio::sync::watch::channel(session.pwm_output());
    let (state_tx, state_rx) = tokio::sync::watch::channel(None);
    let tick = std::time::Duration::from_secs_f64(1.0 / super::DEFAULT_TICK_HZ);
    tokio::spawn(run(
        handle,
        session,
        Receivers {
            commands: cmd_rx,
            priority: priority_rx,
            epoch: command_epoch.clone(),
        },
        EventSink {
            events: evt_tx,
            dropped: dropped_events.clone(),
        },
        Outputs {
            values: output_tx,
            pwm: pwm_tx,
            state: state_tx,
        },
        tick,
    ));
    (
        MotorTestService {
            cmd_tx,
            priority_tx,
            pwm_rx,
            state_rx,
            dropped_events,
            command_epoch,
        },
        evt_rx,
        output_rx,
    )
}

struct Outputs {
    values: tokio::sync::watch::Sender<Vec<f32>>,
    pwm: tokio::sync::watch::Sender<Vec<Option<f32>>>,
    state: tokio::sync::watch::Sender<Option<SessionState>>,
}

struct EventSink {
    events: mpsc::Sender<MotorTestEvent>,
    dropped: Arc<AtomicU64>,
}
impl EventSink {
    fn publish(&self, event: MotorTestEvent) {
        if self.events.try_send(event).is_err() {
            let _ = self
                .dropped
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    Some(count.saturating_add(1))
                });
        }
    }
}

type StopReply = (
    tokio::time::Instant,
    tokio::sync::oneshot::Sender<Result<StopReceipt, MotorTestStopError>>,
);

fn apply_priority(
    session: &mut MotorTestSession<AnySource>,
    command: MotorTestCommand,
    epoch: &AtomicU64,
) -> Option<StopReply> {
    // Fence both buffered and blocked producers. Draining a queue can otherwise
    // wake an old Start sender after STOP, or loop indefinitely under refill.
    if epoch
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current != 0).then(|| current.checked_add(1)).flatten()
        })
        .is_err()
    {
        epoch.store(0, Ordering::Release); // Exhaustion retires ordinary commands; never wrap/replay.
    }
    match command {
        MotorTestCommand::Emergency => {
            session.emergency_stop();
            None
        }
        MotorTestCommand::EmergencyAndWait { deadline, reply } => {
            session.emergency_stop();
            Some((deadline, reply))
        }
        MotorTestCommand::StopAndWait { deadline, reply } => {
            session.stop(StopReason::Command);
            Some((deadline, reply))
        }
        _ => {
            session.stop(StopReason::Command);
            None
        }
    }
}

/// The shell loop. The session is the pure core; this only moves frames.
async fn run(
    handle: impl MotorTestTransport,
    mut session: MotorTestSession<AnySource>,
    mut receivers: Receivers,
    events: EventSink,
    output: Outputs,
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

        if events.events.is_closed() {
            session.stop(StopReason::LinkLost);
            exit_after_release = true;
        } else if let Ok(command) = receivers.priority.try_recv() {
            stop_reply = apply_priority(&mut session, command, &receivers.epoch);
        } else {
            tokio::select! {
                command = receivers.priority.recv() => {
                    match command {
                        Some(command) => stop_reply = apply_priority(&mut session, command, &receivers.epoch),
                        None => { session.stop(StopReason::LinkLost); exit_after_release = true; }
                    }
                }
                cmd = receivers.commands.recv() => {
                    match cmd {
                        None => {
                            session.stop(StopReason::LinkLost);
                            exit_after_release = true;
                        }
                        Some(queued) => {
                        let current_epoch = receivers.epoch.load(Ordering::Acquire);
                        if current_epoch == 0 || queued.epoch != current_epoch { continue; }
                        match queued.command {
                        MotorTestCommand::StartManual => session.start_manual(Instant::now()),
                        MotorTestCommand::StartPreset { preset } => {
                            session.start_preset(preset, Instant::now())
                        }
                        MotorTestCommand::StartForward { slot } => {
                            session.start_forward(slot, Instant::now())
                        }
                        MotorTestCommand::SetMotorCount { motors } => {
                            session.set_motor_count(motors);
                        }
                        MotorTestCommand::SetPwmModels { models } => session.set_pwm_models(models),
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
                        } },
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
                        Ok(ConnectionEvent::Failed(_)) | Ok(ConnectionEvent::LinkError(_))
                            | Ok(ConnectionEvent::Connected { .. }) | Ok(ConnectionEvent::HeartbeatLost { .. }) => {
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
        }

        if !session.release_pending() {
            for frame in outgoing {
                // A queued STOP interrupts a blocked control write rather than
                // waiting for the rest of the motor bank or a UI event.
                let result = tokio::select! { biased;
                    command = receivers.priority.recv() => {
                        match command {
                            Some(command) => stop_reply = apply_priority(&mut session, command, &receivers.epoch),
                            None => { session.stop(StopReason::LinkLost); exit_after_release = true; }
                        }
                        break;
                    }
                    result = handle.send(frame) => result,
                };
                if result.is_err() {
                    session.stop(StopReason::LinkLost);
                    break;
                }
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
            Ok(StopReceipt {
                released_motors: 0,
                state: session.state(),
            })
        };

        // Never clear running demands or publish Stopped after a failed pass.
        // The readout is calculated/queued demand for controls, and zero only
        // once release transport writes are confirmed.
        let _ = output.values.send(session.output_values());
        let _ = output.pwm.send(session.pwm_output());
        if session.state() != last_state {
            output.state.send_replace(Some(session.state()));
        }

        // Resolve the transport receipt before any slow event consumer can
        // block notification. Errors leave release pending for a later retry.
        if let Some((_, reply)) = stop_reply {
            let _ = reply.send(result.clone());
        }

        if session.state() != last_state {
            last_state = session.state();
            events.publish(MotorTestEvent::StateChanged(last_state));
        }
        match result {
            Ok(_) if !session.release_pending() => {
                if let Some(reason) = session.take_stop_reason() {
                    events.publish(MotorTestEvent::Stopped(reason));
                }
            }
            Err(error) => {
                // Repeated write failures must not fill the event queue and
                // block retries; a waiting stop caller receives the same error.
                events.publish(MotorTestEvent::ReleaseFailed(error));
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
        block_control: Arc<std::sync::atomic::AtomicBool>,
        control_waiting: Arc<std::sync::atomic::AtomicBool>,
    }

    impl MotorTestTransport for ControlledTransport {
        fn subscribe_route(&self, route: MessageRoute) -> RoutedEvents {
            RoutedEvents::new(self.events.subscribe(), route)
        }

        async fn send(&self, frame: MavMessage) -> Result<(), MavlinkError> {
            if self.block_control.load(Ordering::Acquire) {
                self.control_waiting.store(true, Ordering::Release);
                std::future::pending::<()>().await;
            }
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
                block_control: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                control_waiting: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

    fn heartbeat(transport: &ControlledTransport) {
        transport
            .events
            .send(ConnectionEvent::Message(Box::new(
                crate::mavlink::MessageEnvelope {
                    header: ::mavlink::MavHeader {
                        system_id: 42,
                        component_id: 1,
                        sequence: 0,
                    },
                    message: MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
                        base_mode: ::mavlink::common::MavModeFlag::empty(),
                        ..Default::default()
                    }),
                    received_at: Instant::now(),
                },
            )))
            .expect("heartbeat");
    }

    async fn wait_for(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !predicate() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("predicate deadline");
    }

    async fn stop_confirmed(
        svc: &MotorTestService,
        writes: &mut mpsc::UnboundedReceiver<(MavMessage, WriteResult)>,
    ) {
        let stopping = tokio::spawn({
            let svc = svc.clone();
            async move { svc.stop_and_wait().await }
        });
        for motor in 1..=4 {
            next_release(writes, motor)
                .await
                .send(Ok(()))
                .expect("confirmed release");
        }
        assert_eq!(
            stopping
                .await
                .expect("stop task")
                .expect("stop receipt")
                .released_motors,
            4
        );
    }

    #[tokio::test]
    async fn saturated_observer_cannot_block_stop_or_latest_state() {
        let (transport, mut writes) = transport();
        let (svc, _unread_events) = started(transport.clone()).await;
        heartbeat(&transport);
        wait_for(|| !transport.controls.lock().expect("controls").is_empty()).await;
        for _ in 0..EVENT_CAPACITY / 2 {
            stop_confirmed(&svc, &mut writes).await;
            let before = transport.controls.lock().expect("controls").len();
            svc.send(MotorTestCommand::StartManual)
                .await
                .expect("new start");
            wait_for(|| transport.controls.lock().expect("controls").len() > before).await;
        }
        assert!(
            svc.dropped_events() > 0,
            "full event queue must expose dropped notifications"
        );
        stop_confirmed(&svc, &mut writes).await;
        assert_eq!(svc.session_state(), Some(SessionState::Idle));
    }

    #[tokio::test]
    async fn priority_stop_interrupts_blocked_write_and_discards_queued_restart() {
        let (transport, mut writes) = transport();
        transport.block_control.store(true, Ordering::Release);
        let (svc, _events) = started(transport.clone()).await;
        heartbeat(&transport);
        wait_for(|| transport.control_waiting.load(Ordering::Acquire)).await;
        for _ in 0..COMMAND_CAPACITY {
            svc.try_send(MotorTestCommand::StartManual)
                .expect("fill ordinary queue");
        }
        let blocked_start = tokio::spawn({
            let svc = svc.clone();
            async move { svc.send(MotorTestCommand::StartManual).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !blocked_start.is_finished(),
            "old producer is waiting outside the full queue"
        );
        stop_confirmed(&svc, &mut writes).await;
        blocked_start
            .await
            .expect("producer")
            .expect("enqueue retired intent");
        transport.block_control.store(false, Ordering::Release);
        tokio::time::sleep(Duration::from_millis(220)).await;
        assert_eq!(svc.session_state(), Some(SessionState::Idle));
        assert!(
            transport.controls.lock().expect("controls").is_empty(),
            "cancelled control writes and queued restarts cannot run"
        );
        assert_eq!(
            svc.stop_and_wait()
                .await
                .expect("idle stop")
                .released_motors,
            0
        );
    }

    #[tokio::test]
    async fn closed_observer_releases_before_retiring_the_service() {
        let (transport, mut writes) = transport();
        let (svc, events) = started(transport).await;
        drop(events);
        for motor in 1..=4 {
            next_release(&mut writes, motor)
                .await
                .send(Ok(()))
                .expect("close release");
        }
        wait_for(|| svc.session_state() == Some(SessionState::Idle)).await;
        assert_eq!(
            svc.stop_and_wait().await,
            Err(MotorTestStopError::ServiceStopped)
        );
    }

    #[tokio::test]
    async fn pwm_watch_matches_control_frames_and_clears_after_confirmed_release() {
        let (transport, mut writes) = transport();
        let (svc, _events) = started(transport.clone()).await;
        svc.send(MotorTestCommand::SetPwmModels {
            models: vec![
                super::super::pwm::PwmModel::new(1100, 1900, false, 0.0).expect("FC model");
                4
            ],
        })
        .await
        .expect("models");
        transport
            .events
            .send(ConnectionEvent::Message(Box::new(
                crate::mavlink::MessageEnvelope {
                    header: ::mavlink::MavHeader {
                        system_id: 42,
                        component_id: 1,
                        sequence: 0,
                    },
                    message: MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
                        base_mode: ::mavlink::common::MavModeFlag::empty(),
                        ..Default::default()
                    }),
                    received_at: Instant::now(),
                },
            )))
            .expect("disarmed heartbeat");
        tokio::time::timeout(Duration::from_secs(1), async {
            while svc.pwm_output().first().copied().flatten().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("PWM published");
        assert_eq!(svc.pwm_output(), vec![Some(1420.0); 4]);
        {
            let frames = transport.controls.lock().expect("control frames");
            let MavMessage::COMMAND_LONG(frame) = &frames[0] else {
                panic!("control frame");
            };
            assert_eq!(
                frame.param1, 0.4,
                "wire stays normalized while readback is PWM"
            );
        }

        let stop = tokio::spawn({
            let svc = svc.clone();
            async move { svc.stop_and_wait().await }
        });
        for motor in 1..=4 {
            let confirmation = next_release(&mut writes, motor).await;
            assert_eq!(
                svc.pwm_output(),
                vec![Some(1420.0); 4],
                "no false release gap before confirmed writes"
            );
            confirmation.send(Ok(())).expect("confirm release");
        }
        stop.await.expect("stop task").expect("confirmed stop");
        assert_eq!(svc.pwm_output(), vec![None; 4]);
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
    async fn idle_emergency_never_releases_unowned_outputs() {
        let (transport, mut writes) = transport();
        let controls = transport.controls.clone();
        let (service, _events, _) = spawn_on_transport(transport, limits());
        let receipt = service
            .emergency_stop_and_wait()
            .await
            .expect("local latch");
        assert_eq!(receipt.state, SessionState::Emergency);
        assert_eq!(receipt.released_motors, 0);
        assert!(
            writes.try_recv().is_err(),
            "an idle observer must not release another controller's outputs"
        );
        assert!(controls.lock().expect("controls").is_empty());
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
