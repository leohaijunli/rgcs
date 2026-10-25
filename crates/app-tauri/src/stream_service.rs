//! Motor Test's sole owner of target stream rates. One guarded transport write
//! at a time; no motor-operation lock or firmware-ACK assumption. Preferences
//! are keyed by endpoint/system/component and survive window/process restarts.

#[cfg(test)]
#[path = "stream_sitl_tests.rs"]
mod sitl_tests;

use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use maggcs_core::mavlink::stream_control::{
    validate_request, StreamRate, StreamRequest, StreamRequestError, StreamStatus, StreamTracker,
};
use maggcs_core::mavlink::streams::{search_standard_streams, StandardStream};
use maggcs_core::mavlink::{
    ConnectionEvent, ConnectionHandle, LinkState, MavMessage, MessageRoute,
};
use mavlink::Message;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::{mpsc, oneshot};

use crate::motor_test_state::{MotorTestTargetRef, TARGET_COMPONENT_ID};
use crate::state::AppState;

const PROFILE_FILE: &str = "motor-streams.json";
const PROFILE_VERSION: u32 = 1;
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const APPLY_TIMEOUT: Duration = Duration::from_secs(5);
const PUBLISH_TICK: Duration = Duration::from_millis(200);
const REQUEST_CAPACITY: usize = 16;
pub const DEFAULT_STREAMS: [(u32, f64); 4] = [(36, 20.0), (105, 50.0), (147, 5.0), (291, 10.0)];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProfileKey {
    endpoint: String,
    system_id: u8,
    component_id: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Profile {
    key: ProfileKey,
    requests: Vec<StreamRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Profiles {
    version: u32,
    profiles: Vec<Profile>,
}

impl Default for Profiles {
    fn default() -> Self {
        Self {
            version: PROFILE_VERSION,
            profiles: Vec::new(),
        }
    }
}

#[derive(Default)]
pub struct StreamProfiles {
    path: Option<PathBuf>,
    data: Profiles,
    error: Option<String>,
}

impl StreamProfiles {
    fn open(&mut self, path: PathBuf) {
        if self.path.as_ref() == Some(&path) {
            return;
        }
        self.path = Some(path.clone());
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Profiles>(&text) {
                Ok(data) if data.version == PROFILE_VERSION => {
                    self.data = data;
                    self.error = None;
                }
                Ok(_) => self.error = Some("saved stream profile version is not supported".into()),
                Err(error) => {
                    self.error = Some(format!("cannot read saved stream profiles: {error}"))
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.data = Profiles::default();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("cannot read stream preferences: {error}")),
        }
    }

    fn saved(&self, key: &ProfileKey) -> Vec<StreamRequest> {
        self.data
            .profiles
            .iter()
            .find(|profile| &profile.key == key)
            .map(|profile| profile.requests.clone())
            .unwrap_or_default()
    }

    fn save(&mut self, key: &ProfileKey, request: StreamRequest) -> Result<(), StreamRequestError> {
        validate_request(&request)?;
        if let Some(error) = &self.error {
            return Err(transport(error));
        }
        let mut next = self.data.clone();
        if let Some(profile) = next.profiles.iter_mut().find(|profile| &profile.key == key) {
            profile
                .requests
                .retain(|old| old.message_id != request.message_id);
            profile.requests.push(request);
            profile.requests.sort_by_key(|request| request.message_id);
        } else {
            next.profiles.push(Profile {
                key: key.clone(),
                requests: vec![request],
            });
        }
        if let Some(path) = &self.path {
            write_profiles(path, &next)?;
        }
        self.data = next;
        Ok(())
    }
}

fn transport(message: impl ToString) -> StreamRequestError {
    StreamRequestError::Transport {
        message: message.to_string(),
    }
}

fn write_profiles(path: &Path, data: &Profiles) -> Result<(), StreamRequestError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(transport)?;
    }
    let text = serde_json::to_string_pretty(data).map_err(transport)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, text).map_err(transport)?;
    std::fs::rename(temporary, path).map_err(transport)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct StreamSnapshot {
    pub target: Option<MotorTestTargetRef>,
    pub endpoint: Option<String>,
    pub transport_generation: u64,
    /// This generation has received traffic from the explicitly selected node.
    pub ready: bool,
    pub statuses: Vec<StreamStatus>,
    pub error: Option<String>,
}

