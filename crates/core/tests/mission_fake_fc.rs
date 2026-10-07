//! Lossy fake flight controller for the mission protocol (issue #25).
//!
//! A deterministic simulator drives a real [`MissionProtocol`] against a fake
//! FC over an in-process, faulty channel: frames can be dropped or duplicated.
//! Time is virtual — the simulator advances a synthetic clock by the retry
//! timeout when no frame is in flight and the exchange is waiting on a
//! retransmission — so the tests run instantly and deterministically.
//!
//! Covered: upload, download, clear, and set-current, plus Phase 1 acceptance
//! (100 waypoints round-trip byte-for-byte; 10% loss still completes) and the
//! duplicate-request / source-filtering regressions from issues.md #7–#9.

#![allow(deprecated)] // MISSION_SET_CURRENT is deprecated but still in use.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ::mavlink::common::{
    MavMissionResult, MISSION_ACK_DATA, MISSION_COUNT_DATA, MISSION_CURRENT_DATA,
    MISSION_ITEM_INT_DATA, MISSION_REQUEST_INT_DATA,
};
use maggcs_core::mavlink::message::{MavHeader, MavMessage};
use maggcs_core::mission::protocol::{
    mission_item_to_mav, MissionEvent, MissionOperation, MissionProtocol, RETRY_TIMEOUT,
};
use maggcs_core::mission::types::{MissionFrame, MissionItem};

const GCS_SYS: u8 = 250;
const GCS_COMP: u8 = 250;
const FC_SYS: u8 = 1;
const FC_COMP: u8 = 1;

const FC_HEADER: MavHeader = MavHeader {
    system_id: FC_SYS,
    component_id: FC_COMP,
    sequence: 0,
};

/// Deterministic channel faults.
#[derive(Debug, Clone, Copy, Default)]
struct Faults {
    /// Drop every `drop_period`-th frame (0 = never).
    drop_period: usize,
    /// Send every `duplicate_period`-th frame twice (0 = never).
    duplicate_period: usize,
}

/// Fake flight controller state.
struct FakeFc {
    items: Vec<MissionItem>,
    upload_total: u16,
    collecting: bool,
    /// Last control frame awaiting a GCS response, re-sent on tick.
    pending: Option<MavMessage>,
}

impl FakeFc {
    fn new(items: Vec<MissionItem>) -> Self {
        Self {
            items,
            upload_total: 0,
            collecting: false,
            pending: None,
        }
    }

    fn ack(&mut self, result: MavMissionResult) -> MavMessage {
        MavMessage::MISSION_ACK(MISSION_ACK_DATA {
            target_system: GCS_SYS,
            target_component: GCS_COMP,
            mavtype: result,
        })
    }

    fn request(&mut self, seq: u16) -> MavMessage {
        MavMessage::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
            target_system: GCS_SYS,
            target_component: GCS_COMP,
            seq,
        })
    }

    fn handle(&mut self, msg: &MavMessage) -> Vec<MavMessage> {
        match msg {
            MavMessage::MISSION_COUNT(m) => {
                if m.count == 0 {
                    self.items.clear();
                    let ack = self.ack(MavMissionResult::MAV_MISSION_ACCEPTED);
                    self.pending = None;
                    return vec![ack];
                }
                self.items.clear();
                self.upload_total = m.count;
                self.collecting = true;
                let req = self.request(0);
                self.pending = Some(req.clone());
                vec![req]
            }
            MavMessage::MISSION_ITEM_INT(item) => {
                if !self.collecting {
                    return Vec::new();
                }
                if item.seq == self.items.len() as u16 {
                    self.items.push(match mission_item_from(item) {
                        Some(i) => i,
                        None => return Vec::new(),
                    });
                }
                if self.items.len() as u16 >= self.upload_total {
                    self.collecting = false;
                    self.pending = None;
                    let ack = self.ack(MavMissionResult::MAV_MISSION_ACCEPTED);
                    return vec![ack];
                }
                // Duplicate or missing: (re)send the next request.
                let req = self.request(self.items.len() as u16);
                self.pending = Some(req.clone());
                vec![req]
            }
            MavMessage::MISSION_REQUEST_LIST(_) => {
                // The GCS drives the download by re-requesting items, so the
                // FC must not keep re-announcing the count (that would restart
                // the download).
                vec![MavMessage::MISSION_COUNT(MISSION_COUNT_DATA {
                    target_system: GCS_SYS,
                    target_component: GCS_COMP,
                    count: self.items.len() as u16,
                })]
            }
            MavMessage::MISSION_REQUEST_INT(req) => {
                let Some(item) = self.items.get(req.seq as usize) else {
                    return Vec::new();
                };
                vec![MavMessage::MISSION_ITEM_INT(mission_item_to_mav(
                    GCS_SYS, GCS_COMP, item,
                ))]
            }
            MavMessage::MISSION_ACK(_) => {
                // End of download.
                self.pending = None;
                Vec::new()
            }
            MavMessage::MISSION_CLEAR_ALL(_) => {
                self.items.clear();
                let ack = self.ack(MavMissionResult::MAV_MISSION_ACCEPTED);
                self.pending = Some(ack.clone());
                vec![ack]
            }
            MavMessage::MISSION_SET_CURRENT(m) => {
                vec![MavMessage::MISSION_CURRENT(MISSION_CURRENT_DATA {
                    seq: m.seq,
                })]
            }
            _ => Vec::new(),
        }
    }

    /// Re-send the last control frame (models an FC that times out waiting).
    fn on_tick(&mut self) -> Vec<MavMessage> {
        self.pending.clone().into_iter().collect()
    }
}

