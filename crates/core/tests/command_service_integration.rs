//! Integration tests for the command service over real UDP sockets (issue #5).
//!
//! A synthetic peer acts as the flight controller and drives the three
//! acceptance cases: acked, denied (`MAV_RESULT_DENIED`), and an ACK that is
//! lost and recovered by a retransmission with an incremented `confirmation`.

use std::net::UdpSocket;
use std::time::Duration;

use maggcs_core::commands::{
    CommandError, CommandEvent, CommandResult, CommandService, CommandTransaction,
};
use maggcs_core::mavlink::{
    connection::spawn_connection, message::MavMessage, ConnectionConfig, ConnectionEvent, Endpoint,
    MavHeader,
};
use tokio::time::timeout;

const RTL: mavlink::common::MavCmd = mavlink::common::MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH;
/// The normal-priority command the A7 acceptance case preempts.
const GOTO: mavlink::common::MavCmd = mavlink::common::MavCmd::MAV_CMD_DO_REPOSITION;

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

async fn peer_send_ack(
    peer: &mut Peer,
    command: mavlink::common::MavCmd,
    result: mavlink::common::MavResult,
) {
    let ack = MavMessage::COMMAND_ACK(mavlink::common::COMMAND_ACK_DATA {
        command,
        result,
        ..Default::default()
    });
    peer.send(
        &MavHeader {
            system_id: 1,
            component_id: 1,
            sequence: 0,
        },
        &ack,
    )
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

        // The `udpin` worker learns our address from the first inbound packet.
        peer.send(
            &MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            &heartbeat(),
        )
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

    // Skip the worker's own GCS heartbeat announcements (1 Hz) — they race
    // the command onto the wire.
    let cmd = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(cmd.command, RTL);
    assert_eq!(cmd.confirmation, 0);

    peer_send_ack(
        &mut h.peer,
        cmd.command,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;

    let e = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(
            e,
            CommandEvent::Completed {
                result: CommandResult::Accepted,
                ..
            }
        )
    })
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

    // Skip the worker's own GCS heartbeat announcements (1 Hz) — they race
    // the command onto the wire.
    let cmd = peer_recv_command_long(&mut h.peer).await;
    peer_send_ack(
        &mut h.peer,
        cmd.command,
        mavlink::common::MavResult::MAV_RESULT_DENIED,
    )
    .await;

    let e = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(
            e,
            CommandEvent::Completed {
                result: CommandResult::Denied,
                ..
            }
        )
    })
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
    assert_eq!(
        retry.confirmation, 1,
        "retransmission must bump confirmation"
    );

    peer_send_ack(
        &mut h.peer,
        retry.command,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;

    let e = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(
            e,
            CommandEvent::Completed {
                result: CommandResult::Accepted,
                ..
            }
        )
    })
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
async fn an_emergency_command_preempts_an_in_flight_goto() {
    let mut h = harness().await;

    // A goto is in flight and the FC has not answered it.
    h.svc.send(GOTO, [0.0; 7]).await.expect("enqueue goto");
    let first = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(first.command, GOTO);
    assert_eq!(first.confirmation, 0);

    // RTL behind it: it goes out now instead of waiting out the goto's retry
    // budget (the acceptance case for A7).
    h.svc.send(RTL, [0.0; 7]).await.expect("enqueue rtl");
    let preempting = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(
        preempting.command, RTL,
        "the emergency command must not queue behind the goto"
    );
    assert_eq!(preempting.confirmation, 0);

    // The displaced goto is reported to its caller, not dropped silently.
    let e = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(
            e,
            CommandEvent::Failed {
                error: CommandError::Superseded,
                ..
            }
        )
    })
    .await;
    assert_eq!(
        e,
        CommandEvent::Failed {
            command: GOTO,
            error: CommandError::Superseded,
        }
    );

    // RTL's ack is lost: the retransmission carries RTL with the incremented
    // confirmation — the superseded goto must never come back out of order.
    let retry = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(retry.command, RTL);
    assert_eq!(retry.confirmation, 1);

    peer_send_ack(
        &mut h.peer,
        retry.command,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;
    let done = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(e, CommandEvent::Completed { .. })
    })
    .await;
    assert_eq!(
        done,
        CommandEvent::Completed {
            command: RTL,
            result: CommandResult::Accepted,
        }
    );

    // A late ack for the preempted goto arrives after everything settled: the
    // session is idle again and nothing surfaces.
    peer_send_ack(
        &mut h.peer,
        GOTO,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;
    let quiet = timeout(Duration::from_millis(500), h.evt.recv()).await;
    assert!(
        quiet.is_err(),
        "a late ack for a superseded command must not surface"
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

    let e = wait_for(&mut h.evt, Duration::from_secs(10), |e| {
        matches!(
            e,
            CommandEvent::Failed {
                error: CommandError::NoAck { .. },
                ..
            }
        )
    })
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

#[tokio::test(flavor = "multi_thread")]
async fn send_await_returns_the_ack_result() {
    let mut h = harness().await;
    let svc = h.svc.clone();
    let awaiting =
        tokio::spawn(async move { svc.send_await(RTL, [0.0; 7], Duration::from_secs(5)).await });

    let cmd = peer_recv_command_long(&mut h.peer).await;
    peer_send_ack(
        &mut h.peer,
        cmd.command,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;

    let result = awaiting.await.expect("join").expect("send_await");
    assert_eq!(result, CommandResult::Accepted);
    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn send_await_returns_denied_verdict() {
    let mut h = harness().await;
    let svc = h.svc.clone();
    let awaiting =
        tokio::spawn(async move { svc.send_await(RTL, [0.0; 7], Duration::from_secs(5)).await });

    let cmd = peer_recv_command_long(&mut h.peer).await;
    peer_send_ack(
        &mut h.peer,
        cmd.command,
        mavlink::common::MavResult::MAV_RESULT_DENIED,
    )
    .await;

    // DENIED must reach the caller as data, not as a send failure: SITL
    // denies PREFLIGHT_REBOOT_SHUTDOWN exactly like this.
    let result = awaiting.await.expect("join").expect("send_await");
    assert_eq!(result, CommandResult::Denied);
    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn send_await_while_busy_answers_busy() {
    let h = harness().await;
    // An unacked plain send keeps the single session slot occupied.
    h.svc.send(RTL, [0.0; 7]).await.expect("enqueue");

    let svc = h.svc.clone();
    let err = svc
        .send_await(RTL, [0.0; 7], Duration::from_secs(5))
        .await
        .expect_err("must be busy");
    assert_eq!(err, CommandError::Busy);

    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn send_await_times_out_without_terminal_ack() {
    let h = harness().await;
    // A short await cap fires before the session's own retry budget; the
    // command keeps retransmitting in the background afterwards.
    let err = h
        .svc
        .send_await(RTL, [0.0; 7], Duration::from_millis(300))
        .await
        .expect_err("must time out");
    assert_eq!(err, CommandError::TimedOut);

    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_mode_sends_custom_flag_and_acks() {
    let mut h = harness().await;
    // Hold = Auto(4)/Loiter(3): param1 must carry the CUSTOM_MODE bit.
    const MAIN_AUTO: u8 = 4;
    const SUB_LOITER: u8 = 3;
    h.svc
        .send(
            mavlink::common::MavCmd::MAV_CMD_DO_SET_MODE,
            [1.0, MAIN_AUTO as f32, SUB_LOITER as f32, 0.0, 0.0, 0.0, 0.0],
        )
        .await
        .expect("enqueue");

    let cmd = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(cmd.command, mavlink::common::MavCmd::MAV_CMD_DO_SET_MODE);
    assert_eq!(cmd.param1, 1.0);
    assert_eq!(cmd.param2, MAIN_AUTO as f32);
    assert_eq!(cmd.param3, SUB_LOITER as f32);

    peer_send_ack(
        &mut h.peer,
        cmd.command,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;

    let e = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(
            e,
            CommandEvent::Completed {
                result: CommandResult::Accepted,
                ..
            }
        )
    })
    .await;
    assert_eq!(
        e,
        CommandEvent::Completed {
            command: mavlink::common::MavCmd::MAV_CMD_DO_SET_MODE,
            result: CommandResult::Accepted,
        }
    );

    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn arming_denial_surfaces_temporarily_rejected() {
    let mut h = harness().await;
    // PX4 denies external GCS arming through its preflight checks with
    // TEMPORARILY_REJECTED (ADR-017) — the denial must arrive as data.
    h.svc
        .send(
            mavlink::common::MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .await
        .expect("enqueue");

    let cmd = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(
        cmd.command,
        mavlink::common::MavCmd::MAV_CMD_COMPONENT_ARM_DISARM
    );
    assert_eq!(cmd.param1, 1.0);
    assert_eq!(cmd.param2, 0.0, "the 21196 kill code must never be sent");

    peer_send_ack(
        &mut h.peer,
        cmd.command,
        mavlink::common::MavResult::MAV_RESULT_TEMPORARILY_REJECTED,
    )
    .await;

    let e = wait_for(&mut h.evt, Duration::from_secs(2), |e| {
        matches!(
            e,
            CommandEvent::Completed {
                result: CommandResult::TemporarilyRejected,
                ..
            }
        )
    })
    .await;
    assert_eq!(
        e,
        CommandEvent::Completed {
            command: mavlink::common::MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
            result: CommandResult::TemporarilyRejected,
        }
    );

    h.handle.shutdown().await;
}

/// L10: an operator RTL during a link outage. The heartbeat is gone, so the
/// Emergency send must fail fast with a clear "no link" verdict — never hang
/// the safety path on a retry budget, never pretend it went out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_rtl_during_a_link_outage_fails_fast_with_no_link() {
    let h = harness().await;
    // Kill the peer (the FC goes silent) and drain the heartbeat budget.
    drop(h.peer);
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    let start = std::time::Instant::now();
    let verdict = h
        .svc
        .send_await(RTL, [0.0; 7], Duration::from_secs(1))
        .await;
    let elapsed = start.elapsed();
    // The verdict is either LinkLost (the connection layer saw the heartbeat
    // die first) or TimedOut (the 1 s await budget beat the heartbeat
    // monitor) — both are fast, honest failures; what must never happen is
    // a long hang pretending the link might answer.
    match &verdict {
        Err(maggcs_core::commands::CommandError::LinkLost)
        | Err(maggcs_core::commands::CommandError::TimedOut) => {}
        other => panic!("expected LinkLost or TimedOut, got {other:?}"),
    }
    // Fast-fail: the operator's safety button must not sit out a long
    // retransmission budget against a dead link.
    assert!(
        elapsed < Duration::from_secs(2),
        "the refusal took {elapsed:?} — a dead link must fail fast"
    );
}

/// L10: a STATUSTEXT flood from the FC must neither stall the command path
/// nor lose emergency preemption — 300 status texts, then an RTL overtakes a
/// busy goto exactly as on a quiet link.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_statustext_flood_neither_stalls_commands_nor_breaks_preemption() {
    let mut h = harness().await;
    // Flood: 300 STATUSTEXTs at once (the hub assembles losslessly; the
    // command queue must not care).
    for i in 0..300u16 {
        let text = format!("flood {i}");
        h.peer
            .send(
                &mavlink::MavHeader {
                    system_id: 1,
                    component_id: 1,
                    sequence: (100 + i as u32) as u8,
                },
                &MavMessage::STATUSTEXT(mavlink::common::STATUSTEXT_DATA {
                    severity: ::mavlink::common::MavSeverity::MAV_SEVERITY_INFO,
                    id: 0,
                    chunk_seq: 0,
                    text: text.as_str().into(),
                }),
            )
            .await
            .expect("send statustext");
    }

    // A goto is in flight; the RTL must still preempt immediately.
    h.svc.send(GOTO, [0.0; 7]).await.expect("enqueue goto");
    let _first = peer_recv_command_long(&mut h.peer).await;
    h.svc.send(RTL, [0.0; 7]).await.expect("enqueue rtl");
    let preempting = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(
        preempting.command, RTL,
        "the flood must not delay preemption"
    );
}

fn transaction(h: &Harness, budget: Duration) -> CommandTransaction {
    CommandTransaction::new(
        maggcs_core::runtime::OperationId::fresh().expect("operation ID"),
        maggcs_core::runtime::TransportGeneration::new(h.handle.generation()).expect("generation"),
        budget,
    )
    .expect("transaction")
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_cancel_retires_only_its_command_and_quarantines_ambiguous_ack() {
    let mut h = harness().await;
    let owned = transaction(&h, Duration::from_secs(5));
    let (svc, tx) = (h.svc.clone(), owned.clone());
    let task = tokio::spawn(async move { svc.send_owned(tx, GOTO, [0.0; 7]).await });
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, GOTO);
    let started = std::time::Instant::now();
    assert!(h.svc.cancel_owned(&owned).await.expect("cancel receipt"));
    assert!(started.elapsed() < maggcs_core::commands::OWNED_CANCEL_TIMEOUT);
    assert_eq!(
        task.await.expect("owned waiter"),
        Err(CommandError::Cancelled)
    );
    assert!(owned.may_have_dispatched());
    let repeat = transaction(&h, Duration::from_secs(5));
    assert_eq!(
        h.svc.send_owned(repeat.clone(), GOTO, [0.0; 7]).await,
        Err(CommandError::AmbiguousAck)
    );
    assert!(!repeat.may_have_dispatched());

    let svc = h.svc.clone();
    let emergency =
        tokio::spawn(async move { svc.send_await(RTL, [0.0; 7], Duration::from_secs(5)).await });
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, RTL);
    assert!(h
        .svc
        .cancel_owned(&owned)
        .await
        .expect("old repeated cancel"));
    peer_send_ack(
        &mut h.peer,
        GOTO,
        mavlink::common::MavResult::MAV_RESULT_DENIED,
    )
    .await;
    peer_send_ack(
        &mut h.peer,
        RTL,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;
    assert_eq!(
        emergency.await.expect("emergency waiter"),
        Ok(CommandResult::Accepted)
    );
    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn awaiting_emergency_does_not_overwrite_the_displaced_waiter() {
    let mut h = harness().await;
    let svc = h.svc.clone();
    let normal =
        tokio::spawn(async move { svc.send_await(GOTO, [0.0; 7], Duration::from_secs(5)).await });
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, GOTO);
    let svc = h.svc.clone();
    let emergency =
        tokio::spawn(async move { svc.send_await(RTL, [0.0; 7], Duration::from_secs(5)).await });
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, RTL);
    assert_eq!(
        normal.await.expect("old waiter"),
        Err(CommandError::Superseded)
    );
    let quarantined = transaction(&h, Duration::from_secs(5));
    assert_eq!(
        h.svc.send_owned(quarantined.clone(), GOTO, [0.0; 7]).await,
        Err(CommandError::AmbiguousAck),
        "a legacy command's late ACK cannot complete new owned work"
    );
    assert!(!quarantined.may_have_dispatched());
    peer_send_ack(
        &mut h.peer,
        GOTO,
        mavlink::common::MavResult::MAV_RESULT_DENIED,
    )
    .await;
    peer_send_ack(
        &mut h.peer,
        RTL,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;
    assert_eq!(
        emergency.await.expect("new waiter"),
        Ok(CommandResult::Accepted)
    );

    assert_eq!(
        h.svc
            .send_owned(transaction(&h, Duration::from_secs(5)), GOTO, [0.0; 7])
            .await,
        Err(CommandError::AmbiguousAck),
        "late legacy ACKs cannot clear quarantine in the same generation"
    );
    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ack_quarantine_never_blocks_emergency_send_but_never_reports_stale_success() {
    let mut h = harness().await;
    let old = transaction(&h, Duration::from_secs(5));
    let (svc, transaction) = (h.svc.clone(), old.clone());
    let old_task = tokio::spawn(async move { svc.send_owned(transaction, RTL, [0.0; 7]).await });
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, RTL);
    h.svc.cancel_owned(&old).await.expect("retire old RTL");
    assert_eq!(
        old_task.await.expect("old RTL waiter"),
        Err(CommandError::Cancelled)
    );

    let svc = h.svc.clone();
    let emergency = tokio::spawn(async move {
        svc.send_await(RTL, [0.0; 7], Duration::from_millis(500))
            .await
    });
    let first = peer_recv_command_long(&mut h.peer).await;
    assert_eq!(first.command, RTL, "the emergency is sent despite ACK debt");
    peer_send_ack(
        &mut h.peer,
        RTL,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !emergency.is_finished(),
        "the ambiguous first ACK is not trusted"
    );
    peer_send_ack(
        &mut h.peer,
        RTL,
        mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
    )
    .await;
    assert_eq!(
        emergency.await.expect("emergency waiter"),
        Err(CommandError::TimedOut)
    );
    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dropped_owned_waiter_stops_retransmission_without_touching_transport() {
    let mut h = harness().await;
    let owned = transaction(&h, Duration::from_secs(5));
    let (svc, tx) = (h.svc.clone(), owned.clone());
    let task = tokio::spawn(async move { svc.send_owned(tx, GOTO, [0.0; 7]).await });
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, GOTO);
    task.abort();
    assert!(task.await.expect_err("aborted waiter").is_cancelled());
    assert!(h.svc.cancel_owned(&owned).await.expect("worker retirement"));
    assert!(
        timeout(
            maggcs_core::commands::COMMAND_RETRY_TIMEOUT + Duration::from_millis(200),
            peer_recv_command_long(&mut h.peer)
        )
        .await
        .is_err(),
        "no retry after caller loss"
    );
    assert_eq!(
        h.handle.state().await,
        maggcs_core::mavlink::LinkState::Connected
    );
    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_deadline_and_emergency_survive_a_saturated_event_consumer() {
    let mut h = harness().await;
    h.svc.send(GOTO, [0.0; 7]).await.expect("initial command");
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, GOTO);
    // Keep the real event receiver alive but unread. Busy notifications exceed
    // the actual queue's capacity; command/reply/cancel must remain responsive.
    for _ in 0..48 {
        h.svc
            .send(GOTO, [0.0; 7])
            .await
            .expect("concurrent request");
    }
    let owned = transaction(&h, Duration::from_millis(500));
    let (svc, tx) = (h.svc.clone(), owned.clone());
    let started = std::time::Instant::now();
    let task = tokio::spawn(async move { svc.send_owned(tx, RTL, [0.0; 7]).await });
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, RTL);
    peer_send_ack(
        &mut h.peer,
        RTL,
        mavlink::common::MavResult::MAV_RESULT_IN_PROGRESS,
    )
    .await;
    assert_eq!(
        task.await.expect("bounded waiter"),
        Err(CommandError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(h
        .svc
        .cancel_owned(&owned)
        .await
        .expect("bounded cancellation"));
    assert!(h.svc.dropped_events() > 0, "event loss is counted");
    assert!(timeout(
        maggcs_core::commands::COMMAND_RETRY_TIMEOUT + Duration::from_millis(200),
        peer_recv_command_long(&mut h.peer)
    )
    .await
    .is_err());
    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_or_precancelled_owned_requests_never_write() {
    let mut h = harness().await;
    let cancelled = transaction(&h, Duration::from_secs(5));
    assert!(!h
        .svc
        .cancel_owned(&cancelled)
        .await
        .expect("unsubmitted cancel"));
    assert_eq!(
        h.svc.send_owned(cancelled.clone(), GOTO, [0.0; 7]).await,
        Err(CommandError::Cancelled)
    );
    assert!(!cancelled.may_have_dispatched());
    assert_eq!(
        h.svc.send_owned(cancelled, GOTO, [0.0; 7]).await,
        Err(CommandError::AlreadySubmitted)
    );

    let stale = transaction(&h, Duration::from_secs(5));
    h.handle.reconnect().await.expect("reopen");
    timeout(Duration::from_secs(2), async {
        while h.handle.generation() == stale.generation().get() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("new generation");
    assert_eq!(
        h.svc.send_owned(stale.clone(), GOTO, [0.0; 7]).await,
        Err(CommandError::TargetChanged)
    );
    assert!(!stale.may_have_dispatched());
    assert!(timeout(
        Duration::from_millis(250),
        peer_recv_command_long(&mut h.peer)
    )
    .await
    .is_err());
    h.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_ack_must_match_source_and_gcs_destination() {
    let mut h = harness().await;
    let owned = transaction(&h, Duration::from_secs(5));
    let svc = h.svc.clone();
    let task = tokio::spawn(async move { svc.send_owned(owned, GOTO, [0.0; 7]).await });
    assert_eq!(peer_recv_command_long(&mut h.peer).await.command, GOTO);
    for (source, destination) in [(2, 250), (1, 249)] {
        h.peer
            .send(
                &MavHeader {
                    system_id: source,
                    component_id: 1,
                    sequence: 0,
                },
                &MavMessage::COMMAND_ACK(mavlink::common::COMMAND_ACK_DATA {
                    command: GOTO,
                    result: mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
                    target_system: destination,
                    target_component: 250,
                    ..Default::default()
                }),
            )
            .await
            .expect("foreign ACK");
    }
    peer_send_ack(
        &mut h.peer,
        GOTO,
        mavlink::common::MavResult::MAV_RESULT_DENIED,
    )
    .await;
    assert_eq!(task.await.expect("owned waiter"), Ok(CommandResult::Denied));
    h.handle.shutdown().await;
}
