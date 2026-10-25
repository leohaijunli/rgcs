use super::*;
use maggcs_core::devices::{DeviceId, PortTransport};

#[tokio::test]
async fn reflected_own_corrections_do_not_activate_or_inflate_foreign_injector_status() {
    use maggcs_core::mavlink::ConnectionConfig;
    use tauri::Listener;
    let app = tauri::test::mock_app();
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("peer");
    let (conn, _, first) = maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
        endpoint: Endpoint::UdpClient {
            addr: peer.local_addr().expect("address"),
        },
        ..Default::default()
    })
    .await
    .expect("connection");
    first.await.expect("worker").expect("connected");
    let (tx, mut changes) = tokio::sync::mpsc::unbounded_channel();
    app.listen("rtk_link_status", move |event| {
        if let Ok(status) = serde_json::from_str::<RtkLinkStatus>(event.payload()) {
            if status.foreign.active {
                let _ = tx.send(status.foreign.packets);
            }
        }
    });
    let state = Arc::new(RtkState::default());
    let runtime = RtkRuntime::spawn(state, app.handle().clone(), conn.clone());
    let mut bytes = [0; 512];
    let (_, address) = tokio::time::timeout(Duration::from_secs(1), peer.recv_from(&mut bytes))
        .await
        .expect("GCS heartbeat")
        .expect("source address");
    let correction =
        ::mavlink::common::MavMessage::GPS_RTCM_DATA(::mavlink::common::GPS_RTCM_DATA_DATA {
            flags: 0,
            len: 0,
            data: [0; 180],
        });
    for (system_id, component_id) in [(250, 250), (255, 190)] {
        let mut wire = Vec::new();
        ::mavlink::write_v2_msg(
            &mut wire,
            ::mavlink::MavHeader {
                system_id,
                component_id,
                sequence: 0,
            },
            &correction,
        )
        .expect("encode");
        peer.send_to(&wire, address).await.expect("injector frame");
        if system_id == 250 {
            assert!(
                tokio::time::timeout(Duration::from_millis(50), changes.recv())
                    .await
                    .is_err(),
                "own reflection is not foreign"
            );
        }
    }
    let packets = tokio::time::timeout(Duration::from_secs(1), changes.recv())
        .await
        .expect("foreign event")
        .expect("event channel");
    assert_eq!(
        packets, 1,
        "only the independently addressed injector counts"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), changes.recv())
            .await
            .is_err()
    );
    runtime.stop(Duration::from_secs(1)).await;
    conn.shutdown_and_wait(Duration::from_secs(1))
        .await
        .expect("close");
}

fn device(port: &str, vid: u16, pid: u16) -> SerialDeviceInfo {
    SerialDeviceInfo {
        port_name: port.into(),
        transport: PortTransport::Usb,
        device_id: Some(DeviceId {
            vendor_id: vid,
            product_id: pid,
        }),
        serial_number: Some("receiver-A".into()),
        manufacturer: None,
        product: None,
    }
}

#[test]
fn rtk_selector_distinguishes_fc_receiver_and_generic_bridge_and_excludes_claimed_ports() {
    let devices = vec![
        device("/dev/ttyACM0", 0x26ac, 0x0039),
        device("/dev/ttyACM1", 0x1546, 0x01a9),
        device("/dev/ttyUSB0", 0x1a86, 0x7523),
        device("/dev/ttyACM2", 0x1546, 0x01a8),
    ];
    let ports = classify_ports(devices, &["/dev/ttyUSB0".into()]);
    assert_eq!(ports[0].suggested_role, DeviceRole::FlightController);
    assert!(ports[0].unavailable_reason.is_some());
    assert_eq!(ports[1].suggested_role, DeviceRole::RtkBaseStation);
    assert_eq!(ports[1].unavailable_reason, None);
    assert_eq!(ports[2].suggested_role, DeviceRole::Other);
    assert!(ports[2]
        .unavailable_reason
        .as_deref()
        .is_some_and(|s| s.contains("In use")));
    assert_eq!(
        ports[3].suggested_role,
        DeviceRole::Other,
        "F9R must not be offered as a survey-in base"
    );
}

