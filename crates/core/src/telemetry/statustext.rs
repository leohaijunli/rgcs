//! `STATUSTEXT` decoding, chunk reassembly and the rejection correlation
//! (plan §4 L1).
//!
//! PX4 answers a `COMMAND_LONG` it will not run with
//! `MAV_RESULT_TEMPORARILY_REJECTED` and then sends the reason as a
//! `STATUSTEXT` — for an arm attempt that is the prearm message
//! (`Preflight Fail: …`). The reason is useless without the command it belongs
//! to, and both frames arrive on the same ordered stream, which is why the
//! correlation lives here (in the telemetry hub) and not in the UI.
//!
//! Long messages are split into several frames. MAVLink 2 defines the scheme
//! through two `STATUSTEXT` extension fields:
//!
//! - `id` — one identifier for the whole logical message; **`id == 0` means
//!   the frame is the entire message** and can be emitted immediately;
//! - `chunk_seq` — this chunk's index, counted from zero;
//! - a NUL byte in the text field marks the **last** chunk (the payload is
//!   NUL-padded, so a chunk shorter than the field is the terminator).
//!
//! Reassembly therefore needs no timer: a message ends when the FC says so.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::alerts::Severity;
use crate::commands::CommandResult;
use crate::mavlink::message::{MavMessage, MessageEnvelope};

use ::mavlink::common::MavCmd;

/// How long after a `temporarily_rejected` ack a `STATUSTEXT` is still taken
/// to explain it. PX4 sends the reason in the same tick as the ack; the window
/// only has to cover link jitter and the hub's own scheduling.
pub const REJECTION_WINDOW_MS: u64 = 1_500;

/// The longest reassembled message we keep. A stalled sequence (a lost chunk)
/// must not be able to grow a buffer without bound.
const MAX_ASSEMBLED_CHARS: usize = 512;

/// `MAV_SEVERITY` of a `STATUSTEXT`, in wire order (Emergency is the worst).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StatusSeverity {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

impl StatusSeverity {
    /// Decode the wire value. An unknown value is treated as `Notice`: it is
    /// operator-facing text whose severity we do not understand, which is
    /// exactly what `Notice` means (never silently promoted to a warning).
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Emergency,
            1 => Self::Alert,
            2 => Self::Critical,
            3 => Self::Error,
            4 => Self::Warning,
            5 => Self::Notice,
            6 => Self::Info,
            7 => Self::Debug,
            _ => Self::Notice,
        }
    }

    /// Stable name for the event log and tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Emergency => "emergency",
            Self::Alert => "alert",
            Self::Critical => "critical",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Notice => "notice",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }

    /// Whether this deserves the operator's attention (Emergency…Warning).
    ///
    /// `Notice`/`Info`/`Debug` are chatter: they still land in the event log,
    /// but must not colour the alert badge — PX4 emits them continuously, and
    /// a badge that is never zero tells the operator nothing.
    pub fn is_alarming(self) -> bool {
        self <= Self::Warning
    }

    /// The alert center's severity (L1). Anything the FC calls a problem
    /// (Emergency…Warning) is at least a `Warn`; the rest is informational.
    pub fn alert_severity(self) -> Severity {
        match self {
            Self::Emergency | Self::Alert | Self::Critical => Severity::Critical,
            Self::Error | Self::Warning => Severity::Warn,
            Self::Notice | Self::Info | Self::Debug => Severity::Info,
        }
    }
}

/// One `STATUSTEXT` frame: either a whole message (`id == 0`) or one chunk of
/// a longer one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusChunk {
    pub severity: StatusSeverity,
    /// Message identifier; `0` = this frame is the whole message.
    pub id: u16,
    /// This chunk's index within the message, counted from zero.
    pub chunk_seq: u8,
    /// The FC terminated the payload early (a NUL in the text field), which
    /// the protocol defines as "this was the last chunk".
    pub last: bool,
    /// Text with the padding removed.
    pub text: String,
}

impl StatusChunk {
    /// Decode a `STATUSTEXT` envelope; `None` for any other message.
    pub fn decode(env: &MessageEnvelope) -> Option<Self> {
        let MavMessage::STATUSTEXT(status) = &env.message else {
            return None;
        };
        Some(Self {
            severity: StatusSeverity::from_raw(status.severity as u8),
            id: status.id,
            chunk_seq: status.chunk_seq,
            // The payload is a fixed-width, NUL-padded field: the FC fills the
            // last chunk and leaves the rest zero, so a NUL means "done".
            last: status.text.contains(&0),
            text: status.text.to_str().unwrap_or_default().to_string(),
        })
    }
}

