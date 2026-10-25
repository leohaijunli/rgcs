//! RTK injection service integration (comprehensive plan R2/R3): a
//! `FakeSource` streams RTCM frames; the service reassembles, fragments them
//! into `GPS_RTCM_DATA` and the UDP peer reassembles the byte stream; the
//! stats events carry B/s, per-type counts and the stall alarm.

use std::time::Duration;

use maggcs_core::mavlink::connection::{spawn_connection, ConnectionConfig, ConnectionEvent};
use maggcs_core::mavlink::endpoint::Endpoint;
use maggcs_core::mavlink::MavMessage;
use maggcs_core::rtk::forward::FRAGMENT_LEN;
use maggcs_core::rtk::rtcm::encode_frame;
use maggcs_core::rtk::service::STALL_ALARM_S;
use maggcs_core::rtk::source::FakeSource;
use tokio::time::timeout;

const LINK_TIMEOUT: Duration = Duration::from_secs(3);
const SOURCE_TIMEOUT: Duration = Duration::from_secs(1);
const SOURCE_CHUNK_BYTES: usize = 127;

type Peer = Box<dyn mavlink::AsyncMavConnection<MavMessage> + Send + Sync>;

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

struct SteeredSource(std::sync::mpsc::Receiver<Vec<u8>>);

impl maggcs_core::rtk::source::RtcmSource for SteeredSource {
    fn read(&mut self) -> Result<Option<Vec<u8>>, maggcs_core::rtk::source::SourceError> {
        Ok(self.0.try_recv().ok())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnect_keeps_base_and_forwards_only_fresh_generation_data() {
    use maggcs_core::mavlink::connection::LinkState;
    use maggcs_core::rtk::base::{BasePhase, RtkBaseServiceHandle};
    let (handle, peer) = connected_peer().await;
    let mut events = handle.subscribe();
    let base = RtkBaseServiceHandle::default();
    let (service, mut stats, task) =
        maggcs_core::rtk::service::spawn_for_base(base.clone(), handle.clone());
    let (input, source) = std::sync::mpsc::channel();
    service
        .start(Box::new(SteeredSource(source)), "persistent-base".into())
        .await
        .expect("base starts");
    let first = typed_frame(1005, 20);
    input.send(first.clone()).expect("first frame");
    assert_eq!(
        packet_bytes(&receive_packets(&peer, first.len()).await),
        first
    );
    let before = completed_stats(&mut stats, "persistent-base", 1).await;
    let generation = handle.generation();
    handle.reconnect().await.expect("reconnect");
    timeout(LINK_TIMEOUT, async {
        while handle.state().await == LinkState::Connected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("old transport retired");
    input
        .send(typed_frame(1006, 24))
        .expect("old correction while offline");
    tokio::time::sleep(Duration::from_millis(15)).await;
    timeout(LINK_TIMEOUT, async {
        loop {
            if let Ok(ConnectionEvent::Connected {
                generation: current,
                ..
            }) = events.recv().await
            {
                if current > generation {
                    break;
                }
            }
        }
    })
    .await
    .expect("new transport");
    // The reopened UDP listener needs to learn the peer again.
    peer.send(&FC_HEADER, &heartbeat())
        .await
        .expect("bootstrap reopened listener");
    timeout(LINK_TIMEOUT, async {
        loop {
            if matches!(events.recv().await, Ok(ConnectionEvent::Message(_))) {
                break;
            }
        }
    })
    .await
    .expect("peer learned");
    let fresh = typed_frame(1074, 181);
    input.send(fresh.clone()).expect("fresh correction");
    assert_eq!(
        packet_bytes(&receive_packets(&peer, fresh.len()).await),
        fresh,
        "offline data must never replay on the new transport"
    );
    let after = completed_stats(&mut stats, "persistent-base", 1).await;
    assert!(after.connection_generation > before.connection_generation);
    assert_eq!(after.base_session_id, before.base_session_id);
    service.shutdown().await.expect("detach vehicle");
    task.await.expect("forwarder joined");
    assert_eq!(base.snapshot().phase, BasePhase::Ready);
    base.stop(base.snapshot().session_id.expect("base session"))
        .await
        .expect("stop base");
    handle.shutdown().await;
}

/// Known 1005 and 1074 message numbers, independent of the parser under test.
fn sample_frames() -> Vec<Vec<u8>> {
    let mut base_position = vec![0; 100];
    base_position[..2].copy_from_slice(&[0x3e, 0xd0]);
    let mut msm4 = vec![0; FRAGMENT_LEN + 40];
    msm4[..2].copy_from_slice(&[0x43, 0x20]);
    let short = encode_frame(&base_position).expect("encode");
    let long = encode_frame(&msm4).expect("encode");
    vec![short, long]
}

async fn connected_peer() -> (maggcs_core::mavlink::ConnectionHandle, Peer) {
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
    (handle, peer)
}

#[tokio::test(flavor = "multi_thread")]
async fn injection_forwards_and_reports_stats() {
    let (handle, peer) = connected_peer().await;

    let (svc, mut stats, service_task) = maggcs_core::rtk::service::spawn_with_task(handle.clone());

    let frames = sample_frames();
    let mut source_chunks = frames.clone();
    // Rejected bytes must not inflate the *forwarded* B/s value.
    source_chunks.push(vec![0xF3; 8192]);
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

    // Exact RTCM IDs and counts, with a 20% scheduling tolerance for the
    // first nominal one-second rate window. Pure tests pin exact arithmetic.
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
                    assert_eq!(
                        s.frames_per_type,
                        std::collections::HashMap::from([(1005, 1), (1074, 1)])
                    );
                    let expected_rate = expected.len() as f64;
                    assert!(
                        (s.bytes_per_second - expected_rate).abs() < expected_rate * 0.2,
                        "expected approximately {expected_rate} valid bytes/s, got {}",
                        s.bytes_per_second
                    );
                    assert!(!s.stalled, "data just flowed");
                }
            }
            _ => break,
        }
    }
    assert!(saw_frames, "stats never reflected the forwarded frames");
    assert!(!saw_final_stats, "unexpected early stop stats");

