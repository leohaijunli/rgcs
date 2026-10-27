#![allow(deprecated)]

//! MAVLink mission protocol state machine (Phase 1).
//!
//! Pure state machine: the caller feeds received frames via [`handle`] and
//! sends the returned frames on the link; [`retransmit_due`] returns frames
//! to re-send after a timeout.
//!
//! Upload: MISSION_COUNT → MISSION_REQUEST(_INT) per item → MISSION_ITEM_INT
//!   → MISSION_ACK.
//! Download: MISSION_REQUEST_LIST → MISSION_COUNT → MISSION_REQUEST(_INT)
//!   per item → MISSION_ITEM_INT → MISSION_ACK.
//! Clear: MISSION_CLEAR_ALL → MISSION_ACK.
//! Set current: MISSION_SET_CURRENT → MISSION_CURRENT.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ::mavlink::common::{
    MavMessage, MavMissionResult, MavMissionType, MISSION_ACK_DATA, MISSION_CLEAR_ALL_DATA,
    MISSION_COUNT_DATA, MISSION_CURRENT_DATA, MISSION_ITEM_INT_DATA, MISSION_REQUEST_DATA,
    MISSION_REQUEST_INT_DATA, MISSION_REQUEST_LIST_DATA, MISSION_SET_CURRENT_DATA,
};
use ::mavlink::{MavHeader, MessageData};

use super::error::MissionError;
use super::types::{MissionFrame, MissionItem};

/// Timeout before a sent frame is retransmitted.
pub const RETRY_TIMEOUT: Duration = Duration::from_millis(500);
/// Maximum retransmissions before giving up.
pub const MAX_RETRIES: u32 = 5;
/// How often the caller should call [`MissionProtocol::on_tick`].
pub const RETRY_TICK: Duration = Duration::from_millis(100);
/// Hard deadline for a whole upload/download/clear/set-current operation.
///
/// The per-frame retransmit budget already bounds a single exchange; this
/// bounds the sum of them so a stalled mission cannot hang the UI forever
/// (issues.md #7).
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

/// MAVLink message ids the mission protocol consumes (issues.md #20).
///
/// Used to route the connection's event bus so the mission service is not
/// woken for unrelated telemetry.
pub const MISSION_MESSAGE_IDS: &[u32] = &[
    MISSION_REQUEST_DATA::ID,      // 40 (deprecated non-INT request)
    MISSION_SET_CURRENT_DATA::ID,  // 41
    MISSION_CURRENT_DATA::ID,      // 42
    MISSION_REQUEST_LIST_DATA::ID, // 43
    MISSION_COUNT_DATA::ID,        // 44
    MISSION_CLEAR_ALL_DATA::ID,    // 45
    MISSION_ACK_DATA::ID,          // 47
    MISSION_REQUEST_INT_DATA::ID,  // 51
    MISSION_ITEM_INT_DATA::ID,     // 73
];

/// Mission operation currently in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionOperation {
    Upload,
    Download,
    ClearAll,
    SetCurrent(u16),
}

/// Events emitted by the protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum MissionEvent {
    /// Progress for upload/download.
    Progress {
        operation: MissionOperation,
        sent: u16,
        total: u16,
    },
    /// Operation finished.
    Completed(MissionOperation),
    /// The active waypoint changed (MISSION_CURRENT).
    CurrentChanged { seq: u16 },
    /// A permanent failure.
    Failed(MissionError),
}

/// A frame awaiting retransmission.
#[derive(Debug, Clone)]
struct Outbound {
    msg: MavMessage,
    deadline: Instant,
    retries: u32,
}

#[derive(Debug)]
enum State {
    Idle,
    Upload {
        items: Vec<MissionItem>,
        next_seq: u16,
    },
    Download {
        items: Vec<MissionItem>,
        next_seq: u16,
        total: u16,
    },
    Clear,
    SetCurrent {
        seq: u16,
    },
}

/// MAVLink mission protocol state machine (one instance per FC).
#[derive(Debug)]
pub struct MissionProtocol {
    /// Our own MAVLink system/component id (inbound messages are addressed to us).
    self_sys: u8,
    self_comp: u8,
    target_sys: u8,
    target_comp: u8,
    state: State,
    outbound: VecDeque<Outbound>,
    /// Items from the most recent completed download.
    ///
    /// The protocol owns the downloaded data so the app layer does not have to
    /// keep a second copy (issues.md #9).
    last_downloaded: Vec<MissionItem>,
    /// Wall-clock deadline for the in-flight operation, if any.
    op_deadline: Option<Instant>,
}

impl MissionProtocol {
    /// New idle protocol: `self_sys`/`self_comp` are our own MAVLink ids
    /// (inbound mission messages are addressed to them), `target_sys`/`target_comp`
    /// identify the FC.
    pub fn new(self_sys: u8, self_comp: u8, target_sys: u8, target_comp: u8) -> Self {
        Self {
            self_sys,
            self_comp,
            target_sys,
            target_comp,
            state: State::Idle,
            outbound: VecDeque::new(),
            last_downloaded: Vec::new(),
            op_deadline: None,
        }
    }