/// A fully assembled `STATUSTEXT`, ready for the alert center and the event
/// log. Serialized to the UI (ADR-002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StatusText {
    pub severity: StatusSeverity,
    pub text: String,
    /// The command this text explains, when it arrived right after a
    /// `temporarily_rejected` ack — PX4's prearm reason for a rejected
    /// arm/takeoff/etc. `None` for ordinary status messages.
    pub command: Option<String>,
    #[ts(type = "number")]
    pub at_ms: u64,
}

/// In-progress multi-frame message.
#[derive(Debug)]
struct Partial {
    id: u16,
    severity: StatusSeverity,
    /// The next `chunk_seq` we expect; anything else means a chunk was lost
    /// and the two halves must not be spliced together.
    next_seq: u8,
    text: String,
}

/// Reassembles multi-frame `STATUSTEXT`s from one vehicle.
///
/// Frames from one vehicle arrive in order (a lost frame shows up as a gap in
/// `chunk_seq`, which restarts the message instead of splicing garbage).
#[derive(Debug, Default)]
pub struct StatusTextAssembler {
    pending: Option<Partial>,
}

impl StatusTextAssembler {
    /// A fresh assembler with no partial message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk. `at_ms` is the caller's clock — the assembler holds no
    /// timer, so a sequence that never completes simply stays pending and is
    /// dropped when the next message starts.
    ///
    /// Returns the message once it is complete.
    pub fn push(&mut self, chunk: StatusChunk, at_ms: u64) -> Option<StatusText> {
        if chunk.id == 0 {
            // Not chunked: emit as-is.
            self.pending = None;
            return Some(StatusText {
                severity: chunk.severity,
                text: chunk.text,
                command: None,
                at_ms,
            });
        }

        let continues = match &self.pending {
            Some(partial) => partial.id == chunk.id && chunk.chunk_seq == partial.next_seq,
            None => false,
        };
        if continues {
            let partial = self.pending.as_mut().expect("checked above");
            partial.text.push_str(&chunk.text);
            partial.text.truncate(MAX_ASSEMBLED_CHARS);
            partial.next_seq = chunk.chunk_seq.wrapping_add(1);
        } else {
            self.pending = Some(Partial {
                id: chunk.id,
                severity: chunk.severity,
                next_seq: chunk.chunk_seq.wrapping_add(1),
                text: chunk.text.clone(),
            });
        }
        if !chunk.last {
            return None;
        }
        let partial = self.pending.take()?;
        Some(StatusText {
            severity: partial.severity,
            text: partial.text,
            command: None,
            at_ms,
        })
    }
}

/// A rejected command waiting for the `STATUSTEXT` that explains it.
#[derive(Debug, Clone, Copy)]
struct PendingRejection {
    command: MavCmd,
    at_ms: u64,
}

/// Correlates a `temporarily_rejected` `COMMAND_ACK` with the `STATUSTEXT`
/// that follows it.
///
/// One rejection explains one text: the pending entry is consumed, so a later
/// unrelated status message is not mislabelled.
#[derive(Debug)]
pub struct RejectionCorrelator {
    window_ms: u64,
    pending: Option<PendingRejection>,
}

impl Default for RejectionCorrelator {
    fn default() -> Self {
        Self::new(REJECTION_WINDOW_MS)
    }
}

impl RejectionCorrelator {
    /// Correlator with an explicit window (tests, and any future tuning).
    pub fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            pending: None,
        }
    }

    /// Note a terminal ack. Only `temporarily_rejected` carries a follow-up
    /// reason — PX4 denies with the reason in the acks, and answers
    /// `temporarily_rejected` with a `STATUSTEXT`.
    pub fn note_ack(&mut self, command: MavCmd, result: CommandResult, at_ms: u64) {
        if result == CommandResult::TemporarilyRejected {
            self.pending = Some(PendingRejection { command, at_ms });
        }
    }

    /// The command whose rejection a text arriving at `at_ms` explains, if the
    /// text is inside the window. Consumes the pending rejection.
    pub fn take_explains(&mut self, at_ms: u64) -> Option<MavCmd> {
        let pending = self.pending.take()?;
        (at_ms <= pending.at_ms.saturating_add(self.window_ms)).then_some(pending.command)
    }
}

