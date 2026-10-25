use super::*;
use crate::devices::{
    spawn_serial_discovery, DeviceError, DeviceId, LeaseRequest, PortTransport, SerialDeviceInfo,
    SerialDiscoveryConfig, SerialInventorySource,
};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;

const WINDOW: Duration = Duration::from_millis(40);
const TEST_BUDGET: Duration = Duration::from_secs(1);

fn heartbeat(
    system: u8,
    component: u8,
    sequence: u8,
    autopilot: ::mavlink::common::MavAutopilot,
    kind: ::mavlink::common::MavType,
) -> MessageEnvelope {
    MessageEnvelope {
        header: MavHeader {
            system_id: system,
            component_id: component,
            sequence,
        },
        message: MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: kind,
            autopilot,
            base_mode: ::mavlink::common::MavModeFlag::empty(),
            system_status: ::mavlink::common::MavState::MAV_STATE_STANDBY,
            mavlink_version: 3,
        }),
        received_at: Instant::now(),
    }
}

fn px4(system: u8, sequence: u8) -> MessageEnvelope {
    heartbeat(
        system,
        1,
        sequence,
        ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
        ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
    )
}

fn config() -> FcProbeConfig {
    FcProbeConfig {
        baud: BaudSelection::Fixed(115_200),
        local_system_id: 250,
        expected_target: None,
    }
}

async fn transport() -> (Conn, UdpSocket, SocketAddr) {
    let reservation = std::net::UdpSocket::bind("127.0.0.1:0").expect("reserve loopback port");
    let address = reservation.local_addr().expect("address");
    drop(reservation);
    let mut conn = ::mavlink::connect_async::<MavMessage>(&format!("udpin:{address}"))
        .await
        .expect("wire fixture");
    conn.set_allow_recv_any_version(true);
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("peer");
    (conn, peer, address)
}

async fn send(
    peer: &UdpSocket,
    address: SocketAddr,
    env: &MessageEnvelope,
    v1: bool,
    corrupt: bool,
) {
    let mut bytes = Vec::new();
    if v1 {
        ::mavlink::write_v1_msg(&mut bytes, env.header, &env.message).expect("v1 encode");
    } else {
        ::mavlink::write_v2_msg(&mut bytes, env.header, &env.message).expect("v2 encode");
    }
    if corrupt {
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
    }
    peer.send_to(&bytes, address)
        .await
        .expect("heartbeat on wire");
}

#[test]
fn role_filter_rejects_gcs_invalid_peripherals_wrong_component_and_zero_system() {
    use ::mavlink::common::{MavAutopilot as A, MavType as T};
    for (system, component, autopilot, kind) in [
        (1, 1, A::MAV_AUTOPILOT_INVALID, T::MAV_TYPE_QUADROTOR),
        (1, 1, A::MAV_AUTOPILOT_INVALID, T::MAV_TYPE_GCS),
        (1, 1, A::MAV_AUTOPILOT_PX4, T::MAV_TYPE_GCS),
        (1, 1, A::MAV_AUTOPILOT_PX4, T::MAV_TYPE_ONBOARD_CONTROLLER),
        (1, 1, A::MAV_AUTOPILOT_PX4, T::MAV_TYPE_CAMERA),
        (1, 42, A::MAV_AUTOPILOT_PX4, T::MAV_TYPE_QUADROTOR),
        (0, 1, A::MAV_AUTOPILOT_PX4, T::MAV_TYPE_QUADROTOR),
    ] {
        assert!(
            classify_autopilot_heartbeat(&heartbeat(system, component, 1, autopilot, kind))
                .is_none()
        );
    }
    let generic = classify_autopilot_heartbeat(&heartbeat(
        1,
        1,
        1,
        A::MAV_AUTOPILOT_GENERIC,
        T::MAV_TYPE_QUADROTOR,
    ))
    .expect("generic can have autopilot role");
    assert!(!automatic_fc_supported(&generic));
    assert!(automatic_fc_supported(
        &classify_autopilot_heartbeat(&px4(42, 1)).expect("PX4")
    ));
}