    svc.stop().await.expect("stop first source and join reader");

    // Stall alarm: with the source stopped, stats must flag stalled after
    // STALL_ALARM_S — wait one more tick past the threshold.
    // (Re-start with an exhausted source to observe the alarm.)
    let source = FakeSource::new(vec![], Duration::from_millis(10));
    svc.start(Box::new(source), "fake-rtcm-stalled".into())
        .await
        .expect("restart");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(STALL_ALARM_S + 4);
    let mut saw_waiting = false;
    let mut stalled_seen = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, stats.recv()).await {
            Ok(Some(s)) => {
                if s.source.as_deref() != Some("fake-rtcm-stalled") {
                    continue;
                }
                if !s.stalled && !saw_waiting {
                    // The 5 s alarm must not fire on the first tick just
                    // because no chunk has arrived yet.
                    assert_eq!(
                        s.seconds_since_data, None,
                        "silent source has never received data"
                    );
                    saw_waiting = true;
                }
                if s.stalled {
                    assert_eq!(s.seconds_since_data, None);
                    stalled_seen = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        saw_waiting,
        "source never reported a pre-stall waiting state"
    );
    assert!(stalled_seen, "stall alarm never fired");

    svc.stop()
        .await
        .expect("stop second source and join reader");
    svc.shutdown().await.expect("shutdown RTK service");
    tokio::time::timeout(Duration::from_secs(1), service_task)
        .await
        .expect("RTK service task stops within budget")
        .expect("RTK service task joins");
    handle.shutdown().await;
}

fn typed_frame(message_type: u16, total_len: usize) -> Vec<u8> {
    use maggcs_core::rtk::rtcm::{CRC_LEN, HEADER_LEN};
    let mut payload = vec![0x55; total_len - HEADER_LEN - CRC_LEN];
    payload[0] = (message_type >> 4) as u8;
    payload[1] = ((message_type & 0x0f) << 4) as u8;
    encode_frame(&payload).expect("valid fixture frame")
}

async fn receive_packets(
    peer: &Peer,
    expected_bytes: usize,
) -> Vec<mavlink::common::GPS_RTCM_DATA_DATA> {
    let deadline = tokio::time::Instant::now() + LINK_TIMEOUT;
    let mut packets = Vec::new();
    let mut received = 0;
    while received < expected_bytes && tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Ok((_, MavMessage::GPS_RTCM_DATA(packet)))) =
            timeout(remaining, peer.recv()).await
        {
            assert!(usize::from(packet.len) <= FRAGMENT_LEN);
            received += usize::from(packet.len);
            packets.push(packet);
        }
    }
    assert_eq!(
        received, expected_bytes,
        "all RTCM bytes must reach the peer"
    );
    packets
}

