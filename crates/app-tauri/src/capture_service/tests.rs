//! MR-16: real capture and motor services share one loopback MAVLink link.
//! Only the PX4 peer is simulated; command adapters, shell framing, parameter
//! reads, capture confirmation and motor release writes run as in the app.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use ::mavlink::common::{
    MavAutopilot, MavCmd, MavModeFlag, MavParamType, MavState, MavType, SerialControlDev,
    SerialControlFlag, COMMAND_LONG_DATA, HEARTBEAT_DATA, PARAM_VALUE_DATA, SERIAL_CONTROL_DATA,
};
use ::mavlink::peek_reader::PeekReader;
use ::mavlink::MavHeader;
use maggcs_core::mavlink::stream_control::{
    StreamRate, StreamReceiveState, StreamRequest, StreamStatus,
};
use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
use maggcs_core::motor_test::mag_alarm::{MagAlarm, MagAlarmConfig};
use maggcs_core::motor_test::mapping::{build_output_map_checked, FunctionRead};
use maggcs_core::motor_test::protocol::{DShotRate, OutputProtocol, OutputProtocols};
use maggcs_core::motor_test::pwm::PwmModel;
use maggcs_core::motor_test::run::{
    RunAction, RunConfig, RunCoordinator, RunError, RunPhase, POST_ROLL, PRE_ROLL,
};
use maggcs_core::motor_test::scope::MAX_ESC;
use maggcs_core::motor_test::{MotorTestCommand, MotorTestEvent, MotorTestService, SessionState};
use maggcs_core::signals::derived::EscAggregator;
use maggcs_core::telemetry::{Autopilot, BaseMode, FlightState, Heartbeat, VehicleType};
use tauri::test::{mock_app, MockRuntime};
use tauri::Manager;
use tokio::net::UdpSocket;
use tokio::time::timeout;

use crate::actuator_service::{emergency_stop_active_session, stop_active_session};
use crate::actuator_service::{start_source, StartSource};
use crate::motor_test_state::OutputConfigSnapshot;
use crate::motor_test_state::{
    motor_test_limits, AuxiliaryLinkRuntime, MotorTelemetry, MotorTestTarget, RunningMotorTest,
    TargetLinkRuntime,
};
use crate::motor_tone_service::{set_tones_legacy, MotorTone, MotorToneSettings};
use crate::run_service::ensure_coordinated_run_start;
use crate::runtime_facade::RuntimeFacade;
use maggcs_core::motor_test::operation_types::{
    MotorCaptureStartRequest, MotorCaptureStopRequest, MotorRequestContext,
};
use maggcs_core::runtime::operations::{CleanupOutcome, OperationPhase, RuntimeClientSession};
use maggcs_core::runtime::RequestId;

use super::*;

const FC_SYS: u8 = 42;
const GCS_SYS: u8 = 250;
const GCS_COMP: u8 = 190;
const MOTORS: u8 = 4;
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);
const HEARTBEAT_PERIOD: Duration = Duration::from_millis(50);
const POLL_PERIOD: Duration = Duration::from_millis(5);
const DATAGRAM_BYTES: usize = 1024;
const LOG_PATH: &str = "/fs/microsd/log/2026-10-24/mr16.ulg";

#[derive(Debug, Clone)]
enum Wire {
    Motor(COMMAND_LONG_DATA),
    Interval(COMMAND_LONG_DATA),
    Shell(String),
}

type Trace = Arc<Mutex<Vec<Wire>>>;

#[derive(Default)]
struct ToneFixture {
    pause_after: Mutex<Option<String>>,
    fail: AtomicBool,
}

#[derive(Default)]
struct StreamFixture {
    paused: AtomicBool,
    foreign: AtomicBool,
}

