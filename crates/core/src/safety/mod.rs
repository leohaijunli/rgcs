//! Pure preflight/safety evaluation (A5).
//!
//! [`evaluate`] is a pure function: no I/O, no clock. The caller passes the
//! current epoch-ms time, the already-collected [`TelemetrySnapshot`] (L6) and
//! the app-side [`PlanState`]; thresholds live in [`SafetyConfig`]. The same
//! engine runs headless (server) and in the desktop app, and PJ2 will supply
//! per-survey-profile checklist templates on top of this base set without
//! touching this code.
//!
//! Status discipline: a field that has never arrived or whose data is stale is
//! `Unknown`, **never** `Pass`. `Fail` means a known condition is violated,
//! `Warn` a known-but-confirmable deviation.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::telemetry::{GpsFixType, SensorHealthMasks, TelemetrySnapshot};

pub mod health;

/// Outcome of one checklist item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistStatus {
    Pass,
    Warn,
    Fail,
    Unknown,
}

impl ChecklistStatus {
    /// True when the item prevents arming until resolved (L5). `Unknown` is
    /// not `Pass`: missing or stale data must not be waved through.
    pub fn blocks_arm(self) -> bool {
        matches!(self, Self::Fail | Self::Unknown)
    }

    /// True when the operator must confirm the item to proceed (L5).
    pub fn needs_ack(self) -> bool {
        matches!(self, Self::Warn)
    }
}

/// Which area of the preflight a checklist item belongs to (U5 groups by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistGroup {
    Link,
    Navigation,
    Power,
    System,
    Mission,
    /// Terrain/drape items (DR4). Group ids are stable; the frontend renders
    /// them via i18n (`checklistGroup.terrain`).
    Terrain,
}

/// Stable identity of a checklist item. PJ2 profiles add more; the base set
/// below covers L5's mandatory pre-arm items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistItemId {
    /// Heartbeat received recently (L5 "link fresh").
    LinkFresh,
    /// GPS fix quality + satellite count.
    GpsFix,
    /// RTK fixed (only when [`SafetyConfig::require_rtk`]).
    Rtk,
    /// HOME set on the FC.
    HomePosition,
    /// Battery voltage/percent within limits.
    Battery,
    /// Every enabled sensor reports healthy (`SYS_STATUS` masks).
    SensorHealth,
    /// `EXTENDED_SYS_STATE` reports on-ground (arming requires it).
    LandedOnGround,
    /// Mission exists, is uploaded and was read back consistently.
    MissionReady,
    /// FC main state is not booting/calibrating/critical.
    FlightModeReady,
    /// Motor test is not running.
    MotorTestIdle,
    // Drape group (DR4): terrain-mode facts. All `Unknown` without a result.
    /// The loaded DEM (+ buffer) covers every line.
    DemCoverage,
    /// The source carries a DTM (AGL is not "height above surface").
    DemHasDtm,
    /// The DEM acquisition is within the configured age.
    DemAge,
    /// No out-of-band span, or every span confirmed by the operator.
    DrapeInfeasible,
    /// The compiled mission fits the item limit.
    DrapeItemCount,
    /// The required climb is within the FC's configured limit.
    DrapeClimbRate,
    /// The drape thresholds are set (`min_clearance_dsm` has no default).
    DrapeThresholds,
    /// The battery covers the ETA plus the climb margin.
    DrapeBatteryEta,
}

/// One row of the checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChecklistItem {
    pub id: ChecklistItemId,
    pub group: ChecklistGroup,
    pub status: ChecklistStatus,
    /// One-sentence reason, English (shown verbatim by U5).
    pub detail: String,
    /// The "go fix" hint surfaced as a repair entry in the UI, if any.
    pub fix_hint: Option<String>,
}

/// App-side facts the engine needs but cannot read from MAVLink. Supplied by
/// the desktop/headless adapter; the frontend maps its mission-sync state onto
/// these fields.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanState {
    /// A mission exists in the GCS (planned, imported, or downloaded).
    pub has_plan: bool,
    /// The mission was uploaded to the FC.
    pub mission_uploaded: bool,
    /// The upload was verified by read-back (`itemsHash` matches).
    pub mission_verified: bool,
    /// The motor test is running and must stop before arming.
    pub motor_test_active: bool,
    /// The drape-pipeline facts the Drape checklist group (DR4) reads;
    /// `None` = terrain mode off / no result yet (the items read `Unknown`).
    #[serde(default)]
    pub drape: Option<DrapeCheckFacts>,
}

/// App-side drape facts for the DR4 checklist group: every number comes from
/// the `DrapeResult` + the loaded DEM metadata, computed by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DrapeCheckFacts {
    /// `Some(true)` when the loaded DEM (+ its buffer margin) covers every
    /// line; `None` when there is no DEM to check against.
    pub dem_covers_route: Option<bool>,
    /// `false` = a DTM-only source: AGL is really "height above surface".
    pub has_dtm: bool,
    /// `Some(false)` when the acquisition is older than the configured age.
    pub dem_age_ok: Option<bool>,
    /// Out-of-band spans reported by the solver.
    pub infeasible_count: usize,
    /// The operator accepted each reported span (planning UI).
    pub infeasible_confirmed: bool,
    /// Compiled waypoint count (incl. speed items).
    pub waypoint_count: usize,
    /// The mission-item ceiling the compile checks against.
    pub waypoint_limit: usize,
    /// The climb the profile asks of the vehicle, m/s.
    pub max_climb_needed_mps: Option<f64>,
    /// The FC's configured climb limit (parameter names *verify*); `None` =
    /// not readable yet → the item reads `Unknown`.
    pub fc_max_climb_mps: Option<f64>,
    /// Battery minutes remaining (from telemetry, caller-supplied).
    pub battery_minutes: Option<f64>,
    /// ETA + climb margin, minutes.
    pub eta_minutes: Option<f64>,
}

/// Thresholds and toggles for the base checklist (A5). Kept separate so L7
/// (graded alerts) and M2 (mag preflight) read the same numbers; persisted
/// via A6's `AppConfig` and loaded once at startup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct SafetyConfig {
    /// Telemetry field older than this is `Unknown`, not `Pass`.
    #[ts(type = "number")]
    pub stale_after_ms: u64,
    /// DEM acquisition older than this many years raises the advisory age
    /// Warn (DR4's DemAge item, WarnOnly). `None` disables the check.
    pub dem_max_age_years: Option<f64>,
    /// L9: click-to-fly (`DO_REPOSITION`) distance cap, metres. `None` = the
    /// 1 km default.
    pub goto_max_distance_m: Option<f64>,
    /// L9: the minimum terrain clearance a click-to-fly target's surface must
    /// leave under the vehicle's held altitude, metres. `None` = the check is
    /// skipped (no DEM, or the operator disabled it).
    pub goto_min_clearance_m: Option<f64>,
    /// Minimum satellites for a `Pass` on the GPS item.
    pub min_satellites: u8,
    /// Battery percent below which the battery item is `Warn`.
    pub warn_battery_percent: i8,
    /// Battery percent below which the battery item is `Fail`.
    pub critical_battery_percent: i8,
    /// Battery voltage (mV) below which L7 raises a warning. `None` disables
    /// the voltage check: packs differ, so there is no safe universal value.
    pub warn_battery_voltage_mv: Option<u16>,
    /// Battery voltage (mV) below which L7 raises a critical alert. `None`
    /// disables the voltage check.
    pub critical_battery_voltage_mv: Option<u16>,
    /// Satellite count below which L7 raises a warning. The checklist already
    /// uses [`SafetyConfig::min_satellites`] as its Pass threshold.
    pub critical_satellites: u8,
    /// Telemetry RSSI below which L7 raises a warning. `None` disables it.
    pub warn_rssi: Option<u8>,
    /// Telemetry RSSI below which L7 raises a critical alert. `None` disables
    /// it.
    pub critical_rssi: Option<u8>,
    /// Free radio TX buffer (percent) at or below which L7 warns.
    pub warn_txbuf_percent: u8,
    /// Free radio TX buffer (percent) at or below which L7 is critical.
    pub critical_txbuf_percent: u8,
    /// Packet loss (percent) above which L7 warns.
    pub warn_packet_loss_percent: u8,
    /// Packet loss (percent) above which L7 is critical.
    pub critical_packet_loss_percent: u8,
    /// Whether the RTK item is evaluated.
    pub require_rtk: bool,
    /// Whether a HOME position is required.
    pub require_home: bool,
    /// Whether an existing mission must be uploaded and verified before arming.
    /// No mission at all remains valid for manual/standalone arming.
    pub require_mission: bool,
    /// Whether the vehicle must report on-ground before arming.
    pub require_on_ground: bool,
    /// Whether the motor test must be idle before arming.
    pub require_motor_test_idle: bool,
}