/// A decoded `COMMAND_ACK`: the terminal result for a command we sent.
pub fn decode_command_ack(env: &MessageEnvelope) -> Option<(MavCmd, CommandResult)> {
    let MavMessage::COMMAND_ACK(ack) = &env.message else {
        return None;
    };
    Some((ack.command, CommandResult::from_mav(ack.result)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::message::MavHeader;

    /// The fixture writes severity as a number so the tests read like the
    /// wire; the mapping to the protocol enum is deliberately explicit (the
    /// generated enum has no `TryFrom<u8>`).
    fn mav_severity(raw: u8) -> ::mavlink::common::MavSeverity {
        use ::mavlink::common::MavSeverity as S;
        match raw {
            0 => S::MAV_SEVERITY_EMERGENCY,
            1 => S::MAV_SEVERITY_ALERT,
            2 => S::MAV_SEVERITY_CRITICAL,
            3 => S::MAV_SEVERITY_ERROR,
            4 => S::MAV_SEVERITY_WARNING,
            5 => S::MAV_SEVERITY_NOTICE,
            6 => S::MAV_SEVERITY_INFO,
            _ => S::MAV_SEVERITY_DEBUG,
        }
    }

    fn status_frame(severity: u8, id: u16, chunk_seq: u8, text: &str) -> MessageEnvelope {
        MessageEnvelope {
            header: MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            message: MavMessage::STATUSTEXT(::mavlink::common::STATUSTEXT_DATA {
                severity: mav_severity(severity),
                text: text.into(),
                id,
                chunk_seq,
            }),
            received_at: std::time::Instant::now(),
        }
    }

    fn ack_frame(command: MavCmd, result: ::mavlink::common::MavResult) -> MessageEnvelope {
        MessageEnvelope {
            header: MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            message: MavMessage::COMMAND_ACK(::mavlink::common::COMMAND_ACK_DATA {
                command,
                result,
                ..Default::default()
            }),
            received_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn severity_maps_from_the_wire_and_onto_the_alert_center() {
        assert_eq!(StatusSeverity::from_raw(0), StatusSeverity::Emergency);
        assert_eq!(StatusSeverity::from_raw(6), StatusSeverity::Info);
        assert_eq!(
            StatusSeverity::from_raw(200),
            StatusSeverity::Notice,
            "an unknown severity is noticed, never silently promoted"
        );

        assert_eq!(
            StatusSeverity::Emergency.alert_severity(),
            Severity::Critical
        );
        assert_eq!(
            StatusSeverity::Critical.alert_severity(),
            Severity::Critical
        );
        assert_eq!(StatusSeverity::Error.alert_severity(), Severity::Warn);
        assert_eq!(StatusSeverity::Warning.alert_severity(), Severity::Warn);
        assert_eq!(StatusSeverity::Info.alert_severity(), Severity::Info);
        assert_eq!(StatusSeverity::Debug.alert_severity(), Severity::Info);

        assert!(StatusSeverity::Warning.is_alarming());
        assert!(!StatusSeverity::Notice.is_alarming());
        assert!(!StatusSeverity::Debug.is_alarming());
    }

    #[test]
    fn a_single_frame_message_is_emitted_immediately() {
        let env = status_frame(4, 0, 0, "Preflight Fail: no GPS");
        let chunk = StatusChunk::decode(&env).expect("statustext");
        assert!(chunk.last, "id 0 is a whole message");
        assert_eq!(chunk.text, "Preflight Fail: no GPS");

        let text = StatusTextAssembler::new()
            .push(chunk, 1_000)
            .expect("emitted at once");
        assert_eq!(text.text, "Preflight Fail: no GPS");
        assert_eq!(text.severity, StatusSeverity::Warning);
        assert_eq!(text.at_ms, 1_000);
        assert!(text.command.is_none());
    }

    #[test]
    fn non_statustext_frames_are_ignored() {
        let env = ack_frame(
            MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
            ::mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
        );
        assert!(StatusChunk::decode(&env).is_none());
    }

    #[test]
    fn chunks_are_reassembled_in_order_and_emitted_on_the_terminator() {
        let mut assembler = StatusTextAssembler::new();
        // 50 bytes: no NUL, so more chunks follow.
        let first = status_frame(3, 7, 0, &"A".repeat(50));
        let second = status_frame(3, 7, 1, &"B".repeat(50));
        // Shorter than the field: NUL-padded, i.e. the last chunk.
        let last = status_frame(3, 7, 2, "END");

        assert!(assembler
            .push(StatusChunk::decode(&first).expect("chunk"), 10)
            .is_none());
        assert!(assembler
            .push(StatusChunk::decode(&second).expect("chunk"), 20)
            .is_none());

        let text = assembler
            .push(StatusChunk::decode(&last).expect("chunk"), 30)
            .expect("complete on the terminator");
        assert_eq!(text.text.len(), 103, "50 + 50 + 3");
        assert!(text.text.starts_with(&"A".repeat(50)));
        assert!(text.text.ends_with("END"));
        assert_eq!(text.at_ms, 30);

        // The buffer is cleared: the next message is not joined to it.
        let single = status_frame(6, 0, 0, "next");
        let next = assembler
            .push(StatusChunk::decode(&single).expect("chunk"), 40)
            .expect("an unchunked message is emitted at once");
        assert_eq!(next.text, "next");
    }

    #[test]
    fn a_new_id_mid_sequence_restarts_instead_of_splicing() {
        let mut assembler = StatusTextAssembler::new();
        let first = status_frame(3, 1, 0, &"A".repeat(50));
        let other = status_frame(3, 2, 0, "BBBB");
        assert!(
            assembler
                .push(StatusChunk::decode(&first).expect("chunk"), 10)
                .is_none(),
            "the first message is still waiting for its terminator"
        );
        let text = assembler
            .push(StatusChunk::decode(&other).expect("chunk"), 20)
            .expect("the second message is complete");
        assert_eq!(text.text, "BBBB", "no prefix from the abandoned message");
    }

    #[test]
    fn a_lost_chunk_is_not_spliced_into_the_message() {
        let mut assembler = StatusTextAssembler::new();
        let first = status_frame(3, 5, 0, &"A".repeat(50));
        let last = status_frame(3, 5, 2, "TAIL");
        assert!(assembler
            .push(StatusChunk::decode(&first).expect("chunk"), 10)
            .is_none());
        // chunk_seq 0 then 2: chunk 1 was lost. Emitting keeps only the part
        // after the gap, never a spliced mixture of the two.
        let text = assembler
            .push(StatusChunk::decode(&last).expect("chunk"), 20)
            .expect("the terminator still ends the message");
        assert_eq!(text.text, "TAIL");
    }

    #[test]
    fn a_rejection_is_correlated_with_the_text_that_follows() {
        let mut correlator = RejectionCorrelator::new(1_000);
        correlator.note_ack(
            MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
            CommandResult::TemporarilyRejected,
            5_000,
        );
        assert_eq!(
            correlator.take_explains(5_400),
            Some(MavCmd::MAV_CMD_COMPONENT_ARM_DISARM)
        );
        assert_eq!(correlator.take_explains(5_500), None, "one text, one ack");

        correlator.note_ack(
            MavCmd::MAV_CMD_NAV_TAKEOFF,
            CommandResult::TemporarilyRejected,
            6_000,
        );
        assert_eq!(
            correlator.take_explains(7_600),
            None,
            "outside the window the text is just a status message"
        );
    }

    #[test]
    fn only_temporarily_rejected_acks_arm_the_correlator() {
        let mut correlator = RejectionCorrelator::new(1_000);
        for result in [
            CommandResult::Accepted,
            CommandResult::Denied,
            CommandResult::Failed,
        ] {
            correlator.note_ack(MavCmd::MAV_CMD_NAV_LAND, result, 1_000);
            assert_eq!(correlator.take_explains(1_100), None, "{result:?}");
        }
    }

    #[test]
    fn a_round_trip_through_the_ack_decoder_reports_the_result() {
        let env = ack_frame(
            MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
            ::mavlink::common::MavResult::MAV_RESULT_TEMPORARILY_REJECTED,
        );
        assert_eq!(
            decode_command_ack(&env),
            Some((
                MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
                CommandResult::TemporarilyRejected
            ))
        );
    }
}
