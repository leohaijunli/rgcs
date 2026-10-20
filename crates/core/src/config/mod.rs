//! Versioned desktop application preferences (plan §3 A6).
//!
//! The desktop app owns one JSON file under Tauri's `app_config_dir`
//! ([`CONFIG_FILE_NAME`]). It is the single source of truth for operator
//! preferences that must survive a restart: the connection endpoint and the
//! auto-connect toggle, the initial vehicle position and UAV model
//! orientation, and the alert-sound volume. The file carries a
//! [`CONFIG_VERSION`] so a later schema change can be *migrated* rather than
//! discarded (ADR-002: the type lives in `core`, the file I/O in `app-tauri`).
//!
//! Parsing is deliberately forgiving. Unknown fields are ignored and every
//! field has a default (struct-level `#[serde(default)]`), so a file written
//! by a newer build — or hand-edited by an operator — still loads. A file we
//! genuinely cannot use is reported as an error; the caller (the Tauri
//! `config_load` command) then warns and falls back to the defaults instead of
//! failing to start.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::plan::GeoPoint;
use crate::safety::SafetyConfig;

/// Schema version written into every file. Bump this when a field changes
/// meaning or is removed, and add an arm to [`AppConfig::migrate`].
pub const CONFIG_VERSION: u32 = 1;

/// File name inside Tauri's `app_config_dir`.
pub const CONFIG_FILE_NAME: &str = "config.json";

/// Connection endpoint used when none has been chosen (PX4 SITL default).
pub const DEFAULT_ENDPOINT: &str = "udpin:0.0.0.0:14550";

/// Initial vehicle position used before a link reports its own fix: BC coast
/// default, matching `frontend/src/cesium/constants.ts`.
pub const DEFAULT_HOME_LAT: f64 = 48.6493;
/// See [`DEFAULT_HOME_LAT`].
pub const DEFAULT_HOME_LON: f64 = -123.3982;

/// Alert-sound volume (0..1): audible outdoors without pinning the meter.
pub const DEFAULT_ALERT_VOLUME: f64 = 0.6;

/// Default UAV model yaw correction in degrees (asset nose is not the +Y axis).
pub const DEFAULT_MODEL_YAW_DEG: f64 = -90.0;

/// Why a config file could not be turned into an [`AppConfig`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file is not valid JSON, or a field has the wrong type.
    #[error("config JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// The file was written by a newer build; refuse rather than guess.
    #[error("config version {0} is newer than supported version {CONFIG_VERSION}")]
    UnsupportedVersion(u32),
}

/// UAV 3D-model orientation offsets in degrees. Added to the MAVLink attitude
/// before the model quaternion is composed, so a model whose nose is not the
/// asset's default axis can be re-aligned from Settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelOrientation {
    pub pitch: f64,
    pub roll: f64,
    pub yaw: f64,
}

impl Default for ModelOrientation {
    fn default() -> Self {
        Self {
            pitch: 0.0,
            roll: 0.0,
            yaw: DEFAULT_MODEL_YAW_DEG,
        }
    }
}

impl ModelOrientation {
    /// Whether all three offsets are finite.
    pub fn is_finite(&self) -> bool {
        self.pitch.is_finite() && self.roll.is_finite() && self.yaw.is_finite()
    }
}

/// The whole persisted preference set.
///
/// `#[serde(default)]` fills a missing field from [`AppConfig::default`], so a
/// partial file still loads; `GeoPoint` keeps the height discipline (a bare
/// `f64` is never an absolute height).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct AppConfig {
    /// Schema version; see [`CONFIG_VERSION`].
    pub version: u32,
    /// MAVLink endpoint address string (`udpin:0.0.0.0:14550`).
    pub endpoint: String,
    /// Connect to [`AppConfig::endpoint`] on launch.
    pub auto_connect: bool,
    /// Position used before a vehicle reports its own fix (WGS84 degrees).
    pub initial_position: GeoPoint,
    /// UAV model orientation offsets in degrees.
    pub model_orientation: ModelOrientation,
    /// Alert-sound volume in `0..=1`.
    pub alert_volume: f64,
    /// Preflight thresholds and toggles (L5/A6): the same values the arm gate
    /// and the graded alerts read, so the UI cannot fork its own copy.
    pub safety: SafetyConfig,
    /// Planning preferences (U8): the last survey centre and the named
    /// parameter presets ("40 m AGL · 5 m/s · 5 m spacing").
    #[serde(default)]
    pub plan: PlanPrefs,
}

