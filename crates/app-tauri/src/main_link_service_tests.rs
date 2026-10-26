//! OS pseudo-terminal tests: the serial transport, router, primary parameter
//! worker and Motor Test attachment are production code; only PX4 is simulated.
use super::*;
use std::io::{Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use maggcs_core::mavlink::{ConnectionConfig, ConnectionHandle, MavMessage};
use maggcs_core::params::{codec::decode_param_id, model::ParamValue};
use serialport::SerialPort;
use tauri::test::{mock_app, MockRuntime};

const FC_SYS: u8 = 42;
const WAIT: Duration = Duration::from_secs(8);
type Trace = Arc<parking_lot::Mutex<Vec<MavMessage>>>;

struct Harness {
    app: tauri::App<MockRuntime>,
    handle: ConnectionHandle,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    trace: Trace,
}

impl Harness {
    async fn new() -> Self {
        let (mut master, slave) = serialport::TTYPort::pair().expect("PTY pair");
        master
            .set_timeout(Duration::from_millis(10))
            .expect("PTY timeout");
        let port = slave.name().expect("PTY slave path");
        drop(slave);
        let stop = Arc::new(AtomicBool::new(false));
        let reader_stop = stop.clone();
        let trace = Trace::default();
        let reader_trace = trace.clone();
        let thread = std::thread::spawn(move || fake_px4(master, reader_stop, reader_trace));
        let (handle, events, first) =
            maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
                endpoint: Endpoint::Serial {
                    port: port.into(),
                    baudrate: 115_200,
                },
                target_system_id: FC_SYS,
                target_component_id: TARGET_COMPONENT_ID,
                auto_discover_target: false,
                reconnect_delay: Duration::from_millis(50),
                ..Default::default()
            })
            .await
            .expect("serial connection");
        first.await.expect("first result").expect("serial opened");
        let app = mock_app();
        app.manage(AppState::default());
        let state = app.state::<AppState>();
        let hub = maggcs_core::telemetry::hub::TelemetryHub::spawn(
            events,
            20,
            maggcs_core::telemetry::VehicleId::new(FC_SYS, 1),
        );
        let (mission, _) = maggcs_core::mission::service::MissionService::spawn(
            handle.clone(),
            maggcs_core::mission::service::MissionIds {
                self_system: handle.config().system_id,
                self_component: handle.config().component_id,
                target_system: FC_SYS,
                target_component: TARGET_COMPONENT_ID,
            },
        );
        let (command, _) = maggcs_core::CommandService::spawn(handle.clone(), FC_SYS, 1);
        state.set_primary_link(crate::state::ActiveLink {
            id: crate::state::LinkId::next(),
            connection: handle.clone(),
            hub,
            mission,
            command,
            logs: None,
            params: None,
            rtk: None,
        });
        let params = state.params.spawn(app.handle(), handle.clone());
        state.set_primary_params(params);
        state.publish_link(maggcs_core::mavlink::connection::LinkStatus::alive(
            handle.config().endpoint.to_address_string(),
        ));
        wait_until(|| candidate(&state).is_some()).await;
        Self {
            app,
            handle,
            stop,
            thread: Some(thread),
            trace,
        }
    }

    fn state(&self) -> State<'_, AppState> {
        self.app.state::<AppState>()
    }

    async fn attach(&self) -> MainSerialTarget {
        let expected = candidate(&self.state()).expect("main serial offer");
        actuator_use_main_target(self.app.handle().clone(), self.state(), expected.clone())
            .await
            .expect("attach");
        assert!(
            self.state().actuator_status().target_fc_alive,
            "a borrowed serial FC is available before another heartbeat"
        );
        expected
    }

    async fn shutdown(mut self) {
        cleanup(self.app.handle(), &self.state()).await;
        if let Some(logs) = self.state().take_primary_logs() {
            let _ = logs.stop(Duration::from_secs(1)).await;
        }
        if let Some(params) = self.state().take_primary_params() {
            let _ = params.stop(Duration::from_secs(1)).await;
        }
        if let Some(link) = self.state().take_primary_link() {
            link.hub.shutdown();
            link.connection.shutdown().await;
        }
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("PTY peer joins");
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

async fn wait_until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture deadline");
}

