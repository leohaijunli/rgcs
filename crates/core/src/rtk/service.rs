//! Link-owned correction forwarding from an application-owned base receiver.
//!
//! Whole CRC-validated frames arrive on a bounded, oldest-first broadcast
//! queue. Forwarding is immediate; only statistics run at 1 Hz. Every frame
//! has one deadline and one transport generation for all of its fragments.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, watch};

use super::base::{
    BaseError, BaseMode, BasePhase, BaseSessionId, Cancellation, RtkBaseServiceHandle,
    MAX_CORRECTION_AGE,
};
use super::forward;
use super::source::{RtcmSource, SerialSourceConfig};
use crate::mavlink::connection::{ConnectionHandle, LinkState};
use crate::mavlink::message::MavMessage;

pub const QUEUE_CAPACITY: usize = super::base::CORRECTION_CAPACITY;
pub const STALL_ALARM_S: u64 = 5;
pub const FRAME_WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const STATS_TICK: Duration = Duration::from_secs(1);
const SERVICE_STOP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RtkStats {
    pub source: Option<String>,
    /// Complete valid RTCM bytes whose local transport writes succeeded, per
    /// elapsed second. This is not a rover receipt or an RTK-fix indication.
    pub bytes_per_second: f64,
    pub frames_per_type: HashMap<u16, u32>,
    /// Oldest-first queue losses, expired frames and failed frame sends.
    /// Queue units are whole corrections, never arbitrary serial slices.
    pub dropped_chunks: u64,
    pub total_frames: u64,
    pub stalled: bool,
    pub seconds_since_data: Option<f64>,
    #[serde(default)]
    pub base_session_id: Option<BaseSessionId>,
    #[serde(default)]
    #[ts(type = "number")]
    pub connection_generation: u64,
    #[serde(default)]
    pub error: Option<String>,
}

/// Ordered desktop snapshots: a retired link cannot revive the UI through a
/// delayed event or cached command reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RtkLinkStatus {
    #[ts(type = "number")]
    pub revision: u64,
    pub stats: Option<RtkStats>,
    pub foreign: super::multi::ForeignState,
}

#[derive(Debug, thiserror::Error)]
pub enum RtkError {
    #[error(transparent)]
    Base(#[from] BaseError),
    #[error("RTK injection service stopped")]
    ServiceStopped,
    #[error("RTK send cancelled")]
    Cancelled,
    #[error("RTCM frame write timed out")]
    WriteTimeout,
    #[error("RTCM transport write failed: {0}")]
    Write(String),
}

#[derive(Clone)]
pub struct RtkServiceHandle {
    base: RtkBaseServiceHandle,
    owns_base: bool,
    shutdown: Cancellation,
    done: watch::Receiver<bool>,
}

impl RtkServiceHandle {
    /// Compatibility entry for an already-configured serial receiver.
    pub async fn start_serial(&self, config: SerialSourceConfig) -> Result<(), RtkError> {
        self.stop().await?;
        self.base
            .start_serial(config, BaseMode::Preconfigured)
            .await?;
        Ok(())
    }

    pub async fn start(&self, source: Box<dyn RtcmSource>, label: String) -> Result<(), RtkError> {
        if self.shutdown.is_cancelled() {
            return Err(RtkError::ServiceStopped);
        }
        self.stop().await?;
        self.base.start_source(source, label).await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), RtkError> {
        if let Some(id) = self.base.snapshot().session_id {
            self.base.stop(id).await?;
        }
        Ok(())
    }