struct Apply {
    generation: u64,
    deadline: tokio::time::Instant,
    request: StreamRequest,
    reply: oneshot::Sender<Result<StreamStatus, StreamRequestError>>,
}

pub struct ActuatorStreams {
    target: MotorTestTargetRef,
    commands: mpsc::Sender<Apply>,
    snapshot: Arc<Mutex<StreamSnapshot>>,
    active: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ActuatorStreams {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        self.task.abort();
    }
}

pub fn snapshot(state: &AppState) -> StreamSnapshot {
    let target = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| runtime.target.target_ref());
    state
        .motor_test
        .streams
        .lock()
        .as_ref()
        .filter(|runtime| Some(runtime.target) == target)
        .map(|runtime| runtime.snapshot.lock().clone())
        .unwrap_or_default()
}

pub fn stop(state: &AppState) {
    state.motor_test.streams.lock().take();
}

pub fn attach<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    handle: ConnectionHandle,
    system_id: u8,
) {
    {
        let mut profiles = state.motor_test.stream_profiles.lock();
        match app.path().app_config_dir() {
            Ok(directory) => profiles.open(directory.join(PROFILE_FILE)),
            Err(error) => {
                profiles.error = Some(format!("cannot locate stream preferences: {error}"))
            }
        }
    }
    spawn_for_target(state, handle, system_id);
}

pub(crate) fn spawn_for_target(state: &AppState, handle: ConnectionHandle, system_id: u8) {
    stop(state);
    let target = MotorTestTargetRef {
        connection_id: handle.handle_id(),
        target_system_id: system_id,
        target_component_id: TARGET_COMPONENT_ID,
    };
    let key = ProfileKey {
        endpoint: handle.config().endpoint.to_address_string(),
        system_id,
        component_id: TARGET_COMPONENT_ID,
    };
    let snapshot = Arc::new(Mutex::new(StreamSnapshot {
        target: Some(target),
        endpoint: Some(key.endpoint.clone()),
        ..Default::default()
    }));
    let active = Arc::new(AtomicBool::new(true));
    let (commands, requests) = mpsc::channel(REQUEST_CAPACITY);
    // Subscribe before spawning so first-connect and reconnect boundaries cannot
    // be missed while the new task waits to be scheduled.
    let events = handle.subscribe_route(MessageRoute::all().from(system_id, TARGET_COMPONENT_ID));
    let link = StreamLink {
        handle,
        target,
        key,
        profiles: state.motor_test.stream_profiles.clone(),
        snapshot: snapshot.clone(),
        active: active.clone(),
    };
    let task = tokio::spawn(run(link, requests, events));
    *state.motor_test.streams.lock() = Some(ActuatorStreams {
        target,
        commands,
        snapshot,
        active,
        task,
    });
}

#[tauri::command]
pub fn actuator_stream_catalog(query: String) -> Vec<StandardStream> {
    search_standard_streams(&query)
}

#[tauri::command]
pub fn actuator_stream_status(state: State<'_, AppState>) -> StreamSnapshot {
    snapshot(&state)
}

