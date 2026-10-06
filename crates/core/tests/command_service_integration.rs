//! Integration tests for the command service over real UDP sockets (issue #5).
//!
//! A synthetic peer acts as the flight controller and drives the three
//! acceptance cases: acked, denied (`MAV_RESULT_DENIED`), and an ACK that is
//! lost and recovered by a retransmission with an incremented `confirmation`.

use std::net::UdpSocket;
use std::time::Duration;

use maggcs_core::commands::{CommandError, CommandEvent, CommandResult, CommandService};
use maggcs_core::mavlink::{
    connection::spawn_connection, message::MavMessage, ConnectionConfig, ConnectionEvent, Endpoint,
    MavHeader,
};
use tokio::time::timeout;

const RTL: mavlink::common::MavCmd = mavlink::common::MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH;

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
        // Long timeout so the FC staying silent on acks does not cancel the
        // session via HeartbeatLost before the retry budget runs out.
        heartbeat_timeout: Duration::from_secs(30),
        reconnect_delay: Duration::from_millis(50),
        ..Default::default()
    }
}

type Peer = Box<dyn mavlink::AsyncMavConnection<MavMessage> + Sync + Send>;

/// Wait for the next command event matching the predicate.
async fn wait_for<F>(
    evt: &mut tokio::sync::mpsc::Receiver<CommandEvent>,
    within: Duration,
    mut pred: F,
) -> CommandEvent
where
    F: FnMut(&CommandEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let e = timeout(remaining, evt.recv())
            .await
            .expect("timed out waiting for command event")
            .expect("command event channel closed");
        if pred(&e) {
            return e;
        }
    }
}

/// Receive the next frame from the FC peer; fails the test on timeout.
async fn peer_recv(peer: &mut Peer) -> (MavHeader, MavMessage) {
    timeout(Duration::from_secs(3), peer.recv())
        .await
        .expect("peer recv timed out")
        .expect("peer recv")
}

/// Receive the next `COMMAND_LONG` from the FC peer, skipping the worker's
/// own GCS heartbeats and other traffic.
async fn peer_recv_command_long(peer: &mut Peer) -> mavlink::common::COMMAND_LONG_DATA {
    loop {
        let (_header, msg) = peer_recv(peer).await;
        if let MavMessage::COMMAND_LONG(cmd) = msg {
            return cmd;
        }
    }
}

async fn peer_send_ack(peer: &mut Peer, command: mavlink::common::MavCmd, result: mavlink::common::MavResult) {
    let ack = MavMessage::COMMAND_ACK(mavlink::common::COMMAND_ACK_DATA { command, result });
    peer.send(&MavHeader { system_id: 1, component_id: 1, sequence: 0 }, &ack)
        .await
        .expect("peer send ack");
}

struct Harness {
    handle: maggcs_core::mavlink::ConnectionHandle,
    svc: CommandService,
    evt: tokio::sync::mpsc::Receiver<CommandEvent>,
    peer: Peer,
}

async fn harness() -> Harness {
    // Tests run in parallel and `free_udp_port` can hand out a port a sibling
    // test just bound; retry the whole setup on a fresh port when the bind or
    // the initial handshake fails.
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

        let mut peer = match mavlink::connect_async::<mavlink::common::MavMessage>(
            &format!("udpout:127.0.0.1:{port}"),
        )
        .await
        {
            Ok(p) => p,
            Err(_) => {
                handle.shutdown().await;
                continue;
            }
        };
        peer.set_protocol_version(mavlink::MavlinkVersion::V2);

        // The `udpin` worker learns our address from the first inbound packet.
        peer.send(&MavHeader { system_id: 1, component_id: 1, sequence: 0 }, &heartbeat())
            .await
            .expect("peer heartbeat");

        // Wait for the worker to come up and the FC heartbeat to be seen.
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

        let (svc, evt) = CommandService::spawn(
            handle.clone(),
            config.target_system_id,
            config.target_component_id,
        );

        return Harness {
            handle,
            svc,
            evt,
            peer,
        };
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ack_accepted_completes() {
    let mut h = harness().await;
    h.svc.send(RTL, [0.0; 7]).await.expect("enqueue");

    let (_header, msg) = peer_recv(&mut h.peer).await;
    let mavlink::common::MavMessage::COMMAND_LONG(cmd) = msg else {
        panic!("FC expected COMMAND_LONG, got {msg:?}");
    };
    assert_eq!(cmd.command, RTL);
    assert_eq!(cmd.confirmation, 0);

    peer_send_ack(&mut h.peer, cmd.command, mavlink::common::MavResult::MAV_RESULT_ACCEPTED)
        .await;

    let e = wait_for(
        &mut h.evt,
        Duration::from_secs(2),
        |e| matches!(e, CommandEvent::Completed { result: CommandResult::Accepted, .. }),
    )
    .await;
    assert_eq!(
        e,
        CommandEvent::Completed {
            command: RTL,
            result: CommandResult::Accepted,
        }
    );

    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_reports_result() {
    let mut h = harness().await;
    h.svc.send(RTL, [0.0; 7]).await.expect("enqueue");

    let (_header, msg) = peer_recv(&mut h.peer).await;
    let mavlink::common::MavMessage::COMMAND_LONG(cmd) = msg else {
        panic!("FC expected COMMAND_LONG, got {msg:?}");
    };
    peer_send_ack(&mut h.peer, cmd.command, mavlink::common::MavResult::MAV_RESULT_DENIED)
        .await;

    let e = wait_for(
        &mut h.evt,
        Duration::from_secs(2),
        |e| matches!(e, CommandEvent::Completed { result: CommandResult::Denied, .. }),
    )
    .await;
    assert_eq!(
        e,
        CommandEvent::Completed {
            command: RTL,
            result: CommandResult::Denied,
        }
    );

    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lost_ack_retransmits_with_confirmation() {
    let mut h = harness().await;
    h.svc.send(RTL, [0.0; 7]).await.expect("enqueue");

    let first = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(first.command, RTL);
    assert_eq!(first.confirmation, 0);

    // The service retransmits after the retry timeout with confirmation = 1.
    let retry = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(retry.command, first.command);
    assert_eq!(retry.confirmation, 1, "retransmission must bump confirmation");

    peer_send_ack(&mut h.peer, retry.command, mavlink::common::MavResult::MAV_RESULT_ACCEPTED)
        .await;

    let e = wait_for(
        &mut h.evt,
        Duration::from_secs(2),
        |e| matches!(e, CommandEvent::Completed { result: CommandResult::Accepted, .. }),
    )
    .await;
    assert_eq!(
        e,
        CommandEvent::Completed {
            command: RTL,
            result: CommandResult::Accepted,
        }
    );

    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn never_acked_fails_with_no_ack() {
    let mut h = harness().await;
    h.svc.send(RTL, [0.0; 7]).await.expect("enqueue");

    // FC receives the command but never acknowledges; the session exhausts its
    // retry budget (~4 s) and reports a terminal NoAck failure.
    let (_header, _msg) = peer_recv(&mut h.peer).await;

    let e = wait_for(
        &mut h.evt,
        Duration::from_secs(10),
        |e| matches!(e, CommandEvent::Failed { error: CommandError::NoAck { .. }, .. }),
    )
    .await;
    assert_eq!(
        e,
        CommandEvent::Failed {
            command: RTL,
            error: CommandError::NoAck {
                attempts: maggcs_core::commands::COMMAND_MAX_RETRIES + 1,
            },
        }
    );

    h.handle.shutdown().await;
}