/// Planning preferences persisted with the app config (U8).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanPrefs {
    /// Last survey-area centre `[lat, lon]` — the "last area" of the
    /// default-centre priority (vehicle → HOME → last area → map).
    pub last_center: Option<[f64; 2]>,
    /// Named survey-parameter presets.
    pub presets: Vec<PlanPreset>,
}

/// One named survey-parameter preset (U8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanPreset {
    pub name: String,
    /// `sweep` | `cloverleaf`.
    pub kind: String,
    /// Survey AGL, metres.
    pub agl_m: f64,
    pub speed_mps: f64,
    pub line_spacing_m: f64,
    pub tie_spacing_m: Option<f64>,
    pub width_m: f64,
    pub height_m: f64,
    pub heading_deg: f64,
    /// Drape safety clearance above the DSM, metres (DR1). Optional so older
    /// presets (and the required-value rule) stay intact: `None` still means
    /// "not set", and the drape refuses to generate until it is entered.
    #[serde(default)]
    pub min_clearance_dsm_m: Option<f64>,
    /// Planned-height offset carried with the preset (DR1), metres.
    #[serde(default)]
    pub vertical_offset_m: Option<f64>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            endpoint: DEFAULT_ENDPOINT.to_string(),
            auto_connect: true,
            initial_position: GeoPoint::new(DEFAULT_HOME_LAT, DEFAULT_HOME_LON),
            model_orientation: ModelOrientation::default(),
            alert_volume: DEFAULT_ALERT_VOLUME,
            safety: SafetyConfig::default(),
            plan: PlanPrefs::default(),
        }
    }
}

