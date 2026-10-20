//! Alert center (plan §3 A3): one place where every operator-facing alert is
//! deduplicated, throttled, latched and acknowledged.
//!
//! Pure logic — no I/O, no clock. The caller supplies the timestamp, so every
//! branch (dedup window, hysteresis, latch) is unit-testable and replayable
//! from a recorded session.
//!
//! ```text
//! raise(key)   ─ first, or first after the dedup window ─▶ Raised  (surfaces)
//!              ─ repeat inside the dedup window ────────▶ Repeated (count++)
//! level(false) ─ condition clear for < hysteresis ───────▶ (stays)
//!              ─ clear for ≥ hysteresis, not latched ───▶ Cleared
//! Critical     ─ stays until acknowledge(), whatever the level does ─▶ Acknowledged
//! ```
//!
//! Sources are the ones the plan names (STATUSTEXT, link, battery, GPS/RTK,
//! magnetometer, command failures, DEM, VIC/diurnal) plus a `System` bucket for
//! anything raised by the app itself; the enum is the single vocabulary the UI
//! filters on, so a new source is a one-line change here.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// How loudly an alert asks for attention. The derive order is the sort order
/// (`Info < Warn < Critical`), so `max()` is "the most severe".
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ts_rs::TS,
)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Critical,
}

/// Where an alert came from — the UI filters and groups on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AlertSource {
    /// A `STATUSTEXT` from the flight controller (L1).
    Statustext,
    /// MAVLink link state, framing errors, dropped frames (L3).
    Link,
    /// Battery voltage / remaining (L7).
    Battery,
    /// GNSS fix and satellite count (L7).
    Gps,
    /// RTK correction stream (R4).
    Rtk,
    /// Magnetometer threshold alarm.
    Mag,
    /// A command that the FC rejected or that never got acked.
    Command,
    /// Terrain data: missing coverage, drape failures.
    Dem,
    /// Diurnal (VIC) data source and disturbance levels (M7/M8).
    Diurnal,
    /// Raised by the app itself (config, storage, windows).
    System,
}

impl AlertSource {
    /// Stable name used in the alert id and in the event log.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Statustext => "statustext",
            Self::Link => "link",
            Self::Battery => "battery",
            Self::Gps => "gps",
            Self::Rtk => "rtk",
            Self::Mag => "mag",
            Self::Command => "command",
            Self::Dem => "dem",
            Self::Diurnal => "diurnal",
            Self::System => "system",
        }
    }
}

/// One alert, as the UI renders it. `acked` means "no longer needs the
/// operator's attention": set by [`AlertCenter::acknowledge`] for a latched
/// alert, or by [`AlertCenter::level`] when the condition cleared on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Alert {
    /// Stable key: `<source>|<dedup key>`. Raising the same key again updates
    /// this entry instead of adding a row.
    pub id: String,
    pub severity: Severity,
    pub source: AlertSource,
    pub text: String,
    /// Epoch milliseconds of the first occurrence.
    #[ts(type = "number")]
    pub first_ts_ms: i64,
    /// Epoch milliseconds of the most recent occurrence.
    #[ts(type = "number")]
    pub last_ts_ms: i64,
    /// How many times this key fired (repeats included).
    pub count: u32,
    /// Critical alerts latch: they stay visible until acknowledged.
    pub latched: bool,
    /// See the struct docs.
    pub acked: bool,
}

impl Alert {
    /// Whether this alert is still asking for the operator.
    pub fn needs_attention(&self) -> bool {
        !self.acked
    }
}

/// A new alert (or a repeat of an existing one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct AlertInput {
    pub source: AlertSource,
    pub severity: Severity,
    /// Operator-readable text. Protocol text (STATUSTEXT, FC rejection
    /// reasons) arrives already written; the UI localises its own strings.
    pub text: String,
    /// Dedup key override. Defaults to `source|text`, which is what most
    /// sources want: the same sentence is the same alert.
    pub key: Option<String>,
}

