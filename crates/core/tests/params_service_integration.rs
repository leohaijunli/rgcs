//! Parameter service integration over a real (loopback UDP) link
//! (parameters plan T2/T3): a fake FC streams a parameter list with loss,
//! duplication and reordering; the service must converge to the full set via
//! its fill rounds, and a `PARAM_SET` echo resolves the set request.

use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering},
    Arc,
};
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

#[tokio::test]
async fn shutdown_joins_the_protocol_task_with_live_handles() {
    let port = free_udp_port();
    let (handle, _rx, first_result) = spawn_connection(config_for_port(port))
        .await
        .expect("spawn");
    match timeout(Duration::from_secs(3), first_result).await {
        Ok(Ok(Ok(()))) => {}
        _ => panic!("listener never came up"),
    }

    let (svc, _events, task) = ParamService::spawn_with_task(handle.clone(), 1, 1);
    let live_clone = svc.clone();
    svc.shutdown().await;

    timeout(Duration::from_secs(1), task)
        .await
        .expect("protocol task did not exit after shutdown")
        .expect("protocol task panicked");
    drop(live_clone);
    handle.shutdown().await;
}

/// On the saturated serial link the `AUTOPILOT_VERSION` reply lands long
/// after the request (bench finding 2026-10-23): `wait_fc_version` must wait
/// it out and converge, instead of the single-fire-and-forget request racing
/// the page paint and leaving the params window stuck on "No FC".
#[tokio::test]
async fn fc_version_wait_converges_on_a_saturated_link() {
    let port = free_udp_port();
    let (handle, mut rx, first_result) = spawn_connection(config_for_port(port))
        .await
        .expect("spawn");
    match timeout(Duration::from_secs(3), first_result).await {
        Ok(Ok(Ok(()))) => {}
        _ => panic!("listener never came up"),
    }
    let mut peer = ::mavlink::connect_async::<MavMessage>(&format!("udpout:127.0.0.1:{port}"))
        .await
        .expect("peer");
    peer.set_protocol_version(::mavlink::MavlinkVersion::V2);
    peer.send(&FC_HEADER, &fc_param(0, 1))
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

    // Delayed version responder: 600 ms after the request — later than the
    // poll paint, inside one settle window.
    tokio::spawn(async move {
        loop {
            match timeout(Duration::from_secs(5), peer.recv()).await {
                Ok(Ok((_, MavMessage::COMMAND_LONG(cmd)))) => {
                    // MAV_CMD_REQUEST_MESSAGE with param1 = 148.
                    if cmd.command == ::mavlink::common::MavCmd::MAV_CMD_REQUEST_MESSAGE
                        && cmd.param1 == 148.0
                    {
                        tokio::time::sleep(Duration::from_millis(600)).await;
                        peer.send(
                            &FC_HEADER,
                            &MavMessage::AUTOPILOT_VERSION(
                                ::mavlink::common::AUTOPILOT_VERSION_DATA {
                                    capabilities: ::mavlink::common::MavProtocolCapability::empty(),
                                    uid: 0,
                                    // 1.17.0
                                    flight_sw_version: (1 << 24) | (17 << 16),
                                    middleware_sw_version: 0,
                                    os_sw_version: 0,
                                    board_version: 0,
                                    vendor_id: 0,
                                    product_id: 0,
                                    flight_custom_version: [0; 8],
                                    middleware_custom_version: [0; 8],
                                    os_custom_version: [0; 8],
                                    uid2: [0; 18],
                                },
                            ),
                        )
                        .await
                        .expect("version reply");
                    }
                }
                Ok(Ok(_)) => {}
                _ => break,
            }
        }
    });

    let (svc, _evt) = ParamService::spawn(handle.clone(), 1, 1);
    let started = std::time::Instant::now();
    let version = svc.wait_fc_version(2, Duration::from_millis(1500)).await;
    let elapsed = started.elapsed();
    assert_eq!(
        version.as_deref(),
        Some("1.17.0"),
        "the delayed reply must land inside the accept window"
    );
    assert!(
        elapsed >= Duration::from_millis(550),
        "the wait must actually wait for the reply, took {elapsed:?}"
    );
    assert!(
        elapsed <= Duration::from_secs(2),
        "the wait must not retry past the first arrival, took {elapsed:?}"
    );

    // A settled slot answers instantly from now on.
    let started = std::time::Instant::now();
    let again = svc.wait_fc_version(2, Duration::from_millis(1500)).await;
    assert_eq!(again.as_deref(), Some("1.17.0"));
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "the known version must return instantly"
    );

    handle.shutdown().await;
}

