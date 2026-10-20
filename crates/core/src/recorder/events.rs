//! Event log: one JSON object per line (`events.jsonl`).
//!
//! The recorder keeps the *why* next to the raw bytes: alerts, command
//! outcomes, mode changes, mission syncs, forced arms, walk-survey line
//! markers and quality flags (the latter land with M1/W4/L5 — they call
//! [`crate::recorder::Recorder::record_event`] like every other source).

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{now_utc_us, RecorderError};
use crate::alerts::{AlertSource, Severity};
use crate::safety::{ChecklistItemId, ForceArmReason};

/// One event. The JSON `event` tag is the stable schema for downstream tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RecordEvent {
    /// A recording session began.
    SessionOpened {
        endpoint: String,
        system_id: u8,
        component_id: u8,
    },
    /// A recording session ended cleanly; counts are the recorder's own.
    SessionClosed {
        frames: u64,
        events: u64,
        dropped: u64,
    },
    /// An alert transition from the alert center (A3).
    Alert {
        severity: Severity,
        source: AlertSource,
        text: String,
        key: Option<String>,
    },
    /// A command was sent and how the FC answered.
    Command { name: String, result: String },
    /// The flight mode changed.
    ModeChanged { from: String, to: String },
    /// A mission upload/download stage completed (M1/M4).
    MissionSync { stage: String },
    /// A legacy force-arm event. Kept readable for existing session logs.
    ForceArm { on: bool, reason: Option<String> },
    /// A fully auditable forced-arm authorization (L5).
    ForceArmAudit {
        items: Vec<ChecklistItemId>,
        reason: ForceArmReason,
        note: Option<String>,
        time_utc_us: u64,
    },

    /// A survey line started (M1/W4).
    LineStarted { line_id: String, kind: String },
    /// A survey line ended (M1/W4).
    LineEnded { line_id: String, kind: String },
    /// An operator marker (W4).
    Marker { label: String },
    /// A quality flag on a survey line (W5).
    QualityEvent { line_id: String, flag: String },
    /// Free-form operator note.
    Note { text: String },
}

/// A [`RecordEvent`] stamped with the wall-clock time it was enqueued.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoggedEvent {
    /// Microseconds since the Unix epoch.
    pub ts_us: u64,
    #[serde(flatten)]
    pub event: RecordEvent,
}

impl LoggedEvent {
    /// Stamp an event with the current time.
    pub fn new(event: RecordEvent) -> Self {
        Self {
            ts_us: now_utc_us(),
            event,
        }
    }
}

/// Serialize `event` as one JSON line.
pub(crate) fn write_event<W: Write>(w: &mut W, event: &LoggedEvent) -> Result<(), RecorderError> {
    let line = serde_json::to_vec(event)?;
    w.write_all(&line)?;
    w.write_all(b"\n")?;
    Ok(())
}

/// Read every complete event line. A truncated final line (a crash mid-write)
/// is ignored so the file parses up to the last flush.
pub fn read_events(path: &Path) -> Result<Vec<LoggedEvent>, RecorderError> {
    let text = fs::read_to_string(path).map_err(|e| RecorderError::Open {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(read_events_from(&text))
}

/// Parse events from an in-memory JSONL image.
pub fn read_events_from(text: &str) -> Vec<LoggedEvent> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LoggedEvent>(line) {
            Ok(event) => out.push(event),
            // A partially written tail (or corruption) ends the readable log.
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_variant_through_jsonl() {
        let events = vec![
            RecordEvent::SessionOpened {
                endpoint: "udpin:0.0.0.0:14550".to_string(),
                system_id: 1,
                component_id: 1,
            },
            RecordEvent::Alert {
                severity: Severity::Warn,
                source: AlertSource::Gps,
                text: "RTK float".to_string(),
                key: Some("gnss-fix".to_string()),
            },
            RecordEvent::Command {
                name: "MAV_CMD_DO_SET_MODE".to_string(),
                result: "accepted".to_string(),
            },
            RecordEvent::ModeChanged {
                from: "POSCTL".to_string(),
                to: "AUTO.MISSION".to_string(),
            },
            RecordEvent::MissionSync {
                stage: "uploaded".to_string(),
            },
            RecordEvent::ForceArm {
                on: true,
                reason: Some("battery".to_string()),
            },
            RecordEvent::ForceArmAudit {
                items: vec![ChecklistItemId::Battery],
                reason: ForceArmReason::Weather,
                note: Some("wind shifted".to_string()),
                time_utc_us: 42,
            },
            RecordEvent::LineStarted {
                line_id: "L001".to_string(),
                kind: "survey".to_string(),
            },
            RecordEvent::LineEnded {
                line_id: "L001".to_string(),
                kind: "survey".to_string(),
            },
            RecordEvent::Marker {
                label: "base".to_string(),
            },
            RecordEvent::QualityEvent {
                line_id: "L001".to_string(),
                flag: "diurnal".to_string(),
            },
            RecordEvent::Note {
                text: "wind from the west".to_string(),
            },
        ];
        let mut buf: Vec<u8> = Vec::new();
        for event in &events {
            let logged = LoggedEvent {
                ts_us: 42,
                event: event.clone(),
            };
            write_event(&mut buf, &logged).expect("write");
        }
        let text = String::from_utf8(buf).expect("utf-8");
        let read = read_events_from(&text);
        assert_eq!(read.len(), events.len());
        for (read, expected) in read.iter().zip(&events) {
            assert_eq!(&read.event, expected);
            assert_eq!(read.ts_us, 42);
        }
    }

    #[test]
    fn a_truncated_final_line_is_ignored() {
        let mut buf: Vec<u8> = Vec::new();
        write_event(
            &mut buf,
            &LoggedEvent::new(RecordEvent::Note {
                text: "kept".to_string(),
            }),
        )
        .expect("write");
        // A crash wrote half of the next object.
        buf.extend_from_slice(b"{\"ts_us\":9,\"event\":\"mar");
        let text = String::from_utf8(buf).expect("utf-8");
        let read = read_events_from(&text);
        assert_eq!(read.len(), 1);
        assert_eq!(
            read[0].event,
            RecordEvent::Note {
                text: "kept".to_string()
            }
        );
    }
}