#[tauri::command]
pub async fn actuator_stream_set_rate(
    state: State<'_, AppState>,
    target: MotorTestTargetRef,
    request: StreamRequest,
) -> Result<StreamStatus, String> {
    apply(&state, target, request)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn apply(
    state: &AppState,
    target: MotorTestTargetRef,
    request: StreamRequest,
) -> Result<StreamStatus, StreamRequestError> {
    validate_request(&request)?;
    if !state
        .motor_test
        .target
        .lock()
        .as_ref()
        .is_some_and(|runtime| runtime.target.target_ref() == target)
    {
        return Err(StreamRequestError::TargetChanged);
    }
    let (sender, generation) = {
        let streams = state.motor_test.streams.lock();
        let runtime = streams.as_ref().ok_or(StreamRequestError::NotConnected)?;
        if runtime.target != target || !runtime.active.load(Ordering::Acquire) {
            return Err(StreamRequestError::TargetChanged);
        }
        let snapshot = runtime.snapshot.lock();
        if !snapshot.ready {
            return Err(StreamRequestError::NotConnected);
        }
        (runtime.commands.clone(), snapshot.transport_generation)
    };
    let (reply, response) = oneshot::channel();
    let deadline = tokio::time::Instant::now() + APPLY_TIMEOUT;
    sender
        .try_send(Apply {
            generation,
            deadline,
            request,
            reply,
        })
        .map_err(|_| transport("stream request queue is busy"))?;
    tokio::time::timeout_at(deadline, response)
        .await
        .map_err(|_| transport("stream request timed out"))?
        .map_err(|_| StreamRequestError::TargetChanged)?
}

async fn send_rate(
    handle: &ConnectionHandle,
    target: MotorTestTargetRef,
    active: &AtomicBool,
    generation: u64,
    request: &StreamRequest,
) -> Result<(), StreamRequestError> {
    let interval = validate_request(request)?;
    if !active.load(Ordering::Acquire) || handle.generation() != generation {
        return Err(StreamRequestError::TargetChanged);
    }
    if handle.state().await != LinkState::Connected {
        return Err(StreamRequestError::NotConnected);
    }
    let frame = MavMessage::COMMAND_LONG(mavlink::common::COMMAND_LONG_DATA {
        command: mavlink::common::MavCmd::MAV_CMD_SET_MESSAGE_INTERVAL,
        param1: request.message_id as f32,
        param2: interval as f32,
        target_system: target.target_system_id,
        target_component: target.target_component_id,
        ..Default::default()
    });
    let result = handle
        .send_in_generation(
            frame,
            generation,
            tokio::time::Instant::now() + WRITE_TIMEOUT,
        )
        .await;
    if !active.load(Ordering::Acquire) || handle.generation() != generation {
        return Err(StreamRequestError::TargetChanged);
    }
    result.map_err(transport)
}

struct StreamManager {
    handle: ConnectionHandle,
    target: MotorTestTargetRef,
    key: ProfileKey,
    profiles: Arc<Mutex<StreamProfiles>>,
    snapshot: Arc<Mutex<StreamSnapshot>>,
    active: Arc<AtomicBool>,
    tracker: StreamTracker,
    generation: u64,
    since: Instant,
    restored: Option<u64>,
    ready: bool,
    error: Option<String>,
}

struct StreamLink {
    handle: ConnectionHandle,
    target: MotorTestTargetRef,
    key: ProfileKey,
    profiles: Arc<Mutex<StreamProfiles>>,
    snapshot: Arc<Mutex<StreamSnapshot>>,
    active: Arc<AtomicBool>,
}

impl StreamManager {
    // Lifecycle events can be lost when the shared broadcast receiver lags.
    // The handle's monotonic generation is the authority for resynchronizing.
    async fn sync_transport(&mut self) -> bool {
        if self.handle.state().await != LinkState::Connected {
            self.ready = false;
            return false;
        }
        let generation = self.handle.generation();
        if generation > self.generation {
            self.generation = generation;
            self.since = self
                .handle
                .connected_since()
                .await
                .unwrap_or_else(Instant::now);
            self.ready = false;
            self.tracker.reset_observations(self.since);
        }
        generation == self.generation
    }

    fn publish(&self) {
        *self.snapshot.lock() = StreamSnapshot {
            target: Some(self.target),
            endpoint: Some(self.key.endpoint.clone()),
            transport_generation: self.generation,
            ready: self.ready,
            statuses: self.tracker.snapshot(Instant::now()),
            error: self.error.clone(),
        };
    }

    async fn apply(&mut self, pending: &Apply) -> Result<StreamStatus, StreamRequestError> {
        let request = pending.request.clone();
        let generation = pending.generation;
        validate_request(&request)?;
        if generation != self.generation || generation != self.handle.generation() {
            return Err(StreamRequestError::TargetChanged);
        }
        if !self.ready || !self.active.load(Ordering::Acquire) {
            return Err(StreamRequestError::NotConnected);
        }
        let profiles = self.profiles.clone();
        let key = self.key.clone();
        let saved = request.clone();
        tokio::task::spawn_blocking(move || profiles.lock().save(&key, saved))
            .await
            .map_err(transport)??;
        let id = request.message_id;
        self.tracker.set(request.clone(), Instant::now())?;
        let result = if pending.reply.is_closed() || tokio::time::Instant::now() >= pending.deadline
        {
            Err(transport("stream request expired before transmission"))
        } else {
            send_rate(
                &self.handle,
                self.target,
                &self.active,
                generation,
                &request,
            )
            .await
        };
        self.tracker.note_result(id, &result);
        self.publish();
        result?;
        self.tracker
            .status(id, Instant::now())
            .ok_or_else(|| transport("stream request disappeared"))
    }

    async fn restore(&mut self) {
        if self.restored == Some(self.generation) {
            return;
        }
        self.restored = Some(self.generation);
        for request in self.tracker.requests() {
            let result = send_rate(
                &self.handle,
                self.target,
                &self.active,
                self.generation,
                &request,
            )
            .await;
            self.tracker.note_result(request.message_id, &result);
            if !self.active.load(Ordering::Acquire) || self.handle.generation() != self.generation {
                break;
            }
        }
    }
}

async fn run(
    link: StreamLink,
    mut requests: mpsc::Receiver<Apply>,
    mut events: maggcs_core::mavlink::RoutedEvents,
) {
    let StreamLink {
        handle,
        target,
        key,
        profiles,
        snapshot,
        active,
    } = link;
    let mut tracker = StreamTracker::default();
    let mut error = profiles.lock().error.clone();
    let mut initial: std::collections::BTreeMap<_, _> = DEFAULT_STREAMS
        .into_iter()
        .map(|(message_id, hz)| {
            (
                message_id,
                StreamRequest {
                    message_id,
                    rate: StreamRate::Hertz(hz),
                    allow_unknown_support: false,
                },
            )
        })
        .collect();
    for request in profiles.lock().saved(&key) {
        initial.insert(request.message_id, request);
    }
    for request in initial.into_values() {
        if let Err(problem) = tracker.set(request, Instant::now()) {
            error = Some(format!("saved stream request ignored: {problem}"));
        }
    }
    let generation = handle.generation();
    let since = handle.connected_since().await.unwrap_or_else(Instant::now);
    let mut manager = StreamManager {
        handle,
        target,
        key,
        profiles,
        snapshot,
        active,
        tracker,
        generation,
        since,
        restored: None,
        ready: false,
        error,
    };
    let mut tick = tokio::time::interval(PUBLISH_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    manager.publish();
    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else { break; };
                if request.reply.is_closed() { continue; }
                let result = manager.apply(&request).await;
                let _ = request.reply.send(result);
            }
            event = events.recv() => {
                match event {
                    Ok(ConnectionEvent::Connected { generation, started_at, .. }) => {
                        if generation > manager.generation {
                            manager.generation = generation;
                            manager.since = started_at;
                            manager.ready = false;
                            manager.tracker.reset_observations(started_at);
                        }
                    }
                    Ok(ConnectionEvent::Message(envelope)) => {
                        if !manager.sync_transport().await || envelope.received_at < manager.since { continue; }
                        if !manager.ready {
                            manager.ready = true;
                            manager.restore().await;
                        }
                        manager.tracker.observe(envelope.message.message_id(), envelope.received_at);
                    }
                    Ok(ConnectionEvent::LinkError(_)) | Ok(ConnectionEvent::Failed(_)) => {
                        manager.ready = false;
                        manager.tracker.reset_observations(Instant::now());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        manager.tracker.reset_observations(Instant::now());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
            _ = tick.tick() => {
                manager.sync_transport().await;
                manager.publish();
            }
        }
    }
    manager.ready = false;
    manager.publish();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_connected_event_does_not_strand_stream_observations() {
        use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("peer");
        let (handle, _, first) =
            maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
                endpoint: Endpoint::UdpClient {
                    addr: peer.local_addr().expect("peer address"),
                },
                reconnect_delay: Duration::from_millis(10),
                ..Default::default()
            })
            .await
            .expect("connection");
        first.await.expect("first result").expect("open");
        let target = MotorTestTargetRef {
            connection_id: handle.handle_id(),
            target_system_id: 1,
            target_component_id: 1,
        };
        let key = ProfileKey {
            endpoint: handle.config().endpoint.to_address_string(),
            system_id: 1,
            component_id: 1,
        };
        let since = Instant::now();
        let mut tracker = StreamTracker::default();
        tracker
            .set(
                StreamRequest {
                    message_id: 147,
                    rate: StreamRate::Hertz(10.0),
                    allow_unknown_support: false,
                },
                since,
            )
            .expect("request");
        tracker.observe(147, since);
        let mut manager = StreamManager {
            generation: handle.generation(),
            handle,
            target,
            key,
            profiles: Default::default(),
            snapshot: Default::default(),
            active: Arc::new(AtomicBool::new(true)),
            tracker,
            since,
            restored: Some(1),
            ready: true,
            error: None,
        };
        let original = manager.generation;
        manager.handle.reconnect().await.expect("reconnect");
        tokio::time::timeout(Duration::from_secs(2), async {
            while manager.handle.generation() <= original {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("new transport");
        // Deliberately consume no Connected event, as if the receiver lost it.
        assert!(manager.sync_transport().await);
        assert!(manager.generation > original);
        assert!(
            !manager.ready,
            "a new-generation target frame is still required"
        );
        assert_eq!(
            manager
                .tracker
                .status(147, Instant::now())
                .expect("row")
                .last_seen_age_ms,
            None
        );
        manager.ready = true;
        assert!(manager.sync_transport().await);
        assert!(manager.ready, "the same generation is not reset again");
        manager.handle.shutdown().await;
    }

    #[test]
    fn profiles_round_trip_without_crossing_endpoint_or_node_boundaries() {
        let directory =
            std::env::temp_dir().join(format!("maggcs-stream-profiles-{}", std::process::id()));
        let path = directory.join(PROFILE_FILE);
        let first = ProfileKey {
            endpoint: "serial:/dev/ttyACM0:115200".into(),
            system_id: 1,
            component_id: 1,
        };
        let other_node = ProfileKey {
            system_id: 42,
            ..first.clone()
        };
        let other_port = ProfileKey {
            endpoint: "serial:/dev/ttyACM1:115200".into(),
            ..first.clone()
        };
        let mut profiles = StreamProfiles::default();
        profiles.open(path.clone());
        profiles
            .save(
                &first,
                StreamRequest {
                    message_id: 147,
                    rate: StreamRate::Hertz(25.0),
                    allow_unknown_support: false,
                },
            )
            .expect("save first");
        profiles
            .save(
                &other_node,
                StreamRequest {
                    message_id: 147,
                    rate: StreamRate::Disabled,
                    allow_unknown_support: false,
                },
            )
            .expect("save other node");
        let mut loaded = StreamProfiles::default();
        loaded.open(path);
        assert_eq!(loaded.saved(&first)[0].rate, StreamRate::Hertz(25.0));
        assert_eq!(loaded.saved(&other_node)[0].rate, StreamRate::Disabled);
        assert!(loaded.saved(&other_port).is_empty());
        assert!(!directory.join("motor-streams.json.tmp").exists());
        std::fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn invalid_requests_and_corrupt_preferences_are_not_written_over() {
        let directory =
            std::env::temp_dir().join(format!("maggcs-stream-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("directory");
        let path = directory.join(PROFILE_FILE);
        std::fs::write(&path, "invalid existing settings").expect("corrupt fixture");
        let key = ProfileKey {
            endpoint: "udpout:127.0.0.1:14550".into(),
            system_id: 1,
            component_id: 1,
        };
        let mut profiles = StreamProfiles::default();
        profiles.open(path.clone());
        assert!(profiles
            .save(
                &key,
                StreamRequest {
                    message_id: 147,
                    rate: StreamRate::Hertz(10.0),
                    allow_unknown_support: false
                }
            )
            .is_err());
        assert_eq!(
            std::fs::read_to_string(path).expect("original"),
            "invalid existing settings"
        );
        let mut memory = StreamProfiles::default();
        assert!(memory
            .save(
                &key,
                StreamRequest {
                    message_id: 147,
                    rate: StreamRate::Hertz(f64::NAN),
                    allow_unknown_support: false
                }
            )
            .is_err());
        assert!(memory.saved(&key).is_empty());
        std::fs::remove_dir_all(directory).expect("cleanup");
    }
}