fn send(master: &mut serialport::TTYPort, message: &MavMessage, system_id: u8, sequence: &mut u8) {
    let mut bytes = Vec::new();
    mavlink::write_v2_msg(
        &mut bytes,
        mavlink::MavHeader {
            system_id,
            component_id: 1,
            sequence: *sequence,
        },
        message,
    )
    .expect("encode fixture frame");
    *sequence = sequence.wrapping_add(1);
    let _ = master.write_all(&bytes); // No open slave yet during startup/reconnect.
}

fn fake_px4(master: serialport::TTYPort, stop: Arc<AtomicBool>, trace: Trace) {
    fake_px4_for_system(master, stop, trace, FC_SYS);
}

fn fake_px4_for_system(
    mut master: serialport::TTYPort,
    stop: Arc<AtomicBool>,
    trace: Trace,
    system_id: u8,
) {
    use mavlink::common::*;
    let mut buffer = Vec::new();
    let mut bytes = [0; 2048];
    let mut last_heartbeat = Instant::now() - Duration::from_secs(1);
    let mut sequence = 0;
    while !stop.load(Ordering::Acquire) {
        if last_heartbeat.elapsed() >= Duration::from_millis(50) {
            send(
                &mut master,
                &MavMessage::HEARTBEAT(HEARTBEAT_DATA {
                    mavtype: MavType::MAV_TYPE_QUADROTOR,
                    autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
                    base_mode: MavModeFlag::empty(),
                    system_status: MavState::MAV_STATE_STANDBY,
                    mavlink_version: 3,
                    custom_mode: 0,
                }),
                system_id,
                &mut sequence,
            );
            last_heartbeat = Instant::now();
        }
        match master.read(&mut bytes) {
            Ok(size) => buffer.extend_from_slice(&bytes[..size]),
            Err(_) => {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
        }
        while buffer.len() >= 12 {
            if buffer[0] != 0xfd {
                buffer.remove(0);
                continue;
            }
            let size = usize::from(buffer[1]) + 12;
            if buffer.len() < size {
                break;
            }
            let parsed = mavlink::read_v2_msg::<MavMessage, _>(
                &mut mavlink::peek_reader::PeekReader::new(&buffer[..size]),
            );
            buffer.drain(..size);
            let Ok((_, message)) = parsed else { continue };
            trace.lock().push(message.clone());
            if let MavMessage::PARAM_REQUEST_READ(read) = message {
                let name = decode_param_id(&read.param_id);
                let value = match name.as_str() {
                    "COM_MOT_TEST_EN" | "SDLOG_BACKEND" => 1,
                    "PWM_MAIN_FUNC1" => 101,
                    "MR_SHARED" => 17,
                    name if name.contains("_TIM") => 400,
                    name if name.contains("_MIN") => 1100,
                    name if name.contains("_MAX") => 1900,
                    _ => 0,
                };
                send(
                    &mut master,
                    &MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
                        param_id: read.param_id,
                        param_type: MavParamType::MAV_PARAM_TYPE_INT32,
                        param_value: f32::from_bits(value),
                        param_count: 100,
                        param_index: u16::MAX,
                    }),
                    system_id,
                    &mut sequence,
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dedicated_serial_target_is_the_fc_without_a_primary_connection() {
    let (mut master, slave) = serialport::TTYPort::pair().expect("PTY pair");
    master
        .set_timeout(Duration::from_millis(10))
        .expect("PTY timeout");
    let port = slave.name().expect("PTY slave path");
    drop(slave);
    let stop = Arc::new(AtomicBool::new(false));
    let reader_stop = stop.clone();
    let trace = Trace::default();
    let reader_trace = trace.clone();
    let thread = std::thread::spawn(move || fake_px4(master, reader_stop, reader_trace));
    let endpoint = Endpoint::Serial {
        port: port.into(),
        baudrate: 115_200,
    };
    let (handle, _events, first) =
        maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
            endpoint: endpoint.clone(),
            target_system_id: FC_SYS,
            target_component_id: TARGET_COMPONENT_ID,
            auto_discover_target: false,
            ..Default::default()
        })
        .await
        .expect("dedicated serial connection");
    first.await.expect("first result").expect("serial opened");
    wait_until(|| handle.target_heartbeat().is_some()).await;
    let app = mock_app();
    app.manage(AppState::default());
    let state = app.state::<AppState>();
    let (params, _param_events, parameter_task) =
        maggcs_core::params::ParamService::spawn_with_task(
            handle.clone(),
            FC_SYS,
            TARGET_COMPONENT_ID,
        );
    let runtime = AuxiliaryLinkRuntime::new(
        handle.clone(),
        [tauri::async_runtime::spawn(async move {
            let _ = parameter_task.await;
        })],
    );
    crate::actuator_service::install_target(
        app.handle(),
        &state,
        handle,
        endpoint.to_address_string(),
        FC_SYS,
        params,
        runtime,
    );
    assert!(
        state.actuator_status().target_fc_alive,
        "the dedicated serial FC is known when its observer attaches"
    );
    wait_until(|| {
        let status = state.actuator_status();
        status.fc_alive
            && status.com_mot_test_en == Some(1)
            && status.output_map.state == maggcs_core::motor_test::mapping::OutputMapState::Complete
    })
    .await;
    assert!(state.connection().is_none());
    assert!(state.primary_params().is_none());
    assert_eq!(candidate(&state), None);
    let status = state.actuator_status();
    assert!(status.connected && status.target_connected);
    assert!(status.fc_alive && status.target_fc_alive);
    assert_eq!(
        status.endpoint.as_deref(),
        Some(endpoint.to_address_string().as_str())
    );
    assert_eq!(status.target_system_id, Some(FC_SYS));
    assert_eq!(status.target_origin, Some(TargetLinkOrigin::Dedicated));
    let target = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .expect("dedicated target")
        .target
        .target_ref();
    crate::actuator_service::ensure_target_disarmed(&state, target)
        .expect("selected serial FC ready");
    crate::actuator_service::actuator_disconnect_target(app.handle().clone(), state.clone())
        .await
        .expect("disconnect dedicated target");
    assert!(!state.actuator_status().connected);
    stop.store(true, Ordering::Release);
    thread.join().expect("PTY peer joins");
}

#[tokio::test(flavor = "multi_thread")]
async fn borrowed_serial_target_reuses_worker_and_detach_keeps_primary_alive() {
    let h = Harness::new().await;
    let offer = h.attach().await;
    assert_eq!(offer.target.connection_id, h.handle.handle_id());
    assert!(is_attached(&h.state()));
    assert!(!crate::motor_control_service::status(&h.state()).enabled);
    assert!(ensure_port_not_primary(&h.state(), &h.handle.config().endpoint).is_err());
    assert!(ensure_primary_tools_available(&h.state()).is_err());
    // Same request is idempotent: no replacement owner or repeated rate setup.
    actuator_use_main_target(h.app.handle().clone(), h.state(), offer.clone())
        .await
        .expect("idempotent attach");
    wait_until(|| {
        h.state()
            .motor_test
            .target
            .lock()
            .as_ref()
            .is_some_and(|t| t.target.config.lock().is_some())
    })
    .await;
    let status = h.state().actuator_status();
    assert!(status.connected && status.fc_alive && status.target_fc_alive);
    assert_eq!(status.endpoint.as_deref(), Some(offer.endpoint.as_str()));
    assert_eq!(status.target_system_id, Some(FC_SYS));
    assert_eq!(status.target_origin, Some(TargetLinkOrigin::MainSerial));
    let borrowed_params = h
        .state()
        .motor_test
        .target
        .lock()
        .as_ref()
        .expect("target")
        .target
        .params
        .clone();
    let entry = borrowed_params
        .read("MR_SHARED")
        .await
        .expect("shared parameter read");
    assert_eq!(entry.value, ParamValue::Int(17));
    assert_eq!(
        h.state()
            .primary_params()
            .expect("primary params")
            .cache()
            .get("MR_SHARED")
            .expect("same cache")
            .value,
        entry.value
    );
    assert!(!h.trace.lock().iter().any(|frame| matches!(
        frame,
        MavMessage::PARAM_SET(_) | MavMessage::SERIAL_CONTROL(_)
    )));
    crate::motor_control_service::set_control(&h.state(), true)
        .await
        .expect("explicit enable");
    crate::actuator_service::start_source(
        h.app.handle(),
        &h.state(),
        crate::actuator_service::StartSource::Manual(vec![0.1]),
    )
    .await
    .expect("start one configured motor on the borrowed link");
    wait_until(|| h.trace.lock().iter().any(|frame| matches!(frame, MavMessage::COMMAND_LONG(command)
        if command.command == mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST && command.param2 > 0.0))).await;
    crate::actuator_service::actuator_disconnect_target(h.app.handle().clone(), h.state())
        .await
        .expect("detach");
    wait_until(|| h.trace.lock().iter().any(|frame| matches!(frame, MavMessage::COMMAND_LONG(command)
        if command.command == mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST && command.param2 == 0.0))).await;
    assert!(
        h.trace.lock().iter().all(|frame| match frame {
            MavMessage::COMMAND_LONG(command)
                if command.command == mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST =>
                command.param5 == 1.0
                    && command.target_system == FC_SYS
                    && command.target_component == 1,
            _ => true,
        }),
        "only the owned M1 function receives motor frames"
    );
    assert!(!is_attached(&h.state()));
    assert!(!crate::motor_control_service::status(&h.state()).enabled);
    assert_eq!(h.handle.state().await, LinkState::Connected);
    assert_eq!(
        h.state()
            .connection()
            .expect("primary retained")
            .handle_id(),
        offer.target.connection_id
    );
    assert!(h
        .state()
        .primary_params()
        .expect("primary service retained")
        .read("MR_SHARED")
        .await
        .is_ok());
    assert!(ensure_primary_tools_available(&h.state()).is_ok());
    h.attach().await;
    let old_attachment = h
        .state()
        .motor_test
        .target
        .lock()
        .as_ref()
        .expect("attachment")
        .target
        .config
        .clone();
    crate::actuator_service::actuator_disconnect_target(h.app.handle().clone(), h.state())
        .await
        .expect("detach again");
    let current = h.attach().await;
    assert!(!matches_attachment(&h.state(), &current, &old_attachment), "late cleanup from a detached owner cannot remove a new attachment on the same transport generation");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnect_expires_attachment_and_rejects_old_offer() {
    let h = Harness::new().await;
    let old = h.attach().await;
    crate::motor_control_service::set_control(&h.state(), true)
        .await
        .expect("enable");
    h.handle.reconnect().await.expect("reconnect");
    wait_until(|| h.handle.generation() > old.transport_generation && !is_attached(&h.state()))
        .await;
    assert!(!crate::motor_control_service::status(&h.state()).enabled);
    assert!(
        actuator_use_main_target(h.app.handle().clone(), h.state(), old)
            .await
            .is_err()
    );
    assert_eq!(h.handle.state().await, LinkState::Connected);
    wait_until(|| candidate(&h.state()).is_some()).await;
    h.attach().await;
    assert!(!crate::motor_control_service::status(&h.state()).enabled);
    before_primary_replace(h.app.handle(), &h.state())
        .await
        .expect("detach before primary replacement");
    assert!(!is_attached(&h.state()));
    assert_eq!(h.handle.state().await, LinkState::Connected);
    h.shutdown().await;
}

#[tokio::test]
async fn unavailable_primary_refuses_without_creating_a_target() {
    let app = mock_app();
    app.manage(AppState::default());
    let state = app.state::<AppState>();
    assert_eq!(candidate(&state), None);
    let offer = MainSerialTarget {
        target: MotorTestTargetRef {
            connection_id: 1,
            target_system_id: 1,
            target_component_id: 1,
        },
        transport_generation: 1,
        endpoint: "serial:/dev/ttyACM0:115200".into(),
    };
    assert!(
        actuator_use_main_target(app.handle().clone(), state.clone(), offer)
            .await
            .is_err()
    );
    assert!(state.motor_test.target.lock().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn primary_log_operation_and_shared_target_are_mutually_exclusive() {
    let h = Harness::new().await;
    let logs = h
        .state()
        .logs
        .spawn(h.app.handle().clone(), h.handle.clone());
    h.state().set_primary_logs(logs);
    crate::logs_service::logs_request_list(h.state())
        .await
        .expect("main log request");
    assert!(
        h.state().logs.is_busy(),
        "reserve the protocol before the worker receives its command"
    );
    let offer = candidate(&h.state()).expect("candidate");
    let error = actuator_use_main_target(h.app.handle().clone(), h.state(), offer)
        .await
        .expect_err("busy main log transfer");
    assert!(error.contains("log transfer"));
    crate::logs_service::logs_cancel(h.state())
        .await
        .expect("cancel main log request");
    wait_until(|| !h.state().logs.is_busy()).await;
    h.attach().await;
    assert!(crate::logs_service::logs_request_list(h.state())
        .await
        .is_err());
    h.shutdown().await;
}