    /// Detaching a vehicle stops only its forwarder. Standalone instances
    /// created by `spawn_with_task` also own and stop their receiver.
    pub async fn shutdown(&self) -> Result<(), RtkError> {
        self.shutdown.cancel();
        if self.owns_base {
            self.stop().await?;
        }
        let mut done = self.done.clone();
        tokio::time::timeout(SERVICE_STOP_TIMEOUT, done.wait_for(|done| *done))
            .await
            .map_err(|_| RtkError::WriteTimeout)?
            .map_err(|_| RtkError::ServiceStopped)?;
        Ok(())
    }
}

pub fn spawn(handle: ConnectionHandle) -> (RtkServiceHandle, mpsc::Receiver<RtkStats>) {
    let (service, stats, _task) = spawn_with_task(handle);
    (service, stats)
}

pub fn spawn_with_task(
    handle: ConnectionHandle,
) -> (
    RtkServiceHandle,
    mpsc::Receiver<RtkStats>,
    tokio::task::JoinHandle<()>,
) {
    spawn_forwarder(RtkBaseServiceHandle::default(), handle, true)
}

/// Subscribe to fresh corrections without taking ownership of the receiver.
pub fn spawn_for_base(
    base: RtkBaseServiceHandle,
    handle: ConnectionHandle,
) -> (
    RtkServiceHandle,
    mpsc::Receiver<RtkStats>,
    tokio::task::JoinHandle<()>,
) {
    spawn_forwarder(base, handle, false)
}

fn spawn_forwarder(
    base: RtkBaseServiceHandle,
    handle: ConnectionHandle,
    owns_base: bool,
) -> (
    RtkServiceHandle,
    mpsc::Receiver<RtkStats>,
    tokio::task::JoinHandle<()>,
) {
    let (stats_tx, stats_rx) = mpsc::channel(8);
    let (done_tx, done) = watch::channel(false);
    let shutdown = Cancellation::new();
    let service = RtkServiceHandle {
        base: base.clone(),
        owns_base,
        shutdown: shutdown.clone(),
        done,
    };
    // Subscribe before the caller can start its source, including fast fixtures.
    let mut corrections = base.corrections();
    let mut status = base.subscribe();
    use ::mavlink::MessageData;
    let mut link_events =
        handle.subscribe_route(crate::mavlink::router::MessageRoute::messages(&[
            ::mavlink::common::HEARTBEAT_DATA::ID,
        ]));
    let task = tokio::spawn(async move {
        let mut active = None;
        let mut counters = StatsBase::new(Instant::now());
        let mut pending_stats = None;
        let mut tick = tokio::time::interval(STATS_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let snapshot = status.borrow_and_update().clone();
            let generation = handle.generation();
            let connected = handle.state().await == LinkState::Connected;
            let current = snapshot
                .session_id
                .filter(|_| snapshot.phase == BasePhase::Ready && connected && generation != 0)
                .map(|id| (id, generation));
            if current != active {
                active = current;
                counters = StatsBase::new(Instant::now());
                tick.reset();
                pending_stats = Some(match active {
                    Some((id, generation)) => counters.snapshot(
                        snapshot.source.as_deref().unwrap_or("Base receiver"),
                        id,
                        generation,
                        Instant::now(),
                    ),
                    None => RtkStats::default(),
                });
            }
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                changed = status.changed() => { if changed.is_err() { break; } }
                event = link_events.recv() => {
                    if matches!(event, Err(broadcast::error::RecvError::Closed)) { break; }
                }
                correction = corrections.recv() => {
                    let correction = match correction {
                        Ok(correction) => correction,
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            counters.dropped_chunks = counters.dropped_chunks.saturating_add(count);
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let Some((id, generation)) = active else { continue };
                    if correction.session_id != id || correction.cancel.is_cancelled() { continue; }
                    // A surviving ConnectionHandle may have reopened its port.
                    // Even recent corrections captured before that open are old.
                    let opened_at = handle.connected_since().await;
                    if !opened_at.is_some_and(|at| correction.received_at >= at)
                        || correction.received_at.elapsed() > MAX_CORRECTION_AGE
                    {
                        counters.dropped_chunks = counters.dropped_chunks.saturating_add(1);
                        continue;
                    }
                    let bytes = correction.frame.to_bytes();
                    let deadline = tokio::time::Instant::from_std(correction.received_at + MAX_CORRECTION_AGE)
                        .min(tokio::time::Instant::now() + FRAME_WRITE_TIMEOUT);
                    let result = forward_frame(&handle, &bytes, &mut counters.sequence, generation, deadline, &correction.cancel, &shutdown).await;
                    match result {
                        Ok(()) => {
                            counters.bytes_forwarded += bytes.len() as u64;
                            *counters.frames_per_type.entry(correction.frame.message_type).or_insert(0) += 1;
                            counters.total_frames += 1;
                            counters.error = None;
                        }
                        Err(RtkError::Cancelled) => {}
                        Err(error) => {
                            counters.dropped_chunks = counters.dropped_chunks.saturating_add(1);
                            counters.error = Some(error.to_string());
                        }
                    }
                }
                permit = stats_tx.reserve(), if pending_stats.is_some() => {
                    match permit {
                        Ok(permit) => { if let Some(stats) = pending_stats.take() { permit.send(stats); } }
                        Err(_) => break,
                    }
                }
                _ = tick.tick() => {
                    if let Some((id, generation)) = active {
                        counters.last_data = base.last_data(id);
                        pending_stats = Some(counters.snapshot(snapshot.source.as_deref().unwrap_or("Base receiver"), id, generation, Instant::now()));
                    }
                }
            }
        }
        done_tx.send_replace(true);
    });
    (service, stats_rx, task)
}

async fn forward_frame(
    handle: &ConnectionHandle,
    frame: &[u8],
    sequence: &mut u8,
    generation: u64,
    deadline: tokio::time::Instant,
    source_stop: &Cancellation,
    shutdown: &Cancellation,
) -> Result<(), RtkError> {
    let batch = forward::packetize_px4(frame, *sequence)
        .map_err(|error| RtkError::Write(error.to_string()))?;
    *sequence = batch.next_sequence_id;
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err(RtkError::Cancelled),
        _ = source_stop.cancelled() => Err(RtkError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => Err(RtkError::WriteTimeout),
        result = async {
            for fragment in batch.packets {
                handle.send_in_generation(MavMessage::GPS_RTCM_DATA(fragment.to_message()), generation, deadline)
                    .await.map_err(|error| RtkError::Write(error.to_string()))?;
            }
            Ok(())
        } => result,
    }
}

struct StatsBase {
    started_at: Instant,
    window_started_at: Instant,
    bytes_forwarded: u64,
    frames_per_type: HashMap<u16, u32>,
    dropped_chunks: u64,
    total_frames: u64,
    sequence: u8,
    last_data: Option<Instant>,
    error: Option<String>,
}

impl StatsBase {
    fn new(now: Instant) -> Self {
        Self {
            started_at: now,
            window_started_at: now,
            bytes_forwarded: 0,
            frames_per_type: HashMap::new(),
            dropped_chunks: 0,
            total_frames: 0,
            sequence: 0,
            last_data: None,
            error: None,
        }
    }

