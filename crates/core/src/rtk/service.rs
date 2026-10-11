//! RTCM injection service (comprehensive plan R2/R3): takes byte chunks from
//! an [`RtcmSource`](super::source::RtcmSource), reassembles RTCM3 frames,
//! fragments them into `GPS_RTCM_DATA` (R0: PX4 writes the fragments
//! verbatim to the GNSS serial port in arrival order — order is what
//! matters, flags are advisory) and sends them on the MAVLink link. Single
//! active source; a bounded queue drops oldest on overflow; 1 Hz stats
//! (B/s, per-RTCM-type counts, drops) plus an injection-stall alarm.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::forward;
use super::rtcm::RtcmFramer;
use super::source::{RtcmSource, SerialSource, SerialSourceConfig, SourceError};
use crate::mavlink::connection::ConnectionHandle;
use crate::mavlink::message::MavMessage;

/// Queue capacity between the reader thread and the service task. Full =
/// drop the oldest chunk and count it (a slow link must not buffer up
/// minutes of stale corrections; R2).
pub const QUEUE_CAPACITY: usize = 64;
/// Seconds without any RTCM data before the stall alarm fires (R3; matches
/// the 5 s link-switch timeout in the PX4 GPS driver).
pub const STALL_ALARM_S: u64 = 5;
/// Stats emission period (R3).
const STATS_TICK: Duration = Duration::from_secs(1);

/// What the UI sees each second (R3), and the app layer forwards.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct RtkStats {
    /// The active source label, `None` when injection is off.
    pub source: Option<String>,
    /// Bytes pushed into the link over the last second.
    pub bytes_per_second: f64,
    /// RTCM frames sent per message type over the last second
    /// (e.g. MSM4 = 1074/1084/1124, base position = 1005/1006).
    pub frames_per_type: HashMap<u16, u32>,
    /// Chunks dropped because the queue was full since start.
    pub dropped_chunks: u64,
    /// RTCM frames forwarded since start.
    pub total_frames: u64,
    /// No data from the source for more than [`STALL_ALARM_S`].
    pub stalled: bool,
    /// Seconds since the last data chunk, `None` = none ever received.
    pub seconds_since_data: Option<f64>,
}

/// Lifecycle errors surfaced through command replies.
#[derive(Debug, thiserror::Error)]
pub enum RtkError {
    #[error(transparent)]
    Source(#[from] SourceError),
    /// The service task ended (link or app shutdown).
    #[error("RTK injection service stopped")]
    ServiceStopped,
}

/// Shared queue between the blocking reader thread and the async task.
#[derive(Default)]
struct ChunkQueue {
    inner: Mutex<QueueState>,
}

#[derive(Default)]
struct QueueState {
    chunks: VecDeque<Vec<u8>>,
    dropped: u64,
}

impl ChunkQueue {
    /// Push a chunk; when full, drop the oldest (R2: 丢旧并计数).
    fn push(&self, chunk: Vec<u8>) {
        let mut state = self.inner.lock().expect("queue poisoned");
        while state.chunks.len() >= QUEUE_CAPACITY {
            state.chunks.pop_front();
            state.dropped += 1;
        }
        state.chunks.push_back(chunk);
    }

    fn drain(&self) -> (VecDeque<Vec<u8>>, u64) {
        let mut state = self.inner.lock().expect("queue poisoned");
        let chunks = std::mem::take(&mut state.chunks);
        (chunks, state.dropped)
    }
}

/// A live source session: the reader thread plus its stop flag.
struct SourceSession {
    label: String,
    stop: Arc<AtomicBool>,
    queue: Arc<ChunkQueue>,
}

impl SourceSession {
    /// Spawn the blocking reader thread for `source`.
    fn start(label: String, mut source: Box<dyn RtcmSource>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(ChunkQueue::default());
        let thread_stop = Arc::clone(&stop);
        let thread_queue = Arc::clone(&queue);
        std::thread::Builder::new()
            .name(format!("rtk-reader-{label}"))
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    match source.read() {
                        Ok(Some(chunk)) => thread_queue.push(chunk),
                        Ok(None) => continue,
                        Err(_) => break, // source died; the stall alarm covers it
                    }
                }
            })
            .expect("spawn rtk reader thread");
        Self { label, stop, queue }
    }
}

/// Handle to the running injection service.
#[derive(Clone)]
pub struct RtkServiceHandle {
    cmd_tx: mpsc::Sender<SourceRequest>,
    /// Stop flag of the currently active session, so `stop()` works without
    /// a round-trip through the task.
    active_stop: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

enum SourceRequest {
    /// Start injecting from a source; the reply reports the outcome.
    Start {
        label: String,
        source: Box<dyn RtcmSource>,
        reply: tokio::sync::oneshot::Sender<Result<(), RtkError>>,
    },
    /// Stop the active source (idempotent).
    Stop {
        reply: tokio::sync::oneshot::Sender<()>,
    },
}

impl RtkServiceHandle {
    /// Start injecting from the F9P serial port. Any previously active
    /// source is stopped first (single-source mutual exclusion, R2).
    pub async fn start_serial(&self, config: SerialSourceConfig) -> Result<(), RtkError> {
        let source = SerialSource::open(config.clone())?;
        self.start(Box::new(source), config.port_name).await
    }

