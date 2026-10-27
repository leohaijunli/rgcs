//! One bounded owner-aware MAVLink log worker on an existing connection.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, watch};

use super::operation_types::LogProtocolError;
use super::protocol::{LogEntryInfo, LogEvent, LogProtocol};
use super::transaction::OwnedWait;
use super::LogTransaction;
use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::message::{MavMessage, MessageEnvelope};
use crate::mavlink::router::{MessageRoute, RoutedEvents};
use crate::runtime::{OperationId, TransportGeneration};

const COMMAND_CAPACITY: usize = 4;
const EVENT_CAPACITY: usize = 16;
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const TICK: Duration = Duration::from_millis(200);
const CLEANUP_BUDGET: Duration = Duration::from_millis(750);
const QUIET_PERIOD: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogServiceEvent {
    Progress {
        operation_id: OperationId,
        generation: TransportGeneration,
        entry: LogEntryInfo,
        fetched: u32,
        total: u32,
    },
}

enum Action {
    List,
    Download(LogEntryInfo),
}
enum ResultValue {
    Listed(Vec<LogEntryInfo>),
    Downloaded { entry: LogEntryInfo, bytes: Vec<u8> },
}

struct OwnedLog {
    action: Option<Action>,
    transaction: LogTransaction,
    reply: Option<oneshot::Sender<Result<ResultValue, LogProtocolError>>>,
}
impl Drop for OwnedLog {
    fn drop(&mut self) {
        self.transaction.finish();
    }
}

struct Pending {
    action: Action,
    started_at: Instant,
    generation: u64,
    dispatched: bool,
    owned: Box<OwnedLog>,
}
impl Pending {
    fn check(&self, handle: &ConnectionHandle) -> Result<(), LogProtocolError> {
        self.owned.transaction.check(handle)
    }
    fn entry(&self) -> Option<&LogEntryInfo> {
        match &self.action {
            Action::Download(entry) => Some(entry),
            Action::List => None,
        }
    }
}

#[derive(Clone)]
pub struct LogService {
    commands: mpsc::Sender<Box<OwnedLog>>,
    dropped_events: Arc<AtomicU64>,
    shutdown: watch::Sender<bool>,
}

