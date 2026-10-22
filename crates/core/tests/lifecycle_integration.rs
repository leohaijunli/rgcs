//! Lifecycle supervisor over a real (loopback UDP) link (A1 acceptance).
//!
//! The safety property this pins: a teardown stops the motor test **while the
//! link is still up**, so the release frames reach the flight controller
//! before the connection closes — and the report says which stage did what.
//!
//! The fake FC streams disarmed heartbeats, the motor test sends
//! `MAV_CMD_ACTUATOR_TEST` (`COMMAND_LONG`) at 10 Hz, the supervisor stops it
//! (stage 1) and then closes the link (stage 7). Every `ACTUATOR_TEST` frame
//! the FC sees is recorded, so the test can assert both the order *and* that a
//! release frame arrived before the socket died.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ::mavlink::common::{MavCmd, MavMessage};
use maggcs_core::lifecycle::{
    Lifecycle, LifecyclePolicy, ManagedService, Stage, StopOutcome, StopReason, VehicleState,
};
use maggcs_core::mavlink::connection::{spawn_connection, ConnectionConfig, ConnectionEvent};
use maggcs_core::mavlink::endpoint::Endpoint;
use maggcs_core::mavlink::message::MavHeader;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::motor_test::service::{MotorTestCommand, MotorTestEvent, MotorTestService};
use maggcs_core::motor_test::{SafetyLimits, SessionState};
use tokio::time::timeout;

/// Find a free UDP port by binding to :0 and releasing.
fn free_udp_port() -> u16 {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind ephemeral");
    sock.local_addr().expect("local addr").port()
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
            addr: format!("127.0.0.1:{port}").parse().expect("addr"),
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

type Peer = Box<dyn ::mavlink::AsyncMavConnection<MavMessage> + Sync + Send>;

struct Harness {
    handle: ConnectionHandle,
    svc: MotorTestService,
    evt: tokio::sync::mpsc::Receiver<MotorTestEvent>,
    peer: Peer,
}

/// A live link + motor-test service, retried until the fake FC is bound.
async fn harness() -> Harness {
    loop {
        let port = free_udp_port();
        let (handle, mut rx, first_result) = match spawn_connection(config_for_port(port)).await {
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

        let mut peer =
            match ::mavlink::connect_async::<MavMessage>(&format!("udpout:127.0.0.1:{port}")).await
            {
                Ok(p) => p,
                Err(_) => {
                    handle.shutdown().await;
                    continue;
                }
            };
        peer.set_protocol_version(::mavlink::MavlinkVersion::V2);
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

        let (svc, evt, _output) = MotorTestService::spawn(handle.clone(), limits());
        return Harness {
            handle,
            svc,
            evt,
            peer,
        };
    }
}

/// One `ACTUATOR_TEST` frame the fake FC saw, with the value it carried.
#[derive(Debug, Clone, PartialEq)]
struct ActuatorFrame {
    value: f32,
    timeout_s: f32,
}

/// A stage that stops the motor-test session: send `Stop`, wait for the
/// session to leave the sending state (the release frames are written in the
/// same pass), then report. It never touches the link — that is the next
/// stage's job, which is exactly what the test proves.
struct MotorTestStage {
    svc: MotorTestService,
    states: Arc<Mutex<Vec<SessionState>>>,
    events: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<MotorTestEvent>>,
}

impl ManagedService for MotorTestStage {
    fn name(&self) -> &'static str {
        "motor_test"
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let mut events = self.events.lock().await;
            if timeout(deadline, self.svc.send(MotorTestCommand::Stop))
                .await
                .is_err()
            {
                return StopOutcome::TimedOut { elapsed_ms: 0 };
            }
            // The session emits `Stopping` (release frames already queued)
            // and then `Stopped`; either is proof the stop landed.
            let settled = timeout(deadline, async {
                loop {
                    match events.recv().await {
                        Some(MotorTestEvent::Stopped(_)) => return true,
                        Some(MotorTestEvent::ReleaseFailed(_)) => return false,
                        Some(MotorTestEvent::StateChanged(state)) => {
                            self.states.lock().expect("states").push(state);
                            if !state.is_sending() {
                                return true;
                            }
                        }
                        None => return true,
                    }
                }
            })
            .await;
            match settled {
                Ok(true) => StopOutcome::Stopped { elapsed_ms: 0 },
                Ok(false) => StopOutcome::Failed {
                    message: "session never settled".to_string(),
                },
                Err(_) => StopOutcome::TimedOut { elapsed_ms: 0 },
            }
        })
    }
}

/// A stage that closes the link and records that it ran.
struct LinkStage {
    handle: ConnectionHandle,
    closed: Arc<AtomicBool>,
}

