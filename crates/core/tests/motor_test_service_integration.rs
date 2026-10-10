//! Motor-test service integration over a real (loopback UDP) link
//! (motor-test-plan T1): a fake FC streams disarmed heartbeats, the service
//! starts sending `ACTUATOR_TEST` at 10 Hz, and the moment the FC arms the
//! service must emit release frames and `Stopped(Armed)` — within one tick.

use std::time::Duration;

use ::mavlink::common::MavMessage;
use maggcs_core::mavlink::connection::{spawn_connection, ConnectionConfig, ConnectionEvent};
use maggcs_core::mavlink::endpoint::Endpoint;
use maggcs_core::mavlink::message::MavHeader;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::motor_test::service::{
    MotorTestCommand, MotorTestEvent, MotorTestService, StopReason,
};
use maggcs_core::motor_test::{SafetyLimits, SessionState};
use tokio::time::timeout;

/// Find a free UDP port by binding to :0 and releasing.
fn free_udp_port() -> u16 {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind ephemeral");
    sock.local_addr().unwrap().port()
}

fn heartbeat(armed: bool) -> MavMessage {
    let mut base_mode = ::mavlink::common::MavModeFlag::empty();
    if armed {
        base_mode |= ::mavlink::common::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED;
    }
    MavMessage::HEARTBEAT(::mavlink::common::HEARTBEAT_DATA {
        custom_mode: 0,
        mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
        autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
        base_mode,
        system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
        mavlink_version: 3,
    })
}

const FC_HEADER: MavHeader = MavHeader {
    system_id: 1,
    component_id: 1,
    sequence: 0,
};

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

fn limits() -> SafetyLimits {
    SafetyLimits {
        max_value: 1.0,
        max_slew_per_s: 10.0,
        heartbeat_timeout: Duration::from_millis(500),
        target_sys: 1,
        target_comp: 1,
    }
}

type Peer = Box<dyn mavlink::AsyncMavConnection<MavMessage> + Sync + Send>;

struct Harness {
    handle: ConnectionHandle,
    svc: MotorTestService,
    evt: tokio::sync::mpsc::Receiver<MotorTestEvent>,
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
        peer.send(&FC_HEADER, &heartbeat(false))
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

        let (svc, evt) = MotorTestService::spawn(handle.clone(), limits());
        return Harness {
            handle,
            svc,
            evt,
            peer,
        };
    }
}

/// Next frame the fake FC receives, decoded; None on timeout.
async fn recv_frame(peer: &Peer, within: Duration) -> Option<(MavHeader, MavMessage)> {
    match timeout(within, peer.recv()).await {
        Ok(Ok(frame)) => Some(frame),
        _ => None,
    }
}

type SharedPeer = std::sync::Arc<tokio::sync::Mutex<Peer>>;

/// The next ACTUATOR_TEST COMMAND_LONG the fake FC receives, skipping the
/// GCS's own heartbeat announcements.
async fn next_actuator_frame(
    peer: &SharedPeer,
    within: Duration,
) -> Option<::mavlink::common::COMMAND_LONG_DATA> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let p = peer.lock().await;
        match timeout(remaining, p.recv()).await {
            Ok(Ok((_, MavMessage::COMMAND_LONG(c))))
                if c.command == ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST =>
            {
                return Some(c)
            }
            Ok(Ok(_)) => continue,
            _ => return None,
        }
    }
}

async fn wait_event(
    rx: &mut tokio::sync::mpsc::Receiver<MotorTestEvent>,
    within: Duration,
    pred: impl Fn(&MotorTestEvent) -> bool,
) -> MotorTestEvent {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let ev = timeout(remaining, rx.recv())
            .await
            .expect("timed out waiting for motor-test event")
            .expect("event channel closed");
        if pred(&ev) {
            return ev;
        }
    }
}

#[tokio::test]
async fn arming_mid_test_stops_within_a_tick_and_releases() {
    let Harness {
        handle,
        svc,
        mut evt,
        peer,
    } = harness().await;
    // Shared fake FC: a heartbeat task keeps it alive (a live FC streams
    // heartbeats; the session refuses to send without a fresh one), while
    // the test body receives what the GCS puts on the wire.
    let peer = std::sync::Arc::new(tokio::sync::Mutex::new(peer));
    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hb = {
        let peer = peer.clone();
        let armed = armed.clone();
        tokio::spawn(async move {
            loop {
                let is_armed = armed.load(std::sync::atomic::Ordering::Relaxed);
                let p = peer.lock().await;
                let _ = p.send(&FC_HEADER, &heartbeat(is_armed)).await;
                drop(p);
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        })
    };

    // Let at least one heartbeat land before starting: the session refuses
    // to send without a fresh one (by design — never test blind), and the
    // ticker's first tick fires immediately after Start.
    tokio::time::sleep(Duration::from_millis(400)).await;

    svc.send(MotorTestCommand::StartManual)
        .await
        .expect("start");
    wait_event(&mut evt, Duration::from_secs(2), |e| {
        *e == MotorTestEvent::StateChanged(SessionState::ManualRunning)
    })
    .await;

    // Test frames flow at ~10 Hz; each is an ACTUATOR_TEST COMMAND_LONG.
    // (The GCS also announces its own heartbeat automatically — skip those.)
    let first = next_actuator_frame(&peer, Duration::from_secs(2))
        .await
        .expect("no test frame arrived");
    assert_eq!(
        first.command,
        ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST
    );
    assert_eq!(first.target_system, 1);
    assert!(
        first.param2 > 0.0 && first.param2 <= 3.0,
        "FC-side timeout in (0, 3]"
    );

    // The vehicle arms: stop + release within one tick (100 ms).
    armed.store(true, std::sync::atomic::Ordering::Relaxed);
    let ev = wait_event(&mut evt, Duration::from_secs(2), |e| {
        matches!(e, MotorTestEvent::Stopped(_))
    })
    .await;
    assert_eq!(ev, MotorTestEvent::Stopped(StopReason::Armed));

    // A release frame (ACTUATOR_TEST with timeout <= 0) follows within the
    // drain window — buffered test frames and GCS heartbeats may come first.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut released = false;
    while !released && tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = {
            let p = peer.lock().await;
            recv_frame(&p, remaining).await
        };
        let Some((_, frame)) = frame else { break };
        if let MavMessage::COMMAND_LONG(c) = frame {
            if c.command == ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST && c.param2 <= 0.0 {
                released = true;
            }
        }
    }
    assert!(released, "no release frame after the stop");

    // … and no further ACTUATOR_TEST frames (the GCS's own heartbeat
    // announcements keep flowing and are fine).
    let quiet_deadline = tokio::time::Instant::now() + Duration::from_millis(800);
    while tokio::time::Instant::now() < quiet_deadline {
        let remaining = quiet_deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = {
            let p = peer.lock().await;
            recv_frame(&p, remaining).await
        };
        let Some((_, frame)) = frame else { break };
        if let MavMessage::COMMAND_LONG(c) = frame {
            // Release frames (param2 <= 0) may still be draining from the
            // stop pass — one per motor, sent together. Only a live test
            // frame means sending continued.
            if c.command == ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST && c.param2 > 0.0 {
                panic!("sending continued after the stop");
            }
        }
    }

    hb.abort();
    handle.shutdown().await;
}
