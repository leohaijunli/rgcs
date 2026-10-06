//! Phase 1: mission protocol verification against a real PX4 SITL (task 8).
//!
//! Connects to the SITL ground-control UDP endpoint (default `udpin:0.0.0.0:14550`)
//! and drives the mission protocol using [`maggcs_core::mission::protocol`]:
//!
//! 1. Download the FC mission (MISSION_REQUEST_LIST → MISSION_COUNT → items).
//! 2. Upload a 3-waypoint mission (MISSION_COUNT → MISSION_REQUEST_INT per item
//!    → MISSION_ITEM_INT → MISSION_ACK).
//! 3. Download again and verify the uploaded items round-trip.
//!
//! Exits `0` on success, nonzero on any mismatch. Print the message sequence so it
//! can be diffed against the QGC/SITL wire trace.
//!
//! Usage:
//!   cargo run --release --example mission_drive -- [address] [target_sys_id]
//!
//! Defaults: `udpin:0.0.0.0:14550` 1

#![allow(deprecated)]

use std::time::{Duration, Instant};

use maggcs_core::mavlink::{
    connection::spawn_connection, ConnectionConfig, ConnectionEvent, Endpoint, MavHeader,
    MavMessage,
};
use maggcs_core::mission::protocol::{MissionEvent, MissionOperation, MissionProtocol};
use maggcs_core::mission::types::{MissionFrame, MissionItem};

fn arg(args: &[String], i: usize) -> Option<&str> {
    args.get(i).map(|s| s.as_str())
}

fn wp(seq: u16, lat: f64, lon: f64, alt_m: f32) -> MissionItem {
    let mut item = MissionItem::waypoint(lat, lon, alt_m, MissionFrame::GlobalRelativeAlt);
    item.seq = seq;
    item
}