#[tokio::test]
async fn valid_wire_heartbeats_allow_sequence_wrap_and_probe_writes_nothing() {
    let (conn, peer, address) = transport().await;
    send(&peer, address, &px4(42, 255), false, false).await;
    send(&peer, address, &px4(42, 0), false, false).await;
    let (identity, _) = observe_fc(&conn, &config(), WINDOW)
        .await
        .expect("verified");
    assert_eq!((identity.system_id, identity.component_id), (42, 1));
    let mut bytes = [0; 512];
    assert_eq!(
        peer.try_recv(&mut bytes).expect_err("passive probe").kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn corrupt_frames_and_duplicate_sequences_cannot_establish_an_autopilot() {
    let (conn, peer, address) = transport().await;
    send(&peer, address, &px4(42, 1), false, true).await;
    send(&peer, address, &px4(42, 2), false, false).await;
    send(&peer, address, &px4(42, 2), false, false).await;
    assert!(matches!(
        observe_fc(&conn, &config(), WINDOW).await,
        Err(SerialDiscoveryError::NoAutopilot)
    ));
}

#[tokio::test]
async fn v1_local_id_conflict_and_unsupported_firmware_are_explicit() {
    let (conn, peer, address) = transport().await;
    send(&peer, address, &px4(42, 1), true, false).await;
    assert!(matches!(
        observe_fc(&conn, &config(), WINDOW).await,
        Err(SerialDiscoveryError::UnsupportedMavlinkVersion)
    ));
    let (conn, peer, address) = transport().await;
    send(&peer, address, &px4(250, 1), false, false).await;
    assert!(matches!(
        observe_fc(&conn, &config(), WINDOW).await,
        Err(SerialDiscoveryError::LocalSystemIdConflict { .. })
    ));
    let (conn, peer, address) = transport().await;
    let mut ard = px4(42, 1);
    if let MavMessage::HEARTBEAT(hb) = &mut ard.message {
        hb.autopilot = ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA;
    }
    send(&peer, address, &ard, false, false).await;
    assert!(matches!(
        observe_fc(&conn, &config(), WINDOW).await,
        Err(SerialDiscoveryError::UnsupportedAutopilot { .. })
    ));
}

#[tokio::test]
async fn multiple_nodes_need_selection_and_saved_identity_cannot_fall_back() {
    for expected in [None, Some(42), Some(44)] {
        let (conn, peer, address) = transport().await;
        for sys in [42, 43] {
            for seq in [1, 2] {
                send(&peer, address, &px4(sys, seq), false, false).await;
            }
        }
        let mut config = config();
        config.expected_target =
            expected.map(|system| classify_autopilot_heartbeat(&px4(system, 1)).expect("identity"));
        let result = observe_fc(&conn, &config, WINDOW).await;
        match expected {
            None => assert!(matches!(
                result,
                Err(SerialDiscoveryError::AmbiguousTargets { .. })
            )),
            Some(42) => assert_eq!(result.expect("selected node").0.system_id, 42),
            _ => assert!(matches!(
                result,
                Err(SerialDiscoveryError::UnexpectedTarget { .. })
            )),
        }
    }
}

struct Inventory;
impl SerialInventorySource for Inventory {
    fn enumerate(&self) -> Result<Vec<SerialDeviceInfo>, DeviceError> {
        Ok(vec![SerialDeviceInfo {
            port_name: "/dev/ttyACM900123".into(),
            transport: PortTransport::Usb,
            device_id: Some(DeviceId {
                vendor_id: 0x26ac,
                product_id: 0x0039,
            }),
            serial_number: Some("PX4-fixture".into()),
            manufacturer: None,
            product: None,
        }])
    }
}

#[tokio::test]
async fn handoff_uses_the_probed_transport_and_releases_its_lease_after_shutdown() {
    let (discovery, task) =
        spawn_serial_discovery(Arc::new(Inventory), SerialDiscoveryConfig::default())
            .expect("inventory");
    let port = discovery.refresh().await.expect("scan").ports[0]
        .port
        .clone();
    let lease = discovery
        .reserve(LeaseRequest {
            expected: port,
            owner: SerialOwner::PrimaryFc,
        })
        .await
        .expect("lease");
    let (conn, peer, address) = transport().await;
    send(&peer, address, &px4(42, 1), false, false).await;
    send(&peer, address, &px4(42, 2), false, false).await;
    let opens = Arc::new(AtomicUsize::new(0));
    let count = opens.clone();
    let mut transport = Some(conn);
    let probed = probe_with_opener(
        lease,
        config(),
        SerialCancellation::new(),
        WINDOW,
        TEST_BUDGET,
        move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            let conn = transport.take().expect("one open");
            async move { Ok(conn) }
        },
    )
    .await
    .expect("probe");
    let mut bytes = [0; 512];
    assert!(
        peer.try_recv(&mut bytes).is_err(),
        "no write before handoff"
    );
    let config = ConnectionConfig {
        endpoint: probed.endpoint(),
        target_system_id: 42,
        target_component_id: 1,
        auto_discover_target: false,
        ..Default::default()
    };
    // This endpoint does not exist: reopening it instead of moving the transport
    // would fail, even if an opener counter alone missed the second open.
    let (handle, _, first) = probed.connect(config).await.expect("handoff");
    first.await.expect("startup").expect("same open transport");
    tokio::time::timeout(TEST_BUDGET, peer.recv(&mut bytes))
        .await
        .expect("normal GCS heartbeat")
        .expect("wire bytes");
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    assert_eq!(
        handle.target_identity().expect("seeded evidence").system_id,
        42
    );
    handle
        .shutdown_and_wait(TEST_BUDGET)
        .await
        .expect("actual close");
    assert!(discovery.snapshot().ports[0].lease.is_none());
    discovery
        .shutdown(TEST_BUDGET)
        .await
        .expect("stop inventory");
    task.await.expect("join inventory");
}

#[tokio::test]
async fn cancelled_passive_probe_closes_before_another_owner_can_reserve() {
    let (discovery, task) =
        spawn_serial_discovery(Arc::new(Inventory), SerialDiscoveryConfig::default())
            .expect("inventory");
    let port = discovery.refresh().await.expect("scan").ports[0]
        .port
        .clone();
    let lease = discovery
        .reserve(LeaseRequest {
            expected: port.clone(),
            owner: SerialOwner::PrimaryFc,
        })
        .await
        .expect("lease");
    let (conn, peer, _) = transport().await;
    let cancel = SerialCancellation::new();
    let worker_cancel = cancel.clone();
    let mut conn = Some(conn);
    let probe = tokio::spawn(async move {
        probe_with_opener(
            lease,
            config(),
            worker_cancel,
            Duration::from_secs(30),
            Duration::from_secs(60),
            move |_| {
                let conn = conn.take().expect("one open");
                async move { Ok(conn) }
            },
        )
        .await
    });
    tokio::task::yield_now().await;
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(TEST_BUDGET, probe)
            .await
            .expect("cancel deadline")
            .expect("probe task"),
        Err(SerialDiscoveryError::Cancelled)
    ));
    let mut bytes = [0; 512];
    assert!(peer.try_recv(&mut bytes).is_err());
    let replacement = discovery
        .reserve(LeaseRequest {
            expected: port,
            owner: SerialOwner::MotorTestTarget,
        })
        .await
        .expect("retired lease");
    drop(replacement);
    discovery
        .shutdown(TEST_BUDGET)
        .await
        .expect("stop inventory");
    task.await.expect("join");
}
