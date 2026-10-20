//! Graded battery, GNSS and link alerts (L7).
//!
//! Pure apart from the [`AlertCenter`] it drives: the caller supplies the
//! snapshot and timestamp. Each condition is fed through
//! [`AlertCenter::level`], so the center keeps its dedup window, clear
//! hysteresis and Critical latch. A condition only calls `level(active=true)`
//! when it first appears or changes severity; inactive ticks are still fed so
//! the hysteresis clock can expire without inflating the alert count.

use std::collections::{HashMap, HashSet};

use crate::alerts::{AlertCenter, AlertEvent, AlertEventKind, AlertInput, AlertSource, Severity};
use crate::safety::SafetyConfig;
use crate::telemetry::{GpsFixType, TelemetrySnapshot};

/// Stable condition keys. They double as alert ids (`source|key`).
pub const BATTERY_PERCENT_KEY: &str = "battery.percent";
pub const BATTERY_VOLTAGE_KEY: &str = "battery.voltage";
pub const GPS_FIX_KEY: &str = "gps.fix";
pub const GPS_SATELLITES_KEY: &str = "gps.satellites";
pub const LINK_RSSI_KEY: &str = "link.rssi";
pub const LINK_TXBUF_KEY: &str = "link.txbuf";
pub const LINK_PACKET_LOSS_KEY: &str = "link.packet_loss";

/// One evaluated condition. `active=false` is still returned every tick so a
/// previous alert can clear after the center's hysteresis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReading {
    pub key: &'static str,
    pub source: AlertSource,
    pub severity: Severity,
    pub active: bool,
    pub text: String,
}

