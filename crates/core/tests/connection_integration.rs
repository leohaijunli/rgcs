//! Integration tests for the MAVLink connection worker over real UDP
//! sockets (no SITL required). A synthetic peer acts as the flight
//! controller and sends HEARTBEATs.

use std::net::UdpSocket;
use std::time::Duration;

use maggcs_core::mavlink::{
    connection::{spawn_connection, HEARTBEAT_WATCHDOG_TICK},
    message::MavMessage,
    ConnectionConfig, ConnectionEvent, Endpoint, MavHeader,
};
use tokio::time::timeout;

/// Find a free UDP port by binding to :0 and releasing.
fn free_udp_port() -> u16 {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind ephemeral");
    sock.local_addr().expect("local addr").port()
}

fn heartbeat() -> MavMessage {
    MavMessage::HEARTBEAT(mavlink::common::HEARTBEAT_DATA {
        custom_mode: 0,
        mavtype: mavlink::common::MavType::MAV_TYPE_QUADROTOR,
        autopilot: mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
        base_mode: mavlink::common::MavModeFlag::empty(),
        system_status: mavlink::common::MavState::MAV_STATE_ACTIVE,
        mavlink_version: 3,
    })
}

async fn config_for_port(port: u16) -> ConnectionConfig {
    ConnectionConfig {
        endpoint: Endpoint::UdpListener {
            addr: format!("127.0.0.1:{port}").parse().unwrap(),
        },
        system_id: 250,
        component_id: 250,
        target_system_id: 1,
        target_component_id: 1,
        heartbeat_timeout: Duration::from_millis(250),
        reconnect_delay: Duration::from_millis(50),
        ..Default::default()
    }
}