fn mission_item_from(m: &MISSION_ITEM_INT_DATA) -> Option<MissionItem> {
    let mut item = MissionItem::waypoint(0.0, 0.0, m.z, MissionFrame::from_mav(m.frame).ok()?);
    item.seq = m.seq;
    item.command = m.command as u16;
    item.params = vec![m.param1, m.param2, m.param3, m.param4];
    item.x = m.x;
    item.y = m.y;
    item.autocontinue = m.autocontinue != 0;
    item.current = m.current != 0;
    Some(item)
}

/// Bidirectional faulty channel + protocol driver.
struct Sim {
    proto: MissionProtocol,
    fc: FakeFc,
    to_fc: VecDeque<MavMessage>,
    to_gcs: VecDeque<MavMessage>,
    faults: Faults,
    frame_index: usize,
    clock: Instant,
    events: Vec<MissionEvent>,
}

impl Sim {
    fn new(fc: FakeFc, faults: Faults) -> Self {
        Self {
            proto: MissionProtocol::new(GCS_SYS, GCS_COMP, FC_SYS, FC_COMP),
            fc,
            to_fc: VecDeque::new(),
            to_gcs: VecDeque::new(),
            faults,
            frame_index: 0,
            clock: Instant::now(),
            events: Vec::new(),
        }
    }

    /// Apply drop/duplicate faults to one frame, returning what to send.
    fn faulted(&mut self, frame: MavMessage) -> Vec<MavMessage> {
        self.frame_index += 1;
        if self.faults.drop_period > 0 && self.frame_index.is_multiple_of(self.faults.drop_period) {
            return Vec::new();
        }
        let mut out = vec![frame.clone()];
        if self.faults.duplicate_period > 0
            && self
                .frame_index
                .is_multiple_of(self.faults.duplicate_period)
        {
            out.push(frame);
        }
        out
    }

    fn send_to_fc(&mut self, frames: Vec<MavMessage>) {
        for f in frames {
            let faulted = self.faulted(f);
            self.to_fc.extend(faulted);
        }
    }

    fn send_to_gcs(&mut self, frames: Vec<MavMessage>) {
        for f in frames {
            let faulted = self.faulted(f);
            self.to_gcs.extend(faulted);
        }
    }

    /// Move every in-flight frame until the channel is quiet.
    fn settle(&mut self) {
        loop {
            let mut progressed = false;
            while let Some(msg) = self.to_fc.pop_front() {
                progressed = true;
                let responses = self.fc.handle(&msg);
                self.send_to_gcs(responses);
            }
            while let Some(msg) = self.to_gcs.pop_front() {
                progressed = true;
                let (events, frames) = self.proto.handle(&FC_HEADER, &msg);
                self.events.extend(events);
                self.send_to_fc(frames);
            }
            if !progressed {
                return;
            }
        }
    }

    /// Advance virtual time, retransmitting protocol and FC frames.
    fn tick(&mut self) {
        self.clock += RETRY_TIMEOUT + Duration::from_millis(1);
        let (events, frames) = self.proto.on_tick(self.clock);
        self.events.extend(events);
        self.send_to_fc(frames);
        let fc_frames = self.fc.on_tick();
        self.send_to_gcs(fc_frames);
    }

