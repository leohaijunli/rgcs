//! OS pseudo-terminal tests: the serial transport, router, primary parameter
//! worker and Motor Test attachment are production code; only PX4 is simulated.
use super::*;
use std::io::{Read, Write};
use std::sync::{
    atomic::{AtomicBool, AtomicU16, Ordering},
    Arc,
};
use std::time::Instant;

use crate::runtime_facade::{RuntimeFacade, TargetRuntime};
use maggcs_core::mavlink::{ConnectionConfig, ConnectionHandle, MavMessage};
use maggcs_core::params::{codec::decode_param_id, model::ParamValue};
use serialport::SerialPort;
use tauri::test::{mock_app, MockRuntime};

const FC_SYS: u8 = 42;
const WAIT: Duration = Duration::from_secs(8);
type Trace = Arc<parking_lot::Mutex<Vec<MavMessage>>>;

#[derive(Clone)]
pub(crate) struct CommandPeer {
    pub(crate) hold_ack: Arc<AtomicBool>,
    pub(crate) result: Arc<parking_lot::Mutex<mavlink::common::MavResult>>,
    pub(crate) armed: Arc<AtomicBool>,
    pub(crate) airborne: Arc<AtomicBool>,
    pub(crate) publish_landed: Arc<AtomicBool>,
}

impl Default for CommandPeer {
    fn default() -> Self {
        Self {
            hold_ack: Arc::new(AtomicBool::new(false)),
            result: Arc::new(parking_lot::Mutex::new(
                mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
            )),
            armed: Arc::new(AtomicBool::new(false)),
            airborne: Arc::new(AtomicBool::new(false)),
            publish_landed: Arc::new(AtomicBool::new(true)),
        }
    }
}

#[derive(Clone)]
pub(crate) struct MissionPeer {
    pub(crate) items: Arc<parking_lot::Mutex<Vec<maggcs_core::mission::MissionItem>>>,
    pub(crate) result: Arc<parking_lot::Mutex<mavlink::common::MavMissionResult>>,
    pub(crate) hold_upload: Arc<AtomicBool>,
    pub(crate) hold_download: Arc<AtomicBool>,
    pub(crate) hold_ack: Arc<AtomicBool>,
    pub(crate) hold_current: Arc<AtomicBool>,
    pub(crate) current: Arc<AtomicU16>,
    pub(crate) publish_current: Arc<AtomicBool>,
}

impl Default for MissionPeer {
    fn default() -> Self {
        let items = (0..2)
            .map(|seq| {
                let mut item = maggcs_core::mission::MissionItem::waypoint(
                    49.25 + f64::from(seq) * 0.001,
                    -123.1,
                    50.0,
                    maggcs_core::mission::MissionFrame::GlobalInt,
                );
                item.seq = seq;
                item
            })
            .collect();
        Self {
            items: Arc::new(parking_lot::Mutex::new(items)),
            result: Arc::new(parking_lot::Mutex::new(
                mavlink::common::MavMissionResult::MAV_MISSION_ACCEPTED,
            )),
            hold_upload: Arc::new(AtomicBool::new(false)),
            hold_download: Arc::new(AtomicBool::new(false)),
            hold_ack: Arc::new(AtomicBool::new(false)),
            hold_current: Arc::new(AtomicBool::new(false)),
            current: Arc::new(AtomicU16::new(0)),
            publish_current: Arc::new(AtomicBool::new(true)),
        }
    }
}

pub(crate) struct Harness {
    pub(crate) app: tauri::App<MockRuntime>,
    pub(crate) handle: ConnectionHandle,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub(crate) trace: Trace,
    pub(crate) hold_set_echo: Arc<AtomicBool>,
    pub(crate) hold_pull: Arc<AtomicBool>,
    pub(crate) hold_reads: Arc<AtomicBool>,
    pub(crate) command_peer: Option<CommandPeer>,
    pub(crate) mission_peer: Option<MissionPeer>,
    pub(crate) log_peer: Option<crate::logs_service::operation_tests::LogPeer>,
}

impl Harness {
    pub(crate) async fn new() -> Self {
        Self::with_parameters(false).await
    }