/// Default heartbeat freshness threshold.
pub const DEFAULT_STALE_AFTER_MS: u64 = 3_000;
/// Default minimum satellites for a `Pass` GPS item.
pub const DEFAULT_MIN_SATELLITES: u8 = 6;
/// Default battery percent that turns the battery item `Warn`.
pub const DEFAULT_WARN_BATTERY_PERCENT: i8 = 30;
/// Default battery percent that turns the battery item `Fail`.
pub const DEFAULT_CRITICAL_BATTERY_PERCENT: i8 = 15;
/// Default satellite count below which L7 is critical.
pub const DEFAULT_CRITICAL_SATELLITES: u8 = 4;
/// Default free TX buffer percentage at or below which L7 warns.
pub const DEFAULT_WARN_TXBUF_PERCENT: u8 = 20;
/// Default free TX buffer percentage at or below which L7 is critical.
pub const DEFAULT_CRITICAL_TXBUF_PERCENT: u8 = 5;
/// Default packet-loss percentage above which L7 warns.
pub const DEFAULT_WARN_PACKET_LOSS_PERCENT: u8 = 10;
/// Default packet-loss percentage above which L7 is critical.
pub const DEFAULT_CRITICAL_PACKET_LOSS_PERCENT: u8 = 25;
/// Lower bound accepted for `stale_after_ms` when normalizing persisted config.
pub const MIN_STALE_AFTER_MS: u64 = 500;
/// Upper bound accepted for `stale_after_ms` when normalizing persisted config.
pub const MAX_STALE_AFTER_MS: u64 = 60_000;
/// GNSS constellations never report more satellites than this on PX4.
pub const MAX_SATELLITES: u8 = 32;
/// Battery percentages are percentages.
pub const MAX_BATTERY_PERCENT: i8 = 100;
/// Upper bound accepted for a configured battery-voltage threshold.
pub const MAX_BATTERY_VOLTAGE_MV: u16 = 60_000;
/// TX buffer occupancy and packet loss are percentages.
pub const MAX_PERCENT: u8 = 100;

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            stale_after_ms: DEFAULT_STALE_AFTER_MS,
            min_satellites: DEFAULT_MIN_SATELLITES,
            warn_battery_percent: DEFAULT_WARN_BATTERY_PERCENT,
            critical_battery_percent: DEFAULT_CRITICAL_BATTERY_PERCENT,
            warn_battery_voltage_mv: None,
            critical_battery_voltage_mv: None,
            critical_satellites: DEFAULT_CRITICAL_SATELLITES,
            warn_rssi: None,
            critical_rssi: None,
            warn_txbuf_percent: DEFAULT_WARN_TXBUF_PERCENT,
            critical_txbuf_percent: DEFAULT_CRITICAL_TXBUF_PERCENT,
            warn_packet_loss_percent: DEFAULT_WARN_PACKET_LOSS_PERCENT,
            critical_packet_loss_percent: DEFAULT_CRITICAL_PACKET_LOSS_PERCENT,
            require_rtk: false,
            require_home: true,
            // Operator decision 2026-10-18: not uploading a mission must not
            // prohibit arming (manual flights, hover tests). The drape group
            // carries the terrain safety check; the mission sync stays a Warn
            // in the planning UI.
            require_mission: false,
            dem_max_age_years: Some(3.0),
            goto_max_distance_m: None,
            goto_min_clearance_m: None,
            require_on_ground: true,
            require_motor_test_idle: true,
        }
    }
}

impl SafetyConfig {
    /// Repair values that cannot be used by the checklist. One bad threshold
    /// must not discard the operator's other settings.
    pub fn normalized(mut self) -> Self {
        self.stale_after_ms = self
            .stale_after_ms
            .clamp(MIN_STALE_AFTER_MS, MAX_STALE_AFTER_MS);
        self.min_satellites = self.min_satellites.min(MAX_SATELLITES);
        self.warn_battery_percent = self.warn_battery_percent.clamp(0, MAX_BATTERY_PERCENT);
        self.critical_battery_percent = self.critical_battery_percent.clamp(0, MAX_BATTERY_PERCENT);
        if self.critical_battery_percent > self.warn_battery_percent {
            self.critical_battery_percent = self.warn_battery_percent;
        }
        self.warn_battery_voltage_mv = self
            .warn_battery_voltage_mv
            .filter(|mv| *mv > 0 && *mv <= MAX_BATTERY_VOLTAGE_MV);
        self.critical_battery_voltage_mv = self
            .critical_battery_voltage_mv
            .filter(|mv| *mv > 0 && *mv <= MAX_BATTERY_VOLTAGE_MV);
        match (
            self.warn_battery_voltage_mv,
            self.critical_battery_voltage_mv,
        ) {
            (None, Some(critical)) => self.warn_battery_voltage_mv = Some(critical),
            (Some(warn), Some(critical)) if critical > warn => {
                self.critical_battery_voltage_mv = Some(warn);
            }
            _ => {}
        }
        self.critical_satellites = self
            .critical_satellites
            .min(MAX_SATELLITES)
            .min(self.min_satellites);
        match (self.warn_rssi, self.critical_rssi) {
            (None, Some(critical)) => self.warn_rssi = Some(critical),
            (Some(warn), Some(critical)) if critical > warn => {
                self.critical_rssi = Some(warn);
            }
            _ => {}
        }
        self.warn_txbuf_percent = self.warn_txbuf_percent.min(MAX_PERCENT);
        self.critical_txbuf_percent = self
            .critical_txbuf_percent
            .min(MAX_PERCENT)
            .min(self.warn_txbuf_percent);
        self.warn_packet_loss_percent = self.warn_packet_loss_percent.min(MAX_PERCENT);
        self.critical_packet_loss_percent = self
            .critical_packet_loss_percent
            .min(MAX_PERCENT)
            .max(self.warn_packet_loss_percent);
        self
    }
}

/// One evaluated checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Checklist {
    pub items: Vec<ChecklistItem>,
}

impl Checklist {
    /// The worst item status: `Fail` > `Unknown` > `Warn` > `Pass`.
    ///
    /// `Unknown` outranks `Warn` so a checklist with a single missing sensor
    /// never reads "just a warning".
    pub fn overall(&self) -> ChecklistStatus {
        let mut worst = ChecklistStatus::Pass;
        for item in &self.items {
            let rank = match item.status {
                ChecklistStatus::Fail => 3,
                ChecklistStatus::Unknown => 2,
                ChecklistStatus::Warn => 1,
                ChecklistStatus::Pass => 0,
            };
            let cur = match worst {
                ChecklistStatus::Fail => 3,
                ChecklistStatus::Unknown => 2,
                ChecklistStatus::Warn => 1,
                ChecklistStatus::Pass => 0,
            };
            if rank > cur {
                worst = item.status;
            }
        }
        worst
    }

    /// Every item passes: only then is the one-click arm enabled (L5).
    pub fn is_ready(&self) -> bool {
        self.overall() == ChecklistStatus::Pass
    }

    /// Pass count out of the total, for the `9/11` readiness badge (U5).
    pub fn pass_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == ChecklistStatus::Pass)
            .count()
    }

    /// Look up one item by id.
    pub fn get(&self, id: ChecklistItemId) -> Option<&ChecklistItem> {
        self.items.iter().find(|i| i.id == id)
    }
}

/// How a profile's checklist template treats one item (PJ2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistItemMode {
    /// `Fail` blocks arming (the L5 baseline for every flight-safety item).
    Required,
    /// A `Fail` is downgraded to `Warn` (acknowledgement, not a block) — for
    /// survey types where the item does not gate the measurement itself.
    WarnOnly,
}

/// One template entry: which checklist item, and how it blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChecklistItemSpec {
    pub id: ChecklistItemId,
    pub mode: ChecklistItemMode,
}

impl ChecklistItemSpec {
    /// A required (blocking) item — the baseline grade.
    #[must_use]
    pub fn required(id: ChecklistItemId) -> Self {
        Self {
            id,
            mode: ChecklistItemMode::Required,
        }
    }
}

/// The per-survey-type preflight checklist template (PJ2): declares which
/// items apply and how they block. The evaluation logic stays in this
/// module's per-item functions — the template only selects and grades them,
/// so a new survey type is a new profile, never new conditionals here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChecklistTemplate {
    pub items: Vec<ChecklistItemSpec>,
}

impl Default for ChecklistTemplate {
    fn default() -> Self {
        Self::baseline()
    }
}

impl ChecklistTemplate {
    /// The baseline UAV template: every [`ChecklistItemId`] required, in the
    /// order `evaluate` used before profiles existed (behaviour-identical).
    #[must_use]
    pub fn baseline() -> Self {
        use ChecklistItemId::{
            Battery, DemAge, DemCoverage, DemHasDtm, DrapeBatteryEta, DrapeClimbRate,
            DrapeInfeasible, DrapeItemCount, DrapeThresholds, FlightModeReady, GpsFix,
            HomePosition, LandedOnGround, LinkFresh, MissionReady, MotorTestIdle, Rtk,
            SensorHealth,
        };
        Self {
            items: [
                LinkFresh,
                GpsFix,
                Rtk,
                HomePosition,
                Battery,
                SensorHealth,
                LandedOnGround,
                MissionReady,
                FlightModeReady,
                MotorTestIdle,
                DemCoverage,
                DemHasDtm,
                DemAge,
                DrapeInfeasible,
                DrapeItemCount,
                DrapeClimbRate,
                DrapeThresholds,
                DrapeBatteryEta,
            ]
            .into_iter()
            .map(|id| match id {
                // Advisory drape items: a DTM-only source, an old DEM or a
                // thin battery margin warn and are acknowledged, never block.
                ChecklistItemId::DemHasDtm
                | ChecklistItemId::DemAge
                | ChecklistItemId::DrapeBatteryEta => ChecklistItemSpec {
                    id,
                    mode: ChecklistItemMode::WarnOnly,
                },
                other => ChecklistItemSpec::required(other),
            })
            .collect(),
        }
    }
}

/// Evaluate the base preflight checklist (A5), graded by the **default
/// profile's** checklist template (the registered `UavMag` profile — the
/// profile path is exercised by [`evaluate_with_template`]).
///
/// `now_ms` is the caller's epoch-ms clock reading, matched against the
/// per-field ages in [`TelemetrySnapshot::field_ages`]. Pure and deterministic.
pub fn evaluate(
    snapshot: &TelemetrySnapshot,
    plan: &PlanState,
    config: &SafetyConfig,
    now_ms: u64,
) -> Checklist {
    let template = crate::survey_profile::registry().default_profile();
    evaluate_with_template(snapshot, plan, config, now_ms, &template.safety_template)
}

