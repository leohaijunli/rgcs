//! PX4 flight-mode table (ADR-017): custom-mode encode/decode plus the
//! switchable-mode table the Fly view consumes. The decode used to live in
//! `frontend/src/util/px4mode.ts`; this module is now the single
//! implementation (the frontend reads the pre-decoded `Heartbeat.mode_label`
//! and the generated bindings).
//!
//! Wire layout (`px4_custom_mode.h`, unchanged in v1.17): the u32
//! `custom_mode` packs `main_mode` in bits 16–23 and `sub_mode` in bits
//! 24–31. `MAV_CMD_DO_SET_MODE` carries `param1 = base mode with
//! CUSTOM_MODE_ENABLED (bit 0)`, `param2 = main mode`, `param3 = sub mode`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// PX4 main modes (`PX4_CUSTOM_MAIN_MODE`).
pub mod main_mode {
    pub const MANUAL: u8 = 1;
    pub const ALTCTL: u8 = 2;
    pub const POSCTL: u8 = 3;
    pub const AUTO: u8 = 4;
    pub const ACRO: u8 = 5;
    pub const OFFBOARD: u8 = 6;
    pub const STABILIZED: u8 = 7;
    pub const RATTITUDE: u8 = 8;
    pub const SIMPLE: u8 = 9;
    pub const TERMINATION: u8 = 10;
    pub const ALTITUDE_CRUISE: u8 = 11;
}

/// PX4 `AUTO` sub modes (`PX4_CUSTOM_SUB_MODE_AUTO`).
pub mod auto_sub {
    pub const READY: u8 = 1;
    pub const TAKEOFF: u8 = 2;
    pub const LOITER: u8 = 3;
    pub const MISSION: u8 = 4;
    pub const RTL: u8 = 5;
    pub const LAND: u8 = 6;
    pub const FOLLOW_TARGET: u8 = 8;
    pub const PRECLAND: u8 = 9;
    pub const VTOL_TAKEOFF: u8 = 10;
}

/// PX4 `POSCTL` sub modes (`PX4_CUSTOM_SUB_MODE_POSCTL`).
pub mod posctl_sub {
    pub const POSCTL: u8 = 0;
    pub const ORBIT: u8 = 1;
    pub const SLOW: u8 = 2;
}

/// Display names for the main modes, indexed by value (1..=11).
fn main_mode_label(main: u8) -> Option<&'static str> {
    Some(match main {
        main_mode::MANUAL => "Manual",
        main_mode::ALTCTL => "Altitude",
        main_mode::POSCTL => "Position",
        main_mode::AUTO => "Auto",
        main_mode::ACRO => "Acro",
        main_mode::OFFBOARD => "Offboard",
        main_mode::STABILIZED => "Stabilized",
        main_mode::RATTITUDE => "Rattitude",
        main_mode::SIMPLE => "Simple",
        main_mode::TERMINATION => "Termination",
        main_mode::ALTITUDE_CRUISE => "Altitude Cruise",
        _ => return None,
    })
}

/// Display names for the `AUTO` and `POSCTL` sub modes.
fn sub_mode_label(main: u8, sub: u8) -> Option<&'static str> {
    match main {
        main_mode::AUTO => Some(match sub {
            auto_sub::READY => "Ready",
            auto_sub::TAKEOFF => "Takeoff",
            auto_sub::LOITER => "Loiter",
            auto_sub::MISSION => "Mission",
            auto_sub::RTL => "RTL",
            auto_sub::LAND => "Land",
            auto_sub::FOLLOW_TARGET => "Follow Target",
            auto_sub::PRECLAND => "Precision Land",
            auto_sub::VTOL_TAKEOFF => "VTOL Takeoff",
            _ => return None,
        }),
        main_mode::POSCTL => Some(match sub {
            posctl_sub::ORBIT => "Orbit",
            posctl_sub::SLOW => "Slow",
            _ => return None,
        }),
        _ => None,
    }
}

/// Decode a PX4 `custom_mode` into a display string (e.g. `Auto·Loiter`).
/// Unknown values fall back to `Mode <main>`, mirroring the previous TS
/// implementation.
pub fn decode_custom_mode(custom_mode: u32) -> String {
    let main = ((custom_mode >> 16) & 0xff) as u8;
    let sub = ((custom_mode >> 24) & 0xff) as u8;
    let Some(main_name) = main_mode_label(main) else {
        return format!("Mode {main}");
    };
    match sub_mode_label(main, sub) {
        Some(sub_name) => format!("{main_name}·{sub_name}"),
        None => main_name.to_string(),
    }
}

/// Encode main/sub into the PX4 `custom_mode` u32 — the exact inverse of
/// the decode split above.
pub fn encode_custom_mode(main: u8, sub: u8) -> u32 {
    ((main as u32) << 16) | ((sub as u32) << 24)
}