impl AppConfig {
    /// Parse a config file body.
    ///
    /// Unknown fields are ignored. A missing `version` is treated as `0` and
    /// migrated (the pre-file era stored preferences in `localStorage`); a
    /// version *newer* than [`CONFIG_VERSION`] is an error. Semantically bad
    /// values (out-of-range position, non-finite volume) are repaired in place
    /// rather than thrown away — one bad field must not lose the rest.
    pub fn from_json(text: &str) -> Result<Self, ConfigError> {
        let mut config: AppConfig = serde_json::from_str(text)?;
        if config.version > CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(config.version));
        }
        config.migrate();
        config.normalize();
        Ok(config)
    }

    /// Serialize to pretty JSON (with a trailing newline).
    pub fn to_json_pretty(&self) -> Result<String, ConfigError> {
        let mut config = self.clone();
        config.normalize();
        let mut text = serde_json::to_string_pretty(&config)?;
        text.push('\n');
        Ok(text)
    }

    /// Return a copy with all values repaired (see [`AppConfig::normalize`]).
    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }

    /// Upgrade an older schema in place. v0 (legacy `localStorage`) has the
    /// same fields as v1, so the only work is stamping the current version.
    /// `require_mission` also flips to `false` (operator decision
    /// 2026-10-18: not uploading a mission must not prohibit arming) — the
    /// setting is not exposed in the UI, so every persisted `true` was the
    /// old compiled default, not an operator choice.
    fn migrate(&mut self) {
        if self.version < CONFIG_VERSION {
            self.version = CONFIG_VERSION;
        }
        self.safety.require_mission = false;
    }

    /// Repair values that are unusable while leaving the rest intact.
    fn normalize(&mut self) {
        self.version = CONFIG_VERSION;
        if self.endpoint.trim().is_empty() {
            self.endpoint = DEFAULT_ENDPOINT.to_string();
        }
        if !self.initial_position.is_valid() {
            self.initial_position = GeoPoint::new(DEFAULT_HOME_LAT, DEFAULT_HOME_LON);
        }
        if !self.model_orientation.is_finite() {
            self.model_orientation = ModelOrientation::default();
        }
        if !self.alert_volume.is_finite() {
            self.alert_volume = DEFAULT_ALERT_VOLUME;
        }
        self.alert_volume = self.alert_volume.clamp(0.0, 1.0);
        self.safety = self.safety.clone().normalized();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_the_documented_startup_state() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.version, CONFIG_VERSION);
        assert_eq!(cfg.endpoint, DEFAULT_ENDPOINT);
        assert!(cfg.auto_connect);
        assert_eq!(
            cfg.initial_position,
            GeoPoint::new(DEFAULT_HOME_LAT, DEFAULT_HOME_LON)
        );
        assert_eq!(cfg.model_orientation.yaw, DEFAULT_MODEL_YAW_DEG);
        assert_eq!(cfg.alert_volume, DEFAULT_ALERT_VOLUME);
    }

    #[test]
    fn round_trips_through_json() {
        let cfg = AppConfig {
            endpoint: "udpout:127.0.0.1:14551".to_string(),
            auto_connect: false,
            initial_position: GeoPoint::new(45.0, -75.0),
            model_orientation: ModelOrientation {
                pitch: 1.0,
                roll: 2.0,
                yaw: 3.0,
            },
            alert_volume: 0.25,
            safety: SafetyConfig {
                min_satellites: 10,
                warn_battery_percent: 40,
                ..SafetyConfig::default()
            },
            ..AppConfig::default()
        };
        let text = cfg.to_json_pretty().expect("serialize");
        assert_eq!(
            AppConfig::from_json(&text).expect("parse"),
            cfg.normalized()
        );
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let cfg = AppConfig::from_json(r#"{"endpoint":"udpout:1.2.3.4:14550"}"#).expect("parse");
        assert_eq!(cfg.endpoint, "udpout:1.2.3.4:14550");
        assert!(cfg.auto_connect);
        assert_eq!(cfg.alert_volume, DEFAULT_ALERT_VOLUME);
        assert_eq!(cfg.version, CONFIG_VERSION);
    }

    #[test]
    fn missing_safety_section_uses_checklist_defaults() {
        let cfg = AppConfig::from_json(r#"{"endpoint":"udpout:1.2.3.4:14550"}"#)
            .expect("legacy v1 config without safety");
        assert_eq!(cfg.safety, SafetyConfig::default());
    }

    #[test]
    fn unusable_safety_values_are_repaired() {
        let cfg = AppConfig::from_json(
            r#"{"safety":{"stale_after_ms":1,"min_satellites":200,
                "warn_battery_percent":-4,"critical_battery_percent":90,
                "require_rtk":true,"require_home":false,"require_mission":true,
                "require_on_ground":true,"require_motor_test_idle":true}}"#,
        )
        .expect("parse");
        assert_eq!(cfg.safety.stale_after_ms, 500);
        assert_eq!(cfg.safety.min_satellites, 32);
        assert_eq!(cfg.safety.warn_battery_percent, 0);
        assert_eq!(
            cfg.safety.critical_battery_percent, 0,
            "critical must not exceed warn"
        );
        assert!(cfg.safety.require_rtk);
        assert!(!cfg.safety.require_home);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let cfg = AppConfig::from_json(r#"{"version":1,"future_field":42}"#).expect("parse");
        assert_eq!(cfg, AppConfig::default());
    }

    #[test]
    fn legacy_version_zero_is_migrated() {
        let cfg = AppConfig::from_json(r#"{"version":0,"endpoint":"serial:/dev/ttyACM0:115200"}"#)
            .expect("parse");
        assert_eq!(cfg.version, CONFIG_VERSION);
        assert_eq!(cfg.endpoint, "serial:/dev/ttyACM0:115200");
    }

    #[test]
    fn newer_version_is_rejected() {
        let err = AppConfig::from_json(r#"{"version":99}"#).expect_err("must reject");
        assert!(matches!(err, ConfigError::UnsupportedVersion(99)));
    }

    #[test]
    fn corrupt_json_errors_without_panicking() {
        assert!(matches!(
            AppConfig::from_json("{ not json"),
            Err(ConfigError::Json(_))
        ));
    }

    #[test]
    fn bad_values_are_repaired_not_fatal() {
        let cfg = AppConfig::from_json(
            r#"{"version":1,"initial_position":{"latitude_deg":200.0,"longitude_deg":0.0},"alert_volume":9.0}"#,
        )
        .expect("parse");
        assert_eq!(
            cfg.initial_position,
            GeoPoint::new(DEFAULT_HOME_LAT, DEFAULT_HOME_LON)
        );
        assert_eq!(cfg.alert_volume, 1.0);
    }

    #[test]
    fn empty_endpoint_falls_back_to_default() {
        let cfg = AppConfig::from_json(r#"{"version":1,"endpoint":"   "}"#).expect("parse");
        assert_eq!(cfg.endpoint, DEFAULT_ENDPOINT);
    }
}