impl LogService {
    pub fn spawn(
        handle: ConnectionHandle,
        target_system: u8,
        target_component: u8,
    ) -> (Self, mpsc::Receiver<LogServiceEvent>) {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (events, event_receiver) = mpsc::channel(EVENT_CAPACITY);
        let (shutdown, shutdown_rx) = watch::channel(false);
        let dropped_events = Arc::new(AtomicU64::new(0));
        tokio::spawn(run(
            handle,
            target_system,
            target_component,
            receiver,
            events,
            dropped_events.clone(),
            shutdown_rx,
        ));
        (
            Self {
                commands,
                dropped_events,
                shutdown,
            },
            event_receiver,
        )
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    pub async fn list_owned(
        &self,
        transaction: LogTransaction,
    ) -> Result<Vec<LogEntryInfo>, LogProtocolError> {
        match self.send(transaction, Action::List).await? {
            ResultValue::Listed(entries) => Ok(entries),
            _ => Err(LogProtocolError::InvalidRequest {
                message: "list received a download result".into(),
            }),
        }
    }

    pub async fn download_owned(
        &self,
        transaction: LogTransaction,
        entry: LogEntryInfo,
    ) -> Result<Vec<u8>, LogProtocolError> {
        if entry.size > super::operation_types::MAX_LOG_DOWNLOAD_BYTES {
            return Err(LogProtocolError::InvalidRequest {
                message: "log exceeds the 256 MiB buffered-download limit".into(),
            });
        }
        match self
            .send(transaction, Action::Download(entry.clone()))
            .await?
        {
            ResultValue::Downloaded {
                entry: actual,
                bytes,
            } if actual == entry => Ok(bytes),
            _ => Err(LogProtocolError::InvalidRequest {
                message: "download result identity changed".into(),
            }),
        }
    }

    pub async fn cancel_owned(
        &self,
        transaction: &LogTransaction,
    ) -> Result<bool, LogProtocolError> {
        transaction.cancel_and_wait().await
    }

    async fn send(
        &self,
        transaction: LogTransaction,
        action: Action,
    ) -> Result<ResultValue, LogProtocolError> {
        transaction.submit()?;
        let _wait = OwnedWait(transaction.clone());
        let (reply, response) = oneshot::channel();
        let command = Box::new(OwnedLog {
            action: Some(action),
            transaction: transaction.clone(),
            reply: Some(reply),
        });
        tokio::select! {
            biased;
            error = transaction.stopped() => Err(error),
            result = async {
                self.commands.send(command).await.map_err(|_| LogProtocolError::Stopped)?;
                response.await.map_err(|_| LogProtocolError::Stopped)?
            } => result,
        }
    }
}

struct Driver {
    protocol: LogProtocol,
    pending: Option<Pending>,
    generation: u64,
    quarantined: bool,
    events: mpsc::Sender<LogServiceEvent>,
    dropped_events: Arc<AtomicU64>,
}

impl Driver {
    fn observe_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.quarantined = false;
        }
    }
    fn publish(&self, event: LogServiceEvent) {
        if self.events.try_send(event).is_err() {
            let _ = self
                .dropped_events
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    Some(count.saturating_add(1))
                });
        }
    }
    fn begin(&mut self, mut request: Box<OwnedLog>, handle: &ConnectionHandle) -> Vec<MavMessage> {
        let Some(action) = request.action.take() else {
            return Vec::new();
        };
        let pending = Pending {
            generation: request.transaction.generation().get(),
            started_at: Instant::now(),
            dispatched: false,
            action,
            owned: request,
        };
        let admission = pending.check(handle).and_then(|_| {
            if self.pending.is_some() {
                Err(LogProtocolError::Busy)
            } else if self.quarantined {
                Err(LogProtocolError::CleanupTimeout)
            } else {
                Ok(())
            }
        });
        if let Err(error) = admission {
            let mut owned = pending.owned;
            owned.transaction.finish();
            if let Some(reply) = owned.reply.take() {
                let _ = reply.send(Err(error));
            }
            return Vec::new();
        }
        if let Action::Download(entry) = &pending.action {
            if entry.size == 0 {
                let entry = entry.clone();
                self.pending = Some(pending);
                self.finish(Ok(ResultValue::Downloaded {
                    entry,
                    bytes: Vec::new(),
                }));
                return Vec::new();
            }
        }
        let frames = match &pending.action {
            Action::List => self.protocol.begin_list(),
            Action::Download(entry) => self.protocol.begin_download(entry.id, entry.size),
        };
        self.pending = Some(pending);
        frames
    }
    fn finish(&mut self, result: Result<ResultValue, LogProtocolError>) {
        self.protocol.cancel();
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        let cleanup = match &result {
            Err(
                error @ (LogProtocolError::CleanupTimeout
                | LogProtocolError::TargetChanged
                | LogProtocolError::Stopped),
            ) if pending.dispatched => Err(error.clone()),
            _ => Ok(()),
        };
        pending.owned.transaction.finish_cleanup(cleanup);
        if let Some(reply) = pending.owned.reply.take() {
            let _ = reply.send(result);
        }
    }
    fn process(&mut self, events: Vec<LogEvent>) {
        for event in events {
            match event {
                LogEvent::List { entries } => self.finish(Ok(ResultValue::Listed(entries))),
                LogEvent::Progress { id, fetched, total } => {
                    let Some(pending) = &self.pending else {
                        continue;
                    };
                    let Some(entry) = pending.entry() else {
                        continue;
                    };
                    if entry.id != id {
                        continue;
                    }
                    self.publish(LogServiceEvent::Progress {
                        operation_id: pending.owned.transaction.id().clone(),
                        generation: pending.owned.transaction.generation(),
                        entry: entry.clone(),
                        fetched,
                        total,
                    });
                }
                LogEvent::Downloaded { id, bytes } => {
                    let Some(entry) = self.pending.as_ref().and_then(Pending::entry).cloned()
                    else {
                        continue;
                    };
                    if entry.id == id {
                        self.finish(Ok(ResultValue::Downloaded { entry, bytes }));
                    }
                }
            }
        }
    }
}