/// Reuse one protocol task through a transport restart and a heartbeat loss.
/// Pending reads must fail promptly, old values/identity must be invalidated,
/// and subsequent requests must reach the recovered FC rather than stay Busy.
#[tokio::test]
async fn reads_and_firmware_recover_after_reconnect_and_heartbeat_loss() {
    let port = free_udp_port();
    let mut config = config_for_port(port);
    config.heartbeat_timeout = Duration::from_millis(600);
    let (handle, mut events, first) = spawn_connection(config).await.expect("spawn");
    timeout(Duration::from_secs(3), first)
        .await
        .expect("bind timeout")
        .expect("worker reply")
        .expect("bound");
    let mut peer = ::mavlink::connect_async::<MavMessage>(&format!("udpout:127.0.0.1:{port}"))
        .await
        .expect("peer");
    peer.set_protocol_version(::mavlink::MavlinkVersion::V2);

    let firmware = Arc::new(AtomicU32::new((1 << 24) | (17 << 16)));
    let value = Arc::new(AtomicI64::new(0));
    let heartbeats = Arc::new(AtomicBool::new(true));
    let (seen_tx, mut seen_rx) = tokio::sync::mpsc::channel(8);
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let responder_firmware = Arc::clone(&firmware);
    let responder_value = Arc::clone(&value);
    let responder_heartbeats = Arc::clone(&heartbeats);
    let responder = tokio::spawn(async move {
        let heartbeat = MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
            autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
            mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
            ..Default::default()
        });
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = tick.tick() => {
                    if responder_heartbeats.load(Ordering::SeqCst) {
                        peer.send(&FC_HEADER, &heartbeat).await.expect("heartbeat");
                    }
                }
                inbound = peer.recv() => {
                    let (_, message) = inbound.expect("GCS request");
                    match message {
                        MavMessage::COMMAND_LONG(command)
                            if command.command == ::mavlink::common::MavCmd::MAV_CMD_REQUEST_MESSAGE
                                && command.param1 == 148.0 => {
                            peer.send(&FC_HEADER, &MavMessage::AUTOPILOT_VERSION(
                                ::mavlink::common::AUTOPILOT_VERSION_DATA {
                                    flight_sw_version: responder_firmware.load(Ordering::SeqCst),
                                    ..Default::default()
                                }
                            )).await.expect("version");
                        }
                        MavMessage::PARAM_REQUEST_READ(read) => {
                            let name = maggcs_core::params::codec::decode_param_id(&read.param_id);
                            if name == "USB_MAV_MODE" {
                                let raw = maggcs_core::params::codec::encode(
                                    ParamValue::Int(responder_value.load(Ordering::SeqCst)),
                                    maggcs_core::params::codec::IntEncoding::ByteConcat,
                                );
                                peer.send(&FC_HEADER, &fc_echo(&read.param_id, raw, 2, 0))
                                    .await.expect("parameter reply");
                            } else if name == "PENDING_TEST" {
                                let _ = seen_tx.send(()).await;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                events.recv().await.expect("initial event"),
                ConnectionEvent::HeartbeatRestored
            ) {
                break;
            }
        }
    })
    .await
    .expect("FC bootstrap");
    let (service, _parameter_events, task) = ParamService::spawn_with_task(handle.clone(), 1, 1);
    assert_eq!(
        service
            .wait_fc_version(2, Duration::from_secs(1))
            .await
            .as_deref(),
        Some("1.17.0")
    );
    assert_eq!(
        service
            .read("USB_MAV_MODE")
            .await
            .expect("initial value")
            .value,
        ParamValue::Int(0)
    );

    let pending_service = service.clone();
    let pending = tokio::spawn(async move { pending_service.read("PENDING_TEST").await });
    timeout(Duration::from_secs(2), seen_rx.recv())
        .await
        .expect("read started")
        .expect("read request");
    firmware.store((1 << 24) | (17 << 16) | (1 << 8), Ordering::SeqCst);
    value.store(5, Ordering::SeqCst);
    let mut restarted = handle.subscribe();
    handle.reconnect().await.expect("restart transport");
    timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                restarted.recv().await.expect("restart event"),
                ConnectionEvent::Connected { .. }
            ) {
                break;
            }
        }
    })
    .await
    .expect("transport recovered");
    assert_eq!(
        timeout(Duration::from_secs(1), pending)
            .await
            .expect("pending read cancelled")
            .expect("read task"),
        Err(ParamError::NotConnected)
    );
    assert!(
        service.cache().is_empty(),
        "old values invalidated before a new request"
    );
    assert!(service.fc_version().is_none(), "old firmware invalidated");
    assert_eq!(
        service
            .wait_fc_version(3, Duration::from_secs(1))
            .await
            .as_deref(),
        Some("1.17.1")
    );
    assert_eq!(
        service
            .read("USB_MAV_MODE")
            .await
            .expect("read after restart")
            .value,
        ParamValue::Int(5)
    );

    let mut lost_events = handle.subscribe();
    heartbeats.store(false, Ordering::SeqCst);
    timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                lost_events.recv().await.expect("loss event"),
                ConnectionEvent::HeartbeatLost { .. }
            ) {
                break;
            }
        }
        while service.fc_version().is_some() || !service.cache().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("heartbeat loss clears observations");
    value.store(2, Ordering::SeqCst);
    heartbeats.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .wait_fc_version(3, Duration::from_secs(1))
            .await
            .as_deref(),
        Some("1.17.1")
    );
    assert_eq!(
        service
            .read("USB_MAV_MODE")
            .await
            .expect("read after heartbeat recovery")
            .value,
        ParamValue::Int(2)
    );

    service.shutdown().await;
    timeout(Duration::from_secs(1), task)
        .await
        .expect("protocol stopped")
        .expect("protocol task");
    let _ = stop_tx.send(());
    responder.await.expect("responder task");
    handle.shutdown().await;
}

