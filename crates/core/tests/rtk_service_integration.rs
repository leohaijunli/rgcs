//! RTK injection service integration (comprehensive plan R2/R3): a
//! `FakeSource` streams RTCM frames; the service reassembles, fragments them
//! into `GPS_RTCM_DATA` and the UDP peer reassembles the byte stream; the
//! stats events carry B/s, per-type counts and the stall alarm.

use std::time::Duration;

use maggcs_core::mavlink::connection::{spawn_connection, ConnectionConfig, ConnectionEvent};
use maggcs_core::mavlink::endpoint::Endpoint;
use maggcs_core::mavlink::MavMessage;
use maggcs_core::rtk::forward::FRAGMENT_LEN;
use maggcs_core::rtk::rtcm::{encode_frame, message_type};
use maggcs_core::rtk::service::STALL_ALARM_S;
use maggcs_core::rtk::source::FakeSource;
use tokio::time::timeout;

fn free_udp_port() -> u16 {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind ephemeral");
    sock.local_addr().expect("local addr").port()
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

const FC_HEADER: mavlink::MavHeader = mavlink::MavHeader {
    system_id: 1,
    component_id: 1,
    sequence: 0,
};

/// Two MSM4-style frames: one fits in a packet, one needs two fragments.
fn sample_frames() -> Vec<Vec<u8>> {
    let short = encode_frame(&[0x05_u8; 100]).expect("encode");
    let long = encode_frame(&[0x07_u8; FRAGMENT_LEN + 40]).expect("encode");
    vec![short, long]
}

#[tokio::test(flavor = "multi_thread")]
async fn injection_forwards_and_reports_stats() {
    let port = free_udp_port();
    let config = config_for_port(port);
    let (handle, mut rx, first_result) = spawn_connection(config.clone()).await.expect("spawn");
    timeout(Duration::from_secs(3), first_result)
        .await
        .expect("bind timeout")
        .expect("bind")
        .expect("bind ok");

    let mut peer = mavlink::connect_async::<MavMessage>(&format!("udpout:127.0.0.1:{port}"))
        .await
        .expect("peer");
    peer.set_protocol_version(mavlink::MavlinkVersion::V2);

    // Bootstrap: teach the udpin listener our address, and wait for the
    // worker to process it (udpin drops sends before learning the peer).
    peer.send(&FC_HEADER, &heartbeat())
        .await
        .expect("bootstrap");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, rx.recv()).await {
            Ok(Ok(ConnectionEvent::Message(_))) => break,
            Ok(Ok(_)) => {}
            _ => panic!("bootstrap never reached the worker"),
        }
    }

    let (svc, mut stats) = maggcs_core::rtk::service::spawn(handle.clone());

    let frames = sample_frames();
    let mut source_chunks = frames.clone();
    source_chunks.push(vec![0xF3]); // stray byte: parser resync territory
    let source = FakeSource::new(source_chunks, Duration::from_millis(10));
    svc.start(Box::new(source), "fake-rtcm".into())
        .await
        .expect("start");

    // Collect every GPS_RTCM_DATA the peer sees for ~2 s and reassemble the
    // byte stream in arrival order (R0: that is exactly what PX4 does).
    let mut stream: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline
        && stream.len() < frames.iter().map(|f| f.len()).sum::<usize>()
    {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Ok((_, MavMessage::GPS_RTCM_DATA(d)))) = timeout(remaining, peer.recv()).await {
            stream.extend_from_slice(&d.data[..d.len as usize]);
        }
    }

    let mut expected: Vec<u8> = Vec::new();
    for f in &frames {
        expected.extend_from_slice(f);
    }
    assert_eq!(stream, expected, "reassembled stream must match the frames");

    svc.stop().await;

    // Stats: bytes/s, per-type counts and total frames all reflect the two
    // frames (one 1005-ish type = whatever encode_frame derives).
    let types: Vec<u16> = frames.iter().map(|f| message_type(f)).collect();
    let mut saw_frames = false;
    let mut saw_final_stats = false;
    let stats_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while tokio::time::Instant::now() < stats_deadline && !saw_frames {
        let remaining = stats_deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, stats.recv()).await {
            Ok(Some(s)) => {
                if s.source.is_none() {
                    saw_final_stats = true; // stopped → empty stats
                    break;
                }
                assert_eq!(s.source.as_deref(), Some("fake-rtcm"));
                // 1 Hz stats race the stream; the event that has seen all
                // frames carries the full picture.
                if s.total_frames == frames.len() as u64 {
                    saw_frames = true;
                    for t in &types {
                        assert!(
                            s.frames_per_type.contains_key(t),
                            "type {t} missing from stats"
                        );
                    }
                    assert!(s.bytes_per_second > 0.0);
                    assert!(!s.stalled, "data just flowed");
                }
            }
            _ => break,
        }
    }
    assert!(saw_frames, "stats never reflected the forwarded frames");
    assert!(!saw_final_stats, "unexpected early stop stats");

    // Stall alarm: with the source stopped, stats must flag stalled after
    // STALL_ALARM_S — wait one more tick past the threshold.
    // (Re-start with an exhausted source to observe the alarm.)
    let source = FakeSource::new(vec![], Duration::from_millis(10));
    svc.start(Box::new(source), "fake-rtcm-stalled".into())
        .await
        .expect("restart");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(STALL_ALARM_S + 4);
    let mut stalled_seen = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, stats.recv()).await {
            Ok(Some(s)) => {
                if s.stalled {
                    assert!(
                        s.seconds_since_data.is_none()
                            || s.seconds_since_data.unwrap() > STALL_ALARM_S as f64
                    );
                    stalled_seen = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(stalled_seen, "stall alarm never fired");

    svc.stop().await;
    handle.shutdown().await;
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
