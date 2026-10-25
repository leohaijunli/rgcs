//! Opt-in firmware E2E: catalog -> app rate owner -> MAVLink -> real PX4 SITL
//! -> compiled decoder -> requested/observed/stale snapshots. No motor writes.
use super::*;
use crate::motor_test_state::{AuxiliaryLinkRuntime, MotorTestTarget, TargetLinkRuntime};
use maggcs_core::mavlink::stream_control::StreamReceiveState;
use maggcs_core::mavlink::streams::{
    resolve_standard_stream, resolve_uorb_stream, StreamLookupError,
};
use maggcs_core::mavlink::{ConnectionConfig, Endpoint};

const SETTLE: Duration = Duration::from_secs(3);
const RATE_TOLERANCE: f64 = 0.2;
const REQUIRED: [(u32, &str, f64, f64); 3] = [
    (147, "BATTERY_STATUS", 2.0, 4.0),
    (26, "SCALED_IMU", 10.0, 20.0),
    (105, "HIGHRES_IMU", 20.0, 40.0),
];

fn status(state: &AppState, id: u32) -> StreamStatus {
    snapshot(state)
        .statuses
        .into_iter()
        .find(|status| status.message_id == id)
        .expect("requested stream")
}

async fn request(state: &AppState, target: MotorTestTargetRef, id: u32, rate: StreamRate) {
    apply(
        state,
        target,
        StreamRequest {
            message_id: id,
            rate,
            allow_unknown_support: false,
        },
    )
    .await
    .expect("production rate request accepted by transport");
}

fn within_rate(status: &StreamStatus, hz: f64) -> bool {
    status.receive_state == StreamReceiveState::Receiving
        && status
            .observed_hz
            .is_some_and(|observed| (observed - hz).abs() <= hz * RATE_TOLERANCE)
}

async fn wait_ready(state: &AppState, minimum_generation: u64) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let current = snapshot(state);
            if current.ready && current.transport_generation >= minimum_generation {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("SITL target traffic reaches the production stream manager");
}

fn record(report: &mut serde_json::Value, path: &Path, phase: &str, state: &AppState) {
    report["observations"]
        .as_array_mut()
        .expect("report observations")
        .push(serde_json::json!({
            "phase": phase, "snapshot": snapshot(state),
        }));
    std::fs::write(
        path,
        serde_json::to_string_pretty(report).expect("report JSON"),
    )
    .expect("write evidence");
    println!(
        "MR-20 {phase}: {}",
        serde_json::to_string(&snapshot(state).statuses).expect("statuses JSON")
    );
}

async fn firmware_version(handle: &ConnectionHandle, system_id: u8) -> serde_json::Value {
    let mut events =
        handle.subscribe_route(MessageRoute::messages(&[148]).from(system_id, TARGET_COMPONENT_ID));
    handle
        .send_in_generation(
            MavMessage::COMMAND_LONG(mavlink::common::COMMAND_LONG_DATA {
                command: mavlink::common::MavCmd::MAV_CMD_REQUEST_MESSAGE,
                param1: 148.0,
                target_system: system_id,
                target_component: TARGET_COMPONENT_ID,
                ..Default::default()
            }),
            handle.generation(),
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await
        .expect("request firmware version");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(ConnectionEvent::Message(envelope)) = events.recv().await {
                if let MavMessage::AUTOPILOT_VERSION(version) = envelope.message {
                    return serde_json::json!({
                        "flight_sw_version": version.flight_sw_version,
                        "version": format!("{}.{}.{}", version.flight_sw_version >> 24, (version.flight_sw_version >> 16) & 255, (version.flight_sw_version >> 8) & 255),
                        "flight_custom_version": version.flight_custom_version,
                        "board_version": version.board_version,
                    });
                }
            }
        }
    }).await.expect("PX4 AUTOPILOT_VERSION reply")
}