impl AlertInput {
    /// Convenience constructor for the common `source|text` key.
    pub fn new(source: AlertSource, severity: Severity, text: impl Into<String>) -> Self {
        Self {
            source,
            severity,
            text: text.into(),
            key: None,
        }
    }

    /// The key this input dedups on (also the alert id).
    pub fn id(&self) -> String {
        format!(
            "{}|{}",
            self.source.as_str(),
            self.key.as_deref().unwrap_or(&self.text)
        )
    }
}

/// What changed, for the caller to forward to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AlertEventKind {
    /// New alert, or a fresh occurrence outside the dedup window.
    Raised,
    /// Repeat inside the dedup window: only `count` and `last_ts_ms` moved.
    Repeated,
    /// The operator acknowledged it.
    Acknowledged,
    /// A level alert's condition cleared on its own.
    Cleared,
}

impl AlertEventKind {
    /// Whether this change should surface itself (banner, sound). A repeat
    /// inside the dedup window must not re-alert the operator.
    pub fn should_notify(self) -> bool {
        matches!(self, Self::Raised)
    }
}

/// One alert transition, carrying the alert's state *after* the transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct AlertEvent {
    pub kind: AlertEventKind,
    pub alert: Alert,
}

/// Repeats of the same key inside this window collapse into one alert.
pub const DEFAULT_DEDUP_WINDOW_MS: i64 = 30_000;
/// A level alert must be clear this long before it disappears.
pub const DEFAULT_CLEAR_HYSTERESIS_MS: i64 = 3_000;
/// Retained alerts (history for the event log drawer).
pub const DEFAULT_CAPACITY: usize = 200;

/// Tunables; A6's app config will override them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertConfig {
    pub dedup_window_ms: i64,
    pub clear_hysteresis_ms: i64,
    /// Critical alerts stay until acknowledged.
    pub latch_critical: bool,
    pub capacity: usize,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            dedup_window_ms: DEFAULT_DEDUP_WINDOW_MS,
            clear_hysteresis_ms: DEFAULT_CLEAR_HYSTERESIS_MS,
            latch_critical: true,
            capacity: DEFAULT_CAPACITY,
        }
    }
}

/// An alert plus state the UI never sees: when the condition first went false.
struct Entry {
    alert: Alert,
    clear_pending_since_ms: Option<i64>,
}

/// The alert center. Feed it alerts in, pull snapshots out.
pub struct AlertCenter {
    config: AlertConfig,
    entries: HashMap<String, Entry>,
}

impl Default for AlertCenter {
    fn default() -> Self {
        Self::new(AlertConfig::default())
    }
}

