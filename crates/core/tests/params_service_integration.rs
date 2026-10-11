//! Parameter service integration over a real (loopback UDP) link
//! (parameters plan T2/T3): a fake FC streams a parameter list with loss,
//! duplication and reordering; the service must converge to the full set via
//! its fill rounds, and a `PARAM_SET` echo resolves the set request.

use std::time::Duration;

use ::mavlink::common::{MavParamType, PARAM_VALUE_DATA};
use ::mavlink::MavHeader;
use maggcs_core::mavlink::connection::{spawn_connection, ConnectionConfig, ConnectionEvent};
use maggcs_core::mavlink::endpoint::Endpoint;
use maggcs_core::mavlink::MavMessage;
use maggcs_core::params::model::{ParamType, ParamValue};
use maggcs_core::params::service::{ParamError, ParamEvent, ParamService, SetResult};
use tokio::time::timeout;

fn free_udp_port() -> u16 {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind ephemeral");
    sock.local_addr().unwrap().port()
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

const FC_HEADER: MavHeader = MavHeader {
    system_id: 1,
    component_id: 1,
    sequence: 0,
};

/// One FC parameter.
fn fc_param(index: u16, count: u16) -> MavMessage {
    let name = format!("P_{index:04}");
    let mut id = [0u8; 16];
    id[..name.len()].copy_from_slice(name.as_bytes());
    MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
        param_id: id.into(),
        param_value: maggcs_core::params::codec::encode(
            ParamValue::Int(index as i64),
            maggcs_core::params::codec::IntEncoding::ByteConcat,
        ),
        param_type: MavParamType::MAV_PARAM_TYPE_INT32,
        param_count: count,
        param_index: index,
    })
}

/// Handle a PARAM_SET echo (the fake FC accepts and echoes the value).
fn fc_echo(param_id: &[u8; 16], value: f32, count: u16, index: u16) -> MavMessage {
    MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
        param_id: (*param_id).into(),
        param_value: value,
        param_type: MavParamType::MAV_PARAM_TYPE_INT32,
        param_count: count,
        param_index: index,
    })
}