#[test]
fn changed_usb_identity_and_fc_port_are_refused_before_opening() {
    let selected = device("/dev/ttyACM1", 0x1546, 0x01a9);
    validate_selection(&selected, vec![selected.clone()], &[]).expect("same device");
    let mut replacement = selected.clone();
    replacement.serial_number = Some("receiver-B".into());
    assert!(validate_selection(&selected, vec![replacement], &[]).is_err());
    assert!(validate_selection(&selected, vec![], &[]).is_err());
    assert!(
        validate_selection(&selected, vec![selected.clone()], &["/dev/ttyACM1".into()]).is_err()
    );
    let fc = device("/dev/ttyACM0", 0x26ac, 0x0039);
    assert!(validate_selection(&fc, vec![fc.clone()], &[]).is_err());
}

#[test]
fn retired_runtime_cannot_replace_new_link_stats_or_clear_new_foreign_state() {
    let app = tauri::test::mock_app();
    let state = RtkState::default();
    let old = state.begin_link(app.handle());
    let current = state.begin_link(app.handle());
    state.publish(current, app.handle(), |s| {
        s.foreign = ForeignState {
            active: true,
            packets: 7,
        }
    });
    let expected = state.link.lock().snapshot.clone();
    state.retire(old, app.handle());
    state.publish(old, app.handle(), |s| s.foreign = ForeignState::default());
    assert_eq!(state.link.lock().snapshot, expected);
    state.retire(current, app.handle());
    let retired = state.link.lock().snapshot.clone();
    state.publish(current, app.handle(), |s| {
        s.stats = Some(RtkStats {
            source: Some("stale".into()),
            ..Default::default()
        })
    });
    assert_eq!(state.link.lock().snapshot, retired);
    assert!(retired.revision > expected.revision);
}

#[tokio::test]
async fn vehicle_disconnect_preserves_base_but_application_shutdown_joins_it() {
    use maggcs_core::lifecycle::StopReason;
    use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
    use maggcs_core::rtk::source::FakeSource;
    use tauri::Manager;
    let app = tauri::test::mock_app();
    app.manage(AppState::default());
    let state = app.state::<AppState>();
    let (conn, _, first) = maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
        endpoint: Endpoint::UdpListener {
            addr: "127.0.0.1:0".parse().expect("addr"),
        },
        ..Default::default()
    })
    .await
    .expect("connection");
    first.await.expect("startup").expect("bound");
    let runtime = RtkRuntime::spawn(state.rtk.clone(), app.handle().clone(), conn.clone());
    runtime
        .service
        .start(
            Box::new(FakeSource::new(vec![], Duration::ZERO)),
            "independent-base".into(),
        )
        .await
        .expect("base");
    let mut status = state.rtk.base.subscribe();
    tokio::time::timeout(
        Duration::from_secs(1),
        status.wait_for(|s| s.phase == BasePhase::Ready),
    )
    .await
    .expect("ready deadline")
    .expect("ready");
    let session = state.rtk.base.snapshot().session_id;
    assert!(matches!(
        runtime.stop(Duration::from_secs(1)).await,
        StopOutcome::Stopped { .. }
    ));
    assert_eq!(state.rtk.base.snapshot().phase, BasePhase::Ready);
    assert_eq!(state.rtk.base.snapshot().session_id, session);
    crate::lifecycle_service::run(app.handle(), StopReason::Disconnect)
        .await
        .expect("vehicle disconnect");
    assert_eq!(state.rtk.base.snapshot().phase, BasePhase::Ready);
    crate::lifecycle_service::run(app.handle(), StopReason::ShutdownApp)
        .await
        .expect("app shutdown");
    assert_eq!(state.rtk.base.snapshot().phase, BasePhase::Stopped);
    conn.shutdown().await;
}