impl AlertCenter {
    pub fn new(config: AlertConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
        }
    }

    pub fn config(&self) -> AlertConfig {
        self.config
    }

    /// Raise (or repeat) an alert. One-shot events — a STATUSTEXT, a failed
    /// command, a DEM that will not load — use this; a condition that comes
    /// and goes uses [`AlertCenter::level`].
    pub fn raise(&mut self, input: AlertInput, now_ms: i64) -> AlertEvent {
        let id = input.id();
        let latch = self.config.latch_critical;
        let dedup_window_ms = self.config.dedup_window_ms;

        let event = match self.entries.get_mut(&id) {
            None => {
                let alert = Alert {
                    id: id.clone(),
                    severity: input.severity,
                    source: input.source,
                    text: input.text,
                    first_ts_ms: now_ms,
                    last_ts_ms: now_ms,
                    count: 1,
                    latched: latch && input.severity == Severity::Critical,
                    acked: false,
                };
                self.entries.insert(
                    id,
                    Entry {
                        alert: alert.clone(),
                        clear_pending_since_ms: None,
                    },
                );
                AlertEvent {
                    kind: AlertEventKind::Raised,
                    alert,
                }
            }
            Some(entry) => {
                let since_last = now_ms.saturating_sub(entry.alert.last_ts_ms);
                let escalated = input.severity > entry.alert.severity;
                let fresh = entry.alert.acked || escalated || since_last >= dedup_window_ms;

                entry.alert.severity = input.severity;
                entry.alert.text = input.text;
                entry.alert.last_ts_ms = now_ms;
                entry.alert.count = entry.alert.count.saturating_add(1);
                // Latching is sticky: a Critical that later repeats at a lower
                // severity must still wait for an acknowledgement.
                entry.alert.latched =
                    entry.alert.latched || (latch && input.severity == Severity::Critical);
                if fresh {
                    // A fresh occurrence needs the operator again, and the
                    // hysteresis clock restarts.
                    entry.alert.acked = false;
                    entry.clear_pending_since_ms = None;
                }
                AlertEvent {
                    kind: if fresh {
                        AlertEventKind::Raised
                    } else {
                        AlertEventKind::Repeated
                    },
                    alert: entry.alert.clone(),
                }
            }
        };

        self.evict_if_over_capacity();
        event
    }

    /// Raise while `active`, and take the alert away once the condition has
    /// been clear for the hysteresis period.
    ///
    /// Returns `None` when nothing changed: still active, still inside the
    /// hysteresis window, or latched and waiting for an acknowledgement.
    pub fn level(&mut self, input: AlertInput, active: bool, now_ms: i64) -> Option<AlertEvent> {
        let id = input.id();
        let hysteresis_ms = self.config.clear_hysteresis_ms;

        if active {
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.clear_pending_since_ms = None;
            }
            return Some(self.raise(input, now_ms));
        }

        let entry = self.entries.get_mut(&id)?;
        if entry.alert.latched || entry.alert.acked {
            // Latched: an acknowledgement is the only way out. Already acked:
            // nothing left to clear.
            return None;
        }

        let since = *entry.clear_pending_since_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) < hysteresis_ms {
            return None;
        }

        entry.alert.acked = true;
        entry.clear_pending_since_ms = None;
        Some(AlertEvent {
            kind: AlertEventKind::Cleared,
            alert: entry.alert.clone(),
        })
    }

    /// Acknowledge one alert. Idempotent: an unknown or already-acked id
    /// returns `None`.
    pub fn acknowledge(&mut self, id: &str) -> Option<AlertEvent> {
        let entry = self.entries.get_mut(id)?;
        if entry.alert.acked {
            return None;
        }
        entry.alert.acked = true;
        entry.clear_pending_since_ms = None;
        Some(AlertEvent {
            kind: AlertEventKind::Acknowledged,
            alert: entry.alert.clone(),
        })
    }

    /// Acknowledge everything the operator can see; one event per alert that
    /// changed.
    pub fn acknowledge_all(&mut self) -> Vec<AlertEvent> {
        let ids: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.alert.needs_attention())
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter().filter_map(|id| self.acknowledge(id)).collect()
    }

    /// Alerts that still need the operator (badge, banner).
    pub fn visible(&self) -> Vec<Alert> {
        let mut alerts: Vec<Alert> = self
            .entries
            .values()
            .filter(|e| e.alert.needs_attention())
            .map(|e| e.alert.clone())
            .collect();
        sort_newest_first(&mut alerts);
        alerts
    }

    /// Every retained alert, newest first (the drawer's history).
    pub fn snapshot(&self) -> Vec<Alert> {
        let mut alerts: Vec<Alert> = self.entries.values().map(|e| e.alert.clone()).collect();
        sort_newest_first(&mut alerts);
        alerts
    }

    /// How many alerts still need attention.
    pub fn visible_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.alert.needs_attention())
            .count()
    }

    /// The most severe alert still needing attention (badge colour).
    pub fn highest_severity(&self) -> Option<Severity> {
        self.entries
            .values()
            .filter(|e| e.alert.needs_attention())
            .map(|e| e.alert.severity)
            .max()
    }

    /// One alert by id, acknowledged or not.
    pub fn get(&self, id: &str) -> Option<&Alert> {
        self.entries.get(id).map(|e| &e.alert)
    }

    /// Drop acknowledged alerts (the drawer's "clear"). Returns how many went.
    pub fn clear_acknowledged(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, e| e.alert.needs_attention());
        before - self.entries.len()
    }

    /// Keep the history bounded, dropping what the operator is least likely to
    /// need: acknowledged alerts first, then unlatched ones, oldest first.
    fn evict_if_over_capacity(&mut self) {
        while self.entries.len() > self.config.capacity {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(id, e)| {
                    (
                        usize::from(!e.alert.acked),
                        usize::from(e.alert.latched),
                        e.alert.last_ts_ms,
                        (*id).clone(),
                    )
                })
                .map(|(id, _)| id.clone());
            match victim {
                Some(id) => {
                    self.entries.remove(&id);
                }
                None => break,
            }
        }
    }
}