struct Harness {
    app: tauri::App<MockRuntime>,
    trace: Trace,
    hold_shell: Arc<AtomicBool>,
    tones: Arc<ToneFixture>,
    streams: Arc<StreamFixture>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Harness {
    async fn new() -> Self {
        let app = mock_app();
        app.manage(AppState::default());
        let state = app.state::<AppState>();
        let socket = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("fake PX4 socket");
        let address = socket.local_addr().expect("fake PX4 address");
        let trace = Trace::default();
        let hold_shell = Arc::new(AtomicBool::new(false));
        let tones = Arc::new(ToneFixture::default());
        let streams = Arc::new(StreamFixture::default());
        let peer = tokio::spawn(fake_px4(
            socket,
            app.handle().clone(),
            trace.clone(),
            hold_shell.clone(),
            tones.clone(),
            streams.clone(),
        ));
        let (handle, _, first) =
            maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
                endpoint: Endpoint::UdpClient { addr: address },
                system_id: GCS_SYS,
                component_id: GCS_COMP,
                target_system_id: FC_SYS,
                target_component_id: TARGET_COMPONENT_ID,
                auto_discover_target: false,
                ..Default::default()
            })
            .await
            .expect("loopback target");
        timeout(TEST_TIMEOUT, first)
            .await
            .expect("target startup deadline")
            .expect("target startup response")
            .expect("target connected");
        let (params, _, param_task) =
            ParamService::spawn_with_task(handle.clone(), FC_SYS, TARGET_COMPONENT_ID);
        *state.motor_test.target.lock() = Some(TargetLinkRuntime {
            target: MotorTestTarget {
                handle: handle.clone(),
                endpoint: format!("udpout:{address}"),
                target_system_id: FC_SYS,
                params,
                restart_pending: Default::default(),
                config: Default::default(),
                config_stale: Default::default(),
                param_names: Default::default(),
            },
            runtime: AuxiliaryLinkRuntime::new(handle.clone(), []),
        });
        {
            let target = state.motor_test.target.lock();
            let target = &target.as_ref().expect("target").target;
            let main = std::array::from_fn(|index| {
                if index < usize::from(MOTORS) {
                    FunctionRead::Value(101 + index as i64)
                } else {
                    FunctionRead::Absent
                }
            });
            let protocol = OutputProtocol::Pwm { rate_hz: Some(400) };
            *target.config.lock() = Some(OutputConfigSnapshot {
                revision: 1,
                target: target.target_ref(),
                map: build_output_map_checked(MOTORS, &main, &[FunctionRead::Absent; 8]),
                models: vec![
                    PwmModel::new(1100, 1900, false, 0.0)
                        .expect("model")
                        .with_protocol(protocol);
                    MOTORS.into()
                ],
                protocols: OutputProtocols {
                    main: protocol,
                    aux: protocol,
                },
                protocols_known: true,
                dshot_3d_enable: Some(0),
                dshot_bidir_en: Some(0),
            });
        }
        *state.motor_test.telemetry.lock() = Some(MotorTelemetry {
            esc: EscAggregator::new(MAX_ESC),
            alarm: MagAlarm::new(MagAlarmConfig::default()),
            calib: None,
            last_mag_total_nt: None,
            hb: Some(Heartbeat {
                system_id: FC_SYS,
                component_id: TARGET_COMPONENT_ID,
                vehicle_type: VehicleType::Quadrotor,
                autopilot: Autopilot::Px4,
                base_mode: BaseMode::from_raw(0),
                custom_mode: 0,
                mode_label: "Manual".into(),
                flight_state: FlightState::Standby,
                mavlink_version: 3,
            }),
            fc_outputs: [None, None],
            last_heartbeat_at: None,
            esc_seen_at: [None; MAX_ESC],
        });
        let (svc, mut events, output) =
            MotorTestService::spawn(handle.clone(), motor_test_limits(FC_SYS));
        let observer_app = app.handle().clone();
        let forwarder = tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                if let MotorTestEvent::StateChanged(session) = event {
                    observer_app
                        .state::<AppState>()
                        .motor_test
                        .note_state(session);
                }
            }
        });
        svc.send(MotorTestCommand::SetPwmModels {
            models: vec![PwmModel::new(1100, 1900, false, 0.0).expect("FC model"); MOTORS.into()],
        })
        .await
        .expect("set models");
        *state.motor_test.running.lock() = Some(RunningMotorTest {
            operation: None,
            svc,
            forwarder,
            output,
            target: target_params(&state).expect("target").0,
            scope_outputs: Vec::new(),
            tone_generation: 0,
        });
        spawn_actuator_capture(app.handle().clone(), &state, handle, FC_SYS);
        wait_until(|| {
            state
                .motor_test
                .telemetry
                .lock()
                .as_ref()
                .is_some_and(|telemetry| telemetry.last_heartbeat_at.is_some())
        })
        .await;
        // A parameter round trip also allows the motor observer to consume the
        // bootstrap heartbeat before its first start/tick.
        assert_eq!(
            read_sdlog_params(&target_params(&state).expect("target").1).await,
            (Some(0), Some(1))
        );
        Self {
            app,
            trace,
            hold_shell,
            tones,
            streams,
            tasks: vec![peer, param_task],
        }
    }

    fn state(&self) -> State<'_, AppState> {
        self.app.state::<AppState>()
    }

    fn handle(&self) -> ConnectionHandle {
        self.state()
            .motor_test
            .target
            .lock()
            .as_ref()
            .expect("target")
            .target
            .handle
            .clone()
    }

    async fn start_stream_manager(&self) -> MotorTestTargetRef {
        crate::stream_service::spawn_for_target(&self.state(), self.handle(), FC_SYS);
        wait_until(|| {
            crate::stream_service::snapshot(&self.state()).ready
                && interval_count(&self.trace.lock(), None) == 4
        })
        .await;
        target_params(&self.state()).expect("target").0
    }

    fn dshot(&self) {
        let state = self.state();
        let target = state.motor_test.target.lock();
        let mut config = target.as_ref().expect("target").target.config.lock();
        let config = config.as_mut().expect("config");
        let aux = [102, 104, 101, 103, 0, 0, 0, 0].map(FunctionRead::Value);
        config.map = build_output_map_checked(MOTORS, &[FunctionRead::Absent; 8], &aux);
        let protocol = OutputProtocol::DShot {
            rate: DShotRate::R600,
        };
        config.models =
            vec![PwmModel::dshot(0.055, false, 0.0, protocol).expect("DShot model"); MOTORS.into()];
        config.protocols.aux = protocol;
        config.revision += 1;
    }

    async fn start_with_tones(&self) {
        self.enable_control().await;
        start_source(
            self.app.handle(),
            &self.state(),
            StartSource::Manual(vec![0.1; MOTORS.into()]),
        )
        .await
        .expect("start with tone fence");
        wait_until(|| self.trace.lock().iter().any(is_active_output)).await;
    }

    async fn start_motors(&self) {
        self.enable_control().await;
        let service = self
            .state()
            .motor_test
            .running
            .lock()
            .as_ref()
            .expect("motor service")
            .svc
            .clone();
        let since = self.trace.lock().len();
        service
            .send(MotorTestCommand::SetValues(vec![0.1; MOTORS.into()]))
            .await
            .expect("demand");
        service
            .send(MotorTestCommand::StartManual)
            .await
            .expect("start motors");
        wait_until(|| self.trace.lock()[since..].iter().any(is_active_output)).await;
        assert_eq!(
            self.state().motor_test.session(),
            Some(SessionState::ManualRunning)
        );
    }

    async fn capture(&self, operation: CaptureOperation) -> SdCaptureStatus {
        timeout(TEST_TIMEOUT, operation.invoke(self.state()))
            .await
            .expect("capture deadline")
            .expect("capture succeeds")
    }

    async fn enable_control(&self) {
        crate::motor_control_service::set_control(&self.state(), true)
            .await
            .expect("explicit Motor control ON");
    }

    async fn released_since(&self, since: usize) {
        timeout(
            RELEASE_TIMEOUT,
            wait_until(|| {
                released_motors(&self.trace.lock()[since..]).len() == usize::from(MOTORS)
            }),
        )
        .await
        .expect("every release reaches the peer before capture is unblocked");
        assert_eq!(
            released_motors(&self.trace.lock()[since..]),
            (1..=MOTORS).collect()
        );
    }

    async fn shutdown(mut self) {
        let state = self.state();
        stop_active_session(&state)
            .await
            .expect("cleanup motor release");
        stop_actuator_capture(&state).await;
        crate::stream_service::stop(&state);
        let running = state.motor_test.running.lock().take();
        if let Some(running) = running {
            running.forwarder.abort();
            let _ = running.forwarder.await;
        }
        let target = state.motor_test.target.lock().take();
        if let Some(target) = target {
            target.stop().await;
        }
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
    }
}