/// Feed one incoming frame to the protocol, send what it wants out, collect events.
async fn pump(
    p: &mut MissionProtocol,
    handle: &maggcs_core::mavlink::connection::ConnectionHandle,
    hdr: &MavHeader,
    msg: &MavMessage,
    events: &mut Vec<MissionEvent>,
) -> Result<(), String> {
    let (ev, frames) = p.handle(hdr, msg);
    for e in &ev {
        println!("[event] {e:?}");
    }
    events.extend(ev);
    for f in frames {
        println!("[send]   {f:?}");
        handle.send(f).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let address = arg(&args, 1).unwrap_or("udpin:0.0.0.0:14550");
    let target_sys: u8 = arg(&args, 2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let target_comp: u8 = 1;

    let config = ConnectionConfig {
        endpoint: Endpoint::try_from(address).map_err(|e| e.to_string())?,
        system_id: 250,
        component_id: 250,
        target_system_id: target_sys,
        target_component_id: target_comp,
        heartbeat_timeout: Duration::from_secs(5),
        reconnect_delay: Duration::from_secs(1),
        ..Default::default()
    };
    const SELF_SYS: u8 = 250;
    const SELF_COMP: u8 = 250;
    let (handle, mut rx, _first) = spawn_connection(config).await?;

    let mut proto = MissionProtocol::new(SELF_SYS, SELF_COMP, target_sys, target_comp);
    let mut events: Vec<MissionEvent> = Vec::new();

    // ---- 1. Download (expect empty or existing mission) ----
    println!("== download ==");
    let frames = proto.begin_download();
    for f in frames {
        println!("[send]   {f:?}");
        handle.send(f).await?;
    }
    let mut download_done = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !download_done && Instant::now() < deadline {
        let ev = match tokio::time::timeout(deadline - Instant::now(), rx.recv()).await {
            Ok(Ok(ev)) => ev,
            Ok(Err(_)) => return Err("event channel closed".into()),
            Err(_) => break,
        };
        if let ConnectionEvent::Message(m) = ev {
            match &m.message {
                MavMessage::MISSION_COUNT(c) => {
                    println!("[recv]   MISSION_COUNT count={}", c.count);
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                }
                MavMessage::MISSION_ITEM_INT(i) => {
                    println!(
                        "[recv]   MISSION_ITEM_INT seq={} cmd={} frame={:?} lat={} lon={} z={}",
                        i.seq, i.command as u16, i.frame, i.x, i.y, i.z
                    );
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                    if events
                        .iter()
                        .any(|e| matches!(e, MissionEvent::Completed(MissionOperation::Download)))
                    {
                        download_done = true;
                    }
                }
                _ => {
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                }
            }
        }
    }
    let n_downloaded = events
        .iter()
        .filter_map(|e| match e {
            MissionEvent::Progress {
                operation: MissionOperation::Download,
                sent,
                ..
            } => Some(*sent as usize),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    println!("downloaded items: {n_downloaded}");

    // ---- 2. Upload a 3-waypoint mission ----
    println!("== upload ==");
    let mission = vec![
        wp(0, 48.6489, -123.3989, 50.0),
        wp(1, 48.6510, -123.3950, 60.0),
        wp(2, 48.6540, -123.3920, 70.0),
    ];
    let frames = proto.begin_upload(mission.clone()).unwrap();
    for f in frames {
        println!("[send]   {f:?}");
        handle.send(f).await?;
    }
    let mut upload_done = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !upload_done && Instant::now() < deadline {
        let ev = match tokio::time::timeout(deadline - Instant::now(), rx.recv()).await {
            Ok(Ok(ev)) => ev,
            Ok(Err(_)) => return Err("event channel closed".into()),
            Err(_) => break,
        };
        if let ConnectionEvent::Message(m) = ev {
            match &m.message {
                MavMessage::MISSION_REQUEST_INT(r) => {
                    println!("[recv]   MISSION_REQUEST_INT seq={}", r.seq);
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                }
                MavMessage::MISSION_REQUEST(r) => {
                    println!("[recv]   MISSION_REQUEST seq={}", r.seq);
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                }
                MavMessage::MISSION_ACK(a) => {
                    println!("[recv]   MISSION_ACK result={:?}", a.mavtype);
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                    if events
                        .iter()
                        .any(|e| matches!(e, MissionEvent::Completed(MissionOperation::Upload)))
                    {
                        upload_done = true;
                    }
                }
                _ => {
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                }
            }
        }
    }

    // ---- 3. Download again and verify round-trip ----
    println!("== re-download ==");
    events.clear();
    let frames = proto.begin_download();
    for f in frames {
        println!("[send]   {f:?}");
        handle.send(f).await?;
    }
    let mut roundtrip: Vec<MissionItem> = Vec::new();
    let mut rd_done = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !rd_done && Instant::now() < deadline {
        let ev = match tokio::time::timeout(deadline - Instant::now(), rx.recv()).await {
            Ok(Ok(ev)) => ev,
            Ok(Err(_)) => return Err("event channel closed".into()),
            Err(_) => break,
        };
        if let ConnectionEvent::Message(m) = ev {
            match &m.message {
                MavMessage::MISSION_COUNT(c) => {
                    println!("[recv]   MISSION_COUNT count={}", c.count);
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                }
                MavMessage::MISSION_ITEM_INT(i) => {
                    println!(
                        "[recv]   MISSION_ITEM_INT seq={} x={} y={} z={}",
                        i.seq, i.x, i.y, i.z
                    );
                    pump(&mut proto, &handle, &m.header, &m.message, &mut events).await?;
                    roundtrip.push(maggcs_core::mission::protocol::mission_item_from_mav(i));
                    if events
                        .iter()
                        .any(|e| matches!(e, MissionEvent::Completed(MissionOperation::Download)))
                    {
                        rd_done = true;
                    }
                }
                _ => {}
            }
        }
    }

    // ---- Verdict ----
    println!("uploaded:   {mission:?}");
    println!("roundtrip:  {roundtrip:?}");
    let mut ok = true;
    if roundtrip.len() != mission.len() {
        eprintln!(
            "FAIL: expected {} items on re-download, got {}",
            mission.len(),
            roundtrip.len()
        );
        ok = false;
    }
    for (a, b) in mission.iter().zip(&roundtrip) {
        if a.x != b.x || a.y != b.y || (a.z - b.z).abs() > 0.01 || a.command != b.command {
            eprintln!("FAIL: item mismatch {a:?} vs {b:?}");
            ok = false;
        }
    }
    if !ok {
        std::process::exit(1);
    }
    println!("mission_drive: PASS");
    Ok(())
}
