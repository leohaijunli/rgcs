//! Signal catalog: the live `(msg, field)` table the inspector's signal tree is
//! built from (plan §4). Tracks the last value, an EMA arrival rate, and the
//! last-seen host time for each signal.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::SignalId;

/// One row of the signal tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CatalogEntry {
    pub signal: SignalId,
    /// MAVLink message name (`ATTITUDE`, `HIGHRES_IMU`, …), from
    /// `mavlink_core::Message::message_name`. Lets the signal tree group and
    /// label without a hand-maintained id → name map.
    pub message_name: String,
    pub last_value: f64,
    /// EMA of the arrival rate, Hz.
    pub rate_hz: f64,
    /// Host time of the last sample, ms.
    pub last_seen_ms: f64,
}

/// EMA smoothing for the rate estimate.
const RATE_ALPHA: f64 = 0.2;
/// Minimum observed interval used to clamp the instantaneous rate.
const MIN_DT_S: f64 = 1e-6;

#[derive(Debug)]
struct EntryState {
    message_name: String,
    last_value: f64,
    rate_hz: f64,
    last_at: Option<Instant>,
}

/// An append-only catalog of every field seen on the link.
#[derive(Debug, Default)]
pub struct SignalCatalog {
    entries: HashMap<SignalId, EntryState>,
}

impl SignalCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a sample; updates the message name, the last value and the EMA
    /// rate.
    pub fn observe(&mut self, id: &SignalId, message_name: &str, value: f64, now: Instant) {
        let e = self.entries.entry(id.clone()).or_insert(EntryState {
            message_name: message_name.to_string(),
            last_value: 0.0,
            rate_hz: 0.0,
            last_at: None,
        });
        if e.message_name != message_name {
            e.message_name = message_name.to_string();
        }
        if value.is_finite() {
            e.last_value = value;
        }
        match e.last_at {
            None => e.rate_hz = 0.0,
            Some(at) => {
                let dt = now.duration_since(at).as_secs_f64().max(MIN_DT_S);
                let inst = 1.0 / dt;
                e.rate_hz = if e.rate_hz == 0.0 {
                    inst
                } else {
                    RATE_ALPHA * inst + (1.0 - RATE_ALPHA) * e.rate_hz
                };
            }
        }
        e.last_at = Some(now);
    }

    /// The current table, sorted by message id then field for stable output.
    pub fn snapshot(&self, now: Instant) -> Vec<CatalogEntry> {
        let mut rows: Vec<CatalogEntry> = self
            .entries
            .iter()
            .map(|(id, s)| CatalogEntry {
                signal: id.clone(),
                message_name: s.message_name.clone(),
                last_value: s.last_value,
                rate_hz: s.rate_hz,
                last_seen_ms: now.duration_since(s.last_at.unwrap_or(now)).as_secs_f64() * 1000.0,
            })
            .collect();
        rows.sort_by(|a, b| {
            (a.signal.message_id, &a.signal.field).cmp(&(b.signal.message_id, &b.signal.field))
        });
        rows
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observes_last_value_and_estimates_rate() {
        let mut cat = SignalCatalog::new();
        let id = SignalId::new(1, 1, 30, "roll");
        let t0 = Instant::now();
        cat.observe(&id, "ATTITUDE", 0.1, t0);
        assert_eq!(cat.len(), 1);
        cat.observe(&id, "ATTITUDE", 0.2, t0 + std::time::Duration::from_millis(10));
        cat.observe(&id, "ATTITUDE", 0.3, t0 + std::time::Duration::from_millis(20));
        let rows = cat.snapshot(Instant::now());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message_name, "ATTITUDE");
        assert!((rows[0].last_value - 0.3).abs() < 1e-9, "last value");
        assert!(
            rows[0].rate_hz > 50.0 && rows[0].rate_hz < 150.0,
            "rate {}",
            rows[0].rate_hz
        );
    }

    #[test]
    fn snapshot_is_sorted_by_message_then_field() {
        let mut cat = SignalCatalog::new();
        let t = Instant::now();
        cat.observe(&SignalId::new(1, 1, 105, "zmag"), "HIGHRES_IMU", 1.0, t);
        cat.observe(&SignalId::new(1, 1, 30, "roll"), "ATTITUDE", 1.0, t);
        cat.observe(&SignalId::new(1, 1, 30, "pitch"), "ATTITUDE", 1.0, t);
        let rows = cat.snapshot(t);
        let ids: Vec<&str> = rows.iter().map(|r| r.signal.field.as_str()).collect();
        assert_eq!(ids, vec!["pitch", "roll", "zmag"], "sorted");
    }

    #[test]
    fn catalog_entry_with_nan_serializes_as_null_not_a_panic() {
        // serde_json writes a non-finite float as `null`; the blackscreen bug
        // was the frontend choking on that. Pin the wire shape here (plan S5).
        let entry = CatalogEntry {
            signal: SignalId::new(1, 1, 30, "roll"),
            message_name: "ATTITUDE".into(),
            last_value: f64::NAN,
            rate_hz: 0.0,
            last_seen_ms: 0.0,
        };
        let json = serde_json::to_string(&entry).expect("serializes without failing");
        assert!(json.contains("\"last_value\":null"), "{json}");
        assert!(!json.contains("NaN"), "{json}");
    }
}
