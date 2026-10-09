//! Throwaway repro: arm + takeoff against a live SITL and watch the heartbeat.
//! Usage: cargo run --example sitl_arm -- [udpin:0.0.0.0:14550]

use std::time::Duration;

use mavlink::common::{MavCmd, MavMessage};
use mavlink::MavHeader;

fn gcs_hb() -> MavMessage {
    MavMessage::HEARTBEAT(mavlink::common::HEARTBEAT_DATA {
        custom_mode: 0,
        mavtype: mavlink::common::MavType::MAV_TYPE_GCS,
        autopilot: mavlink::common::MavAutopilot::MAV_AUTOPILOT_GENERIC,
        base_mode: mavlink::common::MavModeFlag::empty(),
        system_status: mavlink::common::MavState::MAV_STATE_ACTIVE,
        mavlink_version: 3,
    })
}

fn cmd(command: MavCmd, params: [f32; 7], confirmation: u8) -> MavMessage {
    MavMessage::COMMAND_LONG(mavlink::common::COMMAND_LONG_DATA {
        param1: params[0],
        param2: params[1],
        param3: params[2],
        param4: params[3],
        param5: params[4],
        param6: params[5],
        param7: params[6],
        command,
        target_system: 1,
        target_component: 1,
        confirmation,
    })
}

fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "udpin:0.0.0.0:14550".to_string());
    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async move {
        let conn = mavlink::connect_async::<MavMessage>(&addr).await.expect("connect");
        let mut conn = conn;
        conn.set_protocol_version(mavlink::MavlinkVersion::V2);

        let header = MavHeader { system_id: 250, component_id: 250, sequence: 0 };
        let start = std::time::Instant::now();
        let mut sent_arm = false;
        let mut sent_takeoff = false;

        loop {
            let t = start.elapsed();
            if !sent_arm && t >= Duration::from_secs(4) {
                sent_arm = true;
                let _ = conn
                    .send(&header, &cmd(MavCmd::MAV_CMD_COMPONENT_ARM_DISARM, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 0))
                    .await;
            }
            if !sent_takeoff && t >= Duration::from_secs(7) {
                sent_takeoff = true;
                let _ = conn
                    .send(&header, &cmd(MavCmd::MAV_CMD_NAV_TAKEOFF, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 10.0], 0))
                    .await;
            }
            let _ = conn.send(&header, &gcs_hb()).await;

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                res = conn.recv() => match res {
                    Ok((header, msg)) => match &msg {
                        MavMessage::HEARTBEAT(hb) => {
                            println!(
                                "HB sys={} comp={} base={:#04x} armed={} state={:?}",
                                header.system_id,
                                header.component_id,
                                hb.base_mode.bits(),
                                hb.base_mode.contains(mavlink::common::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED),
                                hb.system_status
                            );
                        }
                        MavMessage::ATTITUDE(a) => {
                            println!("ATT r={:.2} p={:.2}", a.roll.to_degrees(), a.pitch.to_degrees());
                        }
                        MavMessage::GLOBAL_POSITION_INT(g) => {
                            println!("GPI alt_mm={} rel_alt_mm={}", g.alt, g.relative_alt);
                        }
                        MavMessage::COMMAND_ACK(a) => {
                            println!("ACK cmd={:?} result={:?}", a.command, a.result);
                        }
                        _ => {}
                    },
                    Err(e) => { eprintln!("recv err {e}"); break; }
                },
            }
        }
    });
}
