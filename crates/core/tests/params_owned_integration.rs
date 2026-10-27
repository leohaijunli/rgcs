//! AF-02b.3: production parameter worker/transport with isolated UDP peers.
//! No physical serial port, FC configuration or motor output is used.

use std::net::SocketAddr;
use std::time::Duration;

use maggcs_core::mavlink::{ConnectionConfig, ConnectionHandle, Endpoint, MavMessage};
use maggcs_core::params::codec::{decode_param_id, encode, IntEncoding};
use maggcs_core::params::model::{ParamType, ParamValue};
use maggcs_core::params::service::{
    ParamError, ParamEvent, ParamService, ParamTransaction, SetResult,
};
use maggcs_core::runtime::{OperationId, TransportGeneration};

const WAIT: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(200);
const FC_SYSTEM: u8 = 42;
const FC_COMPONENT: u8 = 1;

struct Harness {
    peer: tokio::net::UdpSocket,
    address: Option<SocketAddr>,
    handle: ConnectionHandle,
    service: ParamService,
    worker: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn new() -> Self {
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("peer");
        let (handle, _, first) =
            maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
                endpoint: Endpoint::UdpClient {
                    addr: peer.local_addr().expect("peer address"),
                },
                target_system_id: FC_SYSTEM,
                target_component_id: FC_COMPONENT,
                auto_discover_target: false,
                heartbeat_timeout: Duration::from_secs(30),
                reconnect_delay: Duration::from_millis(10),
                ..Default::default()
            })
            .await
            .expect("connection");
        first.await.expect("worker startup").expect("opened");
        let (service, _, worker) =
            ParamService::spawn_with_task(handle.clone(), FC_SYSTEM, FC_COMPONENT);
        Self {
            peer,
            address: None,
            handle,
            service,
            worker,
        }
    }

    fn transaction(&self) -> ParamTransaction {
        ParamTransaction::new(
            OperationId::fresh().expect("operation ID"),
            TransportGeneration::new(self.handle.generation()).expect("generation"),
            WAIT,
        )
        .expect("transaction")
    }

    async fn receive(
        &mut self,
        matches: impl Fn(&MavMessage) -> bool,
        wait: Duration,
    ) -> Option<MavMessage> {
        tokio::time::timeout(wait, async {
            loop {
                let mut bytes = [0; 2048];
                let (size, address) = self.peer.recv_from(&mut bytes).await.expect("receive");
                let (_, message) = ::mavlink::read_v2_msg::<MavMessage, _>(
                    &mut ::mavlink::peek_reader::PeekReader::new(&bytes[..size]),
                )
                .expect("decode");
                if matches(&message) {
                    self.address = Some(address);
                    return message;
                }
            }
        })
        .await
        .ok()
    }

    async fn read_request(&mut self, name: &str) {
        self.receive(|message| matches!(message, MavMessage::PARAM_REQUEST_READ(request) if decode_param_id(&request.param_id) == name), WAIT)
            .await.expect("read request");
    }

    async fn reply(&self, name: &str, value: i64, index: u16, count: u16) {
        let mut id = [0; 16];
        id[..name.len()].copy_from_slice(name.as_bytes());
        let message = MavMessage::PARAM_VALUE(::mavlink::common::PARAM_VALUE_DATA {
            param_id: id.into(),
            param_value: encode(ParamValue::Int(value), IntEncoding::ByteConcat),
            param_type: ::mavlink::common::MavParamType::MAV_PARAM_TYPE_INT32,
            param_index: index,
            param_count: count,
        });
        let mut bytes = Vec::new();
        ::mavlink::write_v2_msg(
            &mut bytes,
            ::mavlink::MavHeader {
                system_id: FC_SYSTEM,
                component_id: FC_COMPONENT,
                sequence: index as u8,
            },
            &message,
        )
        .expect("encode");
        self.peer
            .send_to(&bytes, self.address.expect("GCS address"))
            .await
            .expect("reply");
    }

    async fn shutdown(self) {
        self.service.shutdown().await;
        tokio::time::timeout(WAIT, self.worker)
            .await
            .expect("joined worker")
            .expect("worker result");
        self.handle
            .shutdown_and_wait(WAIT)
            .await
            .expect("transport joined");
    }
}

#[tokio::test]
async fn obsolete_cancel_cannot_stop_a_new_owned_read() {
    let mut h = Harness::new().await;
    let a = h.transaction();
    let read_a = tokio::spawn({
        let service = h.service.clone();
        let transaction = a.clone();
        async move { service.read_owned(transaction, "A").await }
    });
    h.read_request("A").await;
    h.service
        .cancel_owned(&a)
        .await
        .expect("confirmed local cancellation");
    assert_eq!(read_a.await.expect("a task"), Err(ParamError::Cancelled));

    let b = h.transaction();
    let read_b = tokio::spawn({
        let service = h.service.clone();
        async move { service.read_owned(b, "B").await }
    });
    h.read_request("B").await;
    assert!(!h.service.cancel_owned(&a).await.expect("obsolete cancel"));
    assert!(!read_b.is_finished());
    h.reply("B", 22, 0, 1).await;
    assert_eq!(
        read_b.await.expect("b task").expect("b read").value,
        ParamValue::Int(22)
    );
    h.shutdown().await;
}