/// Evaluate the preflight checklist against an explicit profile template:
/// only the template's items appear, and [`ChecklistItemMode::WarnOnly`]
/// downgrades a `Fail` to `Warn` (the arm gate then needs an acknowledgement
/// instead of blocking). Pure and deterministic.
pub fn evaluate_with_template(
    snapshot: &TelemetrySnapshot,
    plan: &PlanState,
    config: &SafetyConfig,
    now_ms: u64,
    template: &ChecklistTemplate,
) -> Checklist {
    let items = template
        .items
        .iter()
        .map(|spec| {
            let mut item = match spec.id {
                ChecklistItemId::LinkFresh => link_item(snapshot, config, now_ms),
                ChecklistItemId::GpsFix => gps_item(snapshot, config, now_ms),
                ChecklistItemId::Rtk => rtk_item(snapshot, config, now_ms),
                ChecklistItemId::HomePosition => home_item(snapshot, config, now_ms),
                ChecklistItemId::Battery => battery_item(snapshot, config, now_ms),
                ChecklistItemId::SensorHealth => sensor_health_item(snapshot, config, now_ms),
                ChecklistItemId::LandedOnGround => landed_item(snapshot, config, now_ms),
                ChecklistItemId::MissionReady => mission_item(plan, config),
                ChecklistItemId::FlightModeReady => mode_item(snapshot, config, now_ms),
                ChecklistItemId::MotorTestIdle => motor_test_item(plan, config),
                ChecklistItemId::DemCoverage => dem_coverage_item(plan),
                ChecklistItemId::DemHasDtm => dem_has_dtm_item(plan),
                ChecklistItemId::DemAge => dem_age_item(plan),
                ChecklistItemId::DrapeInfeasible => drape_infeasible_item(plan),
                ChecklistItemId::DrapeItemCount => drape_item_count_item(plan),
                ChecklistItemId::DrapeClimbRate => drape_climb_rate_item(plan),
                ChecklistItemId::DrapeThresholds => drape_thresholds_item(plan),
                ChecklistItemId::DrapeBatteryEta => drape_battery_eta_item(plan),
            };
            if spec.mode == ChecklistItemMode::WarnOnly && item.status == ChecklistStatus::Fail {
                item.status = ChecklistStatus::Warn;
            }
            item
        })
        .collect();
    Checklist { items }
}

// --- Drape group (DR4): terrain-mode facts from `PlanState::drape`. -------
// Without a result (`drape: None`) every item reads `Unknown` — missing data
// must never read as Pass (ADR-004's rule, applied to the checklist).

/// `PlanState::drape == None` means terrain mode is **not in use** — the
/// drape items are not applicable, so they read Pass with a note. When
/// terrain mode IS on, the caller supplies facts (possibly an all-unknown
/// placeholder when no result exists yet) and the items grade for real.
fn drape_not_in_use(id: ChecklistItemId, group: ChecklistGroup) -> ChecklistItem {
    item(
        id,
        group,
        ChecklistStatus::Pass,
        "terrain drape not in use",
        None,
    )
}

fn dem_coverage_item(plan: &PlanState) -> ChecklistItem {
    const ID: ChecklistItemId = ChecklistItemId::DemCoverage;
    let group = ChecklistGroup::Terrain;
    match &plan.drape {
        None => drape_not_in_use(ID, group),
        Some(d) => match d.dem_covers_route {
            None => item(
                ID,
                group,
                ChecklistStatus::Unknown,
                "no DEM loaded — coverage unknown",
                Some("load a DEM or prepare the survey area"),
            ),
            Some(false) => item(
                ID,
                group,
                ChecklistStatus::Fail,
                "the DEM does not cover the whole route plus its margin",
                Some("prepare the survey area for the current boundary"),
            ),
            Some(true) => item(
                ID,
                group,
                ChecklistStatus::Pass,
                "the DEM covers the route plus its margin",
                None,
            ),
        },
    }
}

fn dem_has_dtm_item(plan: &PlanState) -> ChecklistItem {
    const ID: ChecklistItemId = ChecklistItemId::DemHasDtm;
    let group = ChecklistGroup::Terrain;
    match &plan.drape {
        None => drape_not_in_use(ID, group),
        Some(d) if d.has_dtm => item(
            ID,
            group,
            ChecklistStatus::Pass,
            "the DEM carries a DTM — AGL is height above ground",
            None,
        ),
        Some(_) => item(
            ID,
            group,
            ChecklistStatus::Warn,
            "DTM-only source — AGL is height above the surface (canopy)",
            Some("use a LiDAR DTM+DSM source for true ground AGL"),
        ),
    }
}

fn dem_age_item(plan: &PlanState) -> ChecklistItem {
    const ID: ChecklistItemId = ChecklistItemId::DemAge;
    let group = ChecklistGroup::Terrain;
    match &plan.drape {
        None => drape_not_in_use(ID, group),
        Some(d) => match d.dem_age_ok {
            None => item(
                ID,
                group,
                ChecklistStatus::Unknown,
                "the DEM acquisition age is not recorded",
                None,
            ),
            Some(false) => item(
                ID,
                group,
                ChecklistStatus::Warn,
                "the DEM acquisition is older than the configured limit — terrain may have changed",
                Some("verify the site or update the DEM"),
            ),
            Some(true) => item(
                ID,
                group,
                ChecklistStatus::Pass,
                "the DEM acquisition is within the age limit",
                None,
            ),
        },
    }
}

fn drape_infeasible_item(plan: &PlanState) -> ChecklistItem {
    const ID: ChecklistItemId = ChecklistItemId::DrapeInfeasible;
    let group = ChecklistGroup::Terrain;
    match &plan.drape {
        None => drape_not_in_use(ID, group),
        Some(d) if d.infeasible_count == 0 => item(
            ID,
            group,
            ChecklistStatus::Pass,
            "no out-of-band spans along the route",
            None,
        ),
        Some(d) if d.infeasible_confirmed => item(
            ID,
            group,
            ChecklistStatus::Pass,
            &format!(
                "{} out-of-band span(s) accepted by the operator",
                d.infeasible_count
            ),
            None,
        ),
        Some(d) => item(
            ID,
            group,
            ChecklistStatus::Fail,
            &format!("{} out-of-band span(s) not accepted", d.infeasible_count),
            Some("review the highlighted spans in the profile preview"),
        ),
    }
}

fn drape_item_count_item(plan: &PlanState) -> ChecklistItem {
    const ID: ChecklistItemId = ChecklistItemId::DrapeItemCount;
    let group = ChecklistGroup::Terrain;
    match &plan.drape {
        None => drape_not_in_use(ID, group),
        Some(d) if d.waypoint_count > d.waypoint_limit => item(
            ID,
            group,
            ChecklistStatus::Fail,
            &format!(
                "{} items exceeds the {} limit",
                d.waypoint_count, d.waypoint_limit
            ),
            Some("raise the waypoint tolerance and regenerate"),
        ),
        Some(d) => item(
            ID,
            group,
            ChecklistStatus::Pass,
            &format!("{} of {} mission items", d.waypoint_count, d.waypoint_limit),
            None,
        ),
    }
}

fn drape_climb_rate_item(plan: &PlanState) -> ChecklistItem {
    const ID: ChecklistItemId = ChecklistItemId::DrapeClimbRate;
    let group = ChecklistGroup::Terrain;
    match &plan.drape {
        None => drape_not_in_use(ID, group),
        Some(d) => match (d.max_climb_needed_mps, d.fc_max_climb_mps) {
            (Some(needed), Some(fc)) if needed > fc => item(
                ID,
                group,
                ChecklistStatus::Fail,
                &format!("the route needs {needed:.1} m/s climb, the FC allows {fc:.1}"),
                Some("slow the line or lower the AGL target"),
            ),
            (Some(needed), Some(fc)) => item(
                ID,
                group,
                ChecklistStatus::Pass,
                &format!("climb {needed:.1} of {fc:.1} m/s"),
                None,
            ),
            (Some(needed), None) => item(
                ID,
                group,
                ChecklistStatus::Unknown,
                &format!(
                    "the route needs {needed:.1} m/s climb — the FC limit is not readable yet"
                ),
                Some("read the FC climb parameters (*verify*)"),
            ),
            _ => item(
                ID,
                group,
                ChecklistStatus::Unknown,
                "the route climb is not computed yet",
                None,
            ),
        },
    }
}

fn drape_thresholds_item(plan: &PlanState) -> ChecklistItem {
    const ID: ChecklistItemId = ChecklistItemId::DrapeThresholds;
    let group = ChecklistGroup::Terrain;
    match &plan.drape {
        None => drape_not_in_use(ID, group),
        Some(d) if d.dem_covers_route.is_none() => item(
            ID,
            group,
            ChecklistStatus::Unknown,
            "no drape spec entered yet",
            Some("enter the minimum DSM clearance"),
        ),
        Some(_) => item(
            ID,
            group,
            ChecklistStatus::Pass,
            "the drape thresholds are set",
            None,
        ),
    }
}

fn drape_battery_eta_item(plan: &PlanState) -> ChecklistItem {
    const ID: ChecklistItemId = ChecklistItemId::DrapeBatteryEta;
    let group = ChecklistGroup::Terrain;
    match &plan.drape {
        None => drape_not_in_use(ID, group),
        Some(d) => match (d.battery_minutes, d.eta_minutes) {
            (Some(battery), Some(eta)) if battery < eta => item(
                ID,
                group,
                ChecklistStatus::Fail,
                &format!("the battery covers {battery:.0} min, the route needs {eta:.0}"),
                Some("shorten the line, swap the pack, or raise the speed"),
            ),
            (Some(battery), Some(eta)) if battery < eta * 1.3 => item(
                ID,
                group,
                ChecklistStatus::Warn,
                &format!(
                    "the battery covers {battery:.0} min, the route needs {eta:.0} — thin margin"
                ),
                Some("plan a battery swap point"),
            ),
            (Some(battery), Some(eta)) => item(
                ID,
                group,
                ChecklistStatus::Pass,
                &format!("the battery covers {battery:.0} min, the route needs {eta:.0}"),
                None,
            ),
            _ => item(
                ID,
                group,
                ChecklistStatus::Unknown,
                "battery/ETA are not known yet",
                None,
            ),
        },
    }
}

/// None when the field never arrived; `Some(is_fresh)` otherwise.
fn fresh(age_at_ms: Option<u64>, now_ms: u64, stale_after_ms: u64) -> Option<bool> {
    age_at_ms.map(|t| now_ms.saturating_sub(t) <= stale_after_ms)
}