    /// Drive the exchange until `done` matches an event, panicking on failure
    /// events or when the step budget is exhausted.
    fn run_until(
        &mut self,
        what: &str,
        mut done: impl FnMut(&MissionEvent) -> bool,
    ) -> Vec<MissionEvent> {
        for (ticks, _) in (0..200_000u32).enumerate() {
            self.settle();
            if let Some(e) = self.events.iter().find(|e| done(e)) {
                return vec![e.clone()];
            }
            if let Some(MissionEvent::Failed(err)) = self.events.last() {
                panic!("{what} failed after {ticks} ticks: {err}");
            }
            self.tick();
        }
        panic!("{what} did not finish; events: {:?}", self.events);
    }
}

fn waypoints(count: u16) -> Vec<MissionItem> {
    (0..count)
        .map(|seq| {
            let mut item = MissionItem::waypoint(
                49.25 + f64::from(seq) * 0.0001,
                -123.10 - f64::from(seq) * 0.0001,
                40.0 + f32::from(seq % 50),
                MissionFrame::GlobalRelativeAltInt,
            );
            item.seq = seq;
            item
        })
        .collect()
}

#[test]
fn upload_100_waypoints_round_trips_exactly() {
    let items = waypoints(100);
    let mut sim = Sim::new(FakeFc::new(Vec::new()), Faults::default());
    let frames = sim.proto.begin_upload(items.clone()).expect("begin");
    sim.send_to_fc(frames);

    sim.run_until("upload", |e| {
        matches!(e, MissionEvent::Completed(MissionOperation::Upload))
    });

    assert_eq!(sim.fc.items, items, "FC must receive the exact mission");
}

#[test]
fn upload_completes_with_10_percent_loss() {
    let items = waypoints(100);
    let faults = Faults {
        drop_period: 10, // ~10% of all frames dropped
        duplicate_period: 17,
    };
    let mut sim = Sim::new(FakeFc::new(Vec::new()), faults);
    let frames = sim.proto.begin_upload(items.clone()).expect("begin");
    sim.send_to_fc(frames);

    sim.run_until("lossy upload", |e| {
        matches!(e, MissionEvent::Completed(MissionOperation::Upload))
    });

    assert_eq!(sim.fc.items, items);
    assert!(
        sim.events
            .iter()
            .any(|e| matches!(e, MissionEvent::Progress { sent, .. } if *sent < 100)),
        "a retransmit must have re-sent an earlier item"
    );
}

#[test]
fn download_100_waypoints_round_trips_exactly() {
    let items = waypoints(100);
    let mut sim = Sim::new(FakeFc::new(items.clone()), Faults::default());
    let frames = sim.proto.begin_download();
    sim.send_to_fc(frames);

    sim.run_until("download", |e| {
        matches!(e, MissionEvent::Completed(MissionOperation::Download))
    });

    assert_eq!(sim.proto.take_downloaded(), items);
}

#[test]
fn download_completes_with_10_percent_loss() {
    let items = waypoints(100);
    let faults = Faults {
        drop_period: 10,
        duplicate_period: 17,
    };
    let mut sim = Sim::new(FakeFc::new(items.clone()), faults);
    let frames = sim.proto.begin_download();
    sim.send_to_fc(frames);

    sim.run_until("lossy download", |e| {
        matches!(e, MissionEvent::Completed(MissionOperation::Download))
    });

    assert_eq!(sim.proto.take_downloaded(), items);
}

#[test]
fn clear_removes_the_fc_mission() {
    let mut sim = Sim::new(FakeFc::new(waypoints(5)), Faults::default());
    let frames = sim.proto.begin_clear();
    sim.send_to_fc(frames);
    sim.run_until("clear", |e| {
        matches!(e, MissionEvent::Completed(MissionOperation::ClearAll))
    });
    assert!(sim.fc.items.is_empty());
}

#[test]
fn set_current_reports_the_active_waypoint() {
    let mut sim = Sim::new(FakeFc::new(waypoints(5)), Faults::default());
    let frames = sim.proto.begin_set_current(3);
    sim.send_to_fc(frames);
    let done = sim.run_until("set current", |e| {
        matches!(e, MissionEvent::CurrentChanged { .. })
    });
    assert_eq!(done, vec![MissionEvent::CurrentChanged { seq: 3 }]);
}