    /// Start injecting from an arbitrary source (tests use this with the
    /// [`FakeSource`](super::source::FakeSource) double).
    pub async fn start(&self, source: Box<dyn RtcmSource>, label: String) -> Result<(), RtkError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(SourceRequest::Start {
                label,
                source,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RtkError::ServiceStopped)?;
        reply_rx.await.map_err(|_| RtkError::ServiceStopped)?
    }

    /// Stop the active source (idempotent, also safe from the reader side).
    pub async fn stop(&self) {
        // Signal the reader immediately so a blocked read exits fast.
        if let Some(stop) = self
            .active_stop
            .lock()
            .expect("active stop lock poisoned")
            .clone()
        {
            stop.store(true, Ordering::Relaxed);
        }
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if self
            .cmd_tx
            .send(SourceRequest::Stop { reply: reply_tx })
            .await
            .is_ok()
        {
            let _ = reply_rx.await;
        }
    }
}

/// Spawn the injection service on the link. The stats receiver ticks at
/// 1 Hz while a source is active.
pub fn spawn(handle: ConnectionHandle) -> (RtkServiceHandle, mpsc::Receiver<RtkStats>) {
    let (stats_tx, stats_rx) = mpsc::channel(8);
    let (cmd_tx, mut cmd_rx) = mpsc::channel(4);

    tokio::spawn(async move {
        let mut framer = RtcmFramer::new();
        let mut session: Option<SourceSession> = None;
        let mut stats_base = StatsBase::default();
        let mut stats_tick = tokio::time::interval(STATS_TICK);
        stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    match cmd {
                        SourceRequest::Start { label, source, reply } => {
                            // Single-source mutual exclusion: stop the old one first.
                            if let Some(old) = session.take() {
                                old.stop.store(true, Ordering::Relaxed);
                                stats_base = StatsBase::default();
                                framer.reset();
                            }
                            let opened = SourceSession::start(label.clone(), source);
                            let _ = reply.send(Ok(()));
                            session = Some(opened);
                            stats_base.label = Some(label);
                            stats_base.last_data = None;
                        }
                        SourceRequest::Stop { reply } => {
                            if let Some(old) = session.take() {
                                old.stop.store(true, Ordering::Relaxed);
                                stats_base = StatsBase::default();
                                framer.reset();
                            }
                            let _ = stats_tx
                                .send(RtkStats {
                                    source: None,
                                    ..RtkStats::default()
                                })
                                .await;
                            let _ = reply.send(());
                        }
                    }
                }
                _ = stats_tick.tick() => {
                    if let Some(active) = &session {
                        let (chunks, dropped) = active.queue.drain();
                        stats_base.dropped_chunks = dropped;
                        for chunk in &chunks {
                            stats_base.bytes_seen += chunk.len() as u64;
                            for frame in framer.push(chunk) {
                                let bytes = frame.to_bytes();
                                let ftype = super::rtcm::message_type(&bytes);
                                if forward(&handle, &bytes, &mut stats_base.sequence).await.is_err() {
                                    return; // link gone: stop with the connection
                                }
                                *stats_base.frames_per_type.entry(ftype).or_insert(0) += 1;
                                stats_base.total_frames += 1;
                                stats_base.last_data = Some(Instant::now());
                            }
                        }
                        if !chunks.is_empty() {
                            // Fresh data arrived: chunks themselves count as
                            // "the source is alive" even between frames.
                            stats_base.last_data = Some(Instant::now());
                        }
                        let elapsed = stats_base.last_data.map(|t| t.elapsed().as_secs_f64());
                        let stats = RtkStats {
                            source: Some(active.label.clone()),
                            bytes_per_second: stats_base.bytes_seen as f64,
                            frames_per_type: stats_base.frames_per_type.clone(),
                            dropped_chunks: stats_base.dropped_chunks,
                            total_frames: stats_base.total_frames,
                            stalled: elapsed.is_none_or(|s| s > STALL_ALARM_S as f64),
                            seconds_since_data: elapsed,
                        };
                        stats_base.bytes_seen = 0;
                        stats_base.frames_per_type.clear();
                        if stats_tx.send(stats).await.is_err() {
                            return; // subscriber gone
                        }
                    }
                }
            }
        }
    });

    (
        RtkServiceHandle {
            cmd_tx,
            active_stop: Arc::new(Mutex::new(None)),
        },
        stats_rx,
    )
}

/// Fragment one RTCM frame and send every `GPS_RTCM_DATA` packet on the
/// link. Returns an error when the link is gone.
async fn forward(handle: &ConnectionHandle, frame: &[u8], sequence: &mut u8) -> Result<(), ()> {
    let fragments = forward::fragment(frame, *sequence).map_err(|_| ())?;
    *sequence = sequence.wrapping_add(1);
    for fragment in fragments {
        handle
            .send(MavMessage::GPS_RTCM_DATA(fragment.to_message()))
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

/// Per-session counters the stats tick consumes and resets.
#[derive(Default)]
struct StatsBase {
    label: Option<String>,
    bytes_seen: u64,
    frames_per_type: HashMap<u16, u32>,
    dropped_chunks: u64,
    total_frames: u64,
    sequence: u8,
    last_data: Option<Instant>,
}