    /// True when no operation is in progress.
    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    /// Start uploading a mission; returns the frames to send.
    pub fn begin_upload(
        &mut self,
        items: Vec<MissionItem>,
    ) -> Result<Vec<MavMessage>, MissionError> {
        super::types::validate_upload(&items)?;
        let count = u16::try_from(items.len()).map_err(|_| MissionError::InvalidRequest {
            message: "mission item count exceeds the wire limit".into(),
        })?;
        self.start_operation();
        self.state = State::Upload { items, next_seq: 0 };
        let msg = MavMessage::MISSION_COUNT(MISSION_COUNT_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
            count,
            ..Default::default()
        });
        Ok(self.queue(msg, "MISSION_COUNT"))
    }

    /// Start downloading the FC mission; returns the frames to send.
    pub fn begin_download(&mut self) -> Vec<MavMessage> {
        self.start_operation();
        self.last_downloaded.clear();
        self.state = State::Download {
            items: Vec::new(),
            next_seq: 0,
            total: 0,
        };
        let msg = MavMessage::MISSION_REQUEST_LIST(MISSION_REQUEST_LIST_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
            ..Default::default()
        });
        self.queue(msg, "MISSION_REQUEST_LIST")
    }

    /// Take the items from the last completed download (empty if none).
    pub fn take_downloaded(&mut self) -> Vec<MissionItem> {
        std::mem::take(&mut self.last_downloaded)
    }

    /// Clear the FC mission; returns the frames to send.
    pub fn begin_clear(&mut self) -> Vec<MavMessage> {
        self.start_operation();
        self.state = State::Clear;
        let msg = MavMessage::MISSION_CLEAR_ALL(MISSION_CLEAR_ALL_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
            ..Default::default()
        });
        self.queue(msg, "MISSION_CLEAR_ALL")
    }

    /// Set the active waypoint; returns the frames to send.
    pub fn begin_set_current(&mut self, seq: u16) -> Vec<MavMessage> {
        self.start_operation();
        self.state = State::SetCurrent { seq };
        let msg = MavMessage::MISSION_SET_CURRENT(MISSION_SET_CURRENT_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
            seq,
        });
        self.queue(msg, "MISSION_SET_CURRENT")
    }

    /// Source/destination/type checks also apply to COUNT, REQUEST and ACK.
    pub(super) fn accepts(&self, header: &MavHeader, message: &MavMessage) -> bool {
        if header.system_id != self.target_sys || header.component_id != self.target_comp {
            return false;
        }
        if let Some((system, component)) = target_of(message) {
            if (system, component) != (self.self_sys, self.self_comp)
                && (system, component) != (0, 0)
            {
                return false;
            }
        }
        let mission_type = match message {
            MavMessage::MISSION_COUNT(message) => Some(message.mission_type),
            MavMessage::MISSION_REQUEST(message) => Some(message.mission_type),
            MavMessage::MISSION_REQUEST_INT(message) => Some(message.mission_type),
            MavMessage::MISSION_REQUEST_LIST(message) => Some(message.mission_type),
            MavMessage::MISSION_ACK(message) => Some(message.mission_type),
            MavMessage::MISSION_ITEM_INT(message) => Some(message.mission_type),
            MavMessage::MISSION_CLEAR_ALL(message) => Some(message.mission_type),
            MavMessage::MISSION_CURRENT(_) | MavMessage::MISSION_SET_CURRENT(_) => None,
            _ => return false,
        };
        mission_type.is_none_or(|kind| kind == MavMissionType::MAV_MISSION_TYPE_MISSION)
    }

    /// Handle an incoming frame. Returns events and frames to send.
    #[allow(deprecated)]
    pub fn handle(
        &mut self,
        header: &MavHeader,
        msg: &MavMessage,
    ) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        if !self.accepts(header, msg) {
            return (Vec::new(), Vec::new());
        }
        match msg {
            MavMessage::MISSION_REQUEST_INT(m) => self.on_upload_request(m.seq),
            MavMessage::MISSION_REQUEST(m) => self.on_upload_request(m.seq),
            MavMessage::MISSION_ACK(m) => self.on_ack(m.mavtype as u8),
            MavMessage::MISSION_COUNT(m) => self.on_download_count(m.count),
            MavMessage::MISSION_ITEM_INT(m) => self.on_download_item(m),
            MavMessage::MISSION_CURRENT(m) => self.on_set_current(m.seq),
            _ => (Vec::new(), Vec::new()),
        }
    }

    /// Frames due for retransmission (call periodically).
    pub fn retransmit_due(&mut self, now: Instant) -> Vec<MavMessage> {
        let mut due = Vec::new();
        for out in self.outbound.iter_mut() {
            if now >= out.deadline {
                out.retries += 1;
                if out.retries > MAX_RETRIES {
                    out.deadline = now;
                } else {
                    out.deadline = now + RETRY_TIMEOUT;
                    due.push(out.msg.clone());
                }
            }
        }
        due
    }

    /// Report a retransmission-exhausted failure on the oldest pending frame.
    pub fn take_timeout_failure(&mut self) -> Option<MissionError> {
        if let Some(out) = self.outbound.front() {
            if out.retries > MAX_RETRIES {
                self.outbound.pop_front();
                self.state = State::Idle;
                return Some(MissionError::RetriesExhausted);
            }
        }
        None
    }

    /// Drive the protocol's timers. The caller should invoke this every
    /// [`RETRY_TICK`] and send whatever frames are returned.
    ///
    /// Returns retransmits first; when the whole operation runs past
    /// [`OPERATION_TIMEOUT`] it is abandoned with `Timeout` (issues.md #7).
    pub fn on_tick(&mut self, now: Instant) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        if self.is_idle() {
            return (Vec::new(), Vec::new());
        }
        if let Some(deadline) = self.op_deadline {
            if now >= deadline {
                self.abort();
                return (
                    vec![MissionEvent::Failed(MissionError::Timeout("mission"))],
                    Vec::new(),
                );
            }
        }
        let frames = self.retransmit_due(now);
        let mut events = Vec::new();
        if let Some(err) = self.take_timeout_failure() {
            events.push(MissionEvent::Failed(err));
        }
        (events, frames)
    }

    /// Abandon any in-flight operation and drop pending retransmits.
    pub fn abort(&mut self) {
        self.state = State::Idle;
        self.outbound.clear();
        self.op_deadline = None;
    }

    /// Stamp the start of an operation for the overall timeout.
    fn start_operation(&mut self) {
        self.op_deadline = Some(Instant::now() + OPERATION_TIMEOUT);
    }

    fn on_upload_request(&mut self, req_seq: u16) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::Upload {
                items,
                mut next_seq,
            } => {
                let total = items.len() as u16;
                // The FC repeats the same request when it did not receive the
                // item we just sent; resend it rather than failing the upload
                // (issues.md #8).
                if next_seq > 0 && req_seq == next_seq - 1 {
                    let item = items[(next_seq - 1) as usize].clone();
                    let frames = self.queue(
                        MavMessage::MISSION_ITEM_INT(mission_item_to_mav(
                            self.target_sys,
                            self.target_comp,
                            &item,
                        )),
                        "MISSION_ITEM_INT",
                    );
                    self.state = State::Upload { items, next_seq };
                    return (
                        vec![MissionEvent::Progress {
                            operation: MissionOperation::Upload,
                            sent: next_seq,
                            total,
                        }],
                        frames,
                    );
                }
                if req_seq != next_seq {
                    let err = MissionError::SeqMismatch {
                        expected: next_seq,
                        got: req_seq,
                    };
                    self.state = State::Upload { items, next_seq };
                    return (vec![MissionEvent::Failed(err)], Vec::new());
                }
                if next_seq >= total {
                    return (
                        vec![MissionEvent::Failed(MissionError::SeqMismatch {
                            expected: total.saturating_sub(1),
                            got: req_seq,
                        })],
                        Vec::new(),
                    );
                }
                let item = items[next_seq as usize].clone();
                next_seq += 1;
                let frames = self.queue(
                    MavMessage::MISSION_ITEM_INT(mission_item_to_mav(
                        self.target_sys,
                        self.target_comp,
                        &item,
                    )),
                    "MISSION_ITEM_INT",
                );
                self.state = State::Upload { items, next_seq };
                (
                    vec![MissionEvent::Progress {
                        operation: MissionOperation::Upload,
                        sent: next_seq,
                        total,
                    }],
                    frames,
                )
            }
            _ => (Vec::new(), Vec::new()),
        }
    }

    fn on_download_count(&mut self, count: u16) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::Download {
                items,
                next_seq,
                total,
            } => {
                if total != 0 {
                    self.state = State::Download {
                        items,
                        next_seq,
                        total,
                    };
                    return if count == total {
                        (Vec::new(), Vec::new())
                    } else {
                        (
                            vec![MissionEvent::Failed(MissionError::CountMismatch {
                                expected: total,
                                got: count,
                            })],
                            Vec::new(),
                        )
                    };
                }
                if count == 0 {
                    return (
                        vec![MissionEvent::Completed(MissionOperation::Download)],
                        Vec::new(),
                    );
                }
                let msg = MavMessage::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
                    target_system: self.target_sys,
                    target_component: self.target_comp,
                    seq: 0,
                    ..Default::default()
                });
                let frames = self.queue(msg, "MISSION_REQUEST_INT");
                self.state = State::Download {
                    items,
                    next_seq: 0,
                    total: count,
                };
                (Vec::new(), frames)
            }
            _ => (Vec::new(), Vec::new()),
        }
    }

    fn on_download_item(
        &mut self,
        m: &MISSION_ITEM_INT_DATA,
    ) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::Download {
                mut items,
                mut next_seq,
                total,
            } => {
                if total == 0 {
                    self.state = State::Download {
                        items,
                        next_seq,
                        total,
                    };
                    return (
                        vec![MissionEvent::Failed(MissionError::Unexpected(
                            "MISSION_ITEM_INT before MISSION_COUNT",
                        ))],
                        Vec::new(),
                    );
                }
                // A duplicate of the item we just accepted (our request
                // retransmit raced the FC's answer) is ignored.
                if next_seq > 0 && m.seq == next_seq - 1 {
                    self.state = State::Download {
                        items,
                        next_seq,
                        total,
                    };
                    return (Vec::new(), Vec::new());
                }
                if m.seq != next_seq {
                    let err = MissionError::SeqMismatch {
                        expected: next_seq,
                        got: m.seq,
                    };
                    self.state = State::Download {
                        items,
                        next_seq,
                        total,
                    };
                    return (vec![MissionEvent::Failed(err)], Vec::new());
                }
                let item = match mission_item_from_mav(m) {
                    Ok(item) => item,
                    Err(err) => {
                        // A frame we cannot model is terminal: abandon the
                        // download so the UI leaves `busy`.
                        return (vec![MissionEvent::Failed(err)], Vec::new());
                    }
                };
                items.push(item);
                let sent = next_seq + 1;
                next_seq = sent;
                if sent < total {
                    let msg = MavMessage::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
                        target_system: self.target_sys,
                        target_component: self.target_comp,
                        seq: sent,
                        ..Default::default()
                    });
                    let frames = self.queue(msg, "MISSION_REQUEST_INT");
                    self.state = State::Download {
                        items,
                        next_seq,
                        total,
                    };
                    (
                        vec![MissionEvent::Progress {
                            operation: MissionOperation::Download,
                            sent,
                            total,
                        }],
                        frames,
                    )
                } else {
                    let msg = MavMessage::MISSION_ACK(MISSION_ACK_DATA {
                        target_system: self.target_sys,
                        target_component: self.target_comp,
                        mavtype: MavMissionResult::MAV_MISSION_ACCEPTED,
                        ..Default::default()
                    });
                    self.last_downloaded = items;
                    let frames = self.queue(msg, "MISSION_ACK");
                    (
                        vec![MissionEvent::Completed(MissionOperation::Download)],
                        frames,
                    )
                }
            }
            _ => (Vec::new(), Vec::new()),
        }
    }

    fn on_ack(&mut self, ack_type: u8) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        let state = std::mem::replace(&mut self.state, State::Idle);
        let accepted = ack_type == MavMissionResult::MAV_MISSION_ACCEPTED as u8;
        match state {
            State::Upload { items, next_seq } => {
                if accepted && usize::from(next_seq) != items.len() {
                    (
                        vec![MissionEvent::Failed(MissionError::CountMismatch {
                            expected: items.len() as u16,
                            got: next_seq,
                        })],
                        Vec::new(),
                    )
                } else if accepted {
                    (
                        vec![MissionEvent::Completed(MissionOperation::Upload)],
                        Vec::new(),
                    )
                } else {
                    (
                        vec![MissionEvent::Failed(MissionError::AckDenied(ack_type))],
                        Vec::new(),
                    )
                }
            }
            State::Clear => {
                if accepted {
                    (
                        vec![MissionEvent::Completed(MissionOperation::ClearAll)],
                        Vec::new(),
                    )
                } else {
                    (
                        vec![MissionEvent::Failed(MissionError::AckDenied(ack_type))],
                        Vec::new(),
                    )
                }
            }
            other => {
                self.state = other;
                (Vec::new(), Vec::new())
            }
        }
    }

    fn on_set_current(&mut self, seq: u16) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::SetCurrent { seq: requested } if seq == requested => (
                vec![
                    MissionEvent::CurrentChanged { seq },
                    MissionEvent::Completed(MissionOperation::SetCurrent(requested)),
                ],
                Vec::new(),
            ),
            State::SetCurrent { seq: requested } => {
                self.state = State::SetCurrent { seq: requested };
                (vec![MissionEvent::CurrentChanged { seq }], Vec::new())
            }
            other => {
                self.state = other;
                // An unsolicited MISSION_CURRENT is the FC's flight-time
                // broadcast of the active waypoint (S4 fix): the map's
                // blinking target must follow the mission as it flies, not
                // only move when the GCS itself changed the active item.
                (vec![MissionEvent::CurrentChanged { seq }], Vec::new())
            }
        }
    }

    /// Queue a frame for retransmission tracking and return it as "send now".
    fn queue(&mut self, msg: MavMessage, _kind: &'static str) -> Vec<MavMessage> {
        self.outbound.clear();
        self.outbound.push_back(Outbound {
            deadline: Instant::now() + RETRY_TIMEOUT,
            retries: 0,
            msg: msg.clone(),
        });
        vec![msg]
    }
}