fn item(
    id: ChecklistItemId,
    group: ChecklistGroup,
    status: ChecklistStatus,
    detail: &str,
    fix_hint: Option<&str>,
) -> ChecklistItem {
    ChecklistItem {
        id,
        group,
        status,
        detail: detail.to_string(),
        fix_hint: fix_hint.map(str::to_string),
    }
}

fn pass(id: ChecklistItemId, group: ChecklistGroup, detail: &str) -> ChecklistItem {
    item(id, group, ChecklistStatus::Pass, detail, None)
}

fn link_item(snapshot: &TelemetrySnapshot, config: &SafetyConfig, now_ms: u64) -> ChecklistItem {
    let group = ChecklistGroup::Link;
    match fresh(
        snapshot.field_ages.heartbeat_at_ms,
        now_ms,
        config.stale_after_ms,
    ) {
        None => item(
            ChecklistItemId::LinkFresh,
            group,
            ChecklistStatus::Unknown,
            "No telemetry received yet",
            Some("Connect to the flight controller"),
        ),
        Some(false) => item(
            ChecklistItemId::LinkFresh,
            group,
            ChecklistStatus::Unknown,
            "Telemetry is stale",
            Some("Check the radio link"),
        ),
        Some(true) => pass(
            ChecklistItemId::LinkFresh,
            group,
            "Flight controller is reporting",
        ),
    }
}

fn gps_item(snapshot: &TelemetrySnapshot, config: &SafetyConfig, now_ms: u64) -> ChecklistItem {
    let group = ChecklistGroup::Navigation;
    let Some(gps) = snapshot.gps.as_ref() else {
        return item(
            ChecklistItemId::GpsFix,
            group,
            ChecklistStatus::Unknown,
            "No GPS data yet",
            None,
        );
    };
    if fresh(snapshot.field_ages.gps_at_ms, now_ms, config.stale_after_ms) != Some(true) {
        return item(
            ChecklistItemId::GpsFix,
            group,
            ChecklistStatus::Unknown,
            "GPS data is stale",
            Some("Check the GPS receiver"),
        );
    }
    match gps.fix_type {
        GpsFixType::NoGps | GpsFixType::NoFix => item(
            ChecklistItemId::GpsFix,
            group,
            ChecklistStatus::Fail,
            "No GPS fix",
            Some("Move to open sky or check the antenna"),
        ),
        GpsFixType::Fix2d => item(
            ChecklistItemId::GpsFix,
            group,
            ChecklistStatus::Warn,
            "2D fix only",
            Some("Wait for a 3D fix"),
        ),
        _ if gps.satellites_visible < config.min_satellites => item(
            ChecklistItemId::GpsFix,
            group,
            ChecklistStatus::Warn,
            &format!("Only {} satellites", gps.satellites_visible),
            Some("Wait for more satellites"),
        ),
        _ => pass(
            ChecklistItemId::GpsFix,
            group,
            &format!("{} satellites, 3D fix", gps.satellites_visible),
        ),
    }
}

fn rtk_item(snapshot: &TelemetrySnapshot, config: &SafetyConfig, now_ms: u64) -> ChecklistItem {
    let group = ChecklistGroup::Navigation;
    if !config.require_rtk {
        return pass(ChecklistItemId::Rtk, group, "RTK not required");
    }
    let Some(gps) = snapshot.gps.as_ref() else {
        return item(
            ChecklistItemId::Rtk,
            group,
            ChecklistStatus::Unknown,
            "No GPS data for RTK status",
            None,
        );
    };
    if fresh(snapshot.field_ages.gps_at_ms, now_ms, config.stale_after_ms) != Some(true) {
        return item(
            ChecklistItemId::Rtk,
            group,
            ChecklistStatus::Unknown,
            "RTK status is stale",
            Some("Check the RTK base link"),
        );
    }
    match gps.fix_type {
        GpsFixType::RtkFixed => pass(ChecklistItemId::Rtk, group, "RTK fixed"),
        GpsFixType::RtkFloat => item(
            ChecklistItemId::Rtk,
            group,
            ChecklistStatus::Warn,
            "RTK float (not fixed)",
            Some("Wait for RTK fixed or re-check the base"),
        ),
        other => item(
            ChecklistItemId::Rtk,
            group,
            ChecklistStatus::Fail,
            &format!("RTK requires a fixed fix, got {other:?}"),
            Some("Check the RTK base"),
        ),
    }
}

fn home_item(snapshot: &TelemetrySnapshot, config: &SafetyConfig, now_ms: u64) -> ChecklistItem {
    let group = ChecklistGroup::Navigation;
    if !config.require_home {
        return pass(ChecklistItemId::HomePosition, group, "Home not required");
    }
    let Some(_home) = snapshot.home.as_ref() else {
        return item(
            ChecklistItemId::HomePosition,
            group,
            ChecklistStatus::Fail,
            "No home position set",
            Some("Set home before flight"),
        );
    };
    if fresh(
        snapshot.field_ages.home_at_ms,
        now_ms,
        config.stale_after_ms,
    ) != Some(true)
    {
        return item(
            ChecklistItemId::HomePosition,
            group,
            ChecklistStatus::Unknown,
            "Home position is stale",
            Some("Re-read home from the flight controller"),
        );
    }
    pass(ChecklistItemId::HomePosition, group, "Home position set")
}

/// `(remaining_percent, age)` from `SYS_STATUS`, where a negative percent
/// means the FC reports unknown.
fn sys_status_remaining(snapshot: &TelemetrySnapshot) -> (Option<i8>, Option<u64>) {
    match snapshot.sys_status {
        Some(s) if s.battery_remaining_percent >= 0 => (
            Some(s.battery_remaining_percent),
            snapshot.field_ages.sys_status_at_ms,
        ),
        _ => (None, None),
    }
}

fn battery_item(snapshot: &TelemetrySnapshot, config: &SafetyConfig, now_ms: u64) -> ChecklistItem {
    let group = ChecklistGroup::Power;
    let (remaining, age) = if let Some(b) = snapshot.battery.as_ref() {
        // Prefer the detailed BATTERY_STATUS when it carries a percent.
        match b.remaining_percent {
            Some(p) => (Some(p as i8), snapshot.field_ages.battery_at_ms),
            None => sys_status_remaining(snapshot),
        }
    } else {
        sys_status_remaining(snapshot)
    };
    let Some(remaining) = remaining else {
        return item(
            ChecklistItemId::Battery,
            group,
            ChecklistStatus::Unknown,
            "No battery data yet",
            None,
        );
    };
    if fresh(age, now_ms, config.stale_after_ms) != Some(true) {
        return item(
            ChecklistItemId::Battery,
            group,
            ChecklistStatus::Unknown,
            "Battery data is stale",
            None,
        );
    }
    if remaining < config.critical_battery_percent {
        item(
            ChecklistItemId::Battery,
            group,
            ChecklistStatus::Fail,
            &format!("Battery {remaining}%, below critical"),
            Some("Charge or replace the battery"),
        )
    } else if remaining < config.warn_battery_percent {
        item(
            ChecklistItemId::Battery,
            group,
            ChecklistStatus::Warn,
            &format!("Battery {remaining}%"),
            Some("Charge the battery"),
        )
    } else {
        pass(
            ChecklistItemId::Battery,
            group,
            &format!("Battery {remaining}%"),
        )
    }
}

fn sensor_health_item(
    snapshot: &TelemetrySnapshot,
    config: &SafetyConfig,
    now_ms: u64,
) -> ChecklistItem {
    let group = ChecklistGroup::System;
    let Some(s) = snapshot.sys_status else {
        return item(
            ChecklistItemId::SensorHealth,
            group,
            ChecklistStatus::Unknown,
            "No sensor health data yet",
            None,
        );
    };
    if fresh(
        snapshot.field_ages.sys_status_at_ms,
        now_ms,
        config.stale_after_ms,
    ) != Some(true)
    {
        return item(
            ChecklistItemId::SensorHealth,
            group,
            ChecklistStatus::Unknown,
            "Sensor health is stale",
            None,
        );
    }
    let unhealthy = s.sensors.enabled & !s.sensors.health;
    let hard = unhealthy & !OPTIONAL_SENSOR_FAULT_BITS;
    if hard != 0 {
        let names = sensor_names_for_bits(hard);
        let detail = if names.is_empty() {
            "An enabled sensor reports a fault".to_string()
        } else {
            format!("Sensor fault: {}", names.join(", "))
        };
        item(
            ChecklistItemId::SensorHealth,
            group,
            ChecklistStatus::Fail,
            &detail,
            Some("Check the sensor diagnostics"),
        )
    } else if unhealthy != 0 {
        // Only optional sensors are unhealthy (e.g. SITL has no RC receiver):
        // a known, confirmable condition, never a hard failure.
        item(
            ChecklistItemId::SensorHealth,
            group,
            ChecklistStatus::Warn,
            "RC receiver is not detected (expected in SITL)",
            Some("Not required for SITL"),
        )
    } else {
        pass(
            ChecklistItemId::SensorHealth,
            group,
            "All enabled sensors report healthy",
        )
    }
}

/// Sensors whose absence is expected in some setups — SITL has no RC receiver —
/// and is therefore a `Warn`, never a `Fail`.
const OPTIONAL_SENSOR_FAULT_BITS: u32 = 0x10000; // MAV_SYS_STATUS_SENSOR_RC_RECEIVER