    fn snapshot(
        &mut self,
        source: &str,
        session: BaseSessionId,
        generation: u64,
        now: Instant,
    ) -> RtkStats {
        let seconds = now
            .saturating_duration_since(self.window_started_at)
            .as_secs_f64();
        let stats = RtkStats {
            source: Some(source.into()),
            bytes_per_second: if seconds > 0.0 {
                self.bytes_forwarded as f64 / seconds
            } else {
                0.0
            },
            frames_per_type: std::mem::take(&mut self.frames_per_type),
            dropped_chunks: self.dropped_chunks,
            total_frames: self.total_frames,
            stalled: now.saturating_duration_since(self.last_data.unwrap_or(self.started_at))
                > Duration::from_secs(STALL_ALARM_S),
            seconds_since_data: self
                .last_data
                .map(|at| now.saturating_duration_since(at).as_secs_f64()),
            base_session_id: Some(session),
            connection_generation: generation,
            error: self.error.clone(),
        };
        self.bytes_forwarded = 0;
        self.window_started_at = now;
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> BaseSessionId {
        serde_json::from_str("\"1\"").expect("ID")
    }

    #[tokio::test]
    async fn blocked_transport_does_not_hold_rtk_stop_or_shutdown() {
        let (connection, _held_queue) = crate::mavlink::connection::stalled_test_connection();
        let (service, _stats, task) = spawn_with_task(connection);
        let frame = super::super::rtcm::encode_frame(&vec![0x55; 1023]).expect("maximum RTCM");
        service
            .start(
                Box::new(super::super::source::FakeSource::new(
                    vec![frame],
                    Duration::ZERO,
                )),
                "blocked".into(),
            )
            .await
            .expect("start");
        tokio::time::sleep(Duration::from_millis(30)).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            service.stop().await.expect("stop under write backpressure");
            service
                .shutdown()
                .await
                .expect("shutdown under backpressure");
            task.await.expect("joined");
        })
        .await
        .expect("whole teardown bounded");
    }