#[tokio::test]
async fn an_owned_idle_cancel_preserves_a_legacy_client_transaction() {
    let mut h = Harness::new().await;
    let legacy = tokio::spawn({
        let service = h.service.clone();
        async move { service.read("LEGACY").await }
    });
    h.read_request("LEGACY").await;
    let a = h.transaction();
    assert_eq!(
        h.service
            .set_owned(a.clone(), "A", ParamValue::Int(7), ParamType::I32)
            .await,
        Err(ParamError::Busy)
    );
    assert!(!h.service.cancel_owned(&a).await.expect("owner cancel"));
    assert!(!legacy.is_finished());
    h.reply("LEGACY", 33, 0, 1).await;
    assert_eq!(
        legacy
            .await
            .expect("legacy task")
            .expect("legacy read")
            .value,
        ParamValue::Int(33)
    );
    assert!(h
        .receive(|message| matches!(message, MavMessage::PARAM_SET(_)), QUIET)
        .await
        .is_none());
    h.shutdown().await;
}

#[tokio::test]
async fn cancellation_before_dispatch_writes_no_parameter_intent() {
    let mut h = Harness::new().await;
    let a = h.transaction();
    assert!(!h.service.cancel_owned(&a).await.expect("prepared cancel"));
    assert_eq!(
        h.service
            .set_owned(a, "NO_SEND", ParamValue::Int(7), ParamType::I32)
            .await,
        Err(ParamError::Cancelled)
    );
    assert!(h
        .receive(|message| matches!(message, MavMessage::PARAM_SET(_)), QUIET)
        .await
        .is_none());
    h.shutdown().await;
}

#[tokio::test]
async fn a_stale_generation_cannot_write_into_the_reopened_transport() {
    let mut h = Harness::new().await;
    let old = h.transaction();
    let generation = h.handle.generation();
    h.handle.reconnect().await.expect("reconnect");
    tokio::time::timeout(WAIT, async {
        while h.handle.generation() == generation {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("reopen");
    assert_eq!(
        h.service
            .set_owned(old, "OLD", ParamValue::Int(7), ParamType::I32)
            .await,
        Err(ParamError::TargetChanged)
    );
    assert!(h
        .receive(|message| matches!(message, MavMessage::PARAM_SET(_)), QUIET)
        .await
        .is_none());
    let current = h.transaction();
    let set = tokio::spawn({
        let service = h.service.clone();
        async move {
            service
                .set_owned(current, "NEW", ParamValue::Int(8), ParamType::I32)
                .await
        }
    });
    h.receive(|message| matches!(message, MavMessage::PARAM_SET(request) if decode_param_id(&request.param_id) == "NEW"), WAIT).await.expect("current request");
    h.reply("NEW", 8, 0, 1).await;
    assert_eq!(
        set.await.expect("set task").expect("set echo"),
        SetResult::Confirmed(ParamValue::Int(8))
    );
    h.shutdown().await;
}

#[tokio::test]
async fn owned_pull_completion_and_batches_keep_the_original_operation_identity() {
    let mut h = Harness::new().await;
    let transaction = h.transaction();
    let id = transaction.id().clone();
    let generation = transaction.generation().get();
    let mut events = h.service.subscribe_scoped();
    let pull = tokio::spawn({
        let service = h.service.clone();
        async move { service.pull_owned(transaction).await }
    });
    h.receive(
        |message| matches!(message, MavMessage::PARAM_REQUEST_LIST(_)),
        WAIT,
    )
    .await
    .expect("pull request");
    h.reply("P0", 10, 0, 2).await;
    h.reply("P1", 11, 1, 2).await;
    assert!(pull
        .await
        .expect("pull task")
        .expect("pull report")
        .is_complete());
    tokio::time::timeout(WAIT, async {
        loop {
            let event = events.recv().await.expect("scoped event");
            assert_eq!(event.transaction_id.as_ref(), Some(&id));
            assert_eq!(event.generation, generation);
            if matches!(
                event.event,
                ParamEvent::PullDone {
                    received: 2,
                    total: 2
                }
            ) {
                break;
            }
        }
    })
    .await
    .expect("pull event deadline");
    assert_eq!(h.service.cache().len(), 2);
    h.shutdown().await;
}

#[tokio::test]
async fn dropping_a_dispatched_waiter_stops_owned_retries_and_keeps_the_worker_usable() {
    let mut h = Harness::new().await;
    let transaction = h.transaction();
    let waiter = tokio::spawn({
        let service = h.service.clone();
        let transaction = transaction.clone();
        async move {
            service
                .set_owned(transaction, "DROP", ParamValue::Int(4), ParamType::I32)
                .await
        }
    });
    h.receive(|message| matches!(message, MavMessage::PARAM_SET(request) if decode_param_id(&request.param_id) == "DROP"), WAIT).await.expect("first request");
    waiter.abort();
    assert!(waiter.await.expect_err("aborted waiter").is_cancelled());
    h.service
        .cancel_owned(&transaction)
        .await
        .expect("stopped owned worker intent");
    let b = h.transaction();
    let read = tokio::spawn({
        let service = h.service.clone();
        async move { service.read_owned(b, "B").await }
    });
    h.read_request("B").await;
    h.reply("B", 9, 0, 1).await;
    assert_eq!(
        read.await.expect("read task").expect("read result").value,
        ParamValue::Int(9)
    );
    h.shutdown().await;
}
