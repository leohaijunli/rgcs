//! AF-02b.4 owned Mission exchanges over real, isolated loopback UDP transports.
#![allow(deprecated)] // Keep the reviewed MISSION_SET_CURRENT wire compatibility.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use maggcs_core::mavlink::{
    connection::spawn_connection, ConnectionConfig, ConnectionHandle, Endpoint, MavHeader,
    MavMessage,
};
use maggcs_core::mission::operation_types::MissionProtocolError;
use maggcs_core::mission::protocol::mission_item_to_mav;
use maggcs_core::mission::{
    MissionFrame, MissionIds, MissionItem, MissionService, MissionServiceEvent, MissionTransaction,
};
use maggcs_core::runtime::{OperationId, TransportGeneration};
use mavlink::common::*;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(3);
const NO_FRAME: Duration = Duration::from_millis(120);
const GCS_SYSTEM: u8 = 250;
const GCS_COMPONENT: u8 = 190;
const FC_SYSTEM: u8 = 1;
const FC_COMPONENT: u8 = 1;
const FC_HEADER: MavHeader = MavHeader {
    system_id: FC_SYSTEM,
    component_id: FC_COMPONENT,
    sequence: 0,
};
type Peer = Box<dyn mavlink::AsyncMavConnection<MavMessage> + Sync + Send>;

struct Harness {
    connection: ConnectionHandle,
    service: MissionService,
    events: Option<tokio::sync::mpsc::Receiver<MissionServiceEvent>>,
    peer: Peer,
}