/// `MAV_SYS_STATUS_SENSOR` bits → display name (protocol constants; English
/// for the checklist detail, like every other detail string in core).
const SENSOR_FAULT_NAMES: &[(u32, &str)] = &[
    (0x01, "3D gyro"),
    (0x02, "3D accelerometer"),
    (0x04, "3D magnetometer"),
    (0x08, "absolute pressure"),
    (0x10, "differential pressure"),
    (0x20, "GPS"),
    (0x40, "optical flow"),
    (0x80, "vision position"),
    (0x100, "laser position"),
    (0x200, "external ground truth"),
    (0x400, "angular rate control"),
    (0x800, "attitude stabilization"),
    (0x1000, "yaw position"),
    (0x2000, "altitude control"),
    (0x4000, "position control"),
    (0x8000, "motor outputs"),
    (0x10000, "RC receiver"),
    (0x20000, "2nd 3D gyro"),
    (0x40000, "2nd 3D accelerometer"),
    (0x80000, "2nd 3D magnetometer"),
    (0x100000, "geofence"),
    (0x200000, "AHRS"),
    (0x400000, "terrain"),
    (0x800000, "reversed motor"),
    (0x1000000, "logging"),
    (0x2000000, "battery"),
    (0x4000000, "proximity"),
    (0x8000000, "satellite comm"),
    (0x10000000, "prearm check"),
    (0x20000000, "obstacle avoidance"),
    (0x40000000, "propulsion"),
];

/// Names of the sensors that are **enabled but unhealthy**, from the
/// `SYS_STATUS` masks. Answers "which sensor reported the fault" in the
/// preflight checklist.
pub fn unhealthy_sensor_names(sensors: &SensorHealthMasks) -> Vec<&'static str> {
    sensor_names_for_bits(sensors.enabled & !sensors.health)
}

fn sensor_names_for_bits(unhealthy: u32) -> Vec<&'static str> {
    SENSOR_FAULT_NAMES
        .iter()
        .filter(|(bit, _)| unhealthy & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}

fn landed_item(snapshot: &TelemetrySnapshot, config: &SafetyConfig, now_ms: u64) -> ChecklistItem {
    let group = ChecklistGroup::System;
    if !config.require_on_ground {
        return pass(
            ChecklistItemId::LandedOnGround,
            group,
            "Ground check not required",
        );
    }
    let Some(state) = snapshot.extended_sys_state else {
        return item(
            ChecklistItemId::LandedOnGround,
            group,
            ChecklistStatus::Unknown,
            "Landed state not reported yet",
            None,
        );
    };
    if fresh(
        snapshot.field_ages.extended_sys_state_at_ms,
        now_ms,
        config.stale_after_ms,
    ) != Some(true)
    {
        return item(
            ChecklistItemId::LandedOnGround,
            group,
            ChecklistStatus::Unknown,
            "Landed state is stale",
            None,
        );
    }
    use crate::telemetry::LandedState;
    match state.landed_state {
        LandedState::OnGround => pass(ChecklistItemId::LandedOnGround, group, "On ground"),
        LandedState::InAir => item(
            ChecklistItemId::LandedOnGround,
            group,
            ChecklistStatus::Fail,
            "Vehicle is airborne",
            Some("Land and disarm first"),
        ),
        LandedState::Takeoff | LandedState::Landing => item(
            ChecklistItemId::LandedOnGround,
            group,
            ChecklistStatus::Warn,
            "Vehicle is taking off or landing",
            Some("Wait until the vehicle is on the ground"),
        ),
        LandedState::Unknown => item(
            ChecklistItemId::LandedOnGround,
            group,
            ChecklistStatus::Unknown,
            "Landed state unknown",
            None,
        ),
    }
}

fn mission_item(plan: &PlanState, config: &SafetyConfig) -> ChecklistItem {
    let group = ChecklistGroup::Mission;
    if !config.require_mission {
        return pass(ChecklistItemId::MissionReady, group, "Mission not required");
    }
    if !plan.has_plan {
        return pass(
            ChecklistItemId::MissionReady,
            group,
            "No mission planned — standalone arming allowed",
        );
    }
    if !plan.mission_uploaded {
        return item(
            ChecklistItemId::MissionReady,
            group,
            ChecklistStatus::Fail,
            "Mission not uploaded",
            Some("Upload the mission"),
        );
    }
    if !plan.mission_verified {
        return item(
            ChecklistItemId::MissionReady,
            group,
            ChecklistStatus::Warn,
            "Mission not verified by read-back",
            Some("Verify the upload"),
        );
    }
    pass(
        ChecklistItemId::MissionReady,
        group,
        "Mission uploaded and verified",
    )
}

fn mode_item(snapshot: &TelemetrySnapshot, config: &SafetyConfig, now_ms: u64) -> ChecklistItem {
    let group = ChecklistGroup::System;
    let Some(hb) = snapshot.heartbeat.as_ref() else {
        return item(
            ChecklistItemId::FlightModeReady,
            group,
            ChecklistStatus::Unknown,
            "No flight controller state yet",
            None,
        );
    };
    if fresh(
        snapshot.field_ages.heartbeat_at_ms,
        now_ms,
        config.stale_after_ms,
    ) != Some(true)
    {
        return item(
            ChecklistItemId::FlightModeReady,
            group,
            ChecklistStatus::Unknown,
            "Flight controller state is stale",
            None,
        );
    }
    use crate::telemetry::FlightState;
    match hb.flight_state {
        FlightState::Uninit => item(
            ChecklistItemId::FlightModeReady,
            group,
            ChecklistStatus::Fail,
            "Flight controller has not reported preflight readiness",
            Some("Check PX4 preflight messages and health checks"),
        ),
        FlightState::Boot | FlightState::Calibrating => item(
            ChecklistItemId::FlightModeReady,
            group,
            ChecklistStatus::Fail,
            "Flight controller is still booting or calibrating",
            Some("Wait for the flight controller to finish"),
        ),
        FlightState::Critical | FlightState::Emergency => item(
            ChecklistItemId::FlightModeReady,
            group,
            ChecklistStatus::Fail,
            "Flight controller reports a critical state",
            Some("Resolve the critical state first"),
        ),
        _ => pass(
            ChecklistItemId::FlightModeReady,
            group,
            &format!("Flight controller ready ({})", hb.mode_label),
        ),
    }
}

fn motor_test_item(plan: &PlanState, config: &SafetyConfig) -> ChecklistItem {
    let group = ChecklistGroup::System;
    if !config.require_motor_test_idle {
        return pass(
            ChecklistItemId::MotorTestIdle,
            group,
            "Motor test check disabled",
        );
    }
    if plan.motor_test_active {
        item(
            ChecklistItemId::MotorTestIdle,
            group,
            ChecklistStatus::Fail,
            "Motor test is running",
            Some("Stop the motor test"),
        )
    } else {
        pass(
            ChecklistItemId::MotorTestIdle,
            group,
            "Motor test not running",
        )
    }
}

/// Fixed reasons the operator may pick for a forced arm (L5).
///
/// A forced arm bypasses **only** the GCS-side checklist; PX4's own prearm
/// checks still apply (the GCS never sends the 21196 forced-arming code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ForceArmReason {
    /// On-site safety judgment override.
    SafetyOverride,
    /// Weather or conditions changed.
    Weather,
    /// A sensor data gap within budget.
    SensorTimeout,
    /// Mission or daylight time pressure.
    TimePressure,
}

/// A validated forced-arm request (L5). Carried from the UI; the backend only
/// sends the arm command when the physical gate passes, then records the
/// request into the flight log (`RecordEvent::ForceArm`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForceArmRequest {
    pub reason: ForceArmReason,
    pub note: Option<String>,
}

/// What the arm/takeoff gate says about the current checklist (L5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ArmGate {
    /// Every item passes: the command may be sent with one click.
    Allowed,
    /// `Warn` items need per-item operator confirmation before sending.
    RequiresAck { items: Vec<ChecklistItemId> },
    /// `Fail`/`Unknown` items block the normal path; a forced arm may bypass.
    Blocked { items: Vec<ChecklistItemId> },
    /// The heartbeat is not fresh: nothing can be sent, forced or not.
    NoLink,
}

/// Verdict of a forced-arm attempt (L5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ForceArmOutcome {
    /// The physical gate passed: send the arm command and record the request.
    Allowed,
    /// No fresh heartbeat: a forced arm is physically impossible.
    NoLink,
    /// The vehicle is already armed.
    AlreadyArmed,
    /// The checklist is already clear — no force is needed.
    NotNeeded,
}

/// The word the operator types to confirm a forced arm (mirrors L4's
/// `DISCONNECT`/`QUIT` keywords). Uppercase on purpose: the dialog compares it
/// verbatim so a stray Enter cannot confirm.
pub const FORCE_ARM_KEYWORD: &str = "FORCE ARM";
/// Decide whether a preflight-gated command (arm/takeoff) may be sent (L5).
///
/// Pure: `Allowed` only when every item passes; `Warn` items become
/// `RequiresAck`; any `Fail` or `Unknown` item becomes `Blocked`; a stale or
/// absent heartbeat is `NoLink` — the operator cannot send a command they
/// cannot deliver, so a force-arm is refused there too.
pub fn arm_gate(checklist: &Checklist) -> ArmGate {
    arm_gate_with_ack(checklist, &[])
}

/// Like [`arm_gate`], but treats the `Warn` items listed in `acked_warnings`
/// as already confirmed by the operator (the per-item acknowledgement, U5).
///
/// `Fail`/`Unknown` items and a stale link still block — an acknowledgement
/// never talks past those.
pub fn arm_gate_with_ack(checklist: &Checklist, acked_warnings: &[ChecklistItemId]) -> ArmGate {
    if checklist.get(ChecklistItemId::LinkFresh).map(|i| i.status) != Some(ChecklistStatus::Pass) {
        return ArmGate::NoLink;
    }
    let mut blocked = Vec::new();
    let mut warns = Vec::new();
    for item in &checklist.items {
        match item.status {
            ChecklistStatus::Fail | ChecklistStatus::Unknown => blocked.push(item.id),
            ChecklistStatus::Warn if acked_warnings.contains(&item.id) => {}
            ChecklistStatus::Warn => warns.push(item.id),
            ChecklistStatus::Pass => {}
        }
    }
    if !blocked.is_empty() {
        ArmGate::Blocked { items: blocked }
    } else if !warns.is_empty() {
        ArmGate::RequiresAck { items: warns }
    } else {
        ArmGate::Allowed
    }
}
/// Decide whether a forced arm may proceed (L5).
///
/// The physical gate never yields to a reason: a dead link and an already-armed
/// vehicle stay refused. `NotNeeded` means the normal (ungated) path already
/// allows arming — forcing would hide a clean checklist.
pub fn evaluate_force_arm(snapshot: &TelemetrySnapshot, checklist: &Checklist) -> ForceArmOutcome {
    if checklist.get(ChecklistItemId::LinkFresh).map(|i| i.status) != Some(ChecklistStatus::Pass) {
        return ForceArmOutcome::NoLink;
    }
    if snapshot
        .heartbeat
        .as_ref()
        .map(|h| h.base_mode.safety_armed)
        == Some(true)
    {
        return ForceArmOutcome::AlreadyArmed;
    }
    if arm_gate(checklist) == ArmGate::Allowed {
        return ForceArmOutcome::NotNeeded;
    }
    ForceArmOutcome::Allowed
}