impl ManagedService for LinkStage {
    fn name(&self) -> &'static str {
        "link"
    }

    fn stop<'a>(
        &'a self,
        _deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            self.handle.shutdown().await;
            self.closed.store(true, Ordering::SeqCst);
            StopOutcome::Stopped { elapsed_ms: 0 }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_motor_test_stops_before_the_link_closes() {
    let Harness {
        handle,
        svc,
        evt,
        peer,
    } = harness().await;

    // The session refuses to send without a fresh heartbeat, so keep the fake
    // FC alive while the test body watches what the GCS puts on the wire.
    let peer = Arc::new(tokio::sync::Mutex::new(peer));
    let hb = {
        let peer = peer.clone();
        tokio::spawn(async move {
            loop {
                let p = peer.lock().await;
                let _ = p.send(&FC_HEADER, &heartbeat(false)).await;
                drop(p);
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        })
    };

    let states = Arc::new(Mutex::new(Vec::new()));
    let frames = Arc::new(Mutex::new(Vec::<ActuatorFrame>::new()));

    // Record every ACTUATOR_TEST frame the FC sees until the link dies.
    let collector = {
        let peer = peer.clone();
        let frames = frames.clone();
        tokio::spawn(async move {
            loop {
                let frame = {
                    let p = peer.lock().await;
                    timeout(Duration::from_millis(200), p.recv()).await
                };
                match frame {
                    Ok(Ok((_, MavMessage::COMMAND_LONG(cmd))))
                        if cmd.command == MavCmd::MAV_CMD_ACTUATOR_TEST =>
                    {
                        frames.lock().expect("frames").push(ActuatorFrame {
                            value: cmd.param1,
                            timeout_s: cmd.param2,
                        });
                    }
                    Ok(Ok(_)) => {}
                    // The link closed or the peer went away: stop collecting.
                    Ok(Err(_)) => return,
                    Err(_) => {}
                }
            }
        })
    };

    // Let heartbeats settle, then run the motor test with non-zero values.
    tokio::time::sleep(Duration::from_millis(400)).await;
    svc.send(MotorTestCommand::StartManual)
        .await
        .expect("start manual");
    svc.send(MotorTestCommand::SetValues(vec![0.4; 4]))
        .await
        .expect("values");

    // The FC must actually see non-zero ACTUATOR_TEST frames.
    let saw_sending = timeout(Duration::from_secs(3), async {
        loop {
            if frames
                .lock()
                .expect("frames")
                .iter()
                .any(|f| f.value > 0.0 && f.timeout_s > 0.0)
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or(false);
    assert!(saw_sending, "the motor test was actually sending");

    // Now tear the app down: motor test first, link last.
    let closed = Arc::new(AtomicBool::new(false));
    let lifecycle = Lifecycle::new(LifecyclePolicy {
        stage_timeout: Duration::from_secs(2),
        total_timeout: Duration::from_secs(5),
        ..LifecyclePolicy::default()
    });
    lifecycle.register(
        Stage::MotorTest,
        Arc::new(MotorTestStage {
            svc,
            states: states.clone(),
            events: tokio::sync::Mutex::new(evt),
        }),
    );
    lifecycle.register(
        Stage::Link,
        Arc::new(LinkStage {
            handle: handle.clone(),
            closed: closed.clone(),
        }),
    );

    let report = lifecycle
        .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
        .await
        .expect("shutdown runs");

    // Let the OS deliver whatever is still in flight, then stop reading.
    tokio::time::sleep(Duration::from_millis(300)).await;
    hb.abort();
    let _ = timeout(Duration::from_secs(2), collector).await;

    // The link closed after the stop, not before.
    assert!(closed.load(Ordering::SeqCst), "the link stage ran");
    assert_eq!(
        report.outcome(Stage::MotorTest),
        Some(&StopOutcome::Stopped { elapsed_ms: 0 }),
        "the motor test stopped cleanly: {report:?}"
    );
    assert!(
        states
            .lock()
            .expect("states")
            .iter()
            .any(|s| !s.is_sending()),
        "the session left the sending state"
    );

    // The release frame (value 0, FC-side timeout 0) must have arrived, and
    // after the live test frames: PX4's own timeout is the backstop, not the
    // plan.
    let seen = frames.lock().expect("frames").clone();
    let sending_at = seen.iter().position(|f| f.value > 0.0 && f.timeout_s > 0.0);
    let release_at = seen
        .iter()
        .position(|f| f.value == 0.0 && f.timeout_s <= 0.0);
    assert!(
        sending_at.is_some(),
        "non-zero test frames arrived: {seen:?}"
    );
    assert!(
        release_at.is_some() && release_at > sending_at,
        "a release frame must follow the sending frames: {seen:?}"
    );
}

#[tokio::test]
async fn an_armed_vehicle_refuses_the_shutdown_before_any_stage_runs() {
    let ran = Arc::new(AtomicBool::new(false));
    let lifecycle = Lifecycle::default();
    // A stage that would be obvious if it ran: it flips `ran`.
    struct Probe(Arc<AtomicBool>);
    impl ManagedService for Probe {
        fn name(&self) -> &'static str {
            "probe"
        }
        fn stop<'a>(
            &'a self,
            _deadline: Duration,
        ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
            Box::pin(async move {
                self.0.store(true, Ordering::SeqCst);
                StopOutcome::Stopped { elapsed_ms: 1 }
            })
        }
    }
    lifecycle.register(Stage::Link, Arc::new(Probe(ran.clone())));

    let err = lifecycle
        .shutdown(
            StopReason::Disconnect,
            VehicleState {
                armed: Some(true),
                on_ground: Some(false),
            },
        )
        .await
        .expect_err("armed refuses");
    assert!(err.to_string().contains("armed"), "{err}");
    assert!(
        !ran.load(Ordering::SeqCst),
        "no stage ran while the vehicle was armed"
    );

    // An unknown vehicle state is allowed through (never refuse on blind data).
    assert_eq!(
        lifecycle
            .shutdown(StopReason::Disconnect, VehicleState::unknown())
            .await
            .expect("unknown state is allowed")
            .outcome(Stage::Link),
        Some(&StopOutcome::Stopped { elapsed_ms: 1 })
    );
    assert!(ran.load(Ordering::SeqCst), "the stage ran once allowed");
}