/// Map a [`MissionItem`] to a MISSION_ITEM_INT payload.
pub fn mission_item_to_mav(
    target_sys: u8,
    target_comp: u8,
    item: &MissionItem,
) -> MISSION_ITEM_INT_DATA {
    let mut p = [0.0f32; 7];
    for (i, v) in item.params.iter().take(4).enumerate() {
        p[i] = *v;
    }
    let command = ::num_traits::FromPrimitive::from_u16(item.command).unwrap_or_default();
    MISSION_ITEM_INT_DATA {
        param1: p[0],
        param2: p[1],
        param3: p[2],
        param4: p[3],
        x: item.x,
        y: item.y,
        z: item.z,
        seq: item.seq,
        command,
        target_system: target_sys,
        target_component: target_comp,
        frame: item.frame.to_mav(),
        current: if item.current { 1 } else { 0 },
        autocontinue: if item.autocontinue { 1 } else { 0 },
        ..Default::default()
    }
}

/// Map a MISSION_ITEM_INT payload to a [`MissionItem`].
pub fn mission_item_from_mav(m: &MISSION_ITEM_INT_DATA) -> Result<MissionItem, MissionError> {
    if !m.z.is_finite()
        || [m.param1, m.param2, m.param3, m.param4]
            .iter()
            .any(|value| value.is_infinite())
    {
        return Err(MissionError::InvalidRequest {
            message: "non-finite mission altitude/parameter on read-back".into(),
        });
    }
    Ok(MissionItem {
        seq: m.seq,
        frame: MissionFrame::from_mav(m.frame)?,
        command: m.command as u16,
        params: vec![m.param1, m.param2, m.param3, m.param4],
        x: m.x,
        y: m.y,
        z: m.z,
        autocontinue: m.autocontinue != 0,
        current: m.current != 0,
    })
}