/// Read-only hardware acceptance: reopen the same serial port twice while
/// retaining one parameter service. No parameter value or motor is commanded.
#[tokio::test]
#[ignore = "real FC: MAGGCS_BENCH=1 MAGGCS_BENCH_UART=/dev/ttyACM0 cargo test -p maggcs-core --test params_service_integration serial_parameter_reads_survive_reopening -- --ignored --nocapture"]
async fn serial_parameter_reads_survive_reopening() {
    assert_eq!(
        std::env::var("MAGGCS_BENCH").as_deref(),
        Ok("1"),
        "set MAGGCS_BENCH=1"
    );
    let port = std::path::PathBuf::from(
        std::env::var("MAGGCS_BENCH_UART").unwrap_or_else(|_| "/dev/ttyACM0".into()),
    );
    let config = ConnectionConfig {
        endpoint: Endpoint::Serial {
            port,
            baudrate: 115200,
        },
        system_id: 250,
        component_id: 250,
        target_system_id: 1,
        target_component_id: 1,
        ..Default::default()
    };
    let (handle, mut events, first) = spawn_connection(config).await.expect("serial connection");
    timeout(Duration::from_secs(10), first)
        .await
        .expect("serial open deadline")
        .expect("worker reply")
        .expect("serial opened");
    timeout(Duration::from_secs(10), async {
        loop {
            if matches!(
                events.recv().await.expect("serial events"),
                ConnectionEvent::HeartbeatRestored
            ) {
                break;
            }
        }
    })
    .await
    .expect("PX4 heartbeat");
    let (service, _events, task) = ParamService::spawn_with_task(handle.clone(), 1, 1);
    let version = service
        .wait_fc_version(3, Duration::from_secs(2))
        .await
        .expect("firmware version");
    let baseline = service
        .read("USB_MAV_MODE")
        .await
        .expect("initial USB_MAV_MODE");
    println!(
        "serial baseline: PX4 {version}, {}={:?} ({:?})",
        baseline.name, baseline.value, baseline.ptype
    );
    assert_eq!(baseline.ptype, ParamType::I32);

    for cycle in 1..=2 {
        let mut lifecycle = handle.subscribe();
        let started = std::time::Instant::now();
        handle.reconnect().await.expect("close and reopen serial");
        timeout(Duration::from_secs(10), async {
            loop {
                if matches!(
                    lifecycle.recv().await.expect("reconnect events"),
                    ConnectionEvent::HeartbeatRestored
                ) {
                    break;
                }
            }
        })
        .await
        .expect("heartbeat recovered after reopening");
        service
            .request_fc_version()
            .await
            .expect("request current firmware");
        let recovered_version = service
            .wait_fc_version(3, Duration::from_secs(2))
            .await
            .expect("recovered version");
        let entry = service
            .read("USB_MAV_MODE")
            .await
            .expect("read on reopened serial port");
        println!(
            "serial reopen {cycle}: PX4 {recovered_version}, {}={:?}, elapsed={:?}",
            entry.name,
            entry.value,
            started.elapsed()
        );
        assert_eq!(
            entry.value, baseline.value,
            "read-only test preserves the FC value"
        );
        assert_eq!(entry.ptype, baseline.ptype);
        assert_eq!(recovered_version, version);
    }

    service.shutdown().await;
    timeout(Duration::from_secs(2), task)
        .await
        .expect("protocol stopped")
        .expect("protocol task");
    handle.shutdown().await;
}