impl Harness {
    async fn new() -> Self {
        for _ in 0..16 {
            let socket = UdpSocket::bind("127.0.0.1:0").expect("ephemeral port");
            let address = socket.local_addr().expect("address");
            drop(socket);
            let config = ConnectionConfig {
                endpoint: Endpoint::UdpListener { addr: address },
                system_id: GCS_SYSTEM,
                component_id: GCS_COMPONENT,
                target_system_id: FC_SYSTEM,
                target_component_id: FC_COMPONENT,
                heartbeat_timeout: Duration::from_secs(30),
                reconnect_delay: Duration::from_millis(30),
                ..Default::default()
            };
            let (connection, _events, opened) = spawn_connection(config).await.expect("connection");
            if !matches!(timeout(WAIT, opened).await, Ok(Ok(Ok(())))) {
                connection.shutdown().await;
                continue;
            }
            let mut peer = mavlink::connect_async::<MavMessage>(&format!("udpout:{address}"))
                .await
                .expect("peer");
            peer.set_protocol_version(mavlink::MavlinkVersion::V2);
            peer.send(&FC_HEADER, &heartbeat())
                .await
                .expect("heartbeat");
            timeout(WAIT, async {
                while connection.target_identity().is_none() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("verified source");
            let (service, events) = MissionService::spawn(
                connection.clone(),
                MissionIds {
                    self_system: GCS_SYSTEM,
                    self_component: GCS_COMPONENT,
                    target_system: FC_SYSTEM,
                    target_component: FC_COMPONENT,
                },
            );
            return Self {
                connection,
                service,
                events: Some(events),
                peer,
            };
        }
        panic!("could not bind isolated Mission fixture");
    }

    fn transaction(&self, budget: Duration) -> MissionTransaction {
        MissionTransaction::new(
            OperationId::fresh().expect("operation id"),
            TransportGeneration::new(self.connection.generation()).expect("generation"),
            budget,
        )
        .expect("transaction")
    }

    async fn receive(
        &mut self,
        pick: impl Fn(&MavMessage) -> bool,
        budget: Duration,
    ) -> Option<MavMessage> {
        timeout(budget, async {
            loop {
                let (_, message) = self.peer.recv().await.expect("receive");
                if pick(&message) {
                    return message;
                }
            }
        })
        .await
        .ok()
    }

    async fn send(&self, message: MavMessage) {
        self.peer
            .send(&FC_HEADER, &message)
            .await
            .expect("peer send");
    }
    async fn ack(&self) {
        self.send(MavMessage::MISSION_ACK(MISSION_ACK_DATA {
            target_system: GCS_SYSTEM,
            target_component: GCS_COMPONENT,
            mavtype: MavMissionResult::MAV_MISSION_ACCEPTED,
            ..Default::default()
        }))
        .await;
    }
    async fn current(&self, seq: u16) {
        self.send(MavMessage::MISSION_CURRENT(MISSION_CURRENT_DATA {
            seq,
            ..Default::default()
        }))
        .await;
    }
    async fn reopen(&self) {
        let old = self.connection.generation();
        self.connection.reconnect().await.expect("reopen request");
        timeout(WAIT, async {
            while self.connection.generation() <= old {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("new generation");
        self.send(heartbeat()).await;
        timeout(WAIT, async {
            while self.connection.target_identity().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("new source verification");
    }
}

fn heartbeat() -> MavMessage {
    MavMessage::HEARTBEAT(HEARTBEAT_DATA {
        mavtype: MavType::MAV_TYPE_QUADROTOR,
        autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
        system_status: MavState::MAV_STATE_STANDBY,
        mavlink_version: 3,
        ..Default::default()
    })
}
fn item(seq: u16) -> MissionItem {
    let mut item = MissionItem::waypoint(
        49.25 + f64::from(seq) * 0.001,
        -123.1,
        50.0,
        MissionFrame::GlobalInt,
    );
    item.seq = seq;
    item
}
fn request(seq: u16) -> MavMessage {
    MavMessage::MISSION_REQUEST_INT(MISSION_REQUEST_INT_DATA {
        target_system: GCS_SYSTEM,
        target_component: GCS_COMPONENT,
        seq,
        ..Default::default()
    })
}
fn is_mission(message: &MavMessage) -> bool {
    matches!(
        message,
        MavMessage::MISSION_COUNT(_)
            | MavMessage::MISSION_REQUEST_LIST(_)
            | MavMessage::MISSION_REQUEST_INT(_)
            | MavMessage::MISSION_ITEM_INT(_)
            | MavMessage::MISSION_CLEAR_ALL(_)
            | MavMessage::MISSION_SET_CURRENT(_)
            | MavMessage::MISSION_ACK(_)
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn all_four_owned_actions_wait_for_their_actual_protocol_verdict() {
    let mut h = Harness::new().await;
    let upload = h.transaction(WAIT);
    let service = h.service.clone();
    let task =
        tokio::spawn(async move { service.upload_owned(upload, vec![item(0), item(1)]).await });
    assert!(
        matches!(h.receive(is_mission, WAIT).await, Some(MavMessage::MISSION_COUNT(count)) if count.count == 2)
    );
    for seq in 0..2 {
        h.send(request(seq)).await;
        assert!(
            matches!(h.receive(is_mission, WAIT).await, Some(MavMessage::MISSION_ITEM_INT(value)) if value.seq == seq)
        );
    }
    assert!(!task.is_finished());
    h.ack().await;
    assert_eq!(task.await.expect("upload driver"), Ok(2));

    let download = h.transaction(WAIT);
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.download_owned(download).await });
    assert!(matches!(
        h.receive(is_mission, WAIT).await,
        Some(MavMessage::MISSION_REQUEST_LIST(_))
    ));
    h.send(MavMessage::MISSION_COUNT(MISSION_COUNT_DATA {
        target_system: GCS_SYSTEM,
        target_component: GCS_COMPONENT,
        count: 2,
        ..Default::default()
    }))
    .await;
    for seq in 0..2 {
        assert!(
            matches!(h.receive(is_mission, WAIT).await, Some(MavMessage::MISSION_REQUEST_INT(value)) if value.seq == seq)
        );
        let mut value = item(seq);
        value.params[3] = f32::NAN;
        h.send(MavMessage::MISSION_ITEM_INT(mission_item_to_mav(
            GCS_SYSTEM,
            GCS_COMPONENT,
            &value,
        )))
        .await;
    }
    assert!(matches!(
        h.receive(is_mission, WAIT).await,
        Some(MavMessage::MISSION_ACK(_))
    ));
    let downloaded = task.await.expect("download driver").expect("plan");
    assert_eq!(downloaded.len(), 2);
    assert!(downloaded[0].params[3].is_nan());
    let restored: Vec<MissionItem> =
        serde_json::from_value(serde_json::to_value(downloaded).expect("JSON"))
            .expect("round-trip");
    assert!(restored[0].params[3].is_nan());

    let clear = h.transaction(WAIT);
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.clear_owned(clear).await });
    assert!(matches!(
        h.receive(is_mission, WAIT).await,
        Some(MavMessage::MISSION_CLEAR_ALL(_))
    ));
    h.ack().await;
    assert_eq!(task.await.expect("clear driver"), Ok(()));

    let current = h.transaction(WAIT);
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.set_current_owned(current, 1).await });
    assert!(
        matches!(h.receive(is_mission, WAIT).await, Some(MavMessage::MISSION_SET_CURRENT(value)) if value.seq == 1)
    );
    h.current(0).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !task.is_finished(),
        "a different passive sequence cannot acknowledge the operation"
    );
    h.current(1).await;
    assert_eq!(task.await.expect("set-current driver"), Ok(1));
    while let Ok(event) = h.events.as_mut().expect("events").try_recv() {
        assert!(
            !matches!(
                event,
                MissionServiceEvent::PlanDownloaded(_) | MissionServiceEvent::Protocol(_)
            ),
            "owned terminal data is not unscoped"
        );
    }
    h.connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_and_completed_transaction_cancellation_cannot_cancel_later_work() {
    let mut h = Harness::new().await;
    let idle = h.transaction(WAIT);
    assert!(!h.service.cancel_owned(&idle).await.expect("idle receipt"));
    let completed = h.transaction(WAIT);
    let copy = completed.clone();
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.clear_owned(copy).await });
    h.receive(is_mission, WAIT).await.expect("clear");
    assert!(!h
        .service
        .cancel_owned(&idle)
        .await
        .expect("old idle cancellation"));
    h.ack().await;
    assert_eq!(task.await.expect("driver"), Ok(()));
    let current = h.transaction(WAIT);
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.set_current_owned(current, 7).await });
    h.receive(is_mission, WAIT).await.expect("current");
    assert!(h
        .service
        .cancel_owned(&completed)
        .await
        .expect("completed receipt"));
    assert!(!task.is_finished());
    h.current(7).await;
    assert_eq!(task.await.expect("driver"), Ok(7));
    h.connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_dispatched_upload_retires_retries_and_quarantines_old_replies() {
    let mut h = Harness::new().await;
    let transaction = h.transaction(WAIT);
    let copy = transaction.clone();
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.upload_owned(copy, vec![item(0)]).await });
    h.receive(is_mission, WAIT).await.expect("upload count");
    let started = Instant::now();
    assert!(h
        .service
        .cancel_owned(&transaction)
        .await
        .expect("retirement"));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        task.await.expect("driver"),
        Err(MissionProtocolError::Cancelled)
    );
    assert!(transaction.may_have_dispatched());
    h.send(request(0)).await;
    h.ack().await;
    assert!(
        h.receive(is_mission, Duration::from_millis(1100))
            .await
            .is_none(),
        "no abandoned item/retry/rollback"
    );
    assert_eq!(
        h.service.clear_owned(h.transaction(WAIT)).await,
        Err(MissionProtocolError::AmbiguousReply)
    );
    assert!(h.receive(is_mission, NO_FRAME).await.is_none());
    h.connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_admission_never_replaces_another_owned_or_legacy_exchange() {
    let mut h = Harness::new().await;
    let transaction = h.transaction(WAIT);
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.clear_owned(transaction).await });
    h.receive(is_mission, WAIT).await.expect("original clear");
    assert_eq!(
        h.service.download_owned(h.transaction(WAIT)).await,
        Err(MissionProtocolError::Busy)
    );
    h.service.set_current(3).await.expect("legacy enqueue");
    assert!(h.receive(is_mission, NO_FRAME).await.is_none());
    h.ack().await;
    assert_eq!(task.await.expect("driver"), Ok(()));
    h.service.download().await.expect("legacy read");
    h.receive(is_mission, WAIT)
        .await
        .expect("legacy request list");
    assert_eq!(
        h.service.clear_owned(h.transaction(WAIT)).await,
        Err(MissionProtocolError::Busy)
    );
    h.send(MavMessage::MISSION_COUNT(MISSION_COUNT_DATA {
        target_system: GCS_SYSTEM,
        target_component: GCS_COMPONENT,
        count: 0,
        ..Default::default()
    }))
    .await;
    h.connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn generation_replacement_expires_prepared_and_active_intent_without_replay() {
    let mut h = Harness::new().await;
    let prepared = h.transaction(WAIT);
    let active = h.transaction(WAIT);
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.download_owned(active).await });
    h.receive(is_mission, WAIT).await.expect("old download");
    h.reopen().await;
    assert!(matches!(
        task.await.expect("old driver"),
        Err(MissionProtocolError::TargetChanged | MissionProtocolError::Stopped)
    ));
    assert_eq!(
        h.service.clear_owned(prepared).await,
        Err(MissionProtocolError::TargetChanged)
    );
    assert!(h.receive(is_mission, NO_FRAME).await.is_none());
    let current = h.transaction(WAIT);
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.clear_owned(current).await });
    assert!(matches!(
        h.receive(is_mission, WAIT).await,
        Some(MavMessage::MISSION_CLEAR_ALL(_))
    ));
    h.ack().await;
    assert_eq!(task.await.expect("new driver"), Ok(()));
    h.connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn saturated_and_closed_observers_cannot_stall_owned_replies_or_cancellation() {
    let mut h = Harness::new().await;
    for seq in 0..32 {
        let transaction = h.transaction(WAIT);
        let service = h.service.clone();
        let task = tokio::spawn(async move { service.set_current_owned(transaction, seq).await });
        h.receive(is_mission, WAIT).await.expect("set current");
        h.current(seq).await;
        assert_eq!(task.await.expect("driver"), Ok(seq));
    }
    assert!(h.service.dropped_events() > 0);
    drop(h.events.take());
    let transaction = h.transaction(WAIT);
    let copy = transaction.clone();
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.clear_owned(copy).await });
    h.receive(is_mission, WAIT)
        .await
        .expect("clear after observer close");
    assert!(h.service.cancel_owned(&transaction).await.expect("receipt"));
    assert_eq!(
        task.await.expect("driver"),
        Err(MissionProtocolError::Cancelled)
    );
    h.connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_source_destination_and_type_cannot_feed_an_owned_upload() {
    let mut h = Harness::new().await;
    let transaction = h.transaction(WAIT);
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.upload_owned(transaction, vec![item(0)]).await });
    h.receive(is_mission, WAIT).await.expect("count");
    h.peer
        .send(
            &MavHeader {
                system_id: FC_SYSTEM + 1,
                ..FC_HEADER
            },
            &request(0),
        )
        .await
        .expect("foreign source");
    let mut wrong_destination = request(0);
    if let MavMessage::MISSION_REQUEST_INT(value) = &mut wrong_destination {
        value.target_component += 1;
    }
    h.send(wrong_destination).await;
    let mut wrong_type = request(0);
    if let MavMessage::MISSION_REQUEST_INT(value) = &mut wrong_type {
        value.mission_type = MavMissionType::MAV_MISSION_TYPE_FENCE;
    }
    h.send(wrong_type).await;
    assert!(h.receive(is_mission, NO_FRAME).await.is_none());
    h.send(request(0)).await;
    h.receive(is_mission, WAIT)
        .await
        .expect("only valid request writes item");
    h.peer
        .send(
            &MavHeader {
                component_id: FC_COMPONENT + 1,
                ..FC_HEADER
            },
            &MavMessage::MISSION_ACK(MISSION_ACK_DATA {
                target_system: GCS_SYSTEM,
                target_component: GCS_COMPONENT,
                mavtype: MavMissionResult::MAV_MISSION_ACCEPTED,
                ..Default::default()
            }),
        )
        .await
        .expect("foreign ACK");
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!task.is_finished());
    h.ack().await;
    assert_eq!(task.await.expect("driver"), Ok(1));
    h.connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn expiry_and_dropped_waiter_stop_retries_and_preserve_reply_ambiguity() {
    let mut h = Harness::new().await;
    let transaction = h.transaction(Duration::from_millis(80));
    let copy = transaction.clone();
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.clear_owned(copy).await });
    h.receive(is_mission, WAIT).await.expect("first clear");
    assert_eq!(
        task.await.expect("driver"),
        Err(MissionProtocolError::TimedOut)
    );
    assert!(h.service.cancel_owned(&transaction).await.expect("retired"));
    assert!(h
        .receive(is_mission, Duration::from_millis(600))
        .await
        .is_none());
    h.reopen().await;
    let transaction = h.transaction(WAIT);
    let copy = transaction.clone();
    let service = h.service.clone();
    let task = tokio::spawn(async move { service.download_owned(copy).await });
    h.receive(is_mission, WAIT).await.expect("read request");
    task.abort();
    assert!(task.await.expect_err("dropped waiter").is_cancelled());
    assert!(h
        .service
        .cancel_owned(&transaction)
        .await
        .expect("dropped request retired"));
    assert_eq!(
        h.service.clear_owned(h.transaction(WAIT)).await,
        Err(MissionProtocolError::AmbiguousReply)
    );
    assert!(h
        .receive(is_mission, Duration::from_millis(600))
        .await
        .is_none());
    h.connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_cancelled_invalid_and_resubmitted_capabilities_do_not_write() {
    let mut h = Harness::new().await;
    let cancelled = h.transaction(WAIT);
    h.service
        .cancel_owned(&cancelled)
        .await
        .expect("pre-cancel");
    assert_eq!(
        h.service.clear_owned(cancelled.clone()).await,
        Err(MissionProtocolError::Cancelled)
    );
    assert_eq!(
        h.service.clear_owned(cancelled).await,
        Err(MissionProtocolError::AlreadySubmitted)
    );
    let mut invalid = item(0);
    invalid.command = u16::MAX;
    assert!(matches!(
        h.service
            .upload_owned(h.transaction(WAIT), vec![invalid])
            .await,
        Err(MissionProtocolError::InvalidRequest { .. })
    ));
    assert!(h.receive(is_mission, NO_FRAME).await.is_none());
    h.connection.shutdown().await;
}
