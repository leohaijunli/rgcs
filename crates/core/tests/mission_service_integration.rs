//! Integration tests for the mission service over real UDP sockets (issue #18).
//!
//! A synthetic peer acts as the flight controller: it walks the upload
//! (MISSION_COUNT → MISSION_REQUEST_INT → MISSION_ITEM_INT → MISSION_ACK) and
//! download (MISSION_REQUEST_LIST → MISSION_COUNT → MISSION_REQUEST_INT →
//! MISSION_ITEM_INT → MISSION_ACK) exchanges and asserts the core service
//! forwards the right events, including the assembled plan.

use std::net::UdpSocket;
use std::time::Duration;

use maggcs_core::mavlink::{
    connection::spawn_connection, message::MavMessage, ConnectionConfig, ConnectionEvent, Endpoint,
    MavHeader,
};
use maggcs_core::mission::protocol::{mission_item_to_mav, MissionEvent, MissionOperation};
use maggcs_core::mission::types::{MissionFrame, MissionItem};
use maggcs_core::mission::{MissionIds, MissionService, MissionServiceEvent};
use tokio::time::timeout;

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

fn config_for_port(port: u16) -> ConnectionConfig {
    ConnectionConfig {
        endpoint: Endpoint::UdpListener {
            addr: format!("127.0.0.1:{port}").parse().unwrap(),
        },
        system_id: 250,
        component_id: 250,
        target_system_id: 1,
        target_component_id: 1,
        heartbeat_timeout: Duration::from_secs(30),
        reconnect_delay: Duration::from_millis(50),
        ..Default::default()
    }
}

type Peer = Box<dyn mavlink::AsyncMavConnection<MavMessage> + Sync + Send>;

const FC_HEADER: MavHeader = MavHeader {
    system_id: 1,
    component_id: 1,
    sequence: 0,
};

fn waypoint(seq: u16, lat: f64, lon: f64, alt: f32) -> MissionItem {
    let mut item = MissionItem::waypoint(lat, lon, alt, MissionFrame::GlobalRelativeAltInt);
    item.seq = seq;
    item
}

struct Harness {
    handle: maggcs_core::mavlink::ConnectionHandle,
    svc: MissionService,
    evt: tokio::sync::mpsc::Receiver<MissionServiceEvent>,
    peer: Peer,
}

async fn harness() -> Harness {
    loop {
        let port = free_udp_port();
        let config = config_for_port(port);
        let (handle, mut rx, first_result) = match spawn_connection(config.clone()).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let bound = matches!(
            timeout(Duration::from_secs(1), first_result).await,
            Ok(Ok(Ok(())))
        );
        if !bound {
            handle.shutdown().await;
            continue;
        }

        let mut peer = match mavlink::connect_async::<mavlink::common::MavMessage>(&format!(
            "udpout:127.0.0.1:{port}"
        ))
        .await
        {
            Ok(p) => p,
            Err(_) => {
                handle.shutdown().await;
                continue;
            }
        };
        peer.set_protocol_version(mavlink::MavlinkVersion::V2);
        peer.send(&FC_HEADER, &heartbeat())
            .await
            .expect("peer heartbeat");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut seen = false;
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            match timeout(remaining, rx.recv()).await {
                Ok(Ok(ConnectionEvent::HeartbeatRestored)) => {
                    seen = true;
                    break;
                }
                Ok(Ok(_)) => {}
                _ => break,
            }
        }
        if !seen {
            handle.shutdown().await;
            continue;
        }

        let ids = MissionIds {
            self_system: config.system_id,
            self_component: config.component_id,
            target_system: config.target_system_id,
            target_component: config.target_component_id,
        };
        let (svc, evt) = MissionService::spawn(handle.clone(), ids);
        return Harness {
            handle,
            svc,
            evt,
            peer,
        };
    }
}

/// Receive the next frame from the FC peer; fails the test on timeout.
async fn peer_recv(peer: &mut Peer) -> (MavHeader, MavMessage) {
    timeout(Duration::from_secs(3), peer.recv())
        .await
        .expect("peer recv timed out")
        .expect("peer recv")
}