#[tokio::test]
async fn lossy_dup_reordered_pull_converges_and_set_echoes() {
    const TOTAL: u16 = 50;
    let port = free_udp_port();
    let config = config_for_port(port);
    let (handle, mut rx, first_result) = spawn_connection(config.clone()).await.expect("spawn");
    match timeout(Duration::from_secs(3), first_result).await {
        Ok(Ok(Ok(()))) => {}
        _ => panic!("listener never came up"),
    }
    let mut peer =
        mavlink::connect_async::<mavlink::common::MavMessage>(&format!("udpout:127.0.0.1:{port}"))
            .await
            .expect("peer");
    peer.set_protocol_version(mavlink::MavlinkVersion::V2);

    // Bootstrap: a udpin listener only learns the peer address from an
    // inbound frame — the fake FC speaks first. Wait until the worker has
    // actually processed that frame before pulling: a send before the peer
    // address is learned is silently dropped by the transport, and the test
    // must not race that (the service re-issues the list request in
    // production, but the test measures the request latency directly).
    peer.send(&FC_HEADER, &fc_param(0, TOTAL))
        .await
        .expect("bootstrap");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, rx.recv()).await {
            Ok(Ok(ConnectionEvent::Message(_))) => break,
            Ok(Ok(_)) => {}
            _ => panic!("bootstrap frame never reached the worker"),
        }
    }

    let (svc, mut evt) = ParamService::spawn(handle.clone(), 1, 1);

    // Pull: the FC answers PARAM_REQUEST_LIST with a lossy, duplicated and
    // reordered stream (T2: 丢包 10–30%、重复、乱序).
    svc.pull().await.expect("pull starts");
    // Wait for the pull request, skipping the GCS's own heartbeat
    // announcements and any other frames (connection bootstraps async).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, peer.recv()).await {
            Ok(Ok((_, req))) => {
                if matches!(req, MavMessage::PARAM_REQUEST_LIST(_)) {
                    break;
                }
            }
            Ok(Err(e)) => panic!("peer closed: {e}"),
            Err(_) => panic!("no PARAM_REQUEST_LIST within 3 s"),
        }
    }

    // Loss ~20%, duplicate ~10%, reverse-order pairs.
    let mut stream: Vec<MavMessage> = (0..TOTAL).map(|i| fc_param(i, TOTAL)).collect();
    let mut rng_state: u64 = 0x9E3779B97F4A7C15;
    let mut keep: Vec<MavMessage> = Vec::new();
    for (i, frame) in stream.drain(..).enumerate() {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let r = (rng_state >> 33) % 100;
        if r < 20 {
            continue; // dropped
        }
        keep.push(frame);
        if r >= 90 && i + 1 < TOTAL as usize {
            keep.push(fc_param((i + 1) as u16, TOTAL)); // duplicate a neighbour
        }
    }
    // Reorder: swap adjacent pairs.
    let mut i = 0;
    while i + 1 < keep.len() {
        keep.swap(i, i + 1);
        i += 2;
    }
    for frame in keep {
        peer.send(&FC_HEADER, &frame).await.expect("send param");
    }
    // Drop the link-events noise: drain a beat, then let the service fill.

    // The fill rounds re-request the missing indexes; the fake FC answers
    // every PARAM_REQUEST_READ with that exact parameter until the service
    // reports completion (or the runaway guard trips).
    let mut answered = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if tokio::time::Instant::now() > deadline {
            break; // the service must still declare Done with what it got
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Ok((_, msg))) = timeout(remaining, peer.recv()).await else {
            break;
        };
        if let MavMessage::PARAM_REQUEST_READ(read) = msg {
            peer.send(&FC_HEADER, &fc_param(read.param_index as u16, TOTAL))
                .await
                .expect("fill answer");
            answered += 1;
            if answered > 300 {
                break; // runaway guard
            }
        }
    }

    // The service must report completion with the full set.
    // Log the event stream while waiting.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut done = None;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, evt.recv()).await {
            Ok(Ok(ev)) => match &ev {
                ParamEvent::Progress { received, total } => println!("progress {received}/{total}"),
                ParamEvent::PullDone { .. } => {
                    done = Some(ev);
                    break;
                }
                ParamEvent::Batch(b) => println!("batch {}", b.len()),
                other => println!("event {other:?}"),
            },
            Ok(Err(e)) => {
                println!("recv err {e}");
                break;
            }
            Err(_) => {
                println!("deadline hit");
                break;
            }
        }
    }
    let Some(done) = done else {
        panic!("no PullDone");
    };
    match done {
        ParamEvent::PullDone { received, total } => {
            assert_eq!(total, TOTAL);
            assert_eq!(received, TOTAL, "lossy/dup/reordered pull converged");
        }
        _ => unreachable!(),
    }
    assert_eq!(svc.cache().len(), TOTAL as usize, "cache complete");

    // Drain any late fill requests still queued on the peer (the pull just
    // finished; the service may have had one more request in flight).
    while let Ok(Ok((_, m))) = timeout(Duration::from_millis(100), peer.recv()).await {
        if let MavMessage::PARAM_REQUEST_READ(read) = m {
            peer.send(&FC_HEADER, &fc_param(read.param_index as u16, TOTAL))
                .await
                .expect("late fill answer");
        }
    }

    // T3: set — the FC echoes the requested value. The echo driver runs
    // while the set future waits (they would otherwise deadlock).
    let svc_set = svc.clone();
    let set_task = tokio::spawn(async move {
        svc_set
            .set("P_0007", ParamValue::Int(7), ParamType::I32)
            .await
    });
    let (_, msg) = timeout(Duration::from_secs(3), peer.recv())
        .await
        .expect("no PARAM_SET received")
        .expect("peer closed");
    let MavMessage::PARAM_SET(set) = msg else {
        panic!("expected PARAM_SET")
    };
    peer.send(
        &FC_HEADER,
        &fc_echo(&set.param_id, set.param_value, TOTAL, 7),
    )
    .await
    .expect("set echo");
    let result = set_task.await.expect("set task").expect("set resolves");
    match result {
        SetResult::Confirmed(v) => assert_eq!(v, ParamValue::Int(7)),
        other => panic!("expected Confirmed, got {other:?}"),
    }

    // T3 (clamped) is covered by the unit tests in session.rs
    // (`alarm_holds_until…`-style echo-mismatch check); over the wire the
    // same code path runs — a second full echo round-trip adds no coverage
    // here.

    // Cancel is idempotent and never panics (also releases the session).
    svc.cancel().await;

    // Exclusivity: two concurrent sets — exactly one is rejected as Busy
    // (the other proceeds; with no echo driver it eventually times out,
    // which is also a valid terminal result).
    let (h1, h2) = (svc.clone(), svc.clone());
    let (r1, r2) = tokio::join!(
        h1.set("P_0001", ParamValue::Int(1), ParamType::I32),
        h2.set("P_0002", ParamValue::Int(2), ParamType::I32)
    );
    let busy_count = [(&r1), (&r2)]
        .iter()
        .filter(|r| ***r == Err(ParamError::Busy))
        .count();
    assert_eq!(
        busy_count, 1,
        "exactly one concurrent set must be rejected as Busy, got {r1:?} / {r2:?}"
    );

    handle.shutdown().await;
}