/// One switchable mode entry: what `DO_SET_MODE` must send and how the UI
/// should behave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct ModeOption {
    /// Stable id for the UI (`hold`, `mission`, ...).
    pub id: String,
    /// Display name (English, operator convention).
    pub label: String,
    /// `DO_SET_MODE` param2 (PX4 main mode).
    pub main_mode: u8,
    /// `DO_SET_MODE` param3 (PX4 sub mode; 0 = default).
    pub sub_mode: u8,
    /// Requires the two-click confirm pattern.
    pub confirm: bool,
    /// Hidden unless the operator enabled "advanced modes" in Settings.
    pub advanced: bool,
}

/// Vehicle/link state the availability rules run against.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct ModeContext {
    /// The GCS link to the FC is up.
    pub link_online: bool,
    /// `HEARTBEAT.base_mode` reports armed.
    pub armed: bool,
    /// The FC holds a mission (`mission_count > 0`).
    pub mission_onboard: bool,
    /// The local plan matches the FC mission (the sync badge).
    pub mission_synced: bool,
    /// PX4 published a valid home position.
    pub home_valid: bool,
    /// An RC/ Manual-control input source is available (Position/Altitude
    /// need sticks).
    pub rc_available: bool,
    /// The motor-test window is driving the actuators.
    pub motor_test_running: bool,
}

/// A mode table entry resolved against a [`ModeContext`]: greyed-out entries
/// carry the operator-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct ModeAvailability {
    pub option: ModeOption,
    pub enabled: bool,
    pub reason: Option<String>,
}

/// The offered mode table (ADR-017): Hold / Mission / Return / Land /
/// Position / Altitude, with Manual / Acro / Offboard as advanced modes.
/// Arming is a separate command, not a mode.
pub fn mode_table() -> Vec<ModeOption> {
    vec![
        ModeOption {
            id: "hold".into(),
            label: "Hold".into(),
            main_mode: main_mode::AUTO,
            sub_mode: auto_sub::LOITER,
            confirm: false,
            advanced: false,
        },
        ModeOption {
            id: "mission".into(),
            label: "Mission".into(),
            main_mode: main_mode::AUTO,
            sub_mode: auto_sub::MISSION,
            confirm: false,
            advanced: false,
        },
        ModeOption {
            id: "return".into(),
            label: "Return".into(),
            main_mode: main_mode::AUTO,
            sub_mode: auto_sub::RTL,
            confirm: true,
            advanced: false,
        },
        ModeOption {
            id: "land".into(),
            label: "Land".into(),
            main_mode: main_mode::AUTO,
            sub_mode: auto_sub::LAND,
            confirm: true,
            advanced: false,
        },
        ModeOption {
            id: "position".into(),
            label: "Position".into(),
            main_mode: main_mode::POSCTL,
            sub_mode: posctl_sub::POSCTL,
            confirm: false,
            advanced: false,
        },
        ModeOption {
            id: "altitude".into(),
            label: "Altitude".into(),
            main_mode: main_mode::ALTCTL,
            sub_mode: 0,
            confirm: false,
            advanced: false,
        },
        ModeOption {
            id: "manual".into(),
            label: "Manual".into(),
            main_mode: main_mode::MANUAL,
            sub_mode: 0,
            confirm: false,
            advanced: true,
        },
        ModeOption {
            id: "acro".into(),
            label: "Acro".into(),
            main_mode: main_mode::ACRO,
            sub_mode: 0,
            confirm: false,
            advanced: true,
        },
        ModeOption {
            id: "offboard".into(),
            label: "Offboard".into(),
            main_mode: main_mode::OFFBOARD,
            sub_mode: 0,
            confirm: false,
            advanced: true,
        },
    ]
}

/// Resolve the mode table against the vehicle state. While the motor test
/// runs every entry is disabled; otherwise each entry's rule decides
/// (ADR-017 availability table).
pub fn available_modes(ctx: &ModeContext) -> Vec<ModeAvailability> {
    mode_table()
        .into_iter()
        .map(|option| {
            let (enabled, reason) = if ctx.motor_test_running {
                (false, "motor test running")
            } else {
                availability(&option, ctx)
            };
            ModeAvailability {
                option,
                enabled,
                reason: (!enabled).then(|| reason.to_string()),
            }
        })
        .collect()
}