    pub(crate) async fn parameters() -> Self {
        Self::with_parameters(true).await
    }

    pub(crate) async fn commands() -> Self {
        Self::with_options(true, Some(CommandPeer::default())).await
    }

    pub(crate) async fn missions() -> Self {
        Self::with_all_options(
            true,
            Some(CommandPeer::default()),
            Some(MissionPeer::default()),
            None,
        )
        .await
    }

    async fn with_parameters(parameter_transactions: bool) -> Self {
        Self::with_options(parameter_transactions, None).await
    }

    async fn with_options(parameter_transactions: bool, command_peer: Option<CommandPeer>) -> Self {
        Self::with_all_options(parameter_transactions, command_peer, None, None).await
    }

    pub(crate) async fn logs() -> Self {
        Self::with_all_options(
            true,
            Some(CommandPeer::default()),
            None,
            Some(crate::logs_service::operation_tests::LogPeer::default()),
        )
        .await
    }

    async fn with_all_options(
        parameter_transactions: bool,
        command_peer: Option<CommandPeer>,
        mission_peer: Option<MissionPeer>,
        log_peer: Option<crate::logs_service::operation_tests::LogPeer>,
    ) -> Self {
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
        let hold_set_echo = Arc::new(AtomicBool::new(false));
        let hold_pull = Arc::new(AtomicBool::new(false));
        let hold_reads = Arc::new(AtomicBool::new(false));
        let holds = (hold_set_echo.clone(), hold_pull.clone(), hold_reads.clone());
        let peer_commands = command_peer.clone();
        let peer_missions = mission_peer.clone();
        let peer_logs = log_peer.clone();
        let thread = std::thread::spawn(move || {
            fake_px4_with_options(
                master,
                reader_stop,
                reader_trace,
                FC_SYS,
                parameter_transactions,
                holds,
                peer_commands,
                peer_missions,
                peer_logs,
            )
        });
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
        let link_id = crate::state::LinkId::next().expect("link ID");
        let mission = crate::mission_service::spawn(
            app.handle().clone(),
            handle.clone(),
            maggcs_core::mission::service::MissionIds {
                self_system: handle.config().system_id,
                self_component: handle.config().component_id,
                target_system: FC_SYS,
                target_component: TARGET_COMPONENT_ID,
            },
            link_id,
        );
        let (command, _) = maggcs_core::CommandService::spawn(handle.clone(), FC_SYS, 1);
        state.set_primary_link(crate::state::ActiveLink {
            id: link_id,
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
            hold_set_echo,
            hold_pull,
            hold_reads,
            command_peer,
            mission_peer,
            log_peer,
        }
    }

    pub(crate) fn state(&self) -> State<'_, AppState> {
        self.app.state::<AppState>()
    }

