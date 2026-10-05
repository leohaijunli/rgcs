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
    MavMessage, MavMissionResult, MISSION_ACK_DATA, MISSION_CLEAR_ALL_DATA, MISSION_COUNT_DATA,
    MISSION_ITEM_INT_DATA, MISSION_REQUEST_INT_DATA, MISSION_REQUEST_LIST_DATA,
    MISSION_SET_CURRENT_DATA,
};
#[cfg(test)]
use ::mavlink::common::MISSION_CURRENT_DATA;
use ::mavlink::MavHeader;

use super::error::MissionError;
use super::types::{MissionFrame, MissionItem};

/// Timeout before a sent frame is retransmitted.
pub const RETRY_TIMEOUT: Duration = Duration::from_millis(500);
/// Maximum retransmissions before giving up.
pub const MAX_RETRIES: u32 = 5;

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
    Upload { items: Vec<MissionItem>, next_seq: u16 },
    Download { items: Vec<MissionItem>, next_seq: u16, total: u16 },
    Clear,
    SetCurrent { seq: u16 },
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
        if items.is_empty() {
            return Err(MissionError::NoItems);
        }
        let count = items.len() as u16;
        self.state = State::Upload { items, next_seq: 0 };
        let msg = MavMessage::MISSION_COUNT(MISSION_COUNT_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
            count,
        });
        Ok(self.queue(msg, "MISSION_COUNT"))
    }

    /// Start downloading the FC mission; returns the frames to send.
    pub fn begin_download(&mut self) -> Vec<MavMessage> {
        self.state = State::Download {
            items: Vec::new(),
            next_seq: 0,
            total: 0,
        };
        let msg = MavMessage::MISSION_REQUEST_LIST(MISSION_REQUEST_LIST_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
        });
        self.queue(msg, "MISSION_REQUEST_LIST")
    }

    /// Clear the FC mission; returns the frames to send.
    pub fn begin_clear(&mut self) -> Vec<MavMessage> {
        self.state = State::Clear;
        let msg = MavMessage::MISSION_CLEAR_ALL(MISSION_CLEAR_ALL_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
        });
        self.queue(msg, "MISSION_CLEAR_ALL")
    }

    /// Set the active waypoint; returns the frames to send.
    pub fn begin_set_current(&mut self, seq: u16) -> Vec<MavMessage> {
        self.state = State::SetCurrent { seq };
        let msg = MavMessage::MISSION_SET_CURRENT(MISSION_SET_CURRENT_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
            seq,
        });
        self.queue(msg, "MISSION_SET_CURRENT")
    }

    /// Handle an incoming frame. Returns events and frames to send.
    #[allow(deprecated)]
    pub fn handle(&mut self, header: &MavHeader, msg: &MavMessage) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        if let Some((sys, comp)) = target_of(msg) {
            let to_us = (sys == self.self_sys && comp == self.self_comp)
                || (sys == 0 && comp == 0);
            if !to_us {
                return (Vec::new(), Vec::new());
            }
        }
        let _ = header;
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

    fn on_upload_request(&mut self, req_seq: u16) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::Upload { items, mut next_seq } => {
                let total = items.len() as u16;
                if req_seq != next_seq {
                    let err = MissionError::SeqMismatch { expected: next_seq, got: req_seq };
                    self.state = State::Upload { items, next_seq };
                    return (vec![MissionEvent::Failed(err)], Vec::new());
                }
                if next_seq >= total {
                    return (vec![MissionEvent::Completed(MissionOperation::Upload)], Vec::new());
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
            State::Download { mut items, .. } => {
                items.clear();
                if count == 0 {
                    return (vec![MissionEvent::Completed(MissionOperation::Download)], Vec::new());
                }
                let msg = MavMessage::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
                    target_system: self.target_sys,
                    target_component: self.target_comp,
                    seq: 0,
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

    fn on_download_item(&mut self, m: &MISSION_ITEM_INT_DATA) -> (Vec<MissionEvent>, Vec<MavMessage>) {
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::Download { mut items, mut next_seq, total } => {
                if m.seq != next_seq {
                    let err = MissionError::SeqMismatch { expected: next_seq, got: m.seq };
                    self.state = State::Download { items, next_seq, total };
                    return (vec![MissionEvent::Failed(err)], Vec::new());
                }
                items.push(mission_item_from_mav(m));
                let sent = next_seq + 1;
                next_seq = sent;
                if sent < total {
                    let msg = MavMessage::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
                        target_system: self.target_sys,
                        target_component: self.target_comp,
                        seq: sent,
                    });
                    let frames = self.queue(msg, "MISSION_REQUEST_INT");
                    self.state = State::Download { items, next_seq, total };
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
                    });
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
            State::Upload { .. } => {
                if accepted {
                    (vec![MissionEvent::Completed(MissionOperation::Upload)], Vec::new())
                } else {
                    (vec![MissionEvent::Failed(MissionError::AckDenied(ack_type))], Vec::new())
                }
            }
            State::Clear => {
                if accepted {
                    (vec![MissionEvent::Completed(MissionOperation::ClearAll)], Vec::new())
                } else {
                    (vec![MissionEvent::Failed(MissionError::AckDenied(ack_type))], Vec::new())
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
            State::SetCurrent { seq: requested } => (
                vec![
                    MissionEvent::CurrentChanged { seq },
                    MissionEvent::Completed(MissionOperation::SetCurrent(requested)),
                ],
                Vec::new(),
            ),
            other => {
                self.state = other;
                (Vec::new(), Vec::new())
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
    for (i, v) in item.params.iter().take(7).enumerate() {
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
    }
}

/// Map a MISSION_ITEM_INT payload to a [`MissionItem`].
pub fn mission_item_from_mav(m: &MISSION_ITEM_INT_DATA) -> MissionItem {
    MissionItem {
        seq: m.seq,
        frame: MissionFrame::from_mav(m.frame),
        command: m.command as u16,
        params: vec![m.param1, m.param2, m.param3, m.param4, m.x as f32, m.y as f32, m.z],
        x: m.x,
        y: m.y,
        z: m.z,
        autocontinue: m.autocontinue != 0,
        current: m.current != 0,
    }
}

/// Extract the target system/component from mission-related messages.
fn target_of(msg: &MavMessage) -> Option<(u8, u8)> {
    use MavMessage as M;
    match msg {
        M::MISSION_COUNT(m) => Some((m.target_system, m.target_component)),
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
        let mut i = MissionItem::waypoint(lat_deg, lon_deg, alt_m, MissionFrame::GlobalRelativeAlt);
        i.seq = seq;
        i
    }

    fn req_int(seq: u16) -> M {
        M::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
            target_system: SELF_SYS,
            target_component: SELF_COMP,
            seq,
        })
    }

    fn item_msg(seq: u16) -> M {
        M::MISSION_ITEM_INT(mission_item_to_mav(FC_SYS, FC_COMP, &mk_item(seq, 48.0, -123.0, 50.0)))
    }

    fn ack_msg(accepted: bool) -> M {
        let ty = if accepted { MavMissionResult::MAV_MISSION_ACCEPTED } else { MavMissionResult::MAV_MISSION_ERROR };
        M::MISSION_ACK(MISSION_ACK_DATA {
            target_system: SELF_SYS,
            target_component: SELF_COMP,
            mavtype: ty,
        })
    }

    #[test]
    fn upload_round_trip() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let frames = p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0), mk_item(1, 48.1, -123.1, 60.0)]).unwrap();
        assert_eq!(frames.len(), 1);
        let M::MISSION_COUNT(c) = &frames[0] else { panic!("expected MISSION_COUNT") };
        assert_eq!(c.count, 2);

        for seq in 0..2u16 {
            let (events, frames) = p.handle(&MavHeader::default(), &req_int(seq));
            assert_eq!(frames.len(), 1);
            assert!(matches!(&frames[0], M::MISSION_ITEM_INT(m) if m.seq == seq && m.x == ((48.0 + seq as f64 * 0.1) * 1e7) as i32));
            assert!(events.iter().any(|e| matches!(e, MissionEvent::Progress { operation: MissionOperation::Upload, sent, total } if *sent == seq + 1 && *total == 2)));
        }

        let (events, frames) = p.handle(&MavHeader::default(), &ack_msg(true));
        assert!(frames.is_empty());
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::Upload)));
        assert!(p.is_idle());
    }

    #[test]
    fn upload_seq_mismatch_fails() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0)]).unwrap();
        let (events, _) = p.handle(&MavHeader::default(), &req_int(1));
        assert!(matches!(&events[0], MissionEvent::Failed(MissionError::SeqMismatch { expected: 0, got: 1 })));
    }

    #[test]
    fn download_round_trip() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let frames = p.begin_download();
        assert!(matches!(&frames[0], M::MISSION_REQUEST_LIST(_)));

        let (events, frames) = p.handle(&MavHeader::default(), &M::MISSION_COUNT(MISSION_COUNT_DATA {
            target_system: SELF_SYS, target_component: SELF_COMP, count: 2,
        }));
        assert!(events.is_empty());
        assert!(matches!(&frames[0], M::MISSION_REQUEST_INT(m) if m.seq == 0));

        for seq in 0..2u16 {
            let (events, frames) = p.handle(&MavHeader::default(), &item_msg(seq));
            assert_eq!(events.len(), 1);
            if seq < 1 {
                assert!(matches!(&frames[0], M::MISSION_REQUEST_INT(m) if m.seq == seq + 1));
                assert!(matches!(&events[0], MissionEvent::Progress { operation: MissionOperation::Download, sent, total } if *sent == seq + 1 && *total == 2));
            } else {
                assert!(matches!(&frames[0], M::MISSION_ACK(_)));
                assert!(matches!(&events[0], MissionEvent::Completed(MissionOperation::Download)));
            }
        }
        assert!(p.is_idle());
    }

    #[test]
    fn clear_all_round_trip() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let frames = p.begin_clear();
        assert!(matches!(&frames[0], M::MISSION_CLEAR_ALL(_)));
        let (events, frames) = p.handle(&MavHeader::default(), &ack_msg(true));
        assert!(frames.is_empty());
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::ClearAll)));
        assert!(p.is_idle());
    }

    #[test]
    fn clear_all_denied() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_clear();
        let (events, _) = p.handle(&MavHeader::default(), &ack_msg(false));
        assert!(matches!(&events[0], MissionEvent::Failed(MissionError::AckDenied(_))));
    }

    #[test]
    fn set_current_round_trip() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        let frames = p.begin_set_current(3);
        assert!(matches!(&frames[0], M::MISSION_SET_CURRENT(_)));
        let (events, _) = p.handle(&MavHeader::default(), &M::MISSION_CURRENT(MISSION_CURRENT_DATA {
            seq: 3,
        }));
        assert!(events.contains(&MissionEvent::CurrentChanged { seq: 3 }));
        assert!(events.contains(&MissionEvent::Completed(MissionOperation::SetCurrent(3))));
        assert!(p.is_idle());
    }

    #[test]
    fn foreign_target_ignored() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0)]).unwrap();
        let (events, frames) = p.handle(&MavHeader::default(), &M::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
            target_system: 99, target_component: 99, seq: 0,
        }));
        assert!(events.is_empty() && frames.is_empty());
    }

    #[test]
    fn retransmit_then_exhaust() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        p.begin_upload(vec![mk_item(0, 48.0, -123.0, 50.0)]).unwrap();
        p.handle(&MavHeader::default(), &req_int(0));

        let mut now = Instant::now() + Duration::from_millis(600);
        for i in 1..=MAX_RETRIES {
            let due = p.retransmit_due(now);
            assert_eq!(due.len(), 1, "retry {i} should return the frame");
            now += Duration::from_millis(600);
        }
        let due = p.retransmit_due(now); // 6th tick: retries > MAX
        assert!(due.is_empty());
        assert_eq!(p.take_timeout_failure(), Some(MissionError::RetriesExhausted));
        assert!(p.is_idle());
    }

    #[test]
    fn encode_decode_round_trip() {
        let item = mk_item(7, 48.64, -123.4, 55.5);
        let msg = mission_item_to_mav(FC_SYS, FC_COMP, &item);
        let back = mission_item_from_mav(&msg);
        assert_eq!(back.seq, item.seq);
        assert_eq!(back.x, item.x);
        assert_eq!(back.y, item.y);
        assert_eq!(back.z, item.z);
        assert_eq!(back.command, item.command);
        assert_eq!(back.autocontinue, item.autocontinue);
        assert_eq!(back.current, item.current);
        assert!(matches!(back.frame, MissionFrame::GlobalRelativeAlt | MissionFrame::GlobalRelativeAltInt));
        assert_eq!(back.params[4], item.x as f32);
        assert_eq!(back.params[5], item.y as f32);
        assert_eq!(back.params[6], item.z);
    }

    #[test]
    fn empty_upload_rejected() {
        let mut p = MissionProtocol::new(SELF_SYS, SELF_COMP, FC_SYS, FC_COMP);
        assert_eq!(p.begin_upload(Vec::new()), Err(MissionError::NoItems));
    }
}