async fn dispatch(
    handle: &ConnectionHandle,
    pending: &mut Pending,
    frames: Vec<MavMessage>,
) -> Result<(), LogProtocolError> {
    for frame in frames {
        pending.check(handle)?;
        pending.dispatched = true;
        pending.owned.transaction.mark_dispatched();
        let deadline =
            (tokio::time::Instant::now() + WRITE_TIMEOUT).min(pending.owned.transaction.deadline);
        tokio::select! {
            biased;
            error = pending.owned.transaction.stopped() => return Err(error),
            result = handle.send_in_generation(frame, pending.generation, deadline) => result,
        }
        .map_err(|_| {
            if handle.generation() != pending.generation {
                LogProtocolError::TargetChanged
            } else {
                LogProtocolError::Stopped
            }
        })?;
    }
    Ok(())
}

async fn wait_for_quiet(
    route: &mut RoutedEvents,
    deadline: tokio::time::Instant,
) -> Result<(), LogProtocolError> {
    let mut quiet = tokio::time::Instant::now() + QUIET_PERIOD;
    loop {
        let wake = deadline.min(quiet);
        match tokio::time::timeout_at(wake, route.recv()).await {
            Err(_) if tokio::time::Instant::now() >= quiet => return Ok(()),
            Err(_) => return Err(LogProtocolError::CleanupTimeout),
            Ok(Ok(ConnectionEvent::Message(_))) => {
                quiet = tokio::time::Instant::now() + QUIET_PERIOD
            }
            Ok(Ok(
                ConnectionEvent::HeartbeatLost { .. }
                | ConnectionEvent::LinkError(_)
                | ConnectionEvent::Failed(_)
                | ConnectionEvent::Connected { .. },
            ))
            | Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return Err(LogProtocolError::CleanupTimeout)
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                return Err(LogProtocolError::CleanupTimeout)
            }
            Ok(Ok(_)) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(LogProtocolError::CleanupTimeout);
        }
    }
}

async fn retire(
    handle: &ConnectionHandle,
    route: &mut RoutedEvents,
    driver: &mut Driver,
    error: LogProtocolError,
) {
    let Some(pending) = driver.pending.as_mut() else {
        return;
    };
    if !pending.dispatched {
        driver.finish(Err(error));
        return;
    }
    if handle.generation() != pending.generation {
        driver.finish(Err(LogProtocolError::TargetChanged));
        return;
    }
    let frames = driver.protocol.retire();
    let deadline = tokio::time::Instant::now() + CLEANUP_BUDGET;
    // Retirement is independently bounded: the cancelled/expired transaction
    // cannot suppress its own LOG_REQUEST_END. Keep the original generation.
    let cleanup: Result<(), LogProtocolError> = async {
        for frame in frames {
            handle
                .send_in_generation(frame, pending.generation, deadline)
                .await
                .map_err(|_| LogProtocolError::CleanupTimeout)?;
        }
        Ok(())
    }
    .await;
    let cleanup = match cleanup {
        Ok(()) => wait_for_quiet(route, deadline).await,
        Err(error) => Err(error),
    };
    if cleanup.is_err() {
        driver.quarantined = true;
        driver.finish(Err(LogProtocolError::CleanupTimeout));
    } else {
        driver.finish(Err(error));
    }
}

async fn stopped(transaction: Option<LogTransaction>) -> LogProtocolError {
    match transaction {
        Some(transaction) => transaction.stopped().await,
        None => std::future::pending().await,
    }
}