/// The per-entry availability rule; `reason` explains a disabled entry.
fn availability(option: &ModeOption, ctx: &ModeContext) -> (bool, &'static str) {
    match option.id.as_str() {
        "hold" => (ctx.link_online, "link offline"),
        "mission" => (
            ctx.mission_onboard && ctx.mission_synced,
            if ctx.mission_onboard {
                "mission not synced"
            } else {
                "no mission on FC"
            },
        ),
        "return" => (ctx.armed && ctx.home_valid, {
            if ctx.armed {
                "no home position"
            } else {
                "not armed"
            }
        }),
        "land" => (ctx.armed, "not armed"),
        "position" | "altitude" => (ctx.rc_available, "RC required"),
        _ => (true, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode → decode must round-trip: every table entry's DO_SET_MODE
    /// params decode back to the canonical PX4 mode name (MS1 acceptance:
    /// 编码与解码互逆). The table's own `label` is the UI name and may differ
    /// ("Hold" vs "Auto·Loiter").
    #[test]
    fn encode_decode_round_trip() {
        let expected = [
            ("hold", "Auto·Loiter"),
            ("mission", "Auto·Mission"),
            ("return", "Auto·RTL"),
            ("land", "Auto·Land"),
            ("position", "Position"),
            ("altitude", "Altitude"),
            ("manual", "Manual"),
            ("acro", "Acro"),
            ("offboard", "Offboard"),
        ];
        for option in mode_table() {
            let px4_name = expected
                .iter()
                .find(|(id, _)| *id == option.id)
                .map(|(_, name)| *name)
                .unwrap_or_else(|| panic!("no expectation for {}", option.id));
            let custom = encode_custom_mode(option.main_mode, option.sub_mode);
            assert_eq!(
                decode_custom_mode(custom),
                px4_name,
                "round trip failed for {}",
                option.id
            );
        }
    }

    #[test]
    fn decodes_display_strings() {
        // custom_mode 0 packs main=0, which is not a PX4 mode.
        assert_eq!(decode_custom_mode(0), "Mode 0");
        assert_eq!(
            decode_custom_mode(encode_custom_mode(main_mode::MANUAL, 0)),
            "Manual"
        );
        assert_eq!(
            decode_custom_mode(encode_custom_mode(main_mode::AUTO, auto_sub::LOITER)),
            "Auto·Loiter"
        );
        assert_eq!(
            decode_custom_mode(encode_custom_mode(main_mode::POSCTL, posctl_sub::SLOW)),
            "Position·Slow"
        );
        assert_eq!(
            decode_custom_mode(encode_custom_mode(main_mode::AUTO, auto_sub::RTL)),
            "Auto·RTL"
        );
        assert_eq!(decode_custom_mode(encode_custom_mode(99, 0)), "Mode 99");
    }

    #[test]
    fn ready_when_all_clear() {
        let ctx = ModeContext {
            link_online: true,
            armed: true,
            mission_onboard: true,
            mission_synced: true,
            home_valid: true,
            rc_available: true,
            motor_test_running: false,
        };
        for entry in available_modes(&ctx) {
            assert!(entry.enabled, "{} should be enabled", entry.option.id);
            assert!(entry.reason.is_none());
        }
    }

    #[test]
    fn greyed_entries_carry_reasons() {
        let ctx = ModeContext::default();
        let entries = available_modes(&ctx);
        let by_id = |id: &str| {
            entries
                .iter()
                .find(|e| e.option.id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        assert!(!by_id("hold").enabled);
        assert_eq!(by_id("hold").reason.as_deref(), Some("link offline"));
        assert!(!by_id("mission").enabled);
        assert_eq!(by_id("mission").reason.as_deref(), Some("no mission on FC"));
        assert!(!by_id("return").enabled);
        assert_eq!(by_id("return").reason.as_deref(), Some("not armed"));
        assert!(!by_id("land").enabled);
        assert_eq!(by_id("land").reason.as_deref(), Some("not armed"));
        assert!(!by_id("position").enabled);
        assert_eq!(by_id("position").reason.as_deref(), Some("RC required"));
    }

    #[test]
    fn mission_sync_reason_when_onboard_but_unsynced() {
        let ctx = ModeContext {
            mission_onboard: true,
            ..Default::default()
        };
        let entry = available_modes(&ctx)
            .into_iter()
            .find(|e| e.option.id == "mission")
            .expect("mission");
        assert!(!entry.enabled);
        assert_eq!(entry.reason.as_deref(), Some("mission not synced"));
    }

    #[test]
    fn return_needs_home_even_when_armed() {
        let ctx = ModeContext {
            armed: true,
            ..Default::default()
        };
        let entry = available_modes(&ctx)
            .into_iter()
            .find(|e| e.option.id == "return")
            .expect("return");
        assert!(!entry.enabled);
        assert_eq!(entry.reason.as_deref(), Some("no home position"));
    }

    #[test]
    fn motor_test_locks_the_whole_table() {
        let ctx = ModeContext {
            link_online: true,
            armed: true,
            home_valid: true,
            rc_available: true,
            motor_test_running: true,
            ..Default::default()
        };
        for entry in available_modes(&ctx) {
            assert!(!entry.enabled, "{} must be locked", entry.option.id);
            assert_eq!(entry.reason.as_deref(), Some("motor test running"));
        }
    }

    #[test]
    fn advanced_modes_flagged() {
        let advanced: Vec<String> = mode_table()
            .iter()
            .filter(|o| o.advanced)
            .map(|o| o.id.clone())
            .collect();
        assert_eq!(advanced, vec!["manual", "acro", "offboard"]);
        let confirms: Vec<String> = mode_table()
            .iter()
            .filter(|o| o.confirm)
            .map(|o| o.id.clone())
            .collect();
        assert_eq!(confirms, vec!["return", "land"]);
    }
}