    pub(crate) async fn attach(&self) -> MainSerialTarget {
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

    pub(crate) async fn shutdown(mut self) {
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

pub(crate) async fn wait_until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture deadline");
}

fn send_from(
    master: &mut serialport::TTYPort,
    message: &MavMessage,
    system_id: u8,
    sequence: &mut u8,
) {
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
    master: serialport::TTYPort,
    stop: Arc<AtomicBool>,
    trace: Trace,
    system_id: u8,
) {
    let idle = Arc::new(AtomicBool::new(false));
    fake_px4_with_options(
        master,
        stop,
        trace,
        system_id,
        false,
        (idle.clone(), idle.clone(), idle),
        None,
        None,
        None,
    );
}

#[allow(deprecated, clippy::too_many_arguments)]
fn fake_px4_with_options(
    mut master: serialport::TTYPort,
    stop: Arc<AtomicBool>,
    trace: Trace,
    system_id: u8,
    parameter_transactions: bool,
    holds: (Arc<AtomicBool>, Arc<AtomicBool>, Arc<AtomicBool>),
    command_peer: Option<CommandPeer>,
    mission_peer: Option<MissionPeer>,
    log_peer: Option<crate::logs_service::operation_tests::LogPeer>,
) {
    use mavlink::common::*;
    let mut buffer = Vec::new();
    let mut bytes = [0; 2048];
    let mut last_heartbeat = Instant::now() - Duration::from_secs(1);
    let mut sequence = 0;
    let mut send = |master: &mut serialport::TTYPort, message: &MavMessage| {
        send_from(master, message, system_id, &mut sequence);
    };
    let mut held_command = None;
    let mut held_upload = None;
    let mut held_download = None;
    let mut held_ack = None;
    let mut held_current = None;
    let mut incoming_mission = Vec::new();
    let mut incoming_count = 0u16;
    let mut parameters: std::collections::BTreeMap<String, (f32, MavParamType)> =
        Default::default();
    while !stop.load(Ordering::Acquire) {
        if log_peer
            .as_ref()
            .is_some_and(|peer| peer.flood.load(Ordering::Acquire))
        {
            send(
                &mut master,
                &MavMessage::LOG_DATA(LOG_DATA_DATA {
                    id: 0,
                    ofs: 0,
                    count: 1,
                    data: [7; 90],
                }),
            );
        }
        if let Some(peer) = &mission_peer {
            if !peer.hold_upload.load(Ordering::Acquire) {
                if let Some(message) = held_upload.take() {
                    send(&mut master, &message);
                }
            }
            if !peer.hold_download.load(Ordering::Acquire) {
                if let Some(message) = held_download.take() {
                    send(&mut master, &message);
                }
            }
            if !peer.hold_ack.load(Ordering::Acquire) {
                if let Some(message) = held_ack.take() {
                    send(&mut master, &message);
                }
            }
            if !peer.hold_current.load(Ordering::Acquire) {
                if let Some(seq) = held_current.take() {
                    peer.current.store(seq, Ordering::Release);
                }
            }
        }
        if let Some(peer) = &command_peer {
            if !peer.hold_ack.load(Ordering::Acquire) {
                if let Some(command) = held_command.take() {
                    send(
                        &mut master,
                        &MavMessage::COMMAND_ACK(COMMAND_ACK_DATA {
                            command,
                            result: *peer.result.lock(),
                            ..Default::default()
                        }),
                    );
                }
            }
        }
        if last_heartbeat.elapsed() >= Duration::from_millis(50) {
            send(
                &mut master,
                &MavMessage::HEARTBEAT(HEARTBEAT_DATA {
                    mavtype: MavType::MAV_TYPE_QUADROTOR,
                    autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
                    base_mode: if command_peer
                        .as_ref()
                        .is_some_and(|peer| peer.armed.load(Ordering::Acquire))
                    {
                        MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED
                    } else {
                        MavModeFlag::empty()
                    },
                    system_status: MavState::MAV_STATE_STANDBY,
                    mavlink_version: 3,
                    custom_mode: 0,
                }),
            );
            if command_peer
                .as_ref()
                .is_some_and(|peer| peer.publish_landed.load(Ordering::Acquire))
            {
                send(
                    &mut master,
                    &MavMessage::EXTENDED_SYS_STATE(EXTENDED_SYS_STATE_DATA {
                        vtol_state: MavVtolState::MAV_VTOL_STATE_UNDEFINED,
                        landed_state: if command_peer
                            .as_ref()
                            .is_some_and(|peer| peer.airborne.load(Ordering::Acquire))
                        {
                            MavLandedState::MAV_LANDED_STATE_IN_AIR
                        } else {
                            MavLandedState::MAV_LANDED_STATE_ON_GROUND
                        },
                    }),
                );
                send(
                    &mut master,
                    &MavMessage::GLOBAL_POSITION_INT(GLOBAL_POSITION_INT_DATA {
                        time_boot_ms: 1,
                        lat: 492_500_000,
                        lon: -1_231_000_000,
                        alt: 50_000,
                        relative_alt: 10_000,
                        vx: 0,
                        vy: 0,
                        vz: 0,
                        hdg: 0,
                    }),
                );
                send(
                    &mut master,
                    &MavMessage::GPS_RAW_INT(GPS_RAW_INT_DATA {
                        time_usec: 1,
                        lat: 492_500_000,
                        lon: -1_231_000_000,
                        alt: 50_000,
                        eph: 50,
                        epv: 80,
                        vel: 0,
                        cog: 0,
                        fix_type: GpsFixType::GPS_FIX_TYPE_3D_FIX,
                        satellites_visible: 12,
                        ..Default::default()
                    }),
                );
                send(
                    &mut master,
                    &MavMessage::SYS_STATUS(SYS_STATUS_DATA {
                        onboard_control_sensors_present: MavSysStatusSensor::empty(),
                        onboard_control_sensors_enabled: MavSysStatusSensor::empty(),
                        onboard_control_sensors_health: MavSysStatusSensor::empty(),
                        voltage_battery: 16_000,
                        current_battery: 100,
                        battery_remaining: 80,
                        ..Default::default()
                    }),
                );
                send(
                    &mut master,
                    &MavMessage::HOME_POSITION(HOME_POSITION_DATA {
                        latitude: 492_500_000,
                        longitude: -1_231_000_000,
                        altitude: 40_000,
                        ..Default::default()
                    }),
                );
            }
            last_heartbeat = Instant::now();
            if let Some(peer) = &mission_peer {
                if peer.publish_current.load(Ordering::Acquire) {
                    send(
                        &mut master,
                        &MavMessage::MISSION_CURRENT(MISSION_CURRENT_DATA {
                            seq: peer.current.load(Ordering::Acquire),
                            ..Default::default()
                        }),
                    );
                }
            }
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
            let Ok((header, message)) = parsed else {
                continue;
            };
            trace.lock().push(message.clone());
            if let Some(peer) = &log_peer {
                for response in peer.respond(&message, header.system_id, header.component_id) {
                    send(&mut master, &response);
                }
            } else if let MavMessage::SERIAL_CONTROL(request) = &message {
                if request.count > 0 && request.device == SerialControlDev::SERIAL_CONTROL_DEV_SHELL
                {
                    let reply = b"shell-output\n";
                    let mut data = [0; 70];
                    data[..reply.len()].copy_from_slice(reply);
                    send(
                        &mut master,
                        &MavMessage::SERIAL_CONTROL(SERIAL_CONTROL_DATA {
                            device: SerialControlDev::SERIAL_CONTROL_DEV_SHELL,
                            flags: SerialControlFlag::SERIAL_CONTROL_FLAG_REPLY,
                            timeout: 0,
                            baudrate: 0,
                            count: reply.len() as u8,
                            data,
                            target_system: header.system_id,
                            target_component: header.component_id,
                        }),
                    );
                }
            }
            if let Some(peer) = &mission_peer {
                let ack = || {
                    MavMessage::MISSION_ACK(MISSION_ACK_DATA {
                        target_system: header.system_id,
                        target_component: header.component_id,
                        mavtype: *peer.result.lock(),
                        ..Default::default()
                    })
                };
                match &message {
                    MavMessage::MISSION_COUNT(count) => {
                        incoming_mission.clear();
                        incoming_count = count.count;
                        let request = MavMessage::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
                            target_system: header.system_id,
                            target_component: header.component_id,
                            seq: 0,
                            ..Default::default()
                        });
                        if peer.hold_upload.load(Ordering::Acquire) {
                            held_upload = Some(request);
                        } else {
                            send(&mut master, &request);
                        }
                    }
                    MavMessage::MISSION_ITEM_INT(item) => {
                        if usize::from(item.seq) == incoming_mission.len() {
                            incoming_mission.push(
                                maggcs_core::mission::protocol::mission_item_from_mav(item)
                                    .expect("fixture item"),
                            );
                        }
                        if incoming_mission.len() == usize::from(incoming_count) {
                            if *peer.result.lock() == MavMissionResult::MAV_MISSION_ACCEPTED {
                                *peer.items.lock() = incoming_mission.clone();
                            }
                            if peer.hold_ack.load(Ordering::Acquire) {
                                held_ack = Some(ack());
                            } else {
                                send(&mut master, &ack());
                            }
                        } else {
                            let request =
                                MavMessage::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
                                    target_system: header.system_id,
                                    target_component: header.component_id,
                                    seq: incoming_mission.len() as u16,
                                    ..Default::default()
                                });
                            if peer.hold_upload.load(Ordering::Acquire) {
                                held_upload = Some(request);
                            } else {
                                send(&mut master, &request);
                            }
                        }
                    }
                    MavMessage::MISSION_REQUEST_LIST(_) => {
                        let count = MavMessage::MISSION_COUNT(MISSION_COUNT_DATA {
                            target_system: header.system_id,
                            target_component: header.component_id,
                            count: peer.items.lock().len() as u16,
                            ..Default::default()
                        });
                        if peer.hold_download.load(Ordering::Acquire) {
                            held_download = Some(count);
                        } else {
                            send(&mut master, &count);
                        }
                    }
                    MavMessage::MISSION_REQUEST_INT(request) => {
                        if let Some(item) = peer.items.lock().get(usize::from(request.seq)) {
                            let message = MavMessage::MISSION_ITEM_INT(
                                maggcs_core::mission::protocol::mission_item_to_mav(
                                    header.system_id,
                                    header.component_id,
                                    item,
                                ),
                            );
                            if peer.hold_download.load(Ordering::Acquire) {
                                held_download = Some(message);
                            } else {
                                send(&mut master, &message);
                            }
                        }
                    }
                    MavMessage::MISSION_CLEAR_ALL(_) => {
                        if *peer.result.lock() == MavMissionResult::MAV_MISSION_ACCEPTED {
                            peer.items.lock().clear();
                        }
                        if peer.hold_ack.load(Ordering::Acquire) {
                            held_ack = Some(ack());
                        } else {
                            send(&mut master, &ack());
                        }
                    }
                    MavMessage::MISSION_SET_CURRENT(request) => {
                        if peer.hold_current.load(Ordering::Acquire) {
                            held_current = Some(request.seq);
                        } else {
                            peer.current.store(request.seq, Ordering::Release);
                        }
                    }
                    _ => {}
                }
            }
            if let (Some(peer), MavMessage::COMMAND_LONG(command)) = (&command_peer, &message) {
                if peer.hold_ack.load(Ordering::Acquire) {
                    held_command = Some(command.command);
                } else {
                    send(
                        &mut master,
                        &MavMessage::COMMAND_ACK(COMMAND_ACK_DATA {
                            command: command.command,
                            result: *peer.result.lock(),
                            ..Default::default()
                        }),
                    );
                }
            }
            if parameter_transactions {
                if let MavMessage::PARAM_SET(set) = &message {
                    parameters.insert(
                        decode_param_id(&set.param_id),
                        (set.param_value, set.param_type),
                    );
                    if !holds.0.load(Ordering::Acquire) {
                        send(
                            &mut master,
                            &MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
                                param_id: set.param_id,
                                param_type: set.param_type,
                                param_value: set.param_value,
                                param_count: 2,
                                param_index: 0,
                            }),
                        );
                    }
                    continue;
                }
                if matches!(message, MavMessage::PARAM_REQUEST_LIST(_)) {
                    for (index, name, value) in [(0, "MR_SHARED", 17u32), (1, "OWNED_EXTRA", 18u32)]
                    {
                        if index == 1 && holds.1.load(Ordering::Acquire) {
                            continue;
                        }
                        let mut id = [0; 16];
                        id[..name.len()].copy_from_slice(name.as_bytes());
                        send(
                            &mut master,
                            &MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
                                param_id: id.into(),
                                param_type: MavParamType::MAV_PARAM_TYPE_INT32,
                                param_value: f32::from_bits(value),
                                param_count: 2,
                                param_index: index,
                            }),
                        );
                    }
                    continue;
                }
            }
            if let MavMessage::PARAM_REQUEST_READ(read) = message {
                if parameter_transactions && holds.2.load(Ordering::Acquire) {
                    continue;
                }
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
                let (raw, ptype) = parameters
                    .get(&name)
                    .copied()
                    .unwrap_or((f32::from_bits(value), MavParamType::MAV_PARAM_TYPE_INT32));
                send(
                    &mut master,
                    &MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
                        param_id: read.param_id,
                        param_type: ptype,
                        param_value: raw,
                        param_count: 100,
                        param_index: u16::MAX,
                    }),
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
    let runtime = RuntimeFacade::new(h.app.handle());
    let primary_context = runtime
        .targets()
        .expect("Runtime targets")
        .remove(0)
        .target
        .expect("verified primary");
    let reader = runtime
        .bind(&primary_context)
        .expect("bind primary context");
    let offer = h.attach().await;
    let shared_context = runtime.targets().expect("shared targets").remove(0);
    assert_eq!(shared_context.target.as_ref(), Some(&primary_context));
    assert_eq!(
        shared_context.roles,
        vec![
            maggcs_core::runtime::TargetRole::Primary,
            maggcs_core::runtime::TargetRole::MotorTest
        ]
    );
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
        reader
            .parameter_observation("MR_SHARED")
            .expect("scoped observation")
            .expect("same live cache")
            .entry,
        entry
    );
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
    assert_eq!(reader.target(), &primary_context);
    assert!(reader
        .parameter_observation("MR_SHARED")
        .expect("main retained")
        .is_some());
    assert_eq!(
        runtime
            .targets()
            .expect("detached targets")
            .remove(0)
            .target
            .as_ref(),
        Some(&primary_context)
    );
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
    let runtime = RuntimeFacade::new(h.app.handle());
    let original = runtime
        .targets()
        .expect("targets")
        .remove(0)
        .target
        .expect("target");
    let reader = runtime.bind(&original).expect("bind");
    let old = h.attach().await;
    crate::motor_control_service::set_control(&h.state(), true)
        .await
        .expect("enable");
    h.handle.reconnect().await.expect("reconnect");
    wait_until(|| h.handle.generation() > old.transport_generation && !is_attached(&h.state()))
        .await;
    assert!(matches!(
        reader.telemetry_snapshot(),
        Err(maggcs_core::runtime::RuntimeError::TargetChanged)
    ));
    assert!(!crate::motor_control_service::status(&h.state()).enabled);
    assert!(
        actuator_use_main_target(h.app.handle().clone(), h.state(), old)
            .await
            .is_err()
    );
    assert_eq!(h.handle.state().await, LinkState::Connected);
    wait_until(|| candidate(&h.state()).is_some()).await;
    let current = runtime
        .targets()
        .expect("new context")
        .remove(0)
        .target
        .expect("fresh target");
    assert_ne!(original.session_id, current.session_id);
    assert_ne!(original.generation, current.generation);
    assert!(runtime
        .bind(&current)
        .expect("new context bind")
        .telemetry_snapshot()
        .is_ok());
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
    let target = RuntimeFacade::new(h.app.handle())
        .targets()
        .expect("targets")
        .remove(0)
        .target
        .expect("target");
    let session = h
        .state()
        .runtime_clients
        .attach("main", "logs".into(), target)
        .expect("attach")
        .0;
    let work = crate::logs_service::begin(
        h.app.handle(),
        "main",
        maggcs_core::logs::operation_types::LogRequestContext {
            session: session.clone(),
            request_id: maggcs_core::runtime::RequestId::fresh().expect("request"),
        },
        crate::logs_service::LogAction::List,
        None,
    )
    .expect("main log request");
    let operation = work.operation().clone();
    assert!(
        h.state().logs.is_busy(),
        "reserve the protocol before the worker receives its command"
    );
    let offer = candidate(&h.state()).expect("candidate");
    let error = actuator_use_main_target(h.app.handle().clone(), h.state(), offer)
        .await
        .expect_err("busy main log transfer");
    assert!(error.contains("log transfer"));
    h.state()
        .runtime_clients
        .operations
        .cancel(&session.owner, &operation)
        .expect("cancel exact log request");
    assert!(work.run().await.is_err());
    wait_until(|| !h.state().logs.is_busy()).await;
    h.attach().await;
    assert!(crate::logs_service::begin(
        h.app.handle(),
        "main",
        maggcs_core::logs::operation_types::LogRequestContext {
            session,
            request_id: maggcs_core::runtime::RequestId::fresh().expect("request"),
        },
        crate::logs_service::LogAction::List,
        None
    )
    .is_err());
    h.shutdown().await;
}