async fn run(
    handle: ConnectionHandle,
    target_system: u8,
    target_component: u8,
    mut commands: mpsc::Receiver<Box<OwnedLog>>,
    events: mpsc::Sender<LogServiceEvent>,
    dropped_events: Arc<AtomicU64>,
    mut shutdown: watch::Receiver<bool>,
) {
    use ::mavlink::MessageData;
    let mut route = handle.subscribe_route(
        MessageRoute::messages(&[
            ::mavlink::common::LOG_ENTRY_DATA::ID,
            ::mavlink::common::LOG_DATA_DATA::ID,
        ])
        .from(target_system, target_component),
    );
    let mut driver = Driver {
        protocol: LogProtocol::new(target_system, target_component),
        pending: None,
        generation: handle.generation(),
        quarantined: false,
        events,
        dropped_events,
    };
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        driver.observe_generation(handle.generation());
        if handle.is_shutdown_requested() {
            retire(&handle, &mut route, &mut driver, LogProtocolError::Stopped).await;
            return;
        }
        if let Some(error) = driver
            .pending
            .as_ref()
            .and_then(|pending| pending.check(&handle).err())
        {
            retire(&handle, &mut route, &mut driver, error).await;
        }
        let transaction = driver
            .pending
            .as_ref()
            .map(|pending| pending.owned.transaction.clone());
        enum Step {
            Command(Box<OwnedLog>),
            Message(Box<MessageEnvelope>),
            Tick,
            Stop(LogProtocolError),
            Closed,
        }
        let step = tokio::select! {
            _ = shutdown.wait_for(|stopped| *stopped) => Step::Closed,
            error = stopped(transaction) => Step::Stop(error),
            command = commands.recv() => match command { Some(command) => Step::Command(command), None => Step::Closed },
            event = route.recv() => match event {
                Ok(ConnectionEvent::Message(message)) => Step::Message(message),
                Ok(ConnectionEvent::HeartbeatLost { .. } | ConnectionEvent::LinkError(_) | ConnectionEvent::Failed(_)
                    | ConnectionEvent::Connected { .. }) | Err(tokio::sync::broadcast::error::RecvError::Closed) => Step::Stop(LogProtocolError::Stopped),
                _ => Step::Tick,
            },
            _ = tick.tick() => Step::Tick,
        };
        let (events, frames) = match step {
            Step::Command(command) => (Vec::new(), driver.begin(command, &handle)),
            Step::Message(message) => {
                let Some(pending) = &driver.pending else {
                    continue;
                };
                if message.received_at < pending.started_at
                    || handle
                        .connected_since_snapshot()
                        .is_none_or(|opened| message.received_at < opened)
                {
                    continue;
                }
                match driver.protocol.handle(&message.message, Instant::now()) {
                    Ok(value) => value,
                    Err(error) => {
                        retire(&handle, &mut route, &mut driver, error.into()).await;
                        continue;
                    }
                }
            }
            Step::Tick => match driver.protocol.tick(Instant::now()) {
                Ok(frames) => (Vec::new(), frames),
                Err(error) => {
                    retire(&handle, &mut route, &mut driver, error.into()).await;
                    continue;
                }
            },
            Step::Stop(error) => {
                retire(&handle, &mut route, &mut driver, error).await;
                continue;
            }
            Step::Closed => {
                retire(&handle, &mut route, &mut driver, LogProtocolError::Stopped).await;
                return;
            }
        };
        if !frames.is_empty() {
            let Some(pending) = &mut driver.pending else {
                continue;
            };
            if let Err(error) = dispatch(&handle, pending, frames).await {
                retire(&handle, &mut route, &mut driver, error).await;
                continue;
            }
        }
        driver.process(events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{OperationId, TransportGeneration};

    #[tokio::test]
    async fn deadline_includes_waiting_for_a_full_log_queue() {
        let (commands, mut receiver) = mpsc::channel(1);
        let service = LogService {
            commands,
            dropped_events: Arc::new(AtomicU64::new(0)),
            shutdown: watch::channel(false).0,
        };
        let first = LogTransaction::new(
            OperationId::fresh().expect("id"),
            TransportGeneration::new(1).expect("gen"),
            Duration::from_secs(1),
        )
        .expect("tx");
        let (reply, _response) = oneshot::channel();
        service
            .commands
            .send(Box::new(OwnedLog {
                action: Some(Action::List),
                transaction: first,
                reply: Some(reply),
            }))
            .await
            .expect("fill");
        let transaction = LogTransaction::new(
            OperationId::fresh().expect("id"),
            TransportGeneration::new(1).expect("gen"),
            Duration::from_millis(30),
        )
        .expect("tx");
        assert_eq!(
            service.list_owned(transaction.clone()).await,
            Err(LogProtocolError::TimedOut)
        );
        assert!(!transaction.may_have_dispatched());
        assert!(service.cancel_owned(&transaction).await.expect("receipt"));
        assert!(receiver.recv().await.is_some());
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn udp_source_isolation_and_expired_transaction_still_send_owned_end() {
        use crate::mavlink::{ConnectionConfig, Endpoint};
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("peer");
        let (handle, _, first) = crate::mavlink::connection::spawn_connection(ConnectionConfig {
            endpoint: Endpoint::UdpClient {
                addr: socket.local_addr().expect("address"),
            },
            target_system_id: 42,
            target_component_id: 1,
            auto_discover_target: false,
            ..Default::default()
        })
        .await
        .expect("connection");
        first.await.expect("first").expect("opened");
        let (service, _events) = LogService::spawn(handle.clone(), 42, 1);
        let transaction = LogTransaction::new(
            OperationId::fresh().expect("id"),
            TransportGeneration::new(handle.generation()).expect("gen"),
            Duration::from_secs(2),
        )
        .expect("tx");
        let actor = service.clone();
        let owned = transaction.clone();
        let mut task = tokio::spawn(async move { actor.list_owned(owned).await });
        let destination = next_list(&socket).await;
        for (system, size) in [(43, 666), (42, 777)] {
            let mut bytes = Vec::new();
            ::mavlink::write_v2_msg(
                &mut bytes,
                ::mavlink::MavHeader {
                    system_id: system,
                    component_id: 1,
                    sequence: 0,
                },
                &MavMessage::LOG_ENTRY(::mavlink::common::LOG_ENTRY_DATA {
                    id: 0,
                    num_logs: 1,
                    last_log_num: 0,
                    size,
                    time_utc: 1,
                }),
            )
            .expect("encode");
            socket.send_to(&bytes, destination).await.expect("reply");
            if system == 43 {
                assert!(
                    tokio::time::timeout(Duration::from_millis(40), &mut task)
                        .await
                        .is_err(),
                    "foreign entry cannot complete A"
                );
            }
        }
        assert_eq!(
            task.await.expect("task").expect("listed"),
            vec![LogEntryInfo {
                id: 0,
                size: 777,
                time_utc: 1
            }]
        );
        let transaction = LogTransaction::new(
            OperationId::fresh().expect("id"),
            TransportGeneration::new(handle.generation()).expect("gen"),
            Duration::from_millis(80),
        )
        .expect("tx");
        let actor = service.clone();
        let owned = transaction.clone();
        let task = tokio::spawn(async move { actor.list_owned(owned).await });
        next_list(&socket).await;
        assert_eq!(task.await.expect("task"), Err(LogProtocolError::TimedOut));
        assert!(service
            .cancel_owned(&transaction)
            .await
            .expect("quiet receipt"));
        let mut ends = 0;
        let mut packet = [0u8; 512];
        while let Ok(Ok((size, _))) =
            tokio::time::timeout(Duration::from_millis(30), socket.recv_from(&mut packet)).await
        {
            let (_, message) = ::mavlink::read_v2_msg::<MavMessage, _>(
                &mut ::mavlink::peek_reader::PeekReader::new(&packet[..size]),
            )
            .expect("decode");
            if let MavMessage::LOG_REQUEST_END(end) = message {
                assert_eq!((end.target_system, end.target_component), (42, 1));
                ends += 1;
            }
        }
        assert!(
            ends > 0,
            "cleanup must send after the transfer deadline expired"
        );
        service.shutdown();
        handle
            .shutdown_and_wait(Duration::from_secs(1))
            .await
            .expect("shutdown");
    }

    async fn next_list(socket: &tokio::net::UdpSocket) -> std::net::SocketAddr {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let mut bytes = [0; 512];
                let (size, source) = socket.recv_from(&mut bytes).await.expect("receive");
                let (_, message) = ::mavlink::read_v2_msg::<MavMessage, _>(
                    &mut ::mavlink::peek_reader::PeekReader::new(&bytes[..size]),
                )
                .expect("decode");
                if let MavMessage::LOG_REQUEST_LIST(request) = message {
                    assert_eq!((request.target_system, request.target_component), (42, 1));
                    return source;
                }
            }
        })
        .await
        .expect("request deadline")
    }
}