/// Wait for an event matching the predicate, skipping others.
async fn wait_for<F>(
    rx: &mut tokio::sync::broadcast::Receiver<ConnectionEvent>,
    within: Duration,
    mut pred: F,
) -> ConnectionEvent
where
    F: FnMut(&ConnectionEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let ev = timeout(remaining, rx.recv())
            .await
            .expect("timed out waiting for event")
            .expect("event channel closed");
        if pred(&ev) {
            return ev;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn udp_heartbeat_flow_alive_lost_restored() {
    let port = free_udp_port();
    let config = config_for_port(port).await;
    let (handle, mut rx, _first) = spawn_connection(config).await.expect("spawn");

    let peer =
        mavlink::connect_async::<mavlink::common::MavMessage>(&format!("udpout:127.0.0.1:{port}"))
            .await
            .expect("peer connect");
    let mut peer = peer;
    peer.set_protocol_version(mavlink::MavlinkVersion::V2);

    // 1. Connected
    let _ = wait_for(&mut rx, Duration::from_secs(2), |e| {
        matches!(e, ConnectionEvent::Connected { .. })
    })
    .await;

    // 2. First heartbeat -> Message + HeartbeatRestored.
    peer.send(
        &MavHeader {
            system_id: 1,
            component_id: 1,
            sequence: 0,
        },
        &heartbeat(),
    )
    .await
    .expect("peer send");

    let _msg = wait_for(
        &mut rx,
        Duration::from_secs(2),
        |e| matches!(e, ConnectionEvent::Message(m) if m.is_heartbeat_from(1, 1)),
    )
    .await;
    let _restored = wait_for(&mut rx, Duration::from_secs(2), |e| {
        matches!(e, ConnectionEvent::HeartbeatRestored)
    })
    .await;
    assert_eq!(
        handle.state().await,
        maggcs_core::mavlink::LinkState::Connected
    );

    // 3. Stop sending -> HeartbeatLost.
    let lost = wait_for(&mut rx, Duration::from_secs(3), |e| {
        matches!(e, ConnectionEvent::HeartbeatLost { .. })
    })
    .await;
    if let ConnectionEvent::HeartbeatLost { last_seen_age } = lost {
        assert!(last_seen_age >= Duration::from_millis(200));
    }

    // 4. Resume sending -> HeartbeatRestored.
    peer.send(
        &MavHeader {
            system_id: 1,
            component_id: 1,
            sequence: 1,
        },
        &heartbeat(),
    )
    .await
    .expect("peer send");
    let _restored2 = wait_for(&mut rx, Duration::from_secs(2), |e| {
        matches!(e, ConnectionEvent::HeartbeatRestored)
    })
    .await;

    // 5. Explicit reconnect request is accepted without error.
    handle.reconnect().await.expect("reconnect request");
}

#[tokio::test(flavor = "multi_thread")]
async fn heartbeat_lost_while_other_nodes_keep_sending() {
    // Issue #1: inbound traffic from non-target nodes must not reset the
    // heartbeat timeout. A QGC-like peer keeps sending while the FC goes
    // silent; `HeartbeatLost` must still fire.
    let port = free_udp_port();
    let config = config_for_port(port).await;
    let heartbeat_timeout = config.heartbeat_timeout;
    let (handle, mut rx, _first) = spawn_connection(config).await.expect("spawn");

    let mut peer =
        mavlink::connect_async::<mavlink::common::MavMessage>(&format!("udpout:127.0.0.1:{port}"))
            .await
            .expect("peer connect");
    peer.set_protocol_version(mavlink::MavlinkVersion::V2);

    let _ = wait_for(&mut rx, Duration::from_secs(2), |e| {
        matches!(e, ConnectionEvent::Connected { .. })
    })
    .await;

    // FC heartbeat once so the link is declared alive.
    peer.send(
        &MavHeader {
            system_id: 1,
            component_id: 1,
            sequence: 0,
        },
        &heartbeat(),
    )
    .await
    .expect("peer send");
    let _ = wait_for(&mut rx, Duration::from_secs(2), |e| {
        matches!(e, ConnectionEvent::HeartbeatRestored)
    })
    .await;

    // Keep non-target (QGC-like) traffic flowing the whole time the FC is
    // silent.
    let noise = tokio::spawn(async move {
        for i in 0..20u8 {
            peer.send(
                &MavHeader {
                    system_id: 2,
                    component_id: 1,
                    sequence: i,
                },
                &heartbeat(),
            )
            .await
            .expect("noise send");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    let lost_at = tokio::time::Instant::now()
        + heartbeat_timeout
        + HEARTBEAT_WATCHDOG_TICK
        + Duration::from_millis(500);
    let lost = wait_for(&mut rx, Duration::from_secs(3), |e| {
        matches!(e, ConnectionEvent::HeartbeatLost { .. })
    })
    .await;
    assert!(
        tokio::time::Instant::now() <= lost_at,
        "HeartbeatLost fired too late despite non-target traffic"
    );
    if let ConnectionEvent::HeartbeatLost { last_seen_age } = lost {
        assert!(last_seen_age >= Duration::from_millis(200));
    }

    noise.await.expect("noise task");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gcs_heartbeat_is_announced_automatically() {
    // Issue #3: the worker must advertise a MAV_TYPE_GCS heartbeat on its own,
    // without the caller sending anything.
    let port = free_udp_port();
    let config = config_for_port(port).await;
    let our_system_id = config.system_id;
    let (handle, _rx, _first) = spawn_connection(config).await.expect("spawn");

    let mut peer =
        mavlink::connect_async::<mavlink::common::MavMessage>(&format!("udpout:127.0.0.1:{port}"))
            .await
            .expect("peer connect");
    peer.set_protocol_version(mavlink::MavlinkVersion::V2);

    // On `udpin` the worker learns the peer address from an inbound packet.
    peer.send(
        &MavHeader {
            system_id: 1,
            component_id: 1,
            sequence: 0,
        },
        &heartbeat(),
    )
    .await
    .expect("peer send");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        if remaining.is_zero() {
            panic!("no GCS heartbeat received within 3 s");
        }
        let (header, msg) = timeout(remaining, peer.recv())
            .await
            .expect("peer recv timeout")
            .expect("peer recv");
        if header.system_id != our_system_id {
            continue; // skip the FC's own heartbeat
        }
        match msg {
            MavMessage::HEARTBEAT(hb) => {
                assert_eq!(
                    hb.mavtype,
                    mavlink::common::MavType::MAV_TYPE_GCS,
                    "worker must announce itself as a GCS"
                );
                break;
            }
            other => panic!("unexpected worker message: {other:?}"),
        }
    }

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn first_attempt_success_is_reported() {
    // Issue #4: a successful bind resolves the first-attempt result with Ok.
    let port = free_udp_port();
    let config = config_for_port(port).await;
    let (handle, _rx, first_result) = spawn_connection(config).await.expect("spawn");

    let outcome = timeout(Duration::from_secs(2), first_result)
        .await
        .expect("first result timed out")
        .expect("first result channel closed");
    assert!(
        outcome.is_ok(),
        "a free UDP bind must succeed on the first attempt"
    );

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn first_attempt_failure_is_reported() {
    // Issue #4: a transport that fails to open must surface synchronously on
    // the first-attempt result instead of "ok" followed by a background event.
    let mut config = config_for_port(0).await;
    config.endpoint = Endpoint::Serial {
        port: "/dev/maggcs-nonexistent".into(),
        baudrate: 115_200,
    };

    let (handle, _rx, first_result) = spawn_connection(config).await.expect("spawn");
    let outcome = timeout(Duration::from_secs(3), first_result)
        .await
        .expect("first result timed out")
        .expect("first result channel closed");
    assert!(
        outcome.is_err(),
        "opening a missing serial device must fail the first attempt"
    );

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn send_round_trip_to_peer() {
    let port = free_udp_port();
    let config = config_for_port(port).await;
    let (handle, mut rx, _first) = spawn_connection(config).await.expect("spawn");

    let peer =
        mavlink::connect_async::<mavlink::common::MavMessage>(&format!("udpout:127.0.0.1:{port}"))
            .await
            .expect("peer connect");

    let _ = wait_for(&mut rx, Duration::from_secs(2), |e| {
        matches!(e, ConnectionEvent::Connected { .. })
    })
    .await;

    // The UDP server learns the peer's address from an inbound packet.
    peer.send(
        &MavHeader {
            system_id: 1,
            component_id: 1,
            sequence: 0,
        },
        &heartbeat(),
    )
    .await
    .expect("peer send");
    let _ = wait_for(
        &mut rx,
        Duration::from_secs(2),
        |e| matches!(e, ConnectionEvent::Message(m) if m.is_heartbeat_from(1, 1)),
    )
    .await;

    // Now send out through the worker; the peer must receive our GCS heartbeat.
    handle
        .send(MavMessage::HEARTBEAT(mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: mavlink::common::MavType::MAV_TYPE_GCS,
            autopilot: mavlink::common::MavAutopilot::MAV_AUTOPILOT_GENERIC,
            base_mode: mavlink::common::MavModeFlag::empty(),
            system_status: mavlink::common::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        }))
        .await
        .expect("send");

    let seen = timeout(Duration::from_secs(2), peer.recv())
        .await
        .expect("peer recv timeout")
        .expect("peer recv");
    assert_eq!(seen.0.system_id, 250, "sender is our system id");
    match seen.1 {
        MavMessage::HEARTBEAT(hb) => {
            assert_eq!(hb.mavtype, mavlink::common::MavType::MAV_TYPE_GCS);
        }
        other => panic!("unexpected peer message: {other:?}"),
    }
}