impl HealthReading {
    fn inactive(key: &'static str, source: AlertSource, text: &str) -> Self {
        Self {
            key,
            source,
            severity: Severity::Info,
            active: false,
            text: text.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RadioSample {
    rxerrors: u16,
    fixed: u16,
}

/// Stateful evaluator: only the RF error counters need history, to turn two
/// cumulative samples into a packet-loss percentage.
#[derive(Debug, Default)]
pub struct HealthMonitor {
    radio_prev: Option<RadioSample>,
    active: HashMap<&'static str, Severity>,
    clearing: HashSet<&'static str>,
}

impl HealthMonitor {
    /// Feed one snapshot to the alert center and return the transitions that
    /// actually happened. Safe to call at the snapshot rate (20 Hz).
    pub fn apply(
        &mut self,
        center: &mut AlertCenter,
        snapshot: &TelemetrySnapshot,
        config: &SafetyConfig,
        now_ms: i64,
    ) -> Vec<AlertEvent> {
        let readings = self.readings(snapshot, config, now_ms);
        let mut events = Vec::new();
        for reading in readings {
            let input = AlertInput {
                source: reading.source,
                severity: reading.severity,
                text: reading.text,
                key: Some(reading.key.to_string()),
            };
            if reading.active {
                self.clearing.remove(reading.key);
                let changed = self.active.get(reading.key) != Some(&reading.severity);
                self.active.insert(reading.key, reading.severity);
                if changed {
                    events.extend(center.level(input, true, now_ms));
                }
                continue;
            }
            if self.active.remove(reading.key).is_some() {
                self.clearing.insert(reading.key);
            }
            if self.clearing.contains(reading.key) {
                if let Some(event) = center.level(input, false, now_ms) {
                    if event.kind == AlertEventKind::Cleared {
                        self.clearing.remove(reading.key);
                    }
                    events.push(event);
                }
            }
        }
        events
    }

    /// Evaluate every condition without touching the alert center.
    pub fn readings(
        &mut self,
        snapshot: &TelemetrySnapshot,
        config: &SafetyConfig,
        now_ms: i64,
    ) -> Vec<HealthReading> {
        let stale = config.stale_after_ms;
        vec![
            battery_percent(snapshot, config, now_ms, stale),
            battery_voltage(snapshot, config, now_ms, stale),
            gps_fix(snapshot, now_ms, stale),
            gps_satellites(snapshot, config, now_ms, stale),
            link_rssi(snapshot, config, now_ms, stale),
            link_txbuf(snapshot, config, now_ms, stale),
            self.link_packet_loss(snapshot, config, now_ms, stale),
        ]
    }

    fn link_packet_loss(
        &mut self,
        snapshot: &TelemetrySnapshot,
        config: &SafetyConfig,
        now_ms: i64,
        stale: u64,
    ) -> HealthReading {
        const KEY: &str = LINK_PACKET_LOSS_KEY;
        let Some(radio) = snapshot
            .radio
            .as_ref()
            .filter(|_| fresh(snapshot.field_ages.radio_at_ms, now_ms, stale))
        else {
            self.radio_prev = None;
            return HealthReading::inactive(
                KEY,
                AlertSource::Link,
                "Telemetry packet loss unknown",
            );
        };
        let sample = RadioSample {
            rxerrors: radio.rxerrors,
            fixed: radio.fixed,
        };
        let previous = self.radio_prev.replace(sample);
        let Some(previous) = previous else {
            return HealthReading::inactive(
                KEY,
                AlertSource::Link,
                "Telemetry packet loss measured",
            );
        };
        if sample.rxerrors < previous.rxerrors || sample.fixed < previous.fixed {
            // Counter reset (link swap or radio reboot): start over.
            return HealthReading::inactive(
                KEY,
                AlertSource::Link,
                "Telemetry packet loss measured",
            );
        }
        let bad = u32::from(sample.rxerrors - previous.rxerrors);
        let good = u32::from(sample.fixed - previous.fixed);
        let total = bad + good;
        if total == 0 {
            return HealthReading::inactive(
                KEY,
                AlertSource::Link,
                "Telemetry packet loss measured",
            );
        }
        let loss = bad.saturating_mul(100) / total;
        let severity = if loss > u32::from(config.critical_packet_loss_percent) {
            Some(Severity::Critical)
        } else if loss > u32::from(config.warn_packet_loss_percent) {
            Some(Severity::Warn)
        } else {
            None
        };
        match severity {
            Some(Severity::Critical) => HealthReading {
                key: KEY,
                source: AlertSource::Link,
                severity: Severity::Critical,
                active: true,
                text: format!(
                    "Telemetry packet loss {loss}% (critical above {}%)",
                    config.critical_packet_loss_percent
                ),
            },
            Some(Severity::Warn) => HealthReading {
                key: KEY,
                source: AlertSource::Link,
                severity: Severity::Warn,
                active: true,
                text: format!(
                    "Telemetry packet loss {loss}% (warning above {}%)",
                    config.warn_packet_loss_percent
                ),
            },
            _ => HealthReading::inactive(KEY, AlertSource::Link, "Telemetry packet loss healthy"),
        }
    }
}

fn fresh(at_ms: Option<u64>, now_ms: i64, stale_after_ms: u64) -> bool {
    let Some(at_ms) = at_ms else {
        return false;
    };
    let now = u64::try_from(now_ms).unwrap_or(0);
    now.saturating_sub(at_ms) <= stale_after_ms
}

fn battery_percent(
    snapshot: &TelemetrySnapshot,
    config: &SafetyConfig,
    now_ms: i64,
    stale: u64,
) -> HealthReading {
    const KEY: &str = BATTERY_PERCENT_KEY;
    let battery_fresh = fresh(snapshot.field_ages.battery_at_ms, now_ms, stale);
    let sys_fresh = fresh(snapshot.field_ages.sys_status_at_ms, now_ms, stale);
    let percent = snapshot
        .battery
        .as_ref()
        .filter(|_| battery_fresh)
        .and_then(|b| b.remaining_percent)
        .map(i16::from)
        .or_else(|| {
            snapshot
                .sys_status
                .as_ref()
                .filter(|_| sys_fresh)
                .and_then(|s| {
                    (s.battery_remaining_percent >= 0).then_some(s.battery_remaining_percent)
                })
                .map(i16::from)
        });
    let Some(percent) = percent else {
        return HealthReading::inactive(KEY, AlertSource::Battery, "Battery percentage unknown");
    };
    let critical = i16::from(config.critical_battery_percent);
    let warn = i16::from(config.warn_battery_percent);
    let severity = if percent < critical {
        Some((Severity::Critical, critical))
    } else if percent < warn {
        Some((Severity::Warn, warn))
    } else {
        None
    };
    match severity {
        Some((severity, threshold)) => HealthReading {
            key: KEY,
            source: AlertSource::Battery,
            severity,
            active: true,
            text: format!(
                "Battery {percent}% ({} below {threshold}%)",
                severity_word(severity)
            ),
        },
        None => HealthReading::inactive(KEY, AlertSource::Battery, "Battery percentage healthy"),
    }
}

fn battery_voltage(
    snapshot: &TelemetrySnapshot,
    config: &SafetyConfig,
    now_ms: i64,
    stale: u64,
) -> HealthReading {
    const KEY: &str = BATTERY_VOLTAGE_KEY;
    let Some(sys) = snapshot
        .sys_status
        .as_ref()
        .filter(|_| fresh(snapshot.field_ages.sys_status_at_ms, now_ms, stale))
    else {
        return HealthReading::inactive(KEY, AlertSource::Battery, "Battery voltage unknown");
    };
    if sys.battery_voltage_mv == 0 {
        return HealthReading::inactive(KEY, AlertSource::Battery, "Battery voltage unknown");
    }
    let mv = u32::from(sys.battery_voltage_mv);
    let severity = if config
        .critical_battery_voltage_mv
        .is_some_and(|threshold| mv < u32::from(threshold))
    {
        Some(Severity::Critical)
    } else if config
        .warn_battery_voltage_mv
        .is_some_and(|threshold| mv < u32::from(threshold))
    {
        Some(Severity::Warn)
    } else {
        None
    };
    match severity {
        Some(severity) => HealthReading {
            key: KEY,
            source: AlertSource::Battery,
            severity,
            active: true,
            text: format!(
                "Battery {:.1} V ({} below threshold)",
                f64::from(mv) / 1000.0,
                severity_word(severity)
            ),
        },
        None => HealthReading::inactive(KEY, AlertSource::Battery, "Battery voltage healthy"),
    }
}

fn gps_fix(snapshot: &TelemetrySnapshot, now_ms: i64, stale: u64) -> HealthReading {
    const KEY: &str = GPS_FIX_KEY;
    let Some(gps) = snapshot
        .gps
        .as_ref()
        .filter(|_| fresh(snapshot.field_ages.gps_at_ms, now_ms, stale))
    else {
        return HealthReading::inactive(KEY, AlertSource::Gps, "GNSS fix unknown");
    };
    match gps.fix_type {
        GpsFixType::NoGps => blocked(KEY, "GNSS receiver reports no GPS"),
        GpsFixType::NoFix => blocked(KEY, "GNSS has no fix"),
        GpsFixType::Fix2d => blocked(KEY, "GNSS fix is 2D"),
        GpsFixType::Fix3d
        | GpsFixType::Dgps
        | GpsFixType::RtkFloat
        | GpsFixType::RtkFixed
        | GpsFixType::Static
        | GpsFixType::Ppp => HealthReading::inactive(KEY, AlertSource::Gps, "GNSS fix healthy"),
    }
}

fn blocked(key: &'static str, text: &str) -> HealthReading {
    HealthReading {
        key,
        source: AlertSource::Gps,
        severity: Severity::Critical,
        active: true,
        text: text.to_string(),
    }
}

fn gps_satellites(
    snapshot: &TelemetrySnapshot,
    config: &SafetyConfig,
    now_ms: i64,
    stale: u64,
) -> HealthReading {
    const KEY: &str = GPS_SATELLITES_KEY;
    let Some(gps) = snapshot
        .gps
        .as_ref()
        .filter(|_| fresh(snapshot.field_ages.gps_at_ms, now_ms, stale))
    else {
        return HealthReading::inactive(KEY, AlertSource::Gps, "GNSS satellites unknown");
    };
    let sats = gps.satellites_visible;
    let severity = if sats < config.critical_satellites {
        Some((Severity::Critical, config.critical_satellites))
    } else if sats < config.min_satellites {
        Some((Severity::Warn, config.min_satellites))
    } else {
        None
    };
    match severity {
        Some((severity, threshold)) => HealthReading {
            key: KEY,
            source: AlertSource::Gps,
            severity,
            active: true,
            text: format!(
                "GNSS satellites {sats} ({} below {threshold})",
                severity_word(severity)
            ),
        },
        None => HealthReading::inactive(KEY, AlertSource::Gps, "GNSS satellites healthy"),
    }
}

fn link_rssi(
    snapshot: &TelemetrySnapshot,
    config: &SafetyConfig,
    now_ms: i64,
    stale: u64,
) -> HealthReading {
    const KEY: &str = LINK_RSSI_KEY;
    let Some(radio) = snapshot
        .radio
        .as_ref()
        .filter(|_| fresh(snapshot.field_ages.radio_at_ms, now_ms, stale))
    else {
        return HealthReading::inactive(KEY, AlertSource::Link, "Telemetry RSSI unknown");
    };
    let Some(rssi) = radio.rssi else {
        return HealthReading::inactive(KEY, AlertSource::Link, "Telemetry RSSI unknown");
    };
    let severity = if config
        .critical_rssi
        .is_some_and(|threshold| rssi < threshold)
    {
        Some(Severity::Critical)
    } else if config.warn_rssi.is_some_and(|threshold| rssi < threshold) {
        Some(Severity::Warn)
    } else {
        None
    };
    match severity {
        Some(severity) => HealthReading {
            key: KEY,
            source: AlertSource::Link,
            severity,
            active: true,
            text: format!(
                "Telemetry RSSI {rssi} ({} link quality)",
                severity_word(severity)
            ),
        },
        None => HealthReading::inactive(KEY, AlertSource::Link, "Telemetry RSSI healthy"),
    }
}

fn link_txbuf(
    snapshot: &TelemetrySnapshot,
    config: &SafetyConfig,
    now_ms: i64,
    stale: u64,
) -> HealthReading {
    const KEY: &str = LINK_TXBUF_KEY;
    let Some(radio) = snapshot
        .radio
        .as_ref()
        .filter(|_| fresh(snapshot.field_ages.radio_at_ms, now_ms, stale))
    else {
        return HealthReading::inactive(KEY, AlertSource::Link, "Telemetry TX buffer unknown");
    };
    let txbuf = radio.txbuf;
    let severity = if txbuf <= config.critical_txbuf_percent {
        Some((Severity::Critical, config.critical_txbuf_percent))
    } else if txbuf <= config.warn_txbuf_percent {
        Some((Severity::Warn, config.warn_txbuf_percent))
    } else {
        None
    };
    match severity {
        Some((severity, threshold)) => HealthReading {
            key: KEY,
            source: AlertSource::Link,
            severity,
            active: true,
            text: format!(
                "Telemetry TX buffer {txbuf}% ({} at {threshold}%)",
                severity_word(severity)
            ),
        },
        None => HealthReading::inactive(KEY, AlertSource::Link, "Telemetry TX buffer healthy"),
    }
}

fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warn => "warning",
        Severity::Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::AlertEventKind;
    use crate::height::{Height, HeightDatum};
    use crate::telemetry::{
        GpsRawInt, RadioStatus, SensorHealthMasks, SysStatus, TelemetrySnapshot,
    };

    const NOW_MS: i64 = 100_000;

    fn fresh(now_ms: u64) -> TelemetrySnapshot {
        let mut snapshot = TelemetrySnapshot::new();
        snapshot.field_ages.sys_status_at_ms = Some(now_ms);
        snapshot.field_ages.battery_at_ms = Some(now_ms);
        snapshot.field_ages.gps_at_ms = Some(now_ms);
        snapshot.field_ages.radio_at_ms = Some(now_ms);
        snapshot.sys_status = Some(SysStatus {
            sensors: SensorHealthMasks {
                present: 0xFFFF,
                enabled: 0xFFFF,
                health: 0xFFFF,
            },
            battery_voltage_mv: 16_800,
            battery_current_ma: 0,
            battery_remaining_percent: 80,
        });
        snapshot.gps = Some(GpsRawInt {
            fix_type: GpsFixType::Fix3d,
            satellites_visible: 12,
            latitude_deg: 48.6493,
            longitude_deg: -123.3982,
            altitude: Height::new(HeightDatum::AmslEgm96, 100.0),
            hdop: 0.8,
            vdop: 1.2,
            velocity_m_s: 0.0,
            course_over_ground_deg: 0.0,
        });
        snapshot.radio = Some(RadioStatus {
            rxerrors: 0,
            fixed: 100,
            rssi: Some(200),
            remrssi: Some(200),
            txbuf: 100,
            noise: Some(20),
            remnoise: Some(20),
        });
        snapshot
    }

    fn only_event(events: &[AlertEvent]) -> &AlertEvent {
        assert_eq!(
            events.len(),
            1,
            "expected exactly one transition: {events:?}"
        );
        &events[0]
    }

    #[test]
    fn unknown_snapshot_raises_nothing() {
        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let events = monitor.apply(
            &mut center,
            &TelemetrySnapshot::new(),
            &SafetyConfig::default(),
            NOW_MS,
        );
        assert!(events.is_empty());
        assert!(center.snapshot().is_empty());
    }

    #[test]
    fn battery_percent_crosses_warn_then_critical_only_once() {
        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let config = SafetyConfig::default();
        let mut snapshot = fresh(NOW_MS as u64);
        if let Some(sys) = snapshot.sys_status.as_mut() {
            sys.battery_remaining_percent = 25;
        }

        let warn = monitor.apply(&mut center, &snapshot, &config, NOW_MS);
        let warn = only_event(&warn);
        assert_eq!(warn.kind, AlertEventKind::Raised);
        assert_eq!(warn.alert.severity, Severity::Warn);
        assert_eq!(warn.alert.id, "battery|battery.percent");

        assert!(monitor
            .apply(&mut center, &snapshot, &config, NOW_MS + 500)
            .is_empty());

        if let Some(sys) = snapshot.sys_status.as_mut() {
            sys.battery_remaining_percent = 10;
        }
        snapshot.field_ages.sys_status_at_ms = Some(NOW_MS as u64 + 1_000);
        let critical = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 1_000);
        let critical = only_event(&critical);
        assert_eq!(critical.kind, AlertEventKind::Raised);
        assert_eq!(critical.alert.severity, Severity::Critical);
        assert!(critical.alert.latched);

        assert!(monitor
            .apply(&mut center, &snapshot, &config, NOW_MS + 1_500)
            .is_empty());
    }

    #[test]
    fn battery_voltage_uses_configured_thresholds_and_is_off_by_default() {
        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let config = SafetyConfig::default();
        let mut snapshot = fresh(NOW_MS as u64);

        assert!(monitor
            .apply(&mut center, &snapshot, &config, NOW_MS)
            .is_empty());

        let config = SafetyConfig {
            warn_battery_voltage_mv: Some(15_000),
            critical_battery_voltage_mv: Some(14_000),
            ..SafetyConfig::default()
        };
        if let Some(sys) = snapshot.sys_status.as_mut() {
            sys.battery_voltage_mv = 14_500;
        }
        let warn = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 100);
        let warn = only_event(&warn);
        assert_eq!(warn.alert.severity, Severity::Warn);
        assert_eq!(warn.alert.id, "battery|battery.voltage");

        if let Some(sys) = snapshot.sys_status.as_mut() {
            sys.battery_voltage_mv = 13_500;
        }
        snapshot.field_ages.sys_status_at_ms = Some(NOW_MS as u64 + 1_000);
        let critical = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 1_000);
        let critical = only_event(&critical);
        assert_eq!(critical.alert.severity, Severity::Critical);
        assert!(critical.alert.latched);
    }

    #[test]
    fn gps_fix_and_satellites_are_graded() {
        let config = SafetyConfig::default();

        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let mut snapshot = fresh(NOW_MS as u64);
        if let Some(gps) = snapshot.gps.as_mut() {
            gps.fix_type = GpsFixType::NoFix;
        }
        let fix = monitor.apply(&mut center, &snapshot, &config, NOW_MS);
        let fix = only_event(&fix);
        assert_eq!(fix.alert.id, "gps|gps.fix");
        assert_eq!(fix.alert.severity, Severity::Critical);

        if let Some(gps) = snapshot.gps.as_mut() {
            gps.fix_type = GpsFixType::Fix2d;
        }
        snapshot.field_ages.gps_at_ms = Some(NOW_MS as u64 + 1_000);
        assert!(
            monitor
                .apply(&mut center, &snapshot, &config, NOW_MS + 1_000)
                .is_empty(),
            "the same Critical severity must not re-notify"
        );

        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let mut snapshot = fresh(NOW_MS as u64);
        if let Some(gps) = snapshot.gps.as_mut() {
            gps.satellites_visible = 5;
        }
        let warn = monitor.apply(&mut center, &snapshot, &config, NOW_MS);
        let warn = only_event(&warn);
        assert_eq!(warn.alert.id, "gps|gps.satellites");
        assert_eq!(warn.alert.severity, Severity::Warn);

        if let Some(gps) = snapshot.gps.as_mut() {
            gps.satellites_visible = 3;
        }
        snapshot.field_ages.gps_at_ms = Some(NOW_MS as u64 + 1_000);
        let critical = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 1_000);
        let critical = only_event(&critical);
        assert_eq!(critical.alert.severity, Severity::Critical);
    }

    #[test]
    fn rssi_and_txbuf_are_graded_when_configured() {
        let config = SafetyConfig {
            warn_rssi: Some(60),
            critical_rssi: Some(30),
            ..SafetyConfig::default()
        };

        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let mut snapshot = fresh(NOW_MS as u64);
        if let Some(radio) = snapshot.radio.as_mut() {
            radio.rssi = Some(50);
        }
        let warn = monitor.apply(&mut center, &snapshot, &config, NOW_MS);
        let warn = only_event(&warn);
        assert_eq!(warn.alert.id, "link|link.rssi");
        assert_eq!(warn.alert.severity, Severity::Warn);

        if let Some(radio) = snapshot.radio.as_mut() {
            radio.rssi = Some(20);
        }
        snapshot.field_ages.radio_at_ms = Some(NOW_MS as u64 + 1_000);
        let critical = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 1_000);
        let critical = only_event(&critical);
        assert_eq!(critical.alert.severity, Severity::Critical);

        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let mut snapshot = fresh(NOW_MS as u64);
        if let Some(radio) = snapshot.radio.as_mut() {
            radio.txbuf = 15;
        }
        let warn = monitor.apply(&mut center, &snapshot, &config, NOW_MS);
        let warn = only_event(&warn);
        assert_eq!(warn.alert.id, "link|link.txbuf");
        assert_eq!(warn.alert.severity, Severity::Warn);

        if let Some(radio) = snapshot.radio.as_mut() {
            radio.txbuf = 3;
        }
        snapshot.field_ages.radio_at_ms = Some(NOW_MS as u64 + 1_000);
        let critical = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 1_000);
        let critical = only_event(&critical);
        assert_eq!(critical.alert.severity, Severity::Critical);
    }

    #[test]
    fn packet_loss_is_computed_from_radio_counter_deltas() {
        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let config = SafetyConfig::default();
        let mut snapshot = fresh(NOW_MS as u64);

        assert!(
            monitor
                .apply(&mut center, &snapshot, &config, NOW_MS)
                .is_empty(),
            "the first sample has no delta yet"
        );

        if let Some(radio) = snapshot.radio.as_mut() {
            radio.rxerrors = 20;
            radio.fixed = 180;
        }
        snapshot.field_ages.radio_at_ms = Some(NOW_MS as u64 + 1_000);
        let warn = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 1_000);
        let warn = only_event(&warn);
        assert_eq!(warn.alert.id, "link|link.packet_loss");
        assert_eq!(warn.alert.severity, Severity::Warn);

        if let Some(radio) = snapshot.radio.as_mut() {
            radio.rxerrors = 50;
            radio.fixed = 250;
        }
        snapshot.field_ages.radio_at_ms = Some(NOW_MS as u64 + 2_000);
        let critical = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 2_000);
        let critical = only_event(&critical);
        assert_eq!(critical.alert.severity, Severity::Critical);
    }

    #[test]
    fn recovery_waits_for_the_clear_hysteresis() {
        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let config = SafetyConfig::default();
        let hysteresis = center.config().clear_hysteresis_ms;

        let mut snapshot = fresh(NOW_MS as u64);
        if let Some(sys) = snapshot.sys_status.as_mut() {
            sys.battery_remaining_percent = 25;
        }
        let raised = monitor.apply(&mut center, &snapshot, &config, NOW_MS);
        assert_eq!(only_event(&raised).alert.severity, Severity::Warn);

        let first_clear_sample = NOW_MS + 1_000;
        snapshot = fresh(first_clear_sample as u64);
        assert!(monitor
            .apply(&mut center, &snapshot, &config, first_clear_sample)
            .is_empty());

        let before = first_clear_sample + hysteresis - 1;
        snapshot = fresh(before as u64);
        assert!(monitor
            .apply(&mut center, &snapshot, &config, before)
            .is_empty());

        let cleared_at = first_clear_sample + hysteresis;
        snapshot = fresh(cleared_at as u64);
        let cleared = monitor.apply(&mut center, &snapshot, &config, cleared_at);
        assert_eq!(only_event(&cleared).kind, AlertEventKind::Cleared);
    }

    #[test]
    fn a_latched_critical_survives_a_lower_severity_and_does_not_clear() {
        let mut monitor = HealthMonitor::default();
        let mut center = AlertCenter::default();
        let config = SafetyConfig::default();

        let mut snapshot = fresh(NOW_MS as u64);
        if let Some(sys) = snapshot.sys_status.as_mut() {
            sys.battery_remaining_percent = 10;
        }
        let critical = monitor.apply(&mut center, &snapshot, &config, NOW_MS);
        assert!(only_event(&critical).alert.latched);

        if let Some(sys) = snapshot.sys_status.as_mut() {
            sys.battery_remaining_percent = 25;
        }
        snapshot.field_ages.sys_status_at_ms = Some(NOW_MS as u64 + 1_000);
        let warn = monitor.apply(&mut center, &snapshot, &config, NOW_MS + 1_000);
        let warn = only_event(&warn);
        assert_eq!(warn.alert.severity, Severity::Warn);
        assert!(warn.alert.latched, "latch is sticky across de-escalation");

        if let Some(sys) = snapshot.sys_status.as_mut() {
            sys.battery_remaining_percent = 80;
        }
        snapshot.field_ages.sys_status_at_ms = Some(NOW_MS as u64 + 2_000);
        assert!(monitor
            .apply(&mut center, &snapshot, &config, NOW_MS + 2_000)
            .is_empty());
        snapshot.field_ages.sys_status_at_ms = Some(NOW_MS as u64 + 20_000);
        assert!(
            monitor
                .apply(&mut center, &snapshot, &config, NOW_MS + 20_000)
                .is_empty(),
            "a latched Critical clears only after acknowledgement"
        );
    }
}