fn stop_sitl_stream(name: &str) {
    let binary = std::env::var("MAGGCS_SITL_MAVLINK_BIN").expect("isolated PX4 CLI path");
    let instance = std::env::var("MAGGCS_SITL_INSTANCE").expect("isolated instance");
    let port = std::env::var("MAGGCS_SITL_FC_PORT").expect("isolated FC port");
    let result = std::process::Command::new(binary)
        .args([
            "--instance",
            &instance,
            "stream",
            "-u",
            &port,
            "-s",
            name,
            "-r",
            "0",
        ])
        .output()
        .expect("SITL-only stream stop");
    assert!(
        result.status.success(),
        "SITL stream CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires an isolated PX4 SITL; run scripts/sitl/check-standard-streams.py"]
async fn standard_streams_sitl_end_to_end() {
    assert_eq!(std::env::var("MAGGCS_SITL_STREAM_TEST").as_deref(), Ok("1"));
    let endpoint = Endpoint::try_from(
        std::env::var("MAGGCS_SITL_ENDPOINT")
            .expect("isolated endpoint")
            .as_str(),
    )
    .expect("endpoint");
    assert!(
        matches!(endpoint, Endpoint::UdpListener { addr } if addr.ip().is_loopback()),
        "firmware E2E only accepts an isolated loopback UDP listener"
    );
    let system_id = std::env::var("MAGGCS_SITL_SYSTEM_ID")
        .expect("SITL system id")
        .parse::<u8>()
        .expect("valid system id");
    let report_path = PathBuf::from(std::env::var("MAGGCS_STREAM_REPORT").expect("evidence path"));
    let profile_path = report_path.with_file_name("stream-preferences.json");
    let (handle, _, first) = maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
        endpoint,
        system_id: 250,
        component_id: 190,
        target_system_id: system_id,
        target_component_id: TARGET_COMPONENT_ID,
        auto_discover_target: false,
        reconnect_delay: Duration::from_millis(50),
        ..Default::default()
    })
    .await
    .expect("GCS connection");
    first.await.expect("bind result").expect("bound");
    let state = AppState::default();
    let (params, _, parameter_task) = maggcs_core::params::ParamService::spawn_with_task(
        handle.clone(),
        system_id,
        TARGET_COMPONENT_ID,
    );
    *state.motor_test.target.lock() = Some(TargetLinkRuntime {
        target: MotorTestTarget {
            handle: handle.clone(),
            endpoint: handle.config().endpoint.to_address_string(),
            target_system_id: system_id,
            params,
            restart_pending: Default::default(),
            config: Default::default(),
            config_stale: Default::default(),
            param_names: Default::default(),
        },
        runtime: AuxiliaryLinkRuntime::new(handle.clone(), []),
    });
    let target = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .expect("target")
        .target
        .target_ref();
    state
        .motor_test
        .stream_profiles
        .lock()
        .open(profile_path.clone());
    spawn_for_target(&state, handle.clone(), system_id);
    wait_ready(&state, 1).await;
    let version = firmware_version(&handle, system_id).await;
    let mut report = serde_json::json!({ "schema_version": 1, "passed": false,
        "firmware": version, "rate_tolerance_fraction": RATE_TOLERANCE,
        "evidence_scope": "production catalog/app stream service/MAVLink/PX4/decoder/stats; UI verified separately by browser IPC regression",
        "observations": [],
    });

    // The actual catalog command and exact resolution drive the same IDs that
    // the app service writes; no arbitrary uORB transport exists.
    for (id, name, hz, _) in REQUIRED {
        for query in [name.to_string(), id.to_string(), name.to_ascii_lowercase()] {
            assert_eq!(actuator_stream_catalog(query.clone())[0].id, id);
            assert_eq!(
                resolve_standard_stream(&query)
                    .expect("exact catalog selection")
                    .id,
                id
            );
        }
        request(&state, target, id, StreamRate::Hertz(hz)).await;
    }
    for (id, hz) in [(24, 2.0), (36, 10.0), (291, 5.0)] {
        request(&state, target, id, StreamRate::Hertz(hz)).await;
    }
    tokio::time::sleep(SETTLE).await;
    record(&mut report, &report_path, "add_and_receive", &state);
    for (id, _, hz, _) in REQUIRED {
        assert!(
            within_rate(&status(&state, id), hz),
            "message {id} did not arrive at {hz} Hz"
        );
    }

    let changed_at = Instant::now();
    for (id, _, _, hz) in REQUIRED {
        request(&state, target, id, StreamRate::Hertz(hz)).await;
    }
    tokio::time::timeout(SETTLE, async {
        while !REQUIRED
            .iter()
            .all(|(id, _, _, hz)| within_rate(&status(&state, *id), *hz))
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("requested rate changes observed within three seconds");
    report["rate_change_observed_ms"] = serde_json::json!(changed_at.elapsed().as_millis());
    tokio::time::sleep(Duration::from_secs(1)).await;
    record(&mut report, &report_path, "rate_change", &state);

    for (id, _, _, _) in REQUIRED {
        request(&state, target, id, StreamRate::Disabled).await;
    }
    tokio::time::sleep(SETTLE).await;
    record(&mut report, &report_path, "disable", &state);
    for (id, _, _, _) in REQUIRED {
        let row = status(&state, id);
        assert_eq!(row.requested_rate, StreamRate::Disabled);
        assert_eq!(
            row.receive_state,
            StreamReceiveState::Stale,
            "disabled {id} still arriving"
        );
        assert_eq!(row.observed_hz, None);
    }

    for (id, _, _, hz) in REQUIRED {
        request(&state, target, id, StreamRate::Hertz(hz)).await;
    }
    request(&state, target, 105, StreamRate::Hertz(1000.0)).await;
    tokio::time::sleep(SETTLE).await;
    record(&mut report, &report_path, "high_rate_probe", &state);
    assert_eq!(
        status(&state, 105).receive_state,
        StreamReceiveState::Receiving
    );
    request(&state, target, 105, StreamRate::Hertz(40.0)).await;
    tokio::time::sleep(SETTLE).await;
    assert!(within_rate(&status(&state, 105), 40.0));

    // Stop firmware output independently of the GCS desired rate.
    stop_sitl_stream("HIGHRES_IMU");
    tokio::time::sleep(SETTLE).await;
    record(&mut report, &report_path, "firmware_stopped_stale", &state);
    let stale = status(&state, 105);
    assert_eq!(stale.requested_rate, StreamRate::Hertz(40.0));
    assert_eq!(stale.receive_state, StreamReceiveState::Stale);
    assert_eq!(stale.observed_hz, None);
    request(&state, target, 105, StreamRate::Hertz(40.0)).await;
    request(&state, target, 24, StreamRate::Disabled).await;

    let previous_generation = handle.generation();
    handle.reconnect().await.expect("reconnect");
    wait_ready(&state, previous_generation + 1).await;
    tokio::time::sleep(SETTLE).await;
    record(&mut report, &report_path, "reconnect_saved_rates", &state);
    for (id, _, _, hz) in REQUIRED {
        assert!(within_rate(&status(&state, id), hz));
    }
    assert_eq!(status(&state, 24).requested_rate, StreamRate::Disabled);
    assert_eq!(status(&state, 24).receive_state, StreamReceiveState::Stale);
    let profiles: Profiles =
        serde_json::from_str(&std::fs::read_to_string(&profile_path).expect("persisted profile"))
            .expect("profile JSON");
    assert_eq!(profiles.profiles.len(), 1);
    assert_eq!(profiles.profiles[0].requests.len(), 6);

    let before = state.motor_test.stream_profiles.lock().data.profiles[0]
        .requests
        .clone();
    assert!(actuator_stream_catalog("mag_sensor_custom_topic".into()).is_empty());
    assert!(matches!(
        resolve_uorb_stream("mag_sensor_custom_topic"),
        Err(StreamLookupError::NoStandardMapping { .. })
    ));
    assert!(matches!(
        resolve_uorb_stream("vehicle_imu"),
        Err(StreamLookupError::AmbiguousSelection { .. })
    ));
    assert_eq!(
        state.motor_test.stream_profiles.lock().data.profiles[0].requests,
        before
    );
    report["unmapped_and_ambiguous_queries_refused"] = serde_json::json!(true);
    report["passed"] = serde_json::json!(true);
    record(&mut report, &report_path, "complete", &state);
    stop(&state);
    parameter_task.abort();
    let target = state.motor_test.target.lock().take();
    if let Some(target) = target {
        target.stop().await;
    }
}