fn sort_newest_first(alerts: &mut [Alert]) {
    alerts.sort_by(|a, b| {
        b.last_ts_ms
            .cmp(&a.last_ts_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(severity: Severity, text: &str) -> AlertInput {
        AlertInput::new(AlertSource::Battery, severity, text)
    }

    #[test]
    fn first_raise_notifies_and_creates_one_row() {
        let mut center = AlertCenter::default();
        let event = center.raise(input(Severity::Warn, "Battery low"), 1_000);

        assert_eq!(event.kind, AlertEventKind::Raised);
        assert!(event.kind.should_notify());
        assert_eq!(event.alert.count, 1);
        assert_eq!(event.alert.first_ts_ms, 1_000);
        assert_eq!(event.alert.last_ts_ms, 1_000);
        assert!(!event.alert.latched);
        assert_eq!(center.snapshot().len(), 1);
        assert_eq!(center.visible_count(), 1);
    }

    #[test]
    fn repeats_inside_the_window_count_instead_of_repeating() {
        let mut center = AlertCenter::default();
        center.raise(input(Severity::Warn, "Battery low"), 1_000);
        let second = center.raise(input(Severity::Warn, "Battery low"), 2_000);
        let third = center.raise(input(Severity::Warn, "Battery low"), 3_000);

        assert_eq!(second.kind, AlertEventKind::Repeated);
        assert!(!second.kind.should_notify());
        assert_eq!(third.alert.count, 3);
        assert_eq!(third.alert.first_ts_ms, 1_000, "first occurrence is kept");
        assert_eq!(third.alert.last_ts_ms, 3_000);
        assert_eq!(center.snapshot().len(), 1, "one row for one key");
    }

    #[test]
    fn a_repeat_after_the_window_is_a_new_occurrence() {
        let mut center = AlertCenter::default();
        center.raise(input(Severity::Warn, "Battery low"), 0);
        let late = center.raise(
            input(Severity::Warn, "Battery low"),
            DEFAULT_DEDUP_WINDOW_MS + 1,
        );

        assert_eq!(late.kind, AlertEventKind::Raised);
        assert!(late.kind.should_notify());
        assert_eq!(late.alert.count, 2);
    }

    #[test]
    fn distinct_text_is_a_distinct_alert() {
        let mut center = AlertCenter::default();
        center.raise(input(Severity::Warn, "Battery low"), 0);
        center.raise(input(Severity::Warn, "Battery critical"), 100);

        assert_eq!(center.visible_count(), 2);
        assert_eq!(center.highest_severity(), Some(Severity::Warn));
    }

    #[test]
    fn explicit_key_dedups_across_text_changes() {
        let mut center = AlertCenter::default();
        center.raise(
            AlertInput {
                key: Some("battery".into()),
                ..input(Severity::Warn, "Battery 21%")
            },
            0,
        );
        let second = center.raise(
            AlertInput {
                key: Some("battery".into()),
                ..input(Severity::Warn, "Battery 18%")
            },
            500,
        );

        assert_eq!(second.kind, AlertEventKind::Repeated);
        assert_eq!(second.alert.text, "Battery 18%", "latest text wins");
        assert_eq!(center.snapshot().len(), 1);
    }

    #[test]
    fn critical_latches_until_acknowledged() {
        let mut center = AlertCenter::default();
        let raised = center.raise(input(Severity::Critical, "GPS lost"), 0);

        assert!(raised.alert.latched);
        // The source reports the condition gone: a latched alert does not
        // clear on its own.
        assert!(center
            .level(input(Severity::Critical, "GPS lost"), false, 60_000)
            .is_none());
        assert_eq!(center.visible_count(), 1);

        let acked = center
            .acknowledge(&raised.alert.id)
            .expect("unknown alert id");
        assert_eq!(acked.kind, AlertEventKind::Acknowledged);
        assert!(acked.alert.acked);
        assert_eq!(center.visible_count(), 0);
        assert_eq!(center.highest_severity(), None);
        assert_eq!(center.snapshot().len(), 1, "history is kept");
    }

    #[test]
    fn acknowledging_is_idempotent_and_ignores_unknown_ids() {
        let mut center = AlertCenter::default();
        let raised = center.raise(input(Severity::Critical, "GPS lost"), 0);
        center.acknowledge(&raised.alert.id);
        assert!(center.acknowledge(&raised.alert.id).is_none());
        assert!(center.acknowledge("nope").is_none());
    }

    #[test]
    fn a_latched_alert_that_recurs_asks_again() {
        let mut center = AlertCenter::default();
        let raised = center.raise(input(Severity::Critical, "GPS lost"), 0);
        center.acknowledge(&raised.alert.id);
        let again = center.raise(
            input(Severity::Critical, "GPS lost"),
            DEFAULT_DEDUP_WINDOW_MS + 1,
        );

        assert_eq!(again.kind, AlertEventKind::Raised);
        assert!(!again.alert.acked, "needs attention again");
        assert!(again.alert.latched);
        assert_eq!(center.visible_count(), 1);
    }

    #[test]
    fn level_alerts_clear_only_after_the_hysteresis() {
        let mut center = AlertCenter::default();
        let cfg = center.config();
        let active = input(Severity::Warn, "Link quality low");

        let raised = center
            .level(active.clone(), true, 0)
            .expect("raised while active");
        assert_eq!(raised.kind, AlertEventKind::Raised);

        // The first clear sample only starts the clock.
        assert!(center.level(active.clone(), false, 10_000).is_none());
        assert!(center
            .level(active.clone(), false, 10_000 + cfg.clear_hysteresis_ms - 1)
            .is_none());
        assert_eq!(center.visible_count(), 1);

        let cleared = center
            .level(active.clone(), false, 10_000 + cfg.clear_hysteresis_ms)
            .expect("cleared after the hysteresis");
        assert_eq!(cleared.kind, AlertEventKind::Cleared);
        assert_eq!(center.visible_count(), 0);
        assert_eq!(center.snapshot().len(), 1, "history is kept");
    }

    #[test]
    fn a_level_alert_that_flaps_inside_the_hysteresis_stays() {
        let mut center = AlertCenter::default();
        let cfg = center.config();
        let active = input(Severity::Warn, "Link quality low");

        // active → clear → active again: the first clear never completes, so
        // the alert stays and its clock restarts at the next false sample.
        center.level(active.clone(), true, 0);
        center.level(active.clone(), false, 1_000);
        center.level(active.clone(), true, 2_000);
        assert_eq!(center.visible_count(), 1);

        let first_false = 3_000;
        assert!(center.level(active.clone(), false, first_false).is_none());
        assert!(center
            .level(
                active.clone(),
                false,
                first_false + cfg.clear_hysteresis_ms - 1
            )
            .is_none());
        assert_eq!(center.visible_count(), 1, "no flap, no premature clear");

        assert!(center
            .level(active.clone(), false, first_false + cfg.clear_hysteresis_ms)
            .is_some());
        assert_eq!(center.visible_count(), 0);
    }

    #[test]
    fn severity_escalation_re_notifies_the_operator() {
        let mut center = AlertCenter::default();
        let key = Some("battery".to_string());
        center.raise(
            AlertInput {
                key: key.clone(),
                ..input(Severity::Warn, "Battery low")
            },
            0,
        );
        let escalated = center.raise(
            AlertInput {
                key,
                ..input(Severity::Critical, "Battery critical")
            },
            100,
        );

        assert_eq!(escalated.kind, AlertEventKind::Raised);
        assert_eq!(escalated.alert.severity, Severity::Critical);
        assert!(escalated.alert.latched);
        assert_eq!(escalated.alert.count, 2);
    }

    #[test]
    fn a_latched_critical_stays_latched_after_a_lower_severity_repeat() {
        let mut center = AlertCenter::default();
        let key = Some("battery".to_string());
        let critical = center.raise(
            AlertInput {
                key: key.clone(),
                ..input(Severity::Critical, "Battery critical")
            },
            0,
        );
        assert!(critical.alert.latched);

        let repeated = center.raise(
            AlertInput {
                key,
                ..input(Severity::Warn, "Battery low")
            },
            100,
        );

        assert_eq!(repeated.alert.severity, Severity::Warn);
        assert!(
            repeated.alert.latched,
            "a lower-severity repeat must not bypass the acknowledgement"
        );
        assert!(center.acknowledge(&critical.alert.id).is_some());
    }

    #[test]
    fn snapshot_is_newest_first_and_highest_severity_wins() {
        let mut center = AlertCenter::default();
        center.raise(input(Severity::Info, "first"), 100);
        center.raise(input(Severity::Warn, "second"), 200);
        center.raise(input(Severity::Critical, "third"), 300);

        let snap = center.snapshot();
        let texts: Vec<&str> = snap.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(texts, vec!["third", "second", "first"]);
        assert_eq!(center.highest_severity(), Some(Severity::Critical));
    }

    #[test]
    fn acknowledge_all_clears_the_visible_set() {
        let mut center = AlertCenter::default();
        center.raise(input(Severity::Warn, "a"), 0);
        center.raise(input(Severity::Warn, "b"), 1);
        center.raise(input(Severity::Critical, "c"), 2);

        let events = center.acknowledge_all();
        assert_eq!(events.len(), 3);
        assert!(events
            .iter()
            .all(|e| e.kind == AlertEventKind::Acknowledged));
        assert_eq!(center.visible_count(), 0);
        assert!(center.acknowledge_all().is_empty(), "idempotent");
    }

    #[test]
    fn capacity_drops_acknowledged_alerts_first() {
        let mut center = AlertCenter::new(AlertConfig {
            capacity: 2,
            ..AlertConfig::default()
        });
        let a = center.raise(input(Severity::Warn, "a"), 0);
        center.acknowledge(&a.alert.id);
        center.raise(input(Severity::Warn, "b"), 1);
        center.raise(input(Severity::Warn, "c"), 2);

        let snap = center.snapshot();
        let texts: Vec<&str> = snap.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(texts, vec!["c", "b"], "the acknowledged one went first");
    }

    #[test]
    fn capacity_never_drops_a_latched_alert_before_an_unlatched_one() {
        let mut center = AlertCenter::new(AlertConfig {
            capacity: 2,
            ..AlertConfig::default()
        });
        let critical = center.raise(input(Severity::Critical, "latched"), 0);
        center.raise(input(Severity::Warn, "old warn"), 1);
        center.raise(input(Severity::Warn, "new warn"), 2);

        let snap = center.snapshot();
        let ids: Vec<&str> = snap.iter().map(|a| a.id.as_str()).collect();
        assert!(ids.contains(&critical.alert.id.as_str()));
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn clear_acknowledged_keeps_only_what_needs_attention() {
        let mut center = AlertCenter::default();
        let a = center.raise(input(Severity::Warn, "a"), 0);
        center.raise(input(Severity::Warn, "b"), 1);
        center.acknowledge(&a.alert.id);

        assert_eq!(center.clear_acknowledged(), 1);
        assert_eq!(center.snapshot().len(), 1);
    }

    #[test]
    fn alert_ids_are_stable_and_source_scoped() {
        let battery = AlertInput::new(AlertSource::Battery, Severity::Warn, "low");
        let gps = AlertInput::new(AlertSource::Gps, Severity::Warn, "low");

        assert_eq!(battery.id(), "battery|low");
        assert_eq!(gps.id(), "gps|low");

        let mut center = AlertCenter::default();
        center.raise(battery, 0);
        center.raise(gps, 0);
        assert_eq!(center.visible_count(), 2);
    }
}
