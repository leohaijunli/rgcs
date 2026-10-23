//! Motor-test service integration over a real (loopback UDP) link
//! (motor-test integration): a fake FC streams disarmed heartbeats, the service
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

fn fc_header(system_id: u8) -> MavHeader {
    MavHeader {
        system_id,
        component_id: 1,
        sequence: 0,
    }
}

fn config_for_port(port: u16, target_system_id: u8) -> ConnectionConfig {
    ConnectionConfig {
        endpoint: Endpoint::UdpListener {
            addr: format!("127.0.0.1:{port}").parse().unwrap(),
        },
        system_id: 250,
        component_id: 250,
        target_system_id,
        target_component_id: 1,
        auto_discover_target: false,
        heartbeat_timeout: Duration::from_secs(30),
        reconnect_delay: Duration::from_millis(50),
        ..Default::default()
    }
}

fn limits(target_system_id: u8) -> SafetyLimits {
    SafetyLimits {
        max_value: 1.0,
        max_slew_per_s: 10.0,
        heartbeat_timeout: Duration::from_millis(500),
        target_sys: target_system_id,
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
    harness_for_system_id(1).await
}

async fn harness_for_system_id(target_system_id: u8) -> Harness {
    loop {
        let port = free_udp_port();
        let config = config_for_port(port, target_system_id);
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
        peer.send(&fc_header(target_system_id), &heartbeat(false))
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

        let (svc, evt, _output) = MotorTestService::spawn(handle.clone(), limits(target_system_id));
        // An active test requires nonzero demand. Zero sliders now correctly
        // leave motors released instead of continuously commanding PWM_MIN.
        svc.send(MotorTestCommand::SetPwmModels {
            models: vec![
                maggcs_core::motor_test::pwm::PwmModel::new(1100, 1900, false, 0.0)
                    .expect("fake FC model");
                4
            ],
        })
        .await
        .expect("FC models");
        svc.send(MotorTestCommand::SetValues(vec![0.4; 4]))
            .await
            .expect("test demand");
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
        // Do not starve the heartbeat sender while waiting for a frame.
        match timeout(remaining.min(Duration::from_millis(50)), p.recv()).await {
            Ok(Ok((_, MavMessage::COMMAND_LONG(c))))
                if c.command == ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST =>
            {
                return Some(c)
            }
            Ok(Ok(_)) => continue,
            Err(_) => {
                drop(p);
                tokio::task::yield_now().await;
            }
            Ok(Err(_)) => return None,
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
async fn stop_and_wait_confirms_release_frames_before_returning() {
    let Harness {
        handle,
        svc,
        mut evt,
        peer,
    } = harness().await;
    let peer = std::sync::Arc::new(tokio::sync::Mutex::new(peer));
    let heartbeat_task = {
        let peer = peer.clone();
        tokio::spawn(async move {
            loop {
                let guard = peer.lock().await;
                let _ = guard.send(&fc_header(1), &heartbeat(false)).await;
                drop(guard);
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    svc.send(MotorTestCommand::StartManual)
        .await
        .expect("start");
    wait_event(&mut evt, Duration::from_secs(2), |event| {
        *event == MotorTestEvent::StateChanged(SessionState::ManualRunning)
    })
    .await;
    let _ = next_actuator_frame(&peer, Duration::from_secs(2))
        .await
        .expect("test frame was sent");

    let sent_at = tokio::time::Instant::now();
    let receipt = timeout(Duration::from_secs(3), svc.stop_and_wait())
        .await
        .expect("stop confirmation is bounded")
        .expect("release pass sent");
    assert_eq!(receipt.released_motors, 4);
    assert_eq!(receipt.state, SessionState::Idle);

    let mut releases = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while releases < 4 && tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = {
            let guard = peer.lock().await;
            recv_frame(&guard, remaining).await
        };
        let Some((_, MavMessage::COMMAND_LONG(command))) = frame else {
            continue;
        };
        if command.command == ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST
            && command.param2 <= 0.0
        {
            releases += 1;
        }
    }
    assert_eq!(releases, 4, "one release frame for each motor");
    assert!(sent_at.elapsed() < Duration::from_secs(2));

    heartbeat_task.abort();
    handle.shutdown().await;
}

#[tokio::test]
async fn stop_and_wait_does_not_confirm_after_transport_shutdown() {
    let Harness {
        handle,
        svc,
        mut evt,
        peer,
    } = harness().await;
    let peer = std::sync::Arc::new(tokio::sync::Mutex::new(peer));
    let heartbeat_task = {
        let peer = peer.clone();
        tokio::spawn(async move {
            loop {
                let guard = peer.lock().await;
                let _ = guard.send(&fc_header(1), &heartbeat(false)).await;
                drop(guard);
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    svc.send(MotorTestCommand::StartManual)
        .await
        .expect("start");
    wait_event(&mut evt, Duration::from_secs(2), |event| {
        *event == MotorTestEvent::StateChanged(SessionState::ManualRunning)
    })
    .await;
    let _ = next_actuator_frame(&peer, Duration::from_secs(2))
        .await
        .expect("test frame was sent");

    // Close the connection worker while the session is actively sending.
    // Probe until the outbound receiver has exited, then make sure the worker
    // cannot mistake a failed release send for a successful stop.
    handle.shutdown().await;
    let probe_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut transport_closed = false;
    while tokio::time::Instant::now() < probe_deadline {
        match timeout(
            Duration::from_millis(500),
            handle.send_confirmed(heartbeat(false)),
        )
        .await
        {
            Ok(Err(_)) => {
                transport_closed = true;
                break;
            }
            Ok(Ok(())) => tokio::task::yield_now().await,
            Err(_) => panic!("transport write confirmation hung after shutdown"),
        }
    }
    assert!(transport_closed, "connection worker did not stop in time");

    let stop_result = timeout(Duration::from_secs(2), svc.stop_and_wait())
        .await
        .expect("stop failure report is bounded");
    assert!(
        stop_result.is_err(),
        "failed release transport must never be reported as a successful stop"
    );

    heartbeat_task.abort();
}

#[tokio::test]
async fn configured_non_default_system_id_routes_commands_and_ignores_other_armed_vehicle() {
    const TARGET_SYSTEM_ID: u8 = 42;
    let Harness {
        handle,
        svc,
        mut evt,
        peer,
    } = harness_for_system_id(TARGET_SYSTEM_ID).await;
    let peer = std::sync::Arc::new(tokio::sync::Mutex::new(peer));
    let target_armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let heartbeat_task = {
        let peer = peer.clone();
        let target_armed = target_armed.clone();
        tokio::spawn(async move {
            loop {
                let guard = peer.lock().await;
                let _ = guard
                    .send(
                        &fc_header(TARGET_SYSTEM_ID),
                        &heartbeat(target_armed.load(std::sync::atomic::Ordering::Relaxed)),
                    )
                    .await;
                // Another armed vehicle on a shared MAVLink bus must not
                // affect this session's target-system interlock.
                let _ = guard.send(&fc_header(1), &heartbeat(true)).await;
                drop(guard);
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(400)).await;

    svc.send(MotorTestCommand::StartManual)
        .await
        .expect("start");
    wait_event(&mut evt, Duration::from_secs(2), |event| {
        *event == MotorTestEvent::StateChanged(SessionState::ManualRunning)
    })
    .await;
    let frame = next_actuator_frame(&peer, Duration::from_secs(2))
        .await
        .expect("test frame was not sent to the configured target");
    assert_eq!(frame.target_system, TARGET_SYSTEM_ID);

    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(
        !matches!(evt.try_recv(), Ok(MotorTestEvent::Stopped(_))),
        "armed heartbeat from another system must not stop this target's session"
    );

    target_armed.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        wait_event(&mut evt, Duration::from_secs(2), |event| {
            matches!(event, MotorTestEvent::Stopped(_))
        })
        .await,
        MotorTestEvent::Stopped(StopReason::Armed)
    );

    heartbeat_task.abort();
    handle.shutdown().await;
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
                let _ = p.send(&fc_header(1), &heartbeat(is_armed)).await;
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