fn owned_session(harness: &Harness, window: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(harness.app.handle())
        .targets()
        .expect("targets")
        .into_iter()
        .find(|target| {
            target
                .roles
                .contains(&maggcs_core::runtime::TargetRole::MotorTest)
        })
        .expect("Motor target")
        .target
        .expect("verified target");
    harness
        .state()
        .runtime_clients
        .attach(window, "motor-test".into(), target)
        .expect("attach")
        .0
}
fn capture_context(session: &RuntimeClientSession) -> MotorRequestContext {
    MotorRequestContext {
        session: session.clone(),
        request_id: RequestId::fresh().expect("request"),
    }
}
fn capture_stop(
    session: &RuntimeClientSession,
    operation: &maggcs_core::runtime::operations::OperationRef,
) -> MotorCaptureStopRequest {
    MotorCaptureStopRequest {
        session: session.clone(),
        operation: operation.clone(),
        request_id: RequestId::fresh().expect("request"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_capture_start_stop_is_exact_and_retains_file_identity() {
    let harness = Harness::new().await;
    let a = owned_session(&harness, "a");
    let b = owned_session(&harness, "b");
    let started = crate::motor_capture_operations::start(
        harness.app.handle(),
        "a",
        MotorCaptureStartRequest {
            context: capture_context(&a),
            test_id: "owned-capture-a".into(),
        },
    )
    .await
    .expect("confirmed recording");
    assert_eq!(started.status.phase, CapturePhase::Recording);
    assert_eq!(started.status.remote_path.as_deref(), Some(LOG_PATH));
    assert!(crate::motor_capture_operations::stop(
        harness.app.handle(),
        "b",
        capture_stop(&b, &started.operation)
    )
    .await
    .is_err());
    let stopped = crate::motor_capture_operations::stop(
        harness.app.handle(),
        "a",
        capture_stop(&a, &started.operation),
    )
    .await
    .expect("confirmed stop");
    assert_eq!(stopped.status.phase, CapturePhase::Complete);
    let status = harness
        .state()
        .runtime_clients
        .operations
        .status(&a.owner, &started.operation)
        .expect("retained");
    assert_eq!(status.phase, OperationPhase::Finished);
    assert_eq!(status.cleanup, CleanupOutcome::Confirmed);
    assert_eq!(shell_count(&harness.trace.lock(), "logger on"), 1);
    assert_eq!(shell_count(&harness.trace.lock(), "logger off"), 1);
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_owner_close_stops_its_file_without_a_renderer_callback() {
    let harness = Harness::new().await;
    let session = owned_session(&harness, "owner");
    let started = crate::motor_capture_operations::start(
        harness.app.handle(),
        "owner",
        MotorCaptureStartRequest {
            context: capture_context(&session),
            test_id: "owned-close".into(),
        },
    )
    .await
    .expect("capture");
    harness
        .state()
        .runtime_clients
        .expire_window(&session.owner.client_id)
        .expect("native close");
    wait_until(|| {
        harness
            .state()
            .runtime_clients
            .operations
            .status(&session.owner, &started.operation)
            .is_ok_and(|status| status.phase == OperationPhase::Finished)
    })
    .await;
    let status = harness
        .state()
        .runtime_clients
        .operations
        .status(&session.owner, &started.operation)
        .expect("retained");
    assert_eq!(status.cleanup, CleanupOutcome::Confirmed);
    wait_until(|| shell_count(&harness.trace.lock(), "logger off") == 1).await;
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_capture_file_is_never_stopped_as_if_it_were_owned() {
    let harness = Harness::new().await;
    let session = owned_session(&harness, "owner");
    let started = crate::motor_capture_operations::start(
        harness.app.handle(),
        "owner",
        MotorCaptureStartRequest {
            context: capture_context(&session),
            test_id: "owned-path".into(),
        },
    )
    .await
    .expect("capture");
    harness
        .state()
        .motor_test
        .capture
        .lock()
        .status
        .lock()
        .remote_path = Some("/fs/microsd/log/external.ulg".into());
    let before = shell_count(&harness.trace.lock(), "logger off");
    assert!(matches!(
        crate::motor_capture_operations::stop(
            harness.app.handle(),
            "owner",
            capture_stop(&session, &started.operation)
        )
        .await,
        Err(
            maggcs_core::motor_test::operation_types::MotorOperationError::ReconciliationRequired { .. }
        )
    ));
    assert_eq!(
        shell_count(&harness.trace.lock(), "logger off"),
        before,
        "changed external file must not receive logger off under the old operation"
    );
    let status = harness
        .state()
        .runtime_clients
        .operations
        .status(&session.owner, &started.operation)
        .expect("retained");
    assert_eq!(status.cleanup, CleanupOutcome::Uncertain);
    harness.shutdown().await;
}

impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn wait_until(mut ready: impl FnMut() -> bool) {
    timeout(TEST_TIMEOUT, async {
        while !ready() {
            tokio::time::sleep(POLL_PERIOD).await;
        }
    })
    .await
    .expect("fixture condition deadline");
}

async fn send(socket: &UdpSocket, address: SocketAddr, message: MavMessage) {
    let mut bytes = Vec::new();
    ::mavlink::write_v2_msg(
        &mut bytes,
        MavHeader {
            system_id: FC_SYS,
            component_id: TARGET_COMPONENT_ID,
            sequence: 0,
        },
        &message,
    )
    .expect("encode fake PX4 frame");
    socket
        .send_to(&bytes, address)
        .await
        .expect("send fake PX4 frame");
}

async fn fake_px4(
    socket: UdpSocket,
    app: AppHandle<MockRuntime>,
    trace: Trace,
    hold_shell: Arc<AtomicBool>,
    tones: Arc<ToneFixture>,
    streams: Arc<StreamFixture>,
) {
    let mut address = None;
    let mut bytes = [0; DATAGRAM_BYTES];
    let mut line_buffer = String::new();
    let mut shell_replies = String::new();
    let mut recording = false;
    let mut heartbeat = tokio::time::interval(HEARTBEAT_PERIOD);
    let mut traffic = tokio::time::interval(POLL_PERIOD);
    traffic.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut rates: std::collections::BTreeMap<u32, (Duration, Instant)> = Default::default();
    loop {
        tokio::select! {
            datagram = socket.recv_from(&mut bytes) => {
                let (size, peer) = datagram.expect("receive GCS frame");
                address = Some(peer);
                let (_, message) = ::mavlink::read_v2_msg::<MavMessage, _>(&mut PeekReader::new(&bytes[..size])).expect("decode GCS frame");
                match message {
                    MavMessage::PARAM_REQUEST_READ(read) => {
                        let name = maggcs_core::params::codec::decode_param_id(&read.param_id);
                        let (index, value) = match name.as_str() {
                            "SDLOG_MODE" => (0, 0),
                            "SDLOG_BACKEND" => (1, 1),
                            _ => panic!("unexpected parameter read: {name}"),
                        };
                        send(&socket, peer, MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
                            param_id: read.param_id,
                            param_type: MavParamType::MAV_PARAM_TYPE_INT32,
                            param_value: f32::from_bits(value), param_count: 2, param_index: index,
                        })).await;
                    }
                    MavMessage::PARAM_SET(_) => panic!("capture must not write parameters"),
                    MavMessage::COMMAND_LONG(command) if command.command == MavCmd::MAV_CMD_ACTUATOR_TEST => {
                        assert_eq!((command.target_system, command.target_component), (FC_SYS, TARGET_COMPONENT_ID));
                        trace.lock().push(Wire::Motor(command));
                    }
                    MavMessage::COMMAND_LONG(command) if command.command == MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL => {
                        assert_eq!((command.target_system, command.target_component), (FC_SYS, TARGET_COMPONENT_ID));
                        let id = command.param1 as u32;
                        if command.param2 < 0.0 { rates.remove(&id); }
                        else { rates.insert(id, (Duration::from_micros(command.param2 as u64), Instant::now())); }
                        trace.lock().push(Wire::Interval(command));
                    }
                    MavMessage::SERIAL_CONTROL(message) => {
                        assert_eq!((message.target_system, message.target_component), (FC_SYS, TARGET_COMPONENT_ID));
                        assert_eq!(message.device, SerialControlDev::SERIAL_CONTROL_DEV_SHELL);
                        line_buffer.push_str(std::str::from_utf8(&message.data[..usize::from(message.count)]).expect("shell UTF-8"));
                        while let Some(end) = line_buffer.find('\n') {
                            let line = line_buffer.drain(..=end).collect::<String>();
                            let line = line.trim();
                            trace.lock().push(Wire::Shell(line.into()));
                            if tones.pause_after.lock().as_deref() == Some(line) {
                                tones.pause_after.lock().take();
                                hold_shell.store(true, Ordering::SeqCst);
                            }
                            match line {
                                "" => {},
                                "logger status" => {
                                    shell_replies.push_str("INFO [logger] Running in mode: file\n");
                                    if recording {
                                        shell_replies.push_str(&format!("INFO [logger] Full File Logging Running:\nINFO [logger] Log file: {LOG_PATH}\n"));
                                    } else { shell_replies.push_str("INFO [logger] Not logging\n"); }
                                }
                                "logger on" => recording = true,
                                "logger off" => recording = false,
                                "dshot status" => {
                                    shell_replies.push_str("INFO [dshot] Outputs initialized: yes\nINFO [dshot] Outputs used: 0xf\nINFO [dshot] Outputs on: yes\nINFO [dshot] Param prefix: PWM_AUX\nChannel Configuration:\n");
                                    for (index, function) in [102, 104, 101, 103].iter().enumerate() {
                                        shell_replies.push_str(&format!("Channel {index}: func: {function}, value: 0, disarmed: 0\n"));
                                    }
                                }
                                command if command.starts_with("dshot beep") => {
                                    let words: Vec<_> = command.split_whitespace().collect();
                                    assert_eq!(words.len(), 4);
                                    assert!(["beep1", "beep2", "beep3", "beep4"].contains(&words[1]));
                                    assert_eq!(words[2], "-m");
                                    assert!(["1", "2", "3", "4"].contains(&words[3]));
                                    if tones.fail.load(Ordering::SeqCst) { shell_replies.push_str("ERROR [dshot] simulated command failure\n"); }
                                }
                                marker if marker.starts_with("echo __MAGDEL_") => {
                                    shell_replies.push_str(&marker[5..]);
                                    shell_replies.push('\n');
                                }
                                marker if marker.starts_with("echo __MAGCAP_") => {
                                    shell_replies.push_str(&marker[5..]);
                                    shell_replies.push('\n');
                                }
                                _ => panic!("unexpected shell command: {line}"),
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                if let Some(peer) = address {
                    send(&socket, peer, MavMessage::HEARTBEAT(HEARTBEAT_DATA {
                        custom_mode: 0, mavtype: MavType::MAV_TYPE_QUADROTOR,
                        autopilot: MavAutopilot::MAV_AUTOPILOT_PX4, base_mode: MavModeFlag::empty(),
                        system_status: MavState::MAV_STATE_STANDBY, mavlink_version: 3,
                    })).await;
                    if let Some(telemetry) = app.state::<AppState>().motor_test.telemetry.lock().as_mut() {
                        telemetry.last_heartbeat_at = Some(Instant::now());
                    }
                }
            }
            _ = traffic.tick() => {
                if let Some(peer) = address {
                    if !streams.paused.load(Ordering::SeqCst) {
                        let now = Instant::now();
                        for (&id, (period, due)) in &mut rates {
                            if now >= *due {
                                *due = now + *period;
                                if let Some(message) = <MavMessage as ::mavlink::Message>::default_message_from_id(id) {
                                    if streams.foreign.load(Ordering::SeqCst) {
                                        let mut wire = Vec::new();
                                        ::mavlink::write_v2_msg(&mut wire, MavHeader { system_id: FC_SYS + 1, component_id: TARGET_COMPONENT_ID, sequence: 0 }, &message).expect("foreign frame");
                                        socket.send_to(&wire, peer).await.expect("foreign send");
                                    } else { send(&socket, peer, message).await; }
                                }
                            }
                        }
                    }
                }
            }
        }
        if !hold_shell.load(Ordering::SeqCst) {
            if let Some(peer) = address {
                for chunk in std::mem::take(&mut shell_replies)
                    .as_bytes()
                    .chunks(px4_shell::SHELL_PAYLOAD)
                {
                    let mut data = [0; px4_shell::SHELL_PAYLOAD];
                    data[..chunk.len()].copy_from_slice(chunk);
                    send(
                        &socket,
                        peer,
                        MavMessage::SERIAL_CONTROL(SERIAL_CONTROL_DATA {
                            device: SerialControlDev::SERIAL_CONTROL_DEV_SHELL,
                            flags: SerialControlFlag::SERIAL_CONTROL_FLAG_REPLY,
                            target_system: GCS_SYS,
                            target_component: GCS_COMP,
                            count: chunk.len() as u8,
                            data,
                            ..Default::default()
                        }),
                    )
                    .await;
                }
            }
        }
    }
}

fn is_active_output(event: &Wire) -> bool {
    matches!(event, Wire::Motor(command) if command.param1.is_finite() && command.param2 > 0.0)
}

fn released_motors(trace: &[Wire]) -> BTreeSet<u8> {
    trace
        .iter()
        .filter_map(|event| match event {
            // Timeout zero releases test control; value zero with a positive
            // timeout would instead remain an active minimum-thrust command.
            Wire::Motor(command) if command.param2 == 0.0 => Some(command.param5 as u8),
            _ => None,
        })
        .collect()
}

fn shell_count(trace: &[Wire], expected: &str) -> usize {
    trace
        .iter()
        .filter(|event| matches!(event, Wire::Shell(line) if line == expected))
        .count()
}

fn interval_count(trace: &[Wire], id: Option<u32>) -> usize {
    trace.iter().filter(|event| matches!(event, Wire::Interval(command) if id.is_none_or(|id| command.param1 == id as f32))).count()
}

#[derive(Debug, Clone, Copy)]
enum CaptureOperation {
    Check,
    Start,
    Stop,
}

impl CaptureOperation {
    async fn invoke(self, state: State<'_, AppState>) -> Result<SdCaptureStatus, String> {
        match self {
            Self::Check => verify_capture_locked(&state).await,
            Self::Start => {
                let _ops = state.ops().lock().await;
                start_capture_locked(&state, "mr16-manual".into()).await
            }
            Self::Stop => {
                let _ops = state.ops().lock().await;
                stop_capture_locked(&state).await
            }
        }
    }
}

#[test]
fn check_and_start_waits_accept_an_existing_recording() {
    let status = SdCaptureStatus {
        phase: CapturePhase::ExistingRecording,
        ..Default::default()
    };
    assert!(wait_done(&WaitKind::Check, &status));
    assert!(wait_done(&WaitKind::Start, &status));
    assert!(!wait_done(&WaitKind::Stop, &status));
}

#[tokio::test]
async fn manual_capture_start_and_stop_leave_running_motors_alone() {
    let harness = Harness::new().await;
    harness.start_motors().await;
    let since = harness.trace.lock().len();
    assert_eq!(
        harness.capture(CaptureOperation::Start).await.phase,
        CapturePhase::Recording
    );
    assert_eq!(
        harness.capture(CaptureOperation::Stop).await.phase,
        CapturePhase::Complete
    );
    assert_eq!(
        harness.state().motor_test.session(),
        Some(SessionState::ManualRunning)
    );
    assert!(harness.state().motor_test.run.lock().active.is_none());
    let trace = harness.trace.lock()[since..].to_vec();
    assert!(trace.iter().any(is_active_output));
    assert!(
        released_motors(&trace).is_empty(),
        "capture never releases motors"
    );
    assert_eq!(shell_count(&trace, "logger on"), 1);
    assert_eq!(shell_count(&trace, "logger off"), 1);
    harness.shutdown().await;
}

#[tokio::test]
async fn manual_motor_start_and_stop_leave_recording_capture_alone() {
    let harness = Harness::new().await;
    let capture = harness.capture(CaptureOperation::Start).await;
    let since = harness.trace.lock().len();
    harness.start_motors().await;
    stop_active_session(&harness.state())
        .await
        .expect("motor stop");
    harness.released_since(since).await;
    assert_eq!(capture_status_blocking(&harness.state()), capture);
    assert_eq!(shell_count(&harness.trace.lock()[since..], "logger on"), 0);
    assert_eq!(shell_count(&harness.trace.lock()[since..], "logger off"), 0);
    harness.shutdown().await;
}

async fn assert_stop_preempts_capture(emergency: bool, operation: CaptureOperation) {
    let harness = Harness::new().await;
    if matches!(operation, CaptureOperation::Stop) {
        harness.capture(CaptureOperation::Start).await;
    }
    harness.start_motors().await;
    harness.hold_shell.store(true, Ordering::SeqCst);
    let since = harness.trace.lock().len();
    let app = harness.app.handle().clone();
    let capture = tokio::spawn(async move { operation.invoke(app.state::<AppState>()).await });
    wait_until(|| {
        harness.trace.lock()[since..]
            .iter()
            .any(|event| matches!(event, Wire::Shell(line) if line.starts_with("echo __MAGCAP_")))
    })
    .await;
    assert!(!capture.is_finished(), "capture reply is held");
    assert!(
        harness.state().ops().try_lock().is_err(),
        "real capture adapter owns ops"
    );

    timeout(RELEASE_TIMEOUT, async {
        if emergency {
            emergency_stop_active_session(&harness.state()).await
        } else {
            stop_active_session(&harness.state()).await
        }
    })
    .await
    .expect("motor stop must not wait for the capture lock")
    .expect("release succeeds");
    harness.released_since(since).await;
    assert!(
        !capture.is_finished(),
        "all releases precede the delayed capture reply"
    );
    let expected = if emergency {
        SessionState::Emergency
    } else {
        SessionState::Idle
    };
    wait_until(|| harness.state().motor_test.session() == Some(expected)).await;
    let released_at = harness.trace.lock().len();
    harness.hold_shell.store(false, Ordering::SeqCst);
    timeout(TEST_TIMEOUT, capture)
        .await
        .expect("capture resumes")
        .expect("capture task")
        .expect("capture result");
    assert_eq!(
        harness.state().motor_test.session(),
        Some(expected),
        "late capture completion cannot restart or reset motors"
    );
    assert!(!harness.trace.lock()[released_at..]
        .iter()
        .any(is_active_output));
    harness.shutdown().await;
}

#[tokio::test]
async fn stop_preempts_pending_capture_check_start_and_stop() {
    for operation in [
        CaptureOperation::Check,
        CaptureOperation::Start,
        CaptureOperation::Stop,
    ] {
        assert_stop_preempts_capture(false, operation).await;
    }
}

#[tokio::test]
async fn estop_preempts_pending_capture_check_start_and_stop() {
    for operation in [
        CaptureOperation::Check,
        CaptureOperation::Start,
        CaptureOperation::Stop,
    ] {
        assert_stop_preempts_capture(true, operation).await;
    }
}

#[tokio::test]
async fn coordinated_capture_path_orders_preroll_release_and_postroll() {
    let harness = Harness::new().await;
    let state = harness.state();
    let capture = {
        let _ops = state.ops().lock().await;
        ensure_coordinated_run_start(&state).expect("coordinated run starts idle");
        start_capture_locked(&state, "mr16-coordinated".into())
            .await
            .expect("new capture")
    };
    let now = Instant::now();
    let (mut run, _) = RunCoordinator::begin(RunConfig::new("mr16-coordinated"), now);
    run.observe_capture(&capture, now);
    let just_before = Duration::from_nanos(1);
    assert_eq!(
        run.note_motor_started(now + PRE_ROLL - just_before),
        Err(RunError::MotorNotReady)
    );
    assert!(!harness.trace.lock().iter().any(is_active_output));
    assert_eq!(
        run.tick(now + PRE_ROLL).status.expect("ready").phase,
        RunPhase::MotorReady
    );
    harness.start_motors().await;
    run.note_motor_started(now + PRE_ROLL)
        .expect("motor transport started");
    assert!(
        ensure_coordinated_run_start(&state).is_err(),
        "a running motor session cannot begin a coordinated capture"
    );
    assert!(
        run.tick(now + PRE_ROLL + POST_ROLL).actions.is_empty(),
        "elapsed time cannot substitute for release"
    );
    let since = harness.trace.lock().len();
    stop_active_session(&state)
        .await
        .expect("confirmed release");
    harness.released_since(since).await;
    let release_at = now + PRE_ROLL + POST_ROLL;
    run.note_motor_released(
        maggcs_core::motor_test::service::StopReason::Command,
        release_at,
    );
    assert!(run
        .tick(release_at + POST_ROLL - just_before)
        .actions
        .is_empty());
    assert_eq!(shell_count(&harness.trace.lock(), "logger off"), 0);
    assert_eq!(
        run.tick(release_at + POST_ROLL).actions,
        vec![RunAction::StopCapture]
    );
    let target = target_params(&state).expect("target").0;
    let complete = request_stop_for_target(&state, target)
        .await
        .expect("coordinated capture stop");
    assert_eq!(
        run.observe_capture(&complete, release_at + POST_ROLL)
            .status
            .expect("complete")
            .phase,
        RunPhase::Complete
    );
    let trace = harness.trace.lock().clone();
    let off = trace
        .iter()
        .position(|event| matches!(event, Wire::Shell(line) if line == "logger off"))
        .expect("logger off was sent");
    assert_eq!(
        released_motors(&trace[since..off]),
        (1..=MOTORS).collect(),
        "all motor releases arrive before logger off"
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn coordinated_capture_stop_rejects_an_obsolete_target() {
    let harness = Harness::new().await;
    let mut obsolete = target_params(&harness.state()).expect("target").0;
    obsolete.connection_id += 1;
    assert_eq!(
        request_stop_for_target(&harness.state(), obsolete).await,
        Err(SdCaptureError::TargetChanged.to_string())
    );
    assert_eq!(shell_count(&harness.trace.lock(), "logger off"), 0);
    harness.shutdown().await;
}

#[tokio::test]
async fn am32_start_precedes_output_and_stop_follows_all_release_writes() {
    let harness = Harness::new().await;
    harness.dshot();
    harness.start_with_tones().await;
    let trace = harness.trace.lock().clone();
    let active = trace
        .iter()
        .position(is_active_output)
        .expect("active output");
    for channel in 1..=MOTORS {
        assert_eq!(
            shell_count(&trace[..active], &format!("dshot beep1 -m {channel}")),
            1
        );
    }
    let since = trace.len();
    timeout(RELEASE_TIMEOUT, stop_active_session(&harness.state()))
        .await
        .expect("stop ignores tone duration")
        .expect("stop");
    harness.released_since(since).await;
    wait_until(|| shell_count(&harness.trace.lock(), "dshot beep2 -m 2") == 1).await;
    let trace = harness.trace.lock().clone();
    let stop_tone = trace
        .iter()
        .position(|event| matches!(event, Wire::Shell(line) if line.starts_with("dshot beep2")))
        .expect("stop tone");
    assert_eq!(
        released_motors(&trace[since..stop_tone]),
        (1..=MOTORS).collect()
    );
    wait_until(|| !harness.state().motor_test.tones.snapshot().busy).await;
    stop_active_session(&harness.state())
        .await
        .expect("idempotent stop");
    assert_eq!(
        shell_count(&harness.trace.lock(), "dshot beep2 -m 3"),
        1,
        "one cue per run"
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn am32_pending_start_is_cancelled_by_both_stop_paths() {
    for emergency in [false, true] {
        let harness = Harness::new().await;
        harness.dshot();
        harness.enable_control().await;
        harness.hold_shell.store(true, Ordering::SeqCst);
        let app = harness.app.handle().clone();
        let start = tokio::spawn(async move {
            start_source(
                &app,
                &app.state::<AppState>(),
                StartSource::Manual(vec![0.1; MOTORS.into()]),
            )
            .await
        });
        wait_until(|| shell_count(&harness.trace.lock(), "dshot status") == 1).await;
        if emergency {
            emergency_stop_active_session(&harness.state())
                .await
                .expect("E-STOP");
        } else {
            stop_active_session(&harness.state()).await.expect("STOP");
        }
        let error = timeout(RELEASE_TIMEOUT, start)
            .await
            .expect("cancel promptly")
            .expect("start task")
            .expect_err("cancelled start");
        assert!(error.contains("cancelled"));
        harness.hold_shell.store(false, Ordering::SeqCst);
        wait_until(|| !harness.state().motor_test.tones.snapshot().busy).await;
        assert!(!harness.trace.lock().iter().any(is_active_output));
        assert_eq!(
            shell_count(&harness.trace.lock(), "dshot beep1 -m 3"),
            0,
            "cancelled queued beeps are not sent"
        );
        if emergency {
            assert_eq!(
                harness.state().motor_test.session(),
                Some(SessionState::Emergency)
            );
            crate::actuator_service::reset_emergency_legacy(&harness.state())
                .await
                .expect("reset without a spawned session");
            assert_eq!(
                harness.state().motor_test.session(),
                Some(SessionState::Idle)
            );
        }
        harness.shutdown().await;
    }
}

#[tokio::test]
async fn am32_next_off_start_waits_for_an_already_dispatched_stop_tone() {
    let harness = Harness::new().await;
    harness.dshot();
    set_tones_legacy(
        &harness.state(),
        MotorToneSettings {
            start: MotorTone::Off,
            stop: MotorTone::Rising,
        },
    )
    .await
    .expect("settings");
    harness.start_with_tones().await;
    *harness.tones.pause_after.lock() = Some("dshot beep1 -m 3".into());
    stop_active_session(&harness.state()).await.expect("stop");
    wait_until(|| harness.hold_shell.load(Ordering::SeqCst)).await;
    let since = harness.trace.lock().len();
    let app = harness.app.handle().clone();
    let next = tokio::spawn(async move {
        start_source(
            &app,
            &app.state::<AppState>(),
            StartSource::Manual(vec![0.1; MOTORS.into()]),
        )
        .await
    });
    wait_until(|| harness.state().motor_test.tones.snapshot().starting).await;
    assert!(!next.is_finished());
    assert!(!harness.trace.lock()[since..].iter().any(is_active_output));
    let resumed = Instant::now();
    harness.hold_shell.store(false, Ordering::SeqCst);
    timeout(TEST_TIMEOUT, next)
        .await
        .expect("next start")
        .expect("task")
        .expect("start after fence");
    assert!(
        resumed.elapsed() >= MotorTone::Rising.duration(),
        "Off cannot bypass the previous sound's quiet fence"
    );
    assert_eq!(shell_count(&harness.trace.lock(), "dshot beep1 -m 3"), 1);
    assert_eq!(
        shell_count(&harness.trace.lock(), "dshot beep1 -m 1"),
        0,
        "cancelled remainder never plays"
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn am32_shell_failure_blocks_start_but_does_not_fail_motor_stop() {
    let harness = Harness::new().await;
    harness.dshot();
    harness.enable_control().await;
    harness.tones.fail.store(true, Ordering::SeqCst);
    let error = start_source(
        harness.app.handle(),
        &harness.state(),
        StartSource::Manual(vec![0.1; MOTORS.into()]),
    )
    .await
    .expect_err("beep failed");
    assert!(error.contains("simulated command failure"));
    assert!(!harness.trace.lock().iter().any(is_active_output));
    set_tones_legacy(
        &harness.state(),
        MotorToneSettings {
            start: MotorTone::Off,
            stop: MotorTone::Falling,
        },
    )
    .await
    .expect("turn off start cue");
    harness.start_with_tones().await;
    stop_active_session(&harness.state())
        .await
        .expect("motor release succeeds despite failing audio");
    wait_until(|| harness.state().motor_test.tones.snapshot().error.is_some()).await;
    assert_eq!(
        harness.state().motor_test.session(),
        Some(SessionState::Idle)
    );
    assert!(harness
        .state()
        .motor_test
        .tones
        .snapshot()
        .error
        .expect("tone diagnostic")
        .contains("simulated command failure"));
    harness.shutdown().await;
}

#[tokio::test]
async fn am32_configuration_change_during_tone_preparation_blocks_start() {
    let harness = Harness::new().await;
    harness.dshot();
    harness.enable_control().await;
    harness.hold_shell.store(true, Ordering::SeqCst);
    let app = harness.app.handle().clone();
    let start = tokio::spawn(async move {
        start_source(
            &app,
            &app.state::<AppState>(),
            StartSource::Manual(vec![0.1; MOTORS.into()]),
        )
        .await
    });
    wait_until(|| shell_count(&harness.trace.lock(), "dshot status") == 1).await;
    {
        let state = harness.state();
        let target = state.motor_test.target.lock();
        target
            .as_ref()
            .expect("target")
            .target
            .config
            .lock()
            .as_mut()
            .expect("config")
            .revision += 1;
    }
    harness.hold_shell.store(false, Ordering::SeqCst);
    assert!(timeout(TEST_TIMEOUT, start)
        .await
        .expect("deadline")
        .expect("task")
        .expect_err("changed config")
        .contains("changed"));
    assert!(!harness.trace.lock().iter().any(is_active_output));
    assert_eq!(shell_count(&harness.trace.lock(), "dshot beep1 -m 3"), 0);
    harness.shutdown().await;
}

#[tokio::test]
async fn pwm_start_ignores_saved_dshot_tones() {
    let harness = Harness::new().await;
    harness.start_with_tones().await;
    stop_active_session(&harness.state())
        .await
        .expect("PWM stop");
    assert!(!harness
        .trace
        .lock()
        .iter()
        .any(|event| matches!(event, Wire::Shell(line) if line.starts_with("dshot "))));
    harness.shutdown().await;
}

#[tokio::test]
async fn am32_preset_completion_plays_one_stop_tone() {
    use maggcs_core::motor_test::source::{PresetKind, WaveformPreset};
    let harness = Harness::new().await;
    harness.dshot();
    harness.enable_control().await;
    set_tones_legacy(
        &harness.state(),
        MotorToneSettings {
            start: MotorTone::Off,
            stop: MotorTone::Triple,
        },
    )
    .await
    .expect("settings");
    start_source(
        harness.app.handle(),
        &harness.state(),
        StartSource::Preset(WaveformPreset {
            kind: PresetKind::Step,
            amplitude: 0.1,
            frequency_hz: 1.0,
            duration_s: 0.3,
        }),
    )
    .await
    .expect("preset");
    wait_until(|| shell_count(&harness.trace.lock(), "dshot beep4 -m 2") == 1).await;
    let trace = harness.trace.lock().clone();
    let cue = trace
        .iter()
        .position(|event| matches!(event, Wire::Shell(line) if line.starts_with("dshot beep4")))
        .expect("completion tone");
    assert_eq!(released_motors(&trace[..cue]), (1..=MOTORS).collect());
    harness.shutdown().await;
}

#[tokio::test]
async fn am32_busy_capture_cannot_delay_release_or_be_changed_by_stop_tones() {
    let harness = Harness::new().await;
    harness.dshot();
    harness.start_with_tones().await;
    harness.hold_shell.store(true, Ordering::SeqCst);
    let app = harness.app.handle().clone();
    let capture = tokio::spawn(async move {
        async {
            let state = app.state::<AppState>();
            let _ops = state.ops().lock().await;
            start_capture_locked(&state, "tone-capture".into()).await
        }
        .await
    });
    wait_until(|| shell_count(&harness.trace.lock(), "logger status") == 1).await;
    let since = harness.trace.lock().len();
    timeout(RELEASE_TIMEOUT, stop_active_session(&harness.state()))
        .await
        .expect("stop independent of capture")
        .expect("release");
    harness.released_since(since).await;
    wait_until(|| harness.state().motor_test.tones.snapshot().error.is_some()).await;
    assert!(harness
        .state()
        .motor_test
        .tones
        .snapshot()
        .error
        .expect("tone error")
        .contains("busy"));
    assert_eq!(shell_count(&harness.trace.lock(), "dshot beep2 -m 3"), 0);
    assert!(!capture.is_finished());
    harness.hold_shell.store(false, Ordering::SeqCst);
    assert_eq!(
        timeout(TEST_TIMEOUT, capture)
            .await
            .expect("capture resumes")
            .expect("task")
            .expect("capture result")
            .phase,
        CapturePhase::Recording
    );
    assert_eq!(shell_count(&harness.trace.lock(), "logger off"), 0);
    harness.shutdown().await;
}

#[tokio::test]
async fn am32_shell_timeout_and_teardown_leave_no_start_or_busy_latch() {
    let harness = Harness::new().await;
    harness.dshot();
    harness.enable_control().await;
    harness.hold_shell.store(true, Ordering::SeqCst);
    let result = timeout(
        Duration::from_secs(8),
        start_source(
            harness.app.handle(),
            &harness.state(),
            StartSource::Manual(vec![0.1; MOTORS.into()]),
        ),
    )
    .await
    .expect("bounded tone timeout");
    assert!(result.expect_err("lost tone reply").contains("timed out"));
    assert!(!harness.trace.lock().iter().any(is_active_output));
    assert!(!harness.state().motor_test.tones.snapshot().busy);
    let app = harness.app.handle().clone();
    let again = tokio::spawn(async move {
        start_source(
            &app,
            &app.state::<AppState>(),
            StartSource::Manual(vec![0.1; MOTORS.into()]),
        )
        .await
    });
    wait_until(|| shell_count(&harness.trace.lock(), "dshot status") == 2).await;
    stop_actuator_capture(&harness.state()).await;
    assert!(timeout(RELEASE_TIMEOUT, again)
        .await
        .expect("cancelled on teardown")
        .expect("task")
        .is_err());
    assert!(
        !harness.state().motor_test.tones.snapshot().busy,
        "dropping queued/running jobs releases the busy count"
    );
    assert!(!harness.trace.lock().iter().any(is_active_output));
    harness.shutdown().await;
}

#[tokio::test]
async fn am32_disconnect_cancels_start_before_acquiring_the_operation_lock() {
    let harness = Harness::new().await;
    harness.dshot();
    harness.enable_control().await;
    harness.hold_shell.store(true, Ordering::SeqCst);
    let app = harness.app.handle().clone();
    let start = tokio::spawn(async move {
        start_source(
            &app,
            &app.state::<AppState>(),
            StartSource::Manual(vec![0.1; MOTORS.into()]),
        )
        .await
    });
    wait_until(|| shell_count(&harness.trace.lock(), "dshot status") == 1).await;
    timeout(
        TEST_TIMEOUT,
        crate::actuator_service::actuator_disconnect_target(
            harness.app.handle().clone(),
            harness.state(),
        ),
    )
    .await
    .expect("disconnect does not wait for a cancelled start")
    .expect("disconnect");
    assert!(start
        .await
        .expect("start task")
        .expect_err("cancelled start")
        .contains("cancelled"));
    assert!(harness.state().motor_test.target.lock().is_none());
    assert!(!harness.state().motor_test.tones.snapshot().busy);
    assert!(!harness.trace.lock().iter().any(is_active_output));
    harness.shutdown().await;
}

fn stream_request(message_id: u32, rate: StreamRate) -> StreamRequest {
    StreamRequest {
        message_id,
        rate,
        allow_unknown_support: false,
    }
}

#[tokio::test]
async fn motor_control_defaults_off_and_blocks_all_sources_and_tones() {
    use maggcs_core::motor_test::source::{PresetKind, WaveformPreset};
    let harness = Harness::new().await;
    harness.dshot();
    assert!(!crate::motor_control_service::status(&harness.state()).enabled);
    for source in [
        StartSource::Manual(vec![0.1; MOTORS.into()]),
        StartSource::Preset(WaveformPreset {
            kind: PresetKind::Step,
            amplitude: 0.1,
            frequency_hz: 1.0,
            duration_s: 1.0,
        }),
        StartSource::Forward,
    ] {
        assert!(start_source(harness.app.handle(), &harness.state(), source)
            .await
            .expect_err("OFF")
            .contains("OFF"));
    }
    assert!(
        crate::actuator_service::set_values_legacy(&harness.state(), vec![0.3; MOTORS.into()])
            .await
            .is_err()
    );
    stop_active_session(&harness.state())
        .await
        .expect("idle stop");
    emergency_stop_active_session(&harness.state())
        .await
        .expect("idle emergency");
    let trace = harness.trace.lock().clone();
    assert!(
        !trace.iter().any(|event| matches!(event, Wire::Motor(_))),
        "no active or release ACTUATOR_TEST frames while unowned"
    );
    assert!(!trace
        .iter()
        .any(|event| matches!(event, Wire::Shell(line) if line.starts_with("dshot "))));
    harness.shutdown().await;
}

#[tokio::test]
async fn motor_control_enable_is_passive_and_off_releases_owned_outputs_without_tone() {
    let harness = Harness::new().await;
    harness.dshot();
    harness.enable_control().await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(!harness
        .trace
        .lock()
        .iter()
        .any(|event| matches!(event, Wire::Motor(_) | Wire::Shell(_))));
    harness.start_with_tones().await;
    let since = harness.trace.lock().len();
    let status = crate::motor_control_service::set_control(&harness.state(), false)
        .await
        .expect("OFF release");
    assert!(!status.enabled);
    harness.released_since(since).await;
    wait_until(|| !harness.state().motor_test.tones.snapshot().busy).await;
    assert!(!harness.trace.lock()[since..].iter().any(is_active_output));
    assert_eq!(
        shell_count(&harness.trace.lock(), "dshot beep2 -m 3"),
        0,
        "OFF suppresses the stop cue"
    );
    assert!(start_source(
        harness.app.handle(),
        &harness.state(),
        StartSource::Manual(vec![0.1; MOTORS.into()])
    )
    .await
    .is_err());
    harness.shutdown().await;
}

#[tokio::test]
async fn motor_control_off_cancels_pending_start_without_taking_output_control() {
    let harness = Harness::new().await;
    harness.dshot();
    harness.enable_control().await;
    harness.hold_shell.store(true, Ordering::SeqCst);
    let app = harness.app.handle().clone();
    let start = tokio::spawn(async move {
        start_source(
            &app,
            &app.state::<AppState>(),
            StartSource::Manual(vec![0.1; MOTORS.into()]),
        )
        .await
    });
    wait_until(|| shell_count(&harness.trace.lock(), "dshot status") == 1).await;
    timeout(
        RELEASE_TIMEOUT,
        crate::motor_control_service::set_control(&harness.state(), false),
    )
    .await
    .expect("OFF does not wait on the shell")
    .expect("OFF");
    assert!(start.await.expect("start task").is_err());
    harness.hold_shell.store(false, Ordering::SeqCst);
    wait_until(|| !harness.state().motor_test.tones.snapshot().busy).await;
    assert!(!harness
        .trace
        .lock()
        .iter()
        .any(|event| matches!(event, Wire::Motor(_))));
    assert_eq!(shell_count(&harness.trace.lock(), "dshot beep1 -m 3"), 0);
    harness.shutdown().await;
}

#[tokio::test]
async fn motor_control_permission_does_not_survive_transport_reconnect() {
    let harness = Harness::new().await;
    harness.enable_control().await;
    let old = harness.handle().generation();
    harness.handle().reconnect().await.expect("reconnect");
    wait_until(|| harness.handle().generation() > old).await;
    assert!(!crate::motor_control_service::status(&harness.state()).enabled);
    assert!(start_source(
        harness.app.handle(),
        &harness.state(),
        StartSource::Manual(vec![0.1; MOTORS.into()])
    )
    .await
    .is_err());
    assert!(!harness
        .trace
        .lock()
        .iter()
        .any(|event| matches!(event, Wire::Motor(_))));
    harness.shutdown().await;
}

#[tokio::test]
async fn an_enable_waiting_on_ops_cannot_undo_a_newer_off_request() {
    let harness = Harness::new().await;
    {
        let state = harness.state();
        let held = state.ops().lock().await;
        let mut enable = std::pin::pin!(crate::motor_control_service::set_control(&state, true));
        tokio::select! {
            biased;
            _ = &mut enable => panic!("enable must wait for ops"),
            _ = tokio::task::yield_now() => {}
        }
        crate::motor_control_service::set_control(&state, false)
            .await
            .expect("OFF bypasses ops");
        drop(held);
        assert!(
            enable.await.is_err(),
            "superseded enable cannot turn control back on"
        );
        assert!(!crate::motor_control_service::status(&state).enabled);
        assert!(!harness
            .trace
            .lock()
            .iter()
            .any(|event| matches!(event, Wire::Motor(_))));
    }
    harness.shutdown().await;
}

#[tokio::test]
async fn failed_off_release_keeps_control_revoked_and_blocks_new_starts() {
    let harness = Harness::new().await;
    harness.start_motors().await;
    let handle = harness.handle();
    handle.shutdown().await;
    timeout(TEST_TIMEOUT, async {
        loop {
            if handle
                .send_confirmed(MavMessage::HEARTBEAT(HEARTBEAT_DATA::default()))
                .await
                .is_err()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker closes");
    let error = crate::motor_control_service::set_control(&harness.state(), false)
        .await
        .expect_err("failed release remains an error");
    assert!(error.to_string().contains("release is incomplete"));
    assert!(!crate::motor_control_service::status(&harness.state()).enabled);
    assert!(start_source(
        harness.app.handle(),
        &harness.state(),
        StartSource::Manual(vec![0.1; MOTORS.into()])
    )
    .await
    .is_err());
    let running = harness.state().motor_test.running.lock().take();
    if let Some(running) = running {
        running.forwarder.abort();
        let _ = running.forwarder.await;
    }
    stop_actuator_capture(&harness.state()).await;
    harness.shutdown().await;
}

fn stream_status(harness: &Harness, id: u32) -> Option<StreamStatus> {
    crate::stream_service::snapshot(&harness.state())
        .statuses
        .into_iter()
        .find(|status| status.message_id == id)
}

#[tokio::test]
async fn stream_rates_apply_once_report_arrivals_and_disable_without_reassertion() {
    let harness = Harness::new().await;
    let target = harness.start_stream_manager().await;
    let before = interval_count(&harness.trace.lock(), Some(147));
    crate::stream_service::apply(
        &harness.state(),
        target,
        stream_request(147, StreamRate::Hertz(20.0)),
    )
    .await
    .expect("set battery rate");
    wait_until(|| {
        stream_status(&harness, 147).is_some_and(|status| {
            status
                .observed_hz
                .is_some_and(|hz| (15.0..=25.0).contains(&hz))
        })
    })
    .await;
    assert_eq!(interval_count(&harness.trace.lock(), Some(147)), before + 1);
    assert!(harness.trace.lock().iter().any(|event| matches!(event, Wire::Interval(command) if command.param1 == 147.0 && command.param2 == 50_000.0)));
    crate::stream_service::apply(
        &harness.state(),
        target,
        stream_request(147, StreamRate::Disabled),
    )
    .await
    .expect("disable");
    wait_until(|| {
        stream_status(&harness, 147)
            .is_some_and(|status| status.receive_state == StreamReceiveState::Stale)
    })
    .await;
    assert_eq!(interval_count(&harness.trace.lock(), Some(147)), before + 2);
    let disabled = stream_status(&harness, 147).expect("disabled row");
    assert_eq!(disabled.requested_rate, StreamRate::Disabled);
    assert_eq!(disabled.observed_hz, None);
    harness.shutdown().await;
}

#[tokio::test]
async fn stream_validation_and_foreign_frames_never_change_the_selected_target() {
    let harness = Harness::new().await;
    let target = harness.start_stream_manager().await;
    let before = interval_count(&harness.trace.lock(), None);
    for request in [
        stream_request(147, StreamRate::Hertz(f64::NAN)),
        stream_request(147, StreamRate::Hertz(0.0)),
        stream_request(999_999, StreamRate::Hertz(10.0)),
        stream_request(43, StreamRate::Hertz(10.0)),
    ] {
        assert!(
            crate::stream_service::apply(&harness.state(), target, request)
                .await
                .is_err()
        );
    }
    let obsolete = MotorTestTargetRef {
        connection_id: target.connection_id + 1,
        ..target
    };
    assert!(crate::stream_service::apply(
        &harness.state(),
        obsolete,
        stream_request(147, StreamRate::Hertz(10.0))
    )
    .await
    .is_err());
    assert_eq!(interval_count(&harness.trace.lock(), None), before);
    harness.streams.foreign.store(true, Ordering::SeqCst);
    crate::stream_service::apply(
        &harness.state(),
        target,
        stream_request(26, StreamRate::Hertz(10.0)),
    )
    .await
    .expect("scaled IMU");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        stream_status(&harness, 26).expect("IMU").last_seen_age_ms,
        None,
        "foreign traffic is not an observation"
    );
    harness.streams.foreign.store(false, Ordering::SeqCst);
    wait_until(|| stream_status(&harness, 26).is_some_and(|status| status.observed_hz.is_some()))
        .await;
    harness.streams.paused.store(true, Ordering::SeqCst);
    wait_until(|| {
        stream_status(&harness, 26)
            .is_some_and(|status| status.receive_state == StreamReceiveState::Stale)
    })
    .await;
    assert_eq!(
        interval_count(&harness.trace.lock(), Some(26)),
        1,
        "staleness never silently overwrites an operator rate"
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn stream_reconnect_restores_overrides_once_and_rejects_old_generation_writes() {
    let harness = Harness::new().await;
    let target = harness.start_stream_manager().await;
    crate::stream_service::apply(
        &harness.state(),
        target,
        stream_request(147, StreamRate::Hertz(25.0)),
    )
    .await
    .expect("override");
    crate::stream_service::apply(
        &harness.state(),
        target,
        stream_request(26, StreamRate::Disabled),
    )
    .await
    .expect("disabled saved request");
    wait_until(|| interval_count(&harness.trace.lock(), None) == 6).await;
    let before = interval_count(&harness.trace.lock(), None);
    let handle = harness.handle();
    let old_generation = handle.generation();
    handle.reconnect().await.expect("reconnect");
    wait_until(|| {
        let snapshot = crate::stream_service::snapshot(&harness.state());
        snapshot.ready
            && snapshot.transport_generation > old_generation
            && interval_count(&harness.trace.lock(), None) >= before + 5
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        interval_count(&harness.trace.lock(), None),
        before + 5,
        "one restore per effective ID, not default plus override"
    );
    let frame = MavMessage::COMMAND_LONG(COMMAND_LONG_DATA {
        command: MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL,
        param1: 147.0,
        param2: 123_456.0,
        target_system: FC_SYS,
        target_component: TARGET_COMPONENT_ID,
        ..Default::default()
    });
    assert!(handle
        .send_in_generation(
            frame.clone(),
            old_generation,
            tokio::time::Instant::now() + RELEASE_TIMEOUT
        )
        .await
        .is_err());
    assert!(handle
        .send_in_generation(
            frame,
            handle.generation(),
            tokio::time::Instant::now() - POLL_PERIOD
        )
        .await
        .is_err());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        interval_count(&harness.trace.lock(), None),
        before + 5,
        "stale/expired frames never reach the new socket"
    );
    let restored = stream_status(&harness, 147).expect("restored battery");
    assert_eq!(restored.requested_rate, StreamRate::Hertz(25.0));
    assert_eq!(
        stream_status(&harness, 26)
            .expect("restored disabled")
            .requested_rate,
        StreamRate::Disabled
    );
    // Window/service reattachment uses the same per-connection preferences.
    crate::stream_service::spawn_for_target(&harness.state(), handle, FC_SYS);
    wait_until(|| {
        crate::stream_service::snapshot(&harness.state()).ready
            && interval_count(&harness.trace.lock(), None) >= before + 10
    })
    .await;
    assert_eq!(
        stream_status(&harness, 147)
            .expect("reattached override")
            .requested_rate,
        StreamRate::Hertz(25.0)
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn stream_preference_io_does_not_hold_up_motor_stop() {
    assert_stream_io_stop(false).await;
    assert_stream_io_stop(true).await;
}

async fn assert_stream_io_stop(emergency: bool) {
    let harness = Harness::new().await;
    let target = harness.start_stream_manager().await;
    harness.start_motors().await;
    let profiles = harness.state().motor_test.stream_profiles.clone();
    let (locked, lock_ready) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let blocker = tokio::task::spawn_blocking(move || {
        let _held = profiles.lock();
        let _ = locked.send(());
        let _ = released.recv();
    });
    lock_ready.await.expect("profile store locked");
    let app = harness.app.handle().clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let apply = tokio::spawn(async move {
        let _ = started.send(());
        crate::stream_service::apply(
            &app.state::<AppState>(),
            target,
            stream_request(147, StreamRate::Hertz(30.0)),
        )
        .await
    });
    waiting.await.expect("request task");
    tokio::task::yield_now().await;
    assert!(!apply.is_finished());
    let since = harness.trace.lock().len();
    timeout(RELEASE_TIMEOUT, async {
        if emergency {
            emergency_stop_active_session(&harness.state()).await
        } else {
            stop_active_session(&harness.state()).await
        }
    })
    .await
    .expect("stop bypasses stream persistence")
    .expect("motor release");
    harness.released_since(since).await;
    assert!(
        !apply.is_finished(),
        "profile I/O is still pending after motor release"
    );
    release.send(()).expect("unblock profile persistence");
    blocker.await.expect("blocker task");
    apply
        .await
        .expect("apply task")
        .expect("request finishes independently");
    if emergency {
        assert_eq!(
            harness.state().motor_test.session(),
            Some(SessionState::Emergency)
        );
    }
    harness.shutdown().await;
}