/// Extract the target system/component from mission-related messages.
fn target_of(msg: &MavMessage) -> Option<(u8, u8)> {
    use MavMessage as M;
    match msg {
        M::MISSION_COUNT(m) => Some((m.target_system, m.target_component)),
        M::MISSION_ITEM_INT(m) => Some((m.target_system, m.target_component)),
        M::MISSION_REQUEST(m) => Some((m.target_system, m.target_component)),
        M::MISSION_REQUEST_INT(m) => Some((m.target_system, m.target_component)),
        M::MISSION_REQUEST_LIST(m) => Some((m.target_system, m.target_component)),
        M::MISSION_ACK(m) => Some((m.target_system, m.target_component)),
        M::MISSION_CLEAR_ALL(m) => Some((m.target_system, m.target_component)),
        M::MISSION_SET_CURRENT(m) => Some((m.target_system, m.target_component)),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ::mavlink::common::MavMessage as M;

    const FC_SYS: u8 = 1;
    const FC_COMP: u8 = 1;
    const SELF_SYS: u8 = 250;
    const SELF_COMP: u8 = 250;

    fn mk_item(seq: u16, lat_deg: f64, lon_deg: f64, alt_m: f32) -> MissionItem {
        let mut i =
            MissionItem::waypoint(lat_deg, lon_deg, alt_m, MissionFrame::GlobalRelativeAltInt);
        i.seq = seq;
        i
    }

    /// Header a frame from our flight controller carries.
    fn fc_header() -> MavHeader {
        MavHeader {
            system_id: FC_SYS,
            component_id: FC_COMP,
            sequence: 0,
        }
    }

    fn req_int(seq: u16) -> M {
        M::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
            target_system: SELF_SYS,
            target_component: SELF_COMP,
            seq,
            ..Default::default()
        })
    }

    fn item_msg(seq: u16) -> M {
        M::MISSION_ITEM_INT(mission_item_to_mav(
            SELF_SYS,
            SELF_COMP,
            &mk_item(seq, 48.0, -123.0, 50.0),
        ))
    }

    fn ack_msg(accepted: bool) -> M {
        let ty = if accepted {
            MavMissionResult::MAV_MISSION_ACCEPTED
        } else {
            MavMissionResult::MAV_MISSION_ERROR
        };
        M::MISSION_ACK(MISSION_ACK_DATA {
            target_system: SELF_SYS,
            target_component: SELF_COMP,
            mavtype: ty,
            ..Default::default()
        })
    }

    #[test]
    fn upload_round_trip() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let frames = p
            .begin_upload(vec![
                mk_item(0, 48.0, -123.0, 50.0),
                mk_item(1, 48.1, -123.1, 60.0),
            ])
            .unwrap();
        assert_eq!(frames.len(), 1);
        let M::MISSION_COUNT(c) = &frames[0] else {
            panic!("expected MISSION_COUNT")
        };
        assert_eq!(c.count, 2);

        for seq in 0..2u16 {
            let (events, frames) = p.handle(&fc_header(), &req_int(seq));
            assert_eq!(frames.len(), 1);
            assert!(
                matches!(&frames[0], M::MISSION_ITEM_INT(m) if m.seq == seq && m.x == ((48.0 + seq as f64 * 0.1) * 1e7) as i32)
            );
            assert!(events.iter().any(|e| matches!(e, MissionEvent::Progress { operation: MissionOperation::Upload, sent, total } if *sent == seq + 1 && *total == 2)));
        }

        let (events, frames) = p.handle(&fc_header(), &ack_msg(true));
        assert!(frames.is_empty());
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::Upload)));
        assert!(p.is_idle());
    }

    #[test]
    fn upload_seq_mismatch_fails() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0)])
            .unwrap();
        let (events, _) = p.handle(&fc_header(), &req_int(1));
        assert!(matches!(
            &events[0],
            MissionEvent::Failed(MissionError::SeqMismatch {
                expected: 0,
                got: 1
            })
        ));
    }

    #[test]
    fn download_round_trip() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let frames = p.begin_download();
        assert!(matches!(&frames[0], M::MISSION_REQUEST_LIST(_)));

        let (events, frames) = p.handle(
            &fc_header(),
            &M::MISSION_COUNT(MISSION_COUNT_DATA {
                target_system: SELF_SYS,
                target_component: SELF_COMP,
                count: 2,
                ..Default::default()
            }),
        );
        assert!(events.is_empty());
        assert!(matches!(&frames[0], M::MISSION_REQUEST_INT(m) if m.seq == 0));

        for seq in 0..2u16 {
            let (events, frames) = p.handle(&fc_header(), &item_msg(seq));
            assert_eq!(events.len(), 1);
            if seq < 1 {
                assert!(matches!(&frames[0], M::MISSION_REQUEST_INT(m) if m.seq == seq + 1));
                assert!(
                    matches!(&events[0], MissionEvent::Progress { operation: MissionOperation::Download, sent, total } if *sent == seq + 1 && *total == 2)
                );
            } else {
                assert!(matches!(&frames[0], M::MISSION_ACK(_)));
                assert!(matches!(
                    &events[0],
                    MissionEvent::Completed(MissionOperation::Download)
                ));
            }
        }
        assert!(p.is_idle());
    }

    #[test]
    fn download_ignores_items_from_foreign_source() {
        // Issue #9: only frames from our FC may feed the download; a QGC or a
        // second airframe on the same link must not leak items into our plan.
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_download();
        p.handle(
            &fc_header(),
            &M::MISSION_COUNT(MISSION_COUNT_DATA {
                target_system: SELF_SYS,
                target_component: SELF_COMP,
                count: 1,
                ..Default::default()
            }),
        );

        let foreign = MavHeader {
            system_id: 42,
            component_id: 1,
            sequence: 0,
        };
        let (events, frames) = p.handle(&foreign, &item_msg(0));
        assert!(
            events.is_empty(),
            "foreign item must not advance the download"
        );
        assert!(frames.is_empty());
        assert!(p.take_downloaded().is_empty());

        let (events, _frames) = p.handle(&fc_header(), &item_msg(0));
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::Download)));
        let items = p.take_downloaded();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].seq, 0);
        assert!(p.take_downloaded().is_empty(), "take is destructive");
    }

    #[test]
    fn mission_current_from_foreign_source_ignored() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let foreign = MavHeader {
            system_id: 42,
            component_id: 1,
            sequence: 0,
        };
        let (events, frames) = p.handle(
            &foreign,
            &M::MISSION_CURRENT(MISSION_CURRENT_DATA {
                seq: 7,
                ..Default::default()
            }),
        );
        assert!(events.is_empty());
        assert!(frames.is_empty());
    }

    #[test]
    fn download_exposes_items_in_seq_order() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_download();
        p.handle(
            &fc_header(),
            &M::MISSION_COUNT(MISSION_COUNT_DATA {
                target_system: SELF_SYS,
                target_component: SELF_COMP,
                count: 3,
                ..Default::default()
            }),
        );
        for seq in 0..3u16 {
            p.handle(&fc_header(), &item_msg(seq));
        }
        let items = p.take_downloaded();
        assert_eq!(
            items.iter().map(|i| i.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn clear_all_round_trip() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let frames = p.begin_clear();
        assert!(matches!(&frames[0], M::MISSION_CLEAR_ALL(_)));
        let (events, frames) = p.handle(&fc_header(), &ack_msg(true));
        assert!(frames.is_empty());
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::ClearAll)));
        assert!(p.is_idle());
    }

    #[test]
    fn clear_all_denied() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_clear();
        let (events, _) = p.handle(&fc_header(), &ack_msg(false));
        assert!(matches!(
            &events[0],
            MissionEvent::Failed(MissionError::AckDenied(_))
        ));
    }

    #[test]
    fn unsolicited_mission_current_reports_the_active_waypoint() {
        // Flight time: the FC broadcasts MISSION_CURRENT as it progresses;
        // the UI's blinking target must follow (S4 fix) even though the GCS
        // never requested a set-current.
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let (events, frames) = p.handle(
            &fc_header(),
            &M::MISSION_CURRENT(MISSION_CURRENT_DATA {
                seq: 5,
                ..Default::default()
            }),
        );
        assert!(events.contains(&MissionEvent::CurrentChanged { seq: 5 }));
        assert!(frames.is_empty(), "no reply to a broadcast");
    }

    #[test]
    fn set_current_round_trip() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let frames = p.begin_set_current(3);
        assert!(matches!(&frames[0], M::MISSION_SET_CURRENT(_)));
        let (events, _) = p.handle(
            &fc_header(),
            &M::MISSION_CURRENT(MISSION_CURRENT_DATA {
                seq: 3,
                ..Default::default()
            }),
        );
        assert!(events.contains(&MissionEvent::CurrentChanged { seq: 3 }));
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::SetCurrent(3))));
        assert!(p.is_idle());
    }

    #[test]
    fn foreign_target_ignored() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0)])
            .unwrap();
        let (events, frames) = p.handle(
            &fc_header(),
            &M::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
                target_system: 99,
                target_component: 99,
                seq: 0,
                ..Default::default()
            }),
        );
        assert!(events.is_empty() && frames.is_empty());
    }

    #[test]
    fn retransmit_then_exhaust() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0)])
            .unwrap();
        p.handle(&fc_header(), &req_int(0));

        let mut now = Instant::now() + Duration::from_millis(600);
        for i in 1..=MAX_RETRIES {
            let due = p.retransmit_due(now);
            assert_eq!(due.len(), 1, "retry {i} should return the frame");
            now += Duration::from_millis(600);
        }
        let due = p.retransmit_due(now); // 6th tick: retries > MAX
        assert!(due.is_empty());
        assert_eq!(
            p.take_timeout_failure(),
            Some(MissionError::RetriesExhausted)
        );
        assert!(p.is_idle());
    }

    #[test]
    fn encode_decode_round_trip() {
        let item = mk_item(7, 48.64, -123.4, 55.5);
        let msg = mission_item_to_mav(FC_SYS, FC_COMP, &item);
        let back = mission_item_from_mav(&msg).expect("frame supported");
        assert_eq!(back.seq, item.seq);
        assert_eq!(back.x, item.x);
        assert_eq!(back.y, item.y);
        assert_eq!(back.z, item.z);
        assert_eq!(back.command, item.command);
        assert_eq!(back.autocontinue, item.autocontinue);
        assert_eq!(back.current, item.current);
        assert_eq!(back.frame, MissionFrame::GlobalRelativeAltInt);
        assert_eq!(back.params.len(), 4);
        assert_eq!(back.params, item.params);
    }

    #[test]
    fn coordinates_round_trip_bit_exact() {
        // Issue #10: lat/lon are `i32` ×1e7 and must survive a round trip
        // unchanged; the old model copied them through `f32` params.
        let coords = [
            (48.649_300_1, -123.398_200_9),
            (-33.868_819_9, 151.209_290_1),
            (0.000_000_1, -0.000_000_1),
            (89.999_999_9, 179.999_999_9),
        ];
        for (seq, (lat, lon)) in coords.iter().enumerate() {
            let mut item = mk_item(seq as u16, *lat, *lon, 50.0);
            item.params = vec![1.0, 2.0, 3.0, 4.0];
            let raw = mission_item_to_mav(FC_SYS, FC_COMP, &item);
            let back = mission_item_from_mav(&raw).expect("frame supported");
            assert_eq!(back.x, item.x, "lat must be bit-exact");
            assert_eq!(back.y, item.y, "lon must be bit-exact");
            assert_eq!(back.x, (*lat * 1e7) as i32);
            assert_eq!(back.params, item.params);
        }
    }

    #[test]
    fn empty_upload_rejected() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        assert_eq!(p.begin_upload(Vec::new()), Err(MissionError::NoItems));
    }

    #[test]
    fn upload_duplicate_request_is_resent() {
        // Issue #8: the FC repeats a request when it did not receive our item;
        // we must resend instead of reporting a sequence mismatch.
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![
            mk_item(0, 48.0, -123.0, 50.0),
            mk_item(1, 48.1, -123.1, 60.0),
        ])
        .unwrap();

        let (_e, f) = p.handle(&fc_header(), &req_int(0));
        assert!(matches!(&f[0], M::MISSION_ITEM_INT(m) if m.seq == 0));

        let (events, f) = p.handle(&fc_header(), &req_int(0));
        assert!(matches!(&f[0], M::MISSION_ITEM_INT(m) if m.seq == 0));
        assert!(
            !events.iter().any(|e| matches!(e, MissionEvent::Failed(_))),
            "a repeated request is not a failure"
        );

        let (_e, f) = p.handle(&fc_header(), &req_int(1));
        assert!(matches!(&f[0], M::MISSION_ITEM_INT(m) if m.seq == 1));

        let (events, _f) = p.handle(&fc_header(), &ack_msg(true));
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::Upload)));
        assert!(p.is_idle());
    }

    #[test]
    fn upload_duplicate_of_last_item_is_resent() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0)])
            .unwrap();
        p.handle(&fc_header(), &req_int(0)); // next_seq == total == 1

        let (events, f) = p.handle(&fc_header(), &req_int(0));
        assert!(matches!(&f[0], M::MISSION_ITEM_INT(m) if m.seq == 0));
        assert!(!events
            .iter()
            .any(|e| matches!(e, MissionEvent::Completed(_))));

        let (events, _f) = p.handle(&fc_header(), &ack_msg(true));
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::Upload)));
    }

    #[test]
    fn on_tick_retransmits_then_exhausts() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0)])
            .unwrap();
        p.handle(&fc_header(), &req_int(0)); // queue MISSION_ITEM_INT(0)

        let (events, frames) = p.on_tick(Instant::now());
        assert!(events.is_empty() && frames.is_empty(), "nothing due yet");

        let mut now = Instant::now() + Duration::from_millis(600);
        for i in 1..=MAX_RETRIES {
            let (events, frames) = p.on_tick(now);
            assert!(events.is_empty(), "no failure before exhaustion ({i})");
            assert_eq!(frames.len(), 1, "retry {i} should resend");
            now += Duration::from_millis(600);
        }
        let (events, frames) = p.on_tick(now);
        assert!(frames.is_empty());
        assert!(events.contains(&MissionEvent::Failed(MissionError::RetriesExhausted)));
        assert!(p.is_idle());
    }

    #[test]
    fn on_tick_aborts_past_operation_timeout() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_download();
        let (events, frames) =
            p.on_tick(Instant::now() + OPERATION_TIMEOUT + Duration::from_secs(1));
        assert!(frames.is_empty());
        assert!(matches!(
            events[0],
            MissionEvent::Failed(MissionError::Timeout(_))
        ));
        assert!(p.is_idle());
    }

    #[test]
    fn download_duplicate_item_ignored() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_download();
        p.handle(
            &fc_header(),
            &M::MISSION_COUNT(MISSION_COUNT_DATA {
                target_system: SELF_SYS,
                target_component: SELF_COMP,
                count: 2,
                ..Default::default()
            }),
        );
        p.handle(&fc_header(), &item_msg(0));

        let (events, frames) = p.handle(&fc_header(), &item_msg(0));
        assert!(events.is_empty() && frames.is_empty(), "duplicate ignored");

        let (events, _f) = p.handle(&fc_header(), &item_msg(1));
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::Download)));
        assert_eq!(p.take_downloaded().len(), 2);
    }

    #[test]
    fn frame_round_trips_and_unknown_errors() {
        // Issue #11: every supported frame must round-trip; an unknown frame is
        // an error rather than a silent fallback to `Global`.
        let frames = [
            MissionFrame::Mission,
            MissionFrame::GlobalInt,
            MissionFrame::GlobalRelativeAltInt,
            MissionFrame::GlobalTerrainAltInt,
            MissionFrame::LocalNed,
            MissionFrame::LocalEnu,
            MissionFrame::LocalOffsetNed,
            MissionFrame::BodyNed,
        ];
        for f in frames {
            assert_eq!(MissionFrame::from_mav(f.to_mav()).unwrap(), f, "{f:?}");
        }
        assert!(matches!(
            MissionFrame::from_mav(::mavlink::common::MavFrame::MAV_FRAME_BODY_FRD),
            Err(MissionError::UnsupportedFrame(_))
        ));
    }

    #[test]
    fn download_fails_on_unsupported_frame() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_download();
        p.handle(
            &fc_header(),
            &M::MISSION_COUNT(MISSION_COUNT_DATA {
                target_system: SELF_SYS,
                target_component: SELF_COMP,
                count: 1,
                ..Default::default()
            }),
        );
        let mut raw = mission_item_to_mav(SELF_SYS, SELF_COMP, &mk_item(0, 48.0, -123.0, 50.0));
        raw.frame = ::mavlink::common::MavFrame::MAV_FRAME_BODY_FRD;
        let (events, _frames) = p.handle(&fc_header(), &M::MISSION_ITEM_INT(raw));
        assert!(matches!(
            events[0],
            MissionEvent::Failed(MissionError::UnsupportedFrame(_))
        ));
        assert!(p.is_idle());
    }

    #[test]
    fn a_different_current_sequence_is_observation_not_completion() {
        let mut protocol = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        protocol.begin_set_current(3);
        let (events, _) = protocol.handle(
            &fc_header(),
            &M::MISSION_CURRENT(MISSION_CURRENT_DATA {
                seq: 2,
                ..Default::default()
            }),
        );
        assert_eq!(events, [MissionEvent::CurrentChanged { seq: 2 }]);
        assert!(!protocol.is_idle());
        assert_eq!(
            protocol
                .retransmit_due(Instant::now() + RETRY_TIMEOUT)
                .len(),
            1
        );
        let (events, _) = protocol.handle(
            &fc_header(),
            &M::MISSION_CURRENT(MISSION_CURRENT_DATA {
                seq: 3,
                ..Default::default()
            }),
        );
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::SetCurrent(3))));
    }

    #[test]
    fn an_early_ack_or_out_of_range_request_cannot_complete_upload() {
        let mut protocol = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        protocol
            .begin_upload(vec![mk_item(0, 49.0, -123.0, 50.0)])
            .expect("upload");
        let (events, _) = protocol.handle(&fc_header(), &ack_msg(true));
        assert_eq!(
            events,
            [MissionEvent::Failed(MissionError::CountMismatch {
                expected: 1,
                got: 0
            })]
        );
        protocol
            .begin_upload(vec![mk_item(0, 49.0, -123.0, 50.0)])
            .expect("upload");
        protocol.handle(&fc_header(), &req_int(0));
        let (events, _) = protocol.handle(&fc_header(), &req_int(1));
        assert!(events
            .iter()
            .all(|event| !matches!(event, MissionEvent::Completed(_))));
        assert!(matches!(events[0], MissionEvent::Failed(_)));
    }

    #[test]
    fn foreign_request_ack_count_and_wrong_mission_type_are_ignored() {
        let mut protocol = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        protocol
            .begin_upload(vec![mk_item(0, 49.0, -123.0, 50.0)])
            .expect("upload");
        let foreign = MavHeader {
            system_id: FC_SYS + 1,
            ..fc_header()
        };
        for message in [req_int(0), ack_msg(true)] {
            assert_eq!(
                protocol.handle(&foreign, &message),
                (Vec::new(), Vec::new())
            );
        }
        let mut wrong_type = req_int(0);
        if let M::MISSION_REQUEST_INT(request) = &mut wrong_type {
            request.mission_type = MavMissionType::MAV_MISSION_TYPE_FENCE;
        }
        assert_eq!(
            protocol.handle(&fc_header(), &wrong_type),
            (Vec::new(), Vec::new())
        );
        protocol.abort();
        protocol.begin_download();
        let count = M::MISSION_COUNT(MISSION_COUNT_DATA {
            target_system: SELF_SYS,
            target_component: SELF_COMP,
            count: 1,
            ..Default::default()
        });
        assert_eq!(protocol.handle(&foreign, &count), (Vec::new(), Vec::new()));
    }

    #[test]
    fn duplicate_count_preserves_items_and_item_before_count_fails() {
        let mut protocol = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        protocol.begin_download();
        let (events, _) = protocol.handle(&fc_header(), &item_msg(0));
        assert!(matches!(
            events[0],
            MissionEvent::Failed(MissionError::Unexpected(_))
        ));
        protocol.abort();
        protocol.begin_download();
        let count = M::MISSION_COUNT(MISSION_COUNT_DATA {
            target_system: SELF_SYS,
            target_component: SELF_COMP,
            count: 2,
            ..Default::default()
        });
        protocol.handle(&fc_header(), &count);
        protocol.handle(&fc_header(), &item_msg(0));
        assert_eq!(
            protocol.handle(&fc_header(), &count),
            (Vec::new(), Vec::new())
        );
        protocol.handle(&fc_header(), &item_msg(1));
        assert_eq!(protocol.take_downloaded().len(), 2);
    }

    #[test]
    fn non_finite_readback_altitude_cannot_become_a_json_zero_height() {
        let mut raw = mission_item_to_mav(SELF_SYS, SELF_COMP, &mk_item(0, 49.0, -123.0, 50.0));
        raw.z = f32::NAN;
        assert!(matches!(
            mission_item_from_mav(&raw),
            Err(MissionError::InvalidRequest { .. })
        ));
        raw.z = 50.0;
        raw.param4 = f32::NAN;
        assert!(mission_item_from_mav(&raw).expect("unspecified yaw").params[3].is_nan());
    }
}