/// Receive the next frame matching the predicate, skipping the GCS's own
/// heartbeat and other unrelated traffic.
async fn peer_recv_until<F, T>(peer: &mut Peer, mut pick: F) -> T
where
    F: FnMut(MavMessage) -> Option<T>,
{
    loop {
        let (_header, msg) = peer_recv(peer).await;
        if let Some(v) = pick(msg) {
            return v;
        }
    }
}

async fn peer_send(peer: &mut Peer, msg: &MavMessage) {
    peer.send(&FC_HEADER, msg).await.expect("peer send");
}

/// Wait for the next service event matching the predicate.
async fn wait_for<F>(
    evt: &mut tokio::sync::mpsc::Receiver<MissionServiceEvent>,
    within: Duration,
    mut pred: F,
) -> MissionServiceEvent
where
    F: FnMut(&MissionServiceEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let e = timeout(remaining, evt.recv())
            .await
            .expect("timed out waiting for mission event")
            .expect("mission event channel closed");
        if pred(&e) {
            return e;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_walks_progress_to_completed() {
    let mut h = harness().await;
    let items = vec![
        waypoint(0, 49.25, -123.10, 40.0),
        waypoint(1, 49.26, -123.09, 45.0),
        waypoint(2, 49.27, -123.08, 50.0),
    ];
    h.svc.upload(items.clone()).await.expect("enqueue upload");

    let count = peer_recv_until(&mut h.peer, |m| match m {
        MavMessage::MISSION_COUNT(c) => Some(c.count),
        _ => None,
    })
    .await;
    assert_eq!(count, 3);

    for seq in 0..3u16 {
        peer_send(
            &mut h.peer,
            &MavMessage::MISSION_REQUEST_INT(mavlink::common::MISSION_REQUEST_INT_DATA {
                target_system: 250,
                target_component: 250,
                seq,
            }),
        )
        .await;
        let received = peer_recv_until(&mut h.peer, |m| match m {
            MavMessage::MISSION_ITEM_INT(item) => Some(item),
            _ => None,
        })
        .await;
        assert_eq!(received.seq, seq, "FC requested seq {seq}");
    }

    peer_send(
        &mut h.peer,
        &MavMessage::MISSION_ACK(mavlink::common::MISSION_ACK_DATA {
            target_system: 250,
            target_component: 250,
            mavtype: mavlink::common::MavMissionResult::MAV_MISSION_ACCEPTED,
        }),
    )
    .await;

    let done = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(
            e,
            MissionServiceEvent::Protocol(MissionEvent::Completed(MissionOperation::Upload))
        )
    })
    .await;
    assert!(matches!(
        done,
        MissionServiceEvent::Protocol(MissionEvent::Completed(MissionOperation::Upload))
    ));

    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn download_emits_assembled_plan() {
    let mut h = harness().await;
    h.svc.download().await.expect("enqueue download");

    peer_recv_until(&mut h.peer, |m| match m {
        MavMessage::MISSION_REQUEST_LIST(_) => Some(()),
        _ => None,
    })
    .await;

    peer_send(
        &mut h.peer,
        &MavMessage::MISSION_COUNT(mavlink::common::MISSION_COUNT_DATA {
            target_system: 250,
            target_component: 250,
            count: 2,
        }),
    )
    .await;

    for seq in 0..2u16 {
        let requested = peer_recv_until(&mut h.peer, |m| match m {
            MavMessage::MISSION_REQUEST_INT(r) => Some(r.seq),
            _ => None,
        })
        .await;
        assert_eq!(requested, seq);
        let item = waypoint(seq, 49.25 + f64::from(seq), -123.10, 40.0);
        peer_send(
            &mut h.peer,
            &MavMessage::MISSION_ITEM_INT(mission_item_to_mav(250, 250, &item)),
        )
        .await;
    }

    peer_recv_until(&mut h.peer, |m| match m {
        MavMessage::MISSION_ACK(_) => Some(()),
        _ => None,
    })
    .await;

    let plan = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(e, MissionServiceEvent::PlanDownloaded(_))
    })
    .await;
    let MissionServiceEvent::PlanDownloaded(items) = plan else {
        unreachable!("predicate matched PlanDownloaded");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].seq, 0);
    assert_eq!(items[1].seq, 1);

    h.handle.shutdown().await;
}