fn packet_bytes(packets: &[mavlink::common::GPS_RTCM_DATA_DATA]) -> Vec<u8> {
    packets
        .iter()
        .flat_map(|packet| packet.data[..usize::from(packet.len)].iter().copied())
        .collect()
}

async fn completed_stats(
    stats: &mut tokio::sync::mpsc::Receiver<maggcs_core::rtk::service::RtkStats>,
    source: &str,
    total: u64,
) -> maggcs_core::rtk::service::RtkStats {
    timeout(LINK_TIMEOUT, async {
        loop {
            let snapshot = stats
                .recv()
                .await
                .expect("injection service remains available");
            if snapshot.source.as_deref() == Some(source) && snapshot.total_frames == total {
                return snapshot;
            }
        }
    })
    .await
    .expect("complete stats within budget")
}

#[tokio::test(flavor = "multi_thread")]
async fn maximum_rtcm_frame_preserves_bytes_counts_sequence_and_restart() {
    use maggcs_core::rtk::rtcm::{parse_frame, CRC_LEN, HEADER_LEN, MAX_PAYLOAD_LEN};

    let (handle, peer) = connected_peer().await;
    let (svc, mut stats, task) = maggcs_core::rtk::service::spawn_with_task(handle.clone());
    // Advance to wire sequence 31 before the maximum-sized RTCM frame, then
    // follow it with both an unfragmented and a fragmented ordinary frame.
    let prefix = typed_frame(1005, 12);
    let mut frames = vec![prefix.clone(); 31];
    let maximum = typed_frame(1074, HEADER_LEN + MAX_PAYLOAD_LEN + CRC_LEN);
    frames.extend([
        maximum.clone(),
        typed_frame(1084, 18),
        typed_frame(1124, 181),
    ]);
    let expected = frames.concat();
    let mut source_bytes = expected.clone();
    let mut corrupt = maximum;
    corrupt[HEADER_LEN + 2] ^= 1; // Keep the old CRC: this frame must be rejected.
    source_bytes.splice(prefix.len() * 31..prefix.len() * 31, corrupt);
    let chunks = source_bytes
        .chunks(SOURCE_CHUNK_BYTES)
        .map(<[u8]>::to_vec)
        .collect();
    timeout(
        SOURCE_TIMEOUT,
        svc.start(
            Box::new(FakeSource::new(chunks, Duration::from_millis(1))),
            "large-rtcm".into(),
        ),
    )
    .await
    .expect("start bounded")
    .expect("start source");

    let packets = receive_packets(&peer, expected.len()).await;
    assert_eq!(
        packet_bytes(&packets),
        expected,
        "ordered byte stream without loss, padding or interleaving"
    );
    // 31 short frames, six large-frame packets, one short frame, two packets.
    assert_eq!(packets.len(), 40);
    assert_eq!(
        packets[31..37]
            .iter()
            .map(|packet| packet.len)
            .collect::<Vec<_>>(),
        vec![180, 180, 180, 180, 180, 129]
    );
    assert_eq!(
        packets[31..37]
            .iter()
            .map(|packet| packet.flags)
            .collect::<Vec<_>>(),
        vec![249, 251, 253, 255, 1, 3]
    );
    assert_eq!(
        packets[38..]
            .iter()
            .map(|packet| packet.flags)
            .collect::<Vec<_>>(),
        vec![17, 19],
        "next fragmented frame uses sequence 2, not the tail group's sequence"
    );
    for frame in &frames {
        assert_eq!(parse_frame(frame).expect("intact RTCM CRC").1, frame.len());
    }
    let snapshot = completed_stats(&mut stats, "large-rtcm", 34).await;
    assert_eq!(
        snapshot.frames_per_type,
        std::collections::HashMap::from([(1005, 31), (1074, 1), (1084, 1), (1124, 1)])
    );
    assert_eq!(snapshot.dropped_chunks, 0);
    assert!(!snapshot.stalled);
    assert!(
        (snapshot.bytes_per_second - expected.len() as f64).abs() < expected.len() as f64 * 0.2,
        "count valid RTCM bytes once, excluding the corrupt frame"
    );
    timeout(SOURCE_TIMEOUT, svc.stop())
        .await
        .expect("stop remains bounded")
        .expect("stop after large frame");

    let restarted = [typed_frame(1074, 721), typed_frame(1006, 24)];
    timeout(
        SOURCE_TIMEOUT,
        svc.start(
            Box::new(FakeSource::new(
                restarted.to_vec(),
                Duration::from_millis(1),
            )),
            "restarted-rtcm".into(),
        ),
    )
    .await
    .expect("restart bounded")
    .expect("restart the same service");
    let expected = restarted.concat();
    let packets = receive_packets(&peer, expected.len()).await;
    assert_eq!(packet_bytes(&packets), expected);
    assert_eq!(
        packets.iter().map(|packet| packet.len).collect::<Vec<_>>(),
        vec![180, 180, 180, 180, 1, 24]
    );
    assert_eq!(
        packets
            .iter()
            .map(|packet| packet.flags)
            .collect::<Vec<_>>(),
        vec![1, 3, 5, 7, 0, 0]
    );
    let snapshot = completed_stats(&mut stats, "restarted-rtcm", 2).await;
    assert_eq!(
        snapshot.frames_per_type,
        std::collections::HashMap::from([(1074, 1), (1006, 1)])
    );
    timeout(SOURCE_TIMEOUT, svc.stop())
        .await
        .expect("stop bounded")
        .expect("stop restarted source");
    timeout(SOURCE_TIMEOUT, svc.shutdown())
        .await
        .expect("shutdown bounded")
        .expect("shutdown");
    timeout(SOURCE_TIMEOUT, task)
        .await
        .expect("join bounded")
        .expect("join service");
    handle.shutdown().await;
}

#[tokio::test]
async fn full_stats_channel_does_not_block_stop_or_shutdown() {
    let (handle, _, first) = spawn_connection(config_for_port(free_udp_port()))
        .await
        .expect("spawn");
    first.await.expect("open result").expect("open");
    let (svc, stats, task) = maggcs_core::rtk::service::spawn_with_task(handle.clone());
    // Keep the receiver alive but never consume it. More lifecycle snapshots
    // than its capacity must not trap the source reader or command replies.
    for _ in 0..16 {
        timeout(
            Duration::from_secs(1),
            svc.start(
                Box::new(FakeSource::new(vec![], Duration::from_millis(1))),
                "silent".into(),
            ),
        )
        .await
        .expect("start bounded")
        .expect("start");
        tokio::task::yield_now().await;
        timeout(Duration::from_secs(1), svc.stop())
            .await
            .expect("stop bounded")
            .expect("stop");
    }
    assert!(!stats.is_empty(), "exercise a retained stats backlog");
    timeout(Duration::from_secs(1), svc.shutdown())
        .await
        .expect("shutdown bounded")
        .expect("shutdown");
    timeout(Duration::from_secs(1), task)
        .await
        .expect("join bounded")
        .expect("join");
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
