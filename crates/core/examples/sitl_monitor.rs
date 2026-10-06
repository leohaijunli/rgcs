//! SITL heartbeat monitor (Phase 0, task 5).
//!
//! Connects to a MAVLink endpoint (PX4 SITL by default: `udpin:0.0.0.0:14550`),
//! counts frames and target-FC heartbeats, and reports any heartbeat loss.
//! Exits `0` when the run completed with zero heartbeat losses, `1` otherwise.
//!
//! Usage:
//!   cargo run --release --example sitl_monitor -- [address] [duration_s] [heartbeat_timeout_s] [target_sys_id]
//!
//! Defaults: `udpin:0.0.0.0:14550` 60 3 1

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use maggcs_core::mavlink::{
    connection::spawn_connection, message::MavMessage, ConnectionConfig, ConnectionEvent, Endpoint,
};

fn arg(args: &[String], i: usize) -> Option<&str> {
    args.get(i).map(|s| s.as_str())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let address = arg(&args, 1).unwrap_or("udpin:0.0.0.0:14550");
    let duration_s: u64 = arg(&args, 2).and_then(|s| s.parse().ok()).unwrap_or(60);
    let timeout_s: u64 = arg(&args, 3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let target_sys: u8 = arg(&args, 4).and_then(|s| s.parse().ok()).unwrap_or(1);

    let config = ConnectionConfig {
        endpoint: Endpoint::try_from(address).map_err(|e| e.to_string())?,
        system_id: 250,
        component_id: 250,
        target_system_id: target_sys,
        target_component_id: 1,
        heartbeat_timeout: Duration::from_secs(timeout_s),
        reconnect_delay: Duration::from_secs(1),
        ..Default::default()
    };

    let our_sys = config.system_id;
    let (handle, mut rx, _first) = spawn_connection(config).await?;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(duration_s);

    let mut frames: u64 = 0;
    let mut heartbeats: u64 = 0;
    let mut losses: u64 = 0;
    let mut first_frame_at: Option<Instant> = None;
    let printed_pos = AtomicBool::new(false);

    println!(
        "sitl_monitor: endpoint={address} duration={duration_s}s heartbeat_timeout={timeout_s}s target_sys={target_sys} system_id={our_sys}"
    );

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;

        let ev = match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ev)) => ev,
            Ok(Err(_)) => return Err("event channel closed".into()),
            Err(_) => break, // deadline reached
        };

        match ev {
            ConnectionEvent::Connected { .. } => {
                println!("[link] connected");
            }
            ConnectionEvent::Message(m) => {
                frames += 1;
                if first_frame_at.is_none() {
                    first_frame_at = Some(Instant::now());
                }
                if m.is_heartbeat_from(target_sys, 1) {
                    heartbeats += 1;
                }
                if let MavMessage::GLOBAL_POSITION_INT(p) = &m.message {
                    if printed_pos.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    println!(
                        "[pos] lat={:.6} lon={:.6} alt_msl_mm={} rel_alt_mm={}",
                        p.lat as f64 / 1e7,
                        p.lon as f64 / 1e7,
                        p.alt,
                        p.relative_alt
                    );
                }
            }
            ConnectionEvent::HeartbeatLost { last_seen_age } => {
                losses += 1;
                println!(
                    "[alarm] heartbeat lost (age={last_seen_age:?}) at t={:.1}s",
                    start.elapsed().as_secs_f64()
                );
            }
            ConnectionEvent::HeartbeatRestored => {
                println!("[alarm] heartbeat restored");
            }
            ConnectionEvent::LinkError(f) => {
                println!("[link] error: {:?}", f);
            }
            ConnectionEvent::Failed(f) => {
                return Err(format!("connection failed: {f:?}").into());
            }
        }
    }

    let _ = handle.reconnect().await;
    let latency_ms = first_frame_at.map(|t| t.elapsed().as_millis() as u64);
    println!("sitl_monitor: result frames={frames} heartbeats={heartbeats} losses={losses} first_frame_latency_ms={latency_ms:?}");

    if losses > 0 {
        eprintln!("sitl_monitor: FAIL: {losses} heartbeat loss(es) detected");
        std::process::exit(1);
    }
    if heartbeats == 0 {
        eprintln!("sitl_monitor: FAIL: no heartbeats received from target system {target_sys}");
        std::process::exit(2);
    }
    println!("sitl_monitor: PASS");
    Ok(())
}
