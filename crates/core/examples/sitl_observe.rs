//! Throwaway observer: print every inbound frame from a live SITL link.
//! Usage: cargo run --example sitl_observe -- [udpin:0.0.0.0:14550]

use std::time::Duration;

fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "udpin:0.0.0.0:14550".to_string());

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async move {
        let conn = mavlink::connect_async::<mavlink::common::MavMessage>(&addr)
            .await
            .expect("connect");
        let mut conn = conn;
        conn.set_protocol_version(mavlink::MavlinkVersion::V2);
        println!("listening on {addr}");

        loop {
            match tokio::time::timeout(Duration::from_secs(10), conn.recv()).await {
                Ok(Ok((header, msg))) => {
                    match &msg {
                        mavlink::common::MavMessage::HEARTBEAT(hb) => {
                            println!(
                                "HEARTBEAT sys={} comp={} base_mode={:#04x} custom={:#010x} armed={} state={:?}",
                                header.system_id,
                                header.component_id,
                                hb.base_mode.bits(),
                                hb.custom_mode,
                                hb.base_mode.contains(
                                    mavlink::common::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED
                                ),
                                hb.system_status
                            );
                        }
                        mavlink::common::MavMessage::ATTITUDE(a) => {
                            println!(
                                "ATTITUDE sys={} comp={} r={:.2} p={:.2} y={:.2}",
                                header.system_id,
                                header.component_id,
                                a.roll.to_degrees(),
                                a.pitch.to_degrees(),
                                a.yaw.to_degrees()
                            );
                        }
                        mavlink::common::MavMessage::GLOBAL_POSITION_INT(g) => {
                            println!(
                                "GPI sys={} comp={} lat={} lon={} alt_mm={} rel_alt_mm={}",
                                header.system_id,
                                header.component_id,
                                g.lat,
                                g.lon,
                                g.alt,
                                g.relative_alt
                            );
                        }
                        _ => {}
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("recv error: {e}");
                    break;
                }
                Err(_) => {
                    println!("--- 10s idle ---");
                }
            }
        }
    });
}