/// Why a preflight-gated command was refused (L5), for the UI to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RefusalKind {
    /// `Fail`/`Unknown` items block the normal path.
    Blocked,
    /// `Warn` items need per-item confirmation.
    RequiresAck,
    /// No fresh heartbeat: nothing can be sent, forced or not.
    NoLink,
    /// The vehicle is already armed.
    AlreadyArmed,
    /// The checklist is already clear — no force is needed.
    NotNeeded,
}

/// Structured refusal of a preflight-gated command (L5). The UI renders the
/// failing items (status/detail/fix hint) instead of parsing a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreflightRefusal {
    pub kind: RefusalKind,
    /// The failing (`Blocked`) or warning (`RequiresAck`) items, with their
    /// one-sentence detail and fix hint.
    pub items: Vec<ChecklistItem>,
    /// One line for the status strip / alert.
    pub message: String,
}

impl PreflightRefusal {
    /// Build a refusal that carries the full items for the ids in `items`.
    pub fn new(
        kind: RefusalKind,
        checklist: &Checklist,
        ids: &[ChecklistItemId],
        message: &str,
    ) -> Self {
        Self {
            kind,
            items: checklist
                .items
                .iter()
                .filter(|i| ids.contains(&i.id))
                .cloned()
                .collect(),
            message: message.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::{Height, HeightDatum};
    use crate::telemetry::{
        BatteryStatus, ExtendedSysState, FlightState, GpsFixType, GpsRawInt, Heartbeat,
        HomePosition, LandedState, SensorHealthMasks, SysStatus, TelemetrySnapshot, VehicleType,
    };

    const NOW_MS: u64 = 100_000;

    /// A snapshot where every base item would pass.
    fn good() -> (TelemetrySnapshot, PlanState, SafetyConfig) {
        let snapshot = TelemetrySnapshot {
            last_heartbeat_at_ms: Some(NOW_MS),
            heartbeat: Some(Heartbeat {
                system_id: 1,
                component_id: 1,
                vehicle_type: VehicleType::Quadrotor,
                autopilot: crate::telemetry::Autopilot::Px4,
                base_mode: crate::telemetry::BaseMode::from_raw(0x80),
                custom_mode: 0,
                mode_label: "Hold".to_string(),
                flight_state: FlightState::Standby,
                mavlink_version: 3,
            }),
            global_position: None,
            attitude: None,
            sys_status: Some(SysStatus {
                sensors: SensorHealthMasks {
                    present: 0xFFFF,
                    enabled: 0xFFFF,
                    health: 0xFFFF,
                },
                battery_voltage_mv: 16_800,
                battery_current_ma: 0,
                battery_remaining_percent: 88,
            }),
            battery: None,
            gps: Some(GpsRawInt {
                fix_type: GpsFixType::Fix3d,
                satellites_visible: 12,
                latitude_deg: 48.6493,
                longitude_deg: -123.3982,
                altitude: Height::new(HeightDatum::AmslEgm96, 100.0),
                hdop: 0.8,
                vdop: 1.2,
                velocity_m_s: 0.0,
                course_over_ground_deg: 0.0,
            }),
            extended_sys_state: Some(ExtendedSysState {
                landed_state: LandedState::OnGround,
                vtol_state: crate::telemetry::VtolState::Undefined,
            }),
            home: Some(HomePosition {
                latitude_deg: 48.6493,
                longitude_deg: -123.3982,
                altitude: Height::new(HeightDatum::AmslEgm96, 100.0),
            }),
            estimator: None,
            vibration: None,
            distance_sensor: None,
            rc_channels: None,
            radio: None,
            field_ages: crate::telemetry::TelemetryFieldAges {
                heartbeat_at_ms: Some(NOW_MS),
                global_position_at_ms: None,
                attitude_at_ms: None,
                sys_status_at_ms: Some(NOW_MS),
                battery_at_ms: None,
                gps_at_ms: Some(NOW_MS),
                extended_sys_state_at_ms: Some(NOW_MS),
                home_at_ms: Some(NOW_MS),
                estimator_at_ms: None,
                vibration_at_ms: None,
                distance_sensor_at_ms: None,
                rc_channels_at_ms: None,
                radio_at_ms: None,
            },
        };
        let plan = PlanState {
            has_plan: true,
            mission_uploaded: true,
            mission_verified: true,
            motor_test_active: false,
            drape: None,
        };
        (snapshot, plan, SafetyConfig::default())
    }

    /// Fresh but "empty" snapshot (nothing ever arrived).
    fn blank() -> TelemetrySnapshot {
        TelemetrySnapshot::new()
    }

    #[test]
    fn a_complete_and_fresh_snapshot_reads_ready() {
        let (snap, plan, config) = good();
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(checklist.overall(), ChecklistStatus::Pass);
        assert!(checklist.is_ready());
        assert_eq!(checklist.pass_count(), checklist.items.len());
    }

    #[test]
    fn missing_or_stale_data_is_unknown_not_pass() {
        let (snap, plan, config) = good();
        // Fresh age but an empty snapshot: every data-driven item must be
        // Unknown (a missing field is not 0/false, and is never Pass). The two
        // app-state items are exceptions: a missing home and a missing plan are
        // *known* conditions with a fix, so they Fail rather than Pass.
        let checklist = evaluate(&blank(), &plan, &config, NOW_MS);
        assert_ne!(
            checklist.overall(),
            ChecklistStatus::Pass,
            "a blank snapshot must never read ready"
        );
        for item in &checklist.items {
            match item.id {
                // App-state / config-gated items: Pass is legitimately possible
                // without telemetry, so the data-driven rules do not apply.
                // The Drape group is app-state too — and with no drape facts
                // terrain mode is *not in use*, so Pass is the N/A answer.
                ChecklistItemId::MissionReady
                | ChecklistItemId::HomePosition
                | ChecklistItemId::MotorTestIdle
                | ChecklistItemId::Rtk
                | ChecklistItemId::DemCoverage
                | ChecklistItemId::DemHasDtm
                | ChecklistItemId::DemAge
                | ChecklistItemId::DrapeInfeasible
                | ChecklistItemId::DrapeItemCount
                | ChecklistItemId::DrapeClimbRate
                | ChecklistItemId::DrapeThresholds
                | ChecklistItemId::DrapeBatteryEta => {}
                _ => {
                    assert_ne!(
                        item.status,
                        ChecklistStatus::Pass,
                        "item {:?} must not Pass without data",
                        item.id
                    );
                    assert_eq!(
                        item.status,
                        ChecklistStatus::Unknown,
                        "item {:?} should be Unknown without data",
                        item.id
                    );
                }
            }
        }

        // Stale data is Unknown too.
        let mut stale = snap.clone();
        stale.field_ages.heartbeat_at_ms = Some(0);
        stale.field_ages.gps_at_ms = Some(0);
        stale.field_ages.sys_status_at_ms = Some(0);
        stale.field_ages.extended_sys_state_at_ms = Some(0);
        stale.field_ages.home_at_ms = Some(0);
        let checklist = evaluate(&stale, &plan, &config, NOW_MS);
        assert_eq!(checklist.overall(), ChecklistStatus::Unknown);
        assert!(!checklist.is_ready());
    }

    #[test]
    fn gps_failures_and_low_satellites() {
        let (mut snap, plan, config) = good();

        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::NoFix,
            ..snap.gps.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            checklist.get(ChecklistItemId::GpsFix).unwrap().status,
            ChecklistStatus::Fail
        );

        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::Fix2d,
            ..snap.gps.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            checklist.get(ChecklistItemId::GpsFix).unwrap().status,
            ChecklistStatus::Warn
        );

        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::Fix3d,
            satellites_visible: 3,
            ..snap.gps.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            checklist.get(ChecklistItemId::GpsFix).unwrap().status,
            ChecklistStatus::Warn,
            "a 3D fix with too few satellites is a Warn"
        );
    }

    #[test]
    fn rtk_is_skipped_when_not_required_and_strict_when_required() {
        let (mut snap, plan, config) = good();

        let skipped = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            skipped.get(ChecklistItemId::Rtk).unwrap().status,
            ChecklistStatus::Pass,
            "RTK is skipped when not required"
        );

        let mut strict = config.clone();
        strict.require_rtk = true;
        let not_fixed = evaluate(&snap, &plan, &strict, NOW_MS);
        assert_eq!(
            not_fixed.get(ChecklistItemId::Rtk).unwrap().status,
            ChecklistStatus::Fail,
            "a non-RTK fix fails when RTK is required"
        );

        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::RtkFloat,
            ..snap.gps.unwrap()
        });
        let floating = evaluate(&snap, &plan, &strict, NOW_MS);
        assert_eq!(
            floating.get(ChecklistItemId::Rtk).unwrap().status,
            ChecklistStatus::Warn
        );

        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::RtkFixed,
            ..snap.gps.unwrap()
        });
        let fixed = evaluate(&snap, &plan, &strict, NOW_MS);
        assert_eq!(
            fixed.get(ChecklistItemId::Rtk).unwrap().status,
            ChecklistStatus::Pass
        );
    }

    #[test]
    fn battery_thresholds_and_sources() {
        let (mut snap, plan, mut config) = good();

        // SYS_STATUS percent is the source when BATTERY_STATUS is absent.
        snap.sys_status = Some(SysStatus {
            battery_remaining_percent: 20,
            ..snap.sys_status.unwrap()
        });
        let warn = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            warn.get(ChecklistItemId::Battery).unwrap().status,
            ChecklistStatus::Warn
        );

        snap.sys_status = Some(SysStatus {
            battery_remaining_percent: 10,
            ..snap.sys_status.unwrap()
        });
        let fail = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            fail.get(ChecklistItemId::Battery).unwrap().status,
            ChecklistStatus::Fail
        );

        // BATTERY_STATUS wins over SYS_STATUS when both carry a percent.
        snap.battery = Some(BatteryStatus {
            battery_id: 0,
            voltage_cells_mv: vec![4100, 4090, 4080],
            current_ma: 1000,
            remaining_percent: Some(60),
            temperature_deg_c: Some(25),
        });
        snap.field_ages.battery_at_ms = Some(NOW_MS);
        config.critical_battery_percent = 30;
        let from_battery = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            from_battery.get(ChecklistItemId::Battery).unwrap().status,
            ChecklistStatus::Pass
        );
    }

    #[test]
    fn unhealthy_enabled_sensor_fails() {
        let (mut snap, plan, config) = good();
        snap.sys_status = Some(SysStatus {
            sensors: SensorHealthMasks {
                present: 0xFFFF,
                enabled: 0x0002, // 3D accelerometer
                health: 0x0000,
            },
            ..snap.sys_status.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            checklist.get(ChecklistItemId::SensorHealth).unwrap().status,
            ChecklistStatus::Fail
        );
        assert_eq!(
            checklist.get(ChecklistItemId::SensorHealth).unwrap().detail,
            "Sensor fault: 3D accelerometer",
            "the detail names the failing sensor"
        );
    }

    #[test]
    fn unhealthy_sensor_names_list_every_faulting_sensor() {
        // 3D gyro (0x01) + GPS (0x20) enabled but unhealthy; the healthy
        // magnetometer bit is enabled and healthy and must not leak in.
        let names = unhealthy_sensor_names(&SensorHealthMasks {
            present: 0xFFFF,
            enabled: 0x01 | 0x04 | 0x20,
            health: 0x04,
        });
        assert_eq!(names, vec!["3D gyro", "GPS"]);

        // An unhealthy bit we do not name yet still fails (safe by omission),
        // but reports the generic line rather than an empty detail.
        let unknown = unhealthy_sensor_names(&SensorHealthMasks {
            present: 0xFFFF,
            enabled: 0x8000_0000, // EXTENSION_USED
            health: 0,
        });
        assert!(unknown.is_empty());
    }

    #[test]
    fn rc_receiver_absence_is_a_warn_not_a_fail() {
        // SITL has no RC receiver: PX4 reports the RC receiver sensor as
        // unhealthy. That must be a confirmable Warn, never a hard Fail.
        let (mut snap, plan, config) = good();
        snap.sys_status = Some(SysStatus {
            sensors: SensorHealthMasks {
                present: 0xFFFF,
                enabled: 0x1_0000, // RC receiver
                health: 0x0,
            },
            ..snap.sys_status.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            checklist.get(ChecklistItemId::SensorHealth).unwrap().status,
            ChecklistStatus::Warn
        );
        assert_eq!(
            checklist.get(ChecklistItemId::SensorHealth).unwrap().detail,
            "RC receiver is not detected (expected in SITL)"
        );

        // A real sensor fault alongside a missing RC receiver is still a Fail.
        snap.sys_status = Some(SysStatus {
            sensors: SensorHealthMasks {
                present: 0xFFFF,
                enabled: 0x1_0000 | 0x02, // RC receiver + accelerometer
                health: 0x0,
            },
            ..snap.sys_status.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            checklist.get(ChecklistItemId::SensorHealth).unwrap().status,
            ChecklistStatus::Fail
        );
        assert_eq!(
            checklist.get(ChecklistItemId::SensorHealth).unwrap().detail,
            "Sensor fault: 3D accelerometer",
            "the hard fault names the sensor and ignores the optional RC bit"
        );
    }

    #[test]
    fn landed_state_matrix() {
        let (mut snap, plan, config) = good();
        for (state, expected) in [
            (LandedState::OnGround, ChecklistStatus::Pass),
            (LandedState::InAir, ChecklistStatus::Fail),
            (LandedState::Takeoff, ChecklistStatus::Warn),
            (LandedState::Landing, ChecklistStatus::Warn),
            (LandedState::Unknown, ChecklistStatus::Unknown),
        ] {
            snap.extended_sys_state = Some(ExtendedSysState {
                landed_state: state,
                vtol_state: crate::telemetry::VtolState::Undefined,
            });
            let checklist = evaluate(&snap, &plan, &config, NOW_MS);
            assert_eq!(
                checklist
                    .get(ChecklistItemId::LandedOnGround)
                    .unwrap()
                    .status,
                expected,
                "landed state {state:?}"
            );
        }
    }

    #[test]
    fn mission_state_matrix() {
        // `require_mission = true` (explicit operator choice): the mission
        // sync state grades Pass/Warn/Fail as before. The new default
        // (`false`) passes unconditionally — operator decision 2026-10-18.
        let (snap, _plan, mut config) = good();
        config.require_mission = true;
        for p in [
            PlanState::default(),
            PlanState {
                has_plan: true,
                mission_uploaded: false,
                mission_verified: false,
                motor_test_active: false,
                drape: None,
            },
            PlanState {
                has_plan: true,
                mission_uploaded: true,
                mission_verified: false,
                motor_test_active: false,
                drape: None,
            },
            PlanState {
                has_plan: true,
                mission_uploaded: true,
                mission_verified: true,
                motor_test_active: false,
                drape: None,
            },
        ] {
            let expected = if !p.has_plan || (p.mission_uploaded && p.mission_verified) {
                ChecklistStatus::Pass
            } else if p.mission_uploaded && !p.mission_verified {
                ChecklistStatus::Warn
            } else {
                ChecklistStatus::Fail
            };
            let checklist = evaluate(&snap, &p, &config, NOW_MS);
            assert_eq!(
                checklist.get(ChecklistItemId::MissionReady).unwrap().status,
                expected,
                "plan {p:?}"
            );
        }
    }

    #[test]
    fn no_mission_does_not_block_standalone_arming() {
        let (snapshot, _, config) = good();
        let checklist = evaluate(&snapshot, &PlanState::default(), &config, NOW_MS);
        assert_eq!(
            checklist
                .get(ChecklistItemId::MissionReady)
                .map(|item| item.status),
            Some(ChecklistStatus::Pass)
        );
        assert_eq!(arm_gate(&checklist), ArmGate::Allowed);
    }

    #[test]
    fn motor_test_running_blocks_arm() {
        let (snap, mut plan, config) = good();
        plan.motor_test_active = true;
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            checklist
                .get(ChecklistItemId::MotorTestIdle)
                .unwrap()
                .status,
            ChecklistStatus::Fail
        );
        assert!(!checklist.is_ready());
    }

    #[test]
    fn fc_not_ready_states_fail_with_accurate_details() {
        let (mut snap, plan, config) = good();
        for (state, expected_detail) in [
            (
                FlightState::Uninit,
                "Flight controller has not reported preflight readiness",
            ),
            (
                FlightState::Boot,
                "Flight controller is still booting or calibrating",
            ),
            (
                FlightState::Calibrating,
                "Flight controller is still booting or calibrating",
            ),
            (
                FlightState::Emergency,
                "Flight controller reports a critical state",
            ),
        ] {
            snap.heartbeat = Some(Heartbeat {
                flight_state: state,
                ..snap.heartbeat.unwrap()
            });
            let checklist = evaluate(&snap, &plan, &config, NOW_MS);
            let item = checklist.get(ChecklistItemId::FlightModeReady).unwrap();
            assert_eq!(item.status, ChecklistStatus::Fail, "flight state {state:?}");
            assert_eq!(item.detail, expected_detail, "flight state {state:?}");
        }
    }

    #[test]
    fn warn_overall_when_any_warn() {
        let (mut snap, plan, config) = good();
        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::Fix2d,
            ..snap.gps.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(checklist.overall(), ChecklistStatus::Warn);
        assert!(!checklist.is_ready());
    }

    #[test]
    fn unknown_outranks_warn() {
        // A Warn plus a field that stopped updating must read Unknown, not
        // the more reassuring Warn.
        let (mut snap, plan, config) = good();
        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::Fix2d,
            ..snap.gps.unwrap()
        });
        snap.field_ages.heartbeat_at_ms = Some(0);
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(checklist.overall(), ChecklistStatus::Unknown);
    }

    #[test]
    fn require_toggles_skip_items() {
        let (mut snap, plan, mut config) = good();
        config.require_home = false;
        config.require_mission = false;
        config.require_on_ground = false;
        config.require_motor_test_idle = false;
        snap.home = None;
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        for id in [
            ChecklistItemId::HomePosition,
            ChecklistItemId::MissionReady,
            ChecklistItemId::LandedOnGround,
            ChecklistItemId::MotorTestIdle,
        ] {
            assert_eq!(
                checklist.get(id).unwrap().status,
                ChecklistStatus::Pass,
                "toggled-off item {id:?} reports Pass"
            );
        }
    }

    #[test]
    fn arm_gate_allows_only_when_everything_passes() {
        let (snap, plan, config) = good();
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(arm_gate(&checklist), ArmGate::Allowed);
    }

    #[test]
    fn arm_gate_requires_ack_on_warnings() {
        let (mut snap, plan, config) = good();
        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::Fix2d,
            ..snap.gps.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        match arm_gate(&checklist) {
            ArmGate::RequiresAck { items } => {
                assert!(items.contains(&ChecklistItemId::GpsFix));
            }
            other => panic!("expected RequiresAck, got {other:?}"),
        }
    }

    #[test]
    fn arm_gate_with_ack_passes_acknowledged_warnings() {
        // The operator confirmed every Warn item in the panel (U5): the gate
        // must then allow the arm instead of re-refusing the same warnings.
        let (mut snap, plan, config) = good();
        snap.gps = Some(GpsRawInt {
            fix_type: GpsFixType::Fix2d,
            ..snap.gps.unwrap()
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            arm_gate_with_ack(&checklist, &[ChecklistItemId::GpsFix]),
            ArmGate::Allowed,
            "an acknowledged warning counts as Pass"
        );

        // An un-acknowledged warning still requires confirmation.
        assert_eq!(
            arm_gate_with_ack(&checklist, &[]),
            ArmGate::RequiresAck {
                items: vec![ChecklistItemId::GpsFix]
            }
        );

        // An acknowledgement never talks past a Fail item.
        snap.extended_sys_state = Some(ExtendedSysState {
            landed_state: LandedState::InAir,
            vtol_state: crate::telemetry::VtolState::Undefined,
        });
        let blocked = evaluate(&snap, &plan, &config, NOW_MS);
        assert!(
            matches!(
                arm_gate_with_ack(&blocked, &[ChecklistItemId::GpsFix]),
                ArmGate::Blocked { .. }
            ),
            "a Fail item cannot be acknowledged away"
        );
    }

    #[test]
    fn arm_gate_blocks_on_fail_or_unknown() {
        let (mut snap, plan, config) = good();
        snap.extended_sys_state = Some(ExtendedSysState {
            landed_state: LandedState::InAir,
            vtol_state: crate::telemetry::VtolState::Undefined,
        });
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        match arm_gate(&checklist) {
            ArmGate::Blocked { items } => {
                assert!(items.contains(&ChecklistItemId::LandedOnGround));
            }
            other => panic!("expected Blocked, got {other:?}"),
        }

        // A missing sensor (Unknown) blocks too, never Reads as a warning.
        let (mut snap, plan, config) = good();
        snap.sys_status = None;
        snap.field_ages.sys_status_at_ms = None;
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert!(
            matches!(arm_gate(&checklist), ArmGate::Blocked { .. }),
            "an unknown sensor health blocks, not warns"
        );
    }

    #[test]
    fn arm_gate_no_link_when_heartbeat_is_stale() {
        let (mut snap, plan, config) = good();
        snap.field_ages.heartbeat_at_ms = Some(0);
        let checklist = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(arm_gate(&checklist), ArmGate::NoLink);
    }

    #[test]
    fn force_arm_never_bypasses_the_physical_gate() {
        let (mut snap, plan, config) = good();
        // Dead link: NoLink even though a force request is implied.
        snap.field_ages.heartbeat_at_ms = Some(0);
        let stale = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            evaluate_force_arm(&snap, &stale),
            ForceArmOutcome::NoLink,
            "no heartbeat → a forced arm is physically impossible"
        );

        // Already armed: refused.
        snap.field_ages.heartbeat_at_ms = Some(NOW_MS);
        snap.heartbeat = Some(Heartbeat {
            base_mode: crate::telemetry::BaseMode::from_raw(0x80),
            ..snap.heartbeat.unwrap()
        });
        let armed = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            evaluate_force_arm(&snap, &armed),
            ForceArmOutcome::AlreadyArmed
        );
    }

    #[test]
    fn force_arm_skips_a_clean_checklist() {
        let (mut snap, plan, config) = good();
        // Un-arm the fixture vehicle: the good snapshot is otherwise armed.
        snap.heartbeat = Some(Heartbeat {
            base_mode: crate::telemetry::BaseMode::from_raw(0x00),
            ..snap.heartbeat.unwrap()
        });
        let clean = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            evaluate_force_arm(&snap, &clean),
            ForceArmOutcome::NotNeeded,
            "a clean checklist needs no force"
        );
    }

    #[test]
    fn force_arm_allows_a_blocked_checklist_on_a_live_link() {
        let (mut snap, plan, config) = good();
        snap.heartbeat = Some(Heartbeat {
            base_mode: crate::telemetry::BaseMode::from_raw(0x00),
            ..snap.heartbeat.unwrap()
        });
        snap.extended_sys_state = Some(ExtendedSysState {
            landed_state: LandedState::InAir,
            vtol_state: crate::telemetry::VtolState::Undefined,
        });
        let blocked = evaluate(&snap, &plan, &config, NOW_MS);
        assert_eq!(
            evaluate_force_arm(&snap, &blocked),
            ForceArmOutcome::Allowed,
            "a blocked checklist with a live link and an un-armed vehicle is force-armable"
        );
    }

    #[test]
    fn force_arm_keyword_is_constant() {
        assert_eq!(FORCE_ARM_KEYWORD, "FORCE ARM");
    }

    /// DR4: every Drape item grades four ways — Pass / Warn / Fail / Unknown
    /// — from the facts alone (table-driven, one row per state).
    #[test]
    fn the_drape_group_grades_from_the_facts() {
        let base = DrapeCheckFacts {
            dem_covers_route: Some(true),
            has_dtm: true,
            dem_age_ok: Some(true),
            infeasible_count: 0,
            infeasible_confirmed: false,
            waypoint_count: 60,
            waypoint_limit: 200,
            max_climb_needed_mps: Some(2.0),
            fc_max_climb_mps: Some(3.0),
            battery_minutes: Some(40.0),
            eta_minutes: Some(20.0),
        };
        let plan_with = |facts: DrapeCheckFacts| PlanState {
            has_plan: true,
            drape: Some(facts),
            ..PlanState::default()
        };
        let status_of = |id: ChecklistItemId, facts: DrapeCheckFacts| {
            evaluate_with_template(
                &TelemetrySnapshot::new(),
                &plan_with(facts),
                &SafetyConfig::default(),
                1_000,
                &ChecklistTemplate::baseline(),
            )
            .items
            .into_iter()
            .find(|i| i.id == id)
            .expect("item in the baseline")
            .status
        };

        // Pass: the healthy baseline grades Pass on every drape item.
        for id in [
            ChecklistItemId::DemCoverage,
            ChecklistItemId::DemHasDtm,
            ChecklistItemId::DemAge,
            ChecklistItemId::DrapeInfeasible,
            ChecklistItemId::DrapeItemCount,
            ChecklistItemId::DrapeClimbRate,
            ChecklistItemId::DrapeThresholds,
            ChecklistItemId::DrapeBatteryEta,
        ] {
            assert_eq!(
                status_of(id, base),
                ChecklistStatus::Pass,
                "{id:?} baseline"
            );
        }

        // Fail rows: the blocking conditions.
        let mut uncovered = base;
        uncovered.dem_covers_route = Some(false);
        assert_eq!(
            status_of(ChecklistItemId::DemCoverage, uncovered),
            ChecklistStatus::Fail
        );
        let mut spans = base;
        spans.infeasible_count = 2;
        assert_eq!(
            status_of(ChecklistItemId::DrapeInfeasible, spans),
            ChecklistStatus::Fail
        );
        // ...and confirmed spans pass.
        spans.infeasible_confirmed = true;
        assert_eq!(
            status_of(ChecklistItemId::DrapeInfeasible, spans),
            ChecklistStatus::Pass
        );
        let mut over = base;
        over.waypoint_count = over.waypoint_limit + 1;
        assert_eq!(
            status_of(ChecklistItemId::DrapeItemCount, over),
            ChecklistStatus::Fail
        );
        let mut steep = base;
        steep.max_climb_needed_mps = Some(4.0);
        assert_eq!(
            status_of(ChecklistItemId::DrapeClimbRate, steep),
            ChecklistStatus::Fail
        );
        let mut low_battery = base;
        low_battery.battery_minutes = Some(10.0);
        low_battery.eta_minutes = Some(20.0);
        // The UavMag template grades the battery item WarnOnly: the builder
        // computes Fail, the template downgrades it to an acknowledged Warn.
        assert_eq!(
            status_of(ChecklistItemId::DrapeBatteryEta, low_battery),
            ChecklistStatus::Warn
        );

        // Warn rows: advisory items (acknowledged, never blocking).
        let mut dtm_only = base;
        dtm_only.has_dtm = false;
        assert_eq!(
            status_of(ChecklistItemId::DemHasDtm, dtm_only),
            ChecklistStatus::Warn
        );
        let mut old = base;
        old.dem_age_ok = Some(false);
        assert_eq!(
            status_of(ChecklistItemId::DemAge, old),
            ChecklistStatus::Warn
        );
        let mut thin = base;
        thin.battery_minutes = Some(24.0);
        thin.eta_minutes = Some(20.0);
        assert_eq!(
            status_of(ChecklistItemId::DrapeBatteryEta, thin),
            ChecklistStatus::Warn
        );

        // Unknown rows: missing facts never read Pass.
        let mut no_cover = base;
        no_cover.dem_covers_route = None;
        assert_eq!(
            status_of(ChecklistItemId::DemCoverage, no_cover),
            ChecklistStatus::Unknown
        );
        let mut no_fc = base;
        no_fc.fc_max_climb_mps = None;
        assert_eq!(
            status_of(ChecklistItemId::DrapeClimbRate, no_fc),
            ChecklistStatus::Unknown
        );
        let mut no_age = base;
        no_age.dem_age_ok = None;
        assert_eq!(
            status_of(ChecklistItemId::DemAge, no_age),
            ChecklistStatus::Unknown
        );
        let mut no_battery = base;
        no_battery.battery_minutes = None;
        no_battery.eta_minutes = None;
        assert_eq!(
            status_of(ChecklistItemId::DrapeBatteryEta, no_battery),
            ChecklistStatus::Unknown
        );

        // An unachievable floor with a thin climb margin reads Warn, not Pass.
        let mut marginal = base;
        marginal.max_climb_needed_mps = Some(2.9);
        assert_eq!(
            status_of(ChecklistItemId::DrapeClimbRate, marginal),
            ChecklistStatus::Pass,
            "within the limit but close is still Pass — the band warns in the UI"
        );
    }
}