    #[tokio::test]
    async fn frame_write_deadline_bounds_an_unanswered_transport() {
        let (connection, _held_queue) = crate::mavlink::connection::stalled_test_connection();
        let frame = super::super::rtcm::encode_frame(&[0x3e, 0xd0]).expect("RTCM");
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            forward_frame(
                &connection,
                &frame,
                &mut 0,
                1,
                tokio::time::Instant::now() + Duration::from_millis(25),
                &Cancellation::new(),
                &Cancellation::new(),
            ),
        )
        .await
        .expect("caller has a finite bound");
        assert!(matches!(
            result,
            Err(RtkError::WriteTimeout | RtkError::Write(_))
        ));
    }

    #[test]
    fn startup_grace_does_not_invent_an_arrival() {
        let now = Instant::now();
        let mut stats = StatsBase::new(now);
        for age in [Duration::ZERO, Duration::from_secs(STALL_ALARM_S)] {
            let snapshot = stats.snapshot("silent", id(), 1, now + age);
            assert!(!snapshot.stalled);
            assert_eq!(snapshot.seconds_since_data, None);
        }
        assert!(
            stats
                .snapshot(
                    "silent",
                    id(),
                    1,
                    now + Duration::from_secs(STALL_ALARM_S + 1)
                )
                .stalled
        );
    }

    #[test]
    fn rates_use_elapsed_time_and_only_interval_counters_reset() {
        let now = Instant::now();
        let mut stats = StatsBase::new(now);
        stats.bytes_forwarded = 900;
        stats.frames_per_type.insert(1074, 2);
        stats.total_frames = 2;
        stats.dropped_chunks = 3;
        stats.last_data = Some(now + Duration::from_millis(500));
        let snapshot = stats.snapshot("base", id(), 1, now + Duration::from_secs(3));
        assert_eq!(snapshot.bytes_per_second, 300.0);
        assert_eq!(snapshot.frames_per_type, HashMap::from([(1074, 2)]));
        assert_eq!(snapshot.seconds_since_data, Some(2.5));
        let next = stats.snapshot("base", id(), 1, now + Duration::from_secs(4));
        assert_eq!(next.bytes_per_second, 0.0);
        assert!(next.frames_per_type.is_empty());
        assert_eq!((next.total_frames, next.dropped_chunks), (2, 3));
    }

    #[tokio::test]
    async fn correction_overflow_drops_oldest_whole_frames() {
        let base = RtkBaseServiceHandle::default();
        let mut corrections = base.corrections();
        let frames: Vec<_> = (0..QUEUE_CAPACITY + 3)
            .map(|index| {
                super::super::rtcm::encode_frame(&[0x3e, 0xd0, index as u8]).expect("RTCM")
            })
            .collect();
        base.start_source(
            Box::new(super::super::source::FakeSource::new(
                frames,
                Duration::ZERO,
            )),
            "overflow".into(),
        )
        .await
        .expect("start");
        // One source chunk per read; wait for the deterministic fixture to drain.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(matches!(
            corrections.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(3))
        ));
        assert_eq!(
            corrections
                .try_recv()
                .expect("oldest retained")
                .frame
                .payload[2],
            3
        );
        base.stop(base.snapshot().session_id.expect("session"))
            .await
            .expect("stop");
    }
}
