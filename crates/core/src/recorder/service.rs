//! The recorder itself: a bounded, drop-oldest queue drained by a dedicated
//! writer thread (plan §3 A4).

use std::collections::VecDeque;
use std::fs::File;
use std::future::Future;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::events::{write_event, LoggedEvent, RecordEvent};
use super::session::{write_manifest, SessionManifest};
use super::{now_utc_us, tlog, RecorderError, EVENTS_FILE, TLOG_FILE};
use crate::lifecycle::{ManagedService, Stage, StopOutcome};
use crate::mavlink::connection::RawFrame;

/// Default bounded-queue capacity. At ~100 Hz mag + telemetry, this is several
/// seconds of headroom before the drop-oldest policy engages.
pub const DEFAULT_QUEUE_CAP: usize = 8192;

/// How often the writer flushes. Bounds how much a crash can lose.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// How a recorder is created.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Session directory (created if missing).
    pub dir: PathBuf,
    /// Bounded queue size; the oldest item is dropped when full.
    pub queue_cap: usize,
    /// Endpoint recorded in `session.json`.
    pub endpoint: String,
    pub target_system_id: u8,
    pub target_component_id: u8,
    pub app_version: String,
}

impl RecorderConfig {
    /// A config for `dir` with the default queue capacity and blank session
    /// metadata (`connect` fills the link details).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            queue_cap: DEFAULT_QUEUE_CAP,
            endpoint: String::new(),
            target_system_id: 0,
            target_component_id: 0,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// A snapshot of the recorder's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RecorderStats {
    /// Frames written to the tlog.
    pub frames: u64,
    /// Events written to `events.jsonl`.
    pub events: u64,
    /// Items dropped because the writer fell behind (queue overflow or a lagged
    /// tap subscriber).
    pub dropped: u64,
    /// Bytes written to the tlog (frames + timestamps).
    pub bytes: u64,
}

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    events: AtomicU64,
    dropped: AtomicU64,
    bytes: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> RecorderStats {
        RecorderStats {
            frames: self.frames.load(Ordering::Relaxed),
            events: self.events.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
        }
    }
}

enum Item {
    Frame(RawFrame),
    Event(LoggedEvent),
}

enum Pop {
    Item(Item),
    /// Nothing available inside the timeout, queue still open.
    Timeout,
    /// Closed and drained.
    Drained,
}

/// A bounded MPSC queue. `push` never blocks: when full it evicts the oldest
/// item and counts the drop, so a stalled writer cannot slow the producers.
struct Queue {
    inner: Mutex<VecDeque<Item>>,
    ready: Condvar,
    closed: Mutex<bool>,
    cap: usize,
}

impl Queue {
    fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            closed: Mutex::new(false),
            cap: cap.max(1),
        }
    }

    fn push(&self, item: Item, counters: &Counters) {
        let mut items = self.inner.lock().expect("recorder queue lock poisoned");
        if items.len() >= self.cap {
            items.pop_front();
            counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
        items.push_back(item);
        drop(items);
        self.ready.notify_one();
    }

    fn note_dropped(&self, counters: &Counters, n: u64) {
        counters.dropped.fetch_add(n, Ordering::Relaxed);
    }

    fn close(&self) {
        *self.closed.lock().expect("recorder closed lock poisoned") = true;
        self.ready.notify_all();
    }

    fn pop_timeout(&self, timeout: Duration) -> Pop {
        let mut items = self.inner.lock().expect("recorder queue lock poisoned");
        loop {
            if let Some(item) = items.pop_front() {
                return Pop::Item(item);
            }
            if *self.closed.lock().expect("recorder closed lock poisoned") {
                return Pop::Drained;
            }
            let (guard, wait) = self
                .ready
                .wait_timeout(items, timeout)
                .expect("recorder queue lock poisoned");
            items = guard;
            if items.is_empty() && wait.timed_out() {
                return Pop::Timeout;
            }
        }
    }
}

/// Handle to a running recorder. Register it as the [`Stage::Recorder`]
/// service so a teardown flushes it after the services that feed it.
pub struct Recorder {
    queue: Arc<Queue>,
    counters: Arc<Counters>,
    join: Mutex<Option<JoinHandle<()>>>,
    dir: PathBuf,
    started_utc_us: u64,
    config: RecorderConfig,
}

impl Recorder {
    /// Create the session directory, open the three files, and start the writer
    /// thread.
    pub fn spawn(config: RecorderConfig) -> Result<Arc<Recorder>, RecorderError> {
        let raw_dir = config.dir.join("raw");
        std::fs::create_dir_all(&raw_dir).map_err(|e| RecorderError::CreateDir {
            path: raw_dir.clone(),
            source: e,
        })?;
        let tlog_path = config.dir.join(TLOG_FILE);
        let events_path = config.dir.join(EVENTS_FILE);
        let tlog_file = File::create(&tlog_path).map_err(|e| RecorderError::Open {
            path: tlog_path.clone(),
            source: e,
        })?;
        let events_file = File::create(&events_path).map_err(|e| RecorderError::Open {
            path: events_path.clone(),
            source: e,
        })?;

        let started_utc_us = now_utc_us();
        write_manifest(&config.dir, &SessionManifest::new(&config, started_utc_us))?;

        let queue = Arc::new(Queue::new(config.queue_cap));
        let counters = Arc::new(Counters::default());
        let writer_queue = queue.clone();
        let writer_counters = counters.clone();
        let join = std::thread::Builder::new()
            .name("maggcs-recorder".to_string())
            .spawn(move || writer_loop(writer_queue, writer_counters, tlog_file, events_file))
            .map_err(RecorderError::Thread)?;

        Ok(Arc::new(Recorder {
            queue,
            counters,
            join: Mutex::new(Some(join)),
            dir: config.dir.clone(),
            started_utc_us,
            config,
        }))
    }

    /// Enqueue one raw frame (never blocks).
    pub fn record_frame(&self, frame: RawFrame) {
        self.queue.push(Item::Frame(frame), &self.counters);
    }

    /// Enqueue one event, stamped now (never blocks).
    pub fn record_event(&self, event: RecordEvent) {
        self.queue
            .push(Item::Event(LoggedEvent::new(event)), &self.counters);
    }

    /// Count frames the raw tap lost to a lagged subscriber.
    pub fn note_lagged(&self, n: u64) {
        self.queue.note_dropped(&self.counters, n);
    }

    /// Stop accepting work and wake the writer.
    pub fn close(&self) {
        self.queue.close();
    }

    /// Current counters.
    pub fn stats(&self) -> RecorderStats {
        self.counters.snapshot()
    }

    /// Session directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Flush, fsync, and join the writer thread.
    ///
    /// Idempotent: a second call (or a call after the thread already exited)
    /// reports `NotRunning`.
    pub async fn flush(&self, _deadline: Duration) -> StopOutcome {
        self.queue.close();
        let join = self
            .join
            .lock()
            .expect("recorder join lock poisoned")
            .take();
        let Some(join) = join else {
            return StopOutcome::NotRunning;
        };
        let started = Instant::now();
        let joined = tokio::task::spawn_blocking(move || join.join()).await;
        let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        match joined {
            Ok(Ok(())) => {
                let stats = self.counters.snapshot();
                let mut manifest = SessionManifest::new(&self.config, self.started_utc_us);
                manifest.closed_utc_us = Some(now_utc_us());
                manifest.frames = stats.frames;
                manifest.events = stats.events;
                manifest.dropped = stats.dropped;
                match write_manifest(&self.dir, &manifest) {
                    Ok(()) => StopOutcome::Stopped { elapsed_ms },
                    Err(e) => StopOutcome::Failed {
                        message: format!("session manifest write failed: {e}"),
                    },
                }
            }
            Ok(Err(_)) => StopOutcome::Failed {
                message: "recorder writer thread panicked".to_string(),
            },
            Err(e) => StopOutcome::Failed {
                message: format!("recorder join failed: {e}"),
            },
        }
    }
}

impl ManagedService for Recorder {
    fn name(&self) -> &'static str {
        Stage::Recorder.as_str()
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(self.flush(deadline))
    }
}

/// Drain the queue to disk until it is closed and empty, flushing periodically
/// and syncing at the end.
fn writer_loop(queue: Arc<Queue>, counters: Arc<Counters>, tlog_file: File, events_file: File) {
    let mut tlog = BufWriter::new(tlog_file);
    let mut events = BufWriter::new(events_file);
    let mut last_flush = Instant::now();
    loop {
        match queue.pop_timeout(FLUSH_INTERVAL) {
            Pop::Item(Item::Frame(frame)) => {
                if tlog::write_frame(&mut tlog, frame.utc_us, &frame.bytes).is_ok() {
                    counters.frames.fetch_add(1, Ordering::Relaxed);
                    counters.bytes.fetch_add(
                        frame.bytes.len() as u64 + tlog::TIMESTAMP_LEN as u64,
                        Ordering::Relaxed,
                    );
                }
            }
            Pop::Item(Item::Event(event)) => {
                if write_event(&mut events, &event).is_ok() {
                    counters.events.fetch_add(1, Ordering::Relaxed);
                }
            }
            Pop::Timeout => {}
            Pop::Drained => break,
        }
        if last_flush.elapsed() >= FLUSH_INTERVAL {
            let _ = tlog.flush();
            let _ = events.flush();
            last_flush = Instant::now();
        }
    }
    let _ = tlog.flush();
    let _ = events.flush();
    let _ = tlog.get_ref().sync_all();
    let _ = events.get_ref().sync_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "maggcs-recorder-{}-{}-{}",
            tag,
            std::process::id(),
            now_utc_us()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn frame(bytes: &[u8]) -> RawFrame {
        RawFrame {
            header: crate::mavlink::MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            bytes: bytes.to_vec(),
            received_at: Instant::now(),
            utc_us: 1_234_567,
        }
    }

    #[test]
    fn the_queue_drops_the_oldest_item_and_never_blocks() {
        let counters = Counters::default();
        let queue = Queue::new(2);
        queue.push(
            Item::Event(LoggedEvent::new(RecordEvent::Note { text: "a".into() })),
            &counters,
        );
        queue.push(
            Item::Event(LoggedEvent::new(RecordEvent::Note { text: "b".into() })),
            &counters,
        );
        let started = Instant::now();
        queue.push(
            Item::Event(LoggedEvent::new(RecordEvent::Note { text: "c".into() })),
            &counters,
        );
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "a full queue must not block the producer"
        );
        assert_eq!(counters.snapshot().dropped, 1);
        let items = queue.inner.lock().expect("lock");
        assert_eq!(items.len(), 2);
        match &items[0] {
            Item::Event(e) => assert_eq!(e.event, RecordEvent::Note { text: "b".into() }),
            _ => panic!("expected an event"),
        }
    }

    #[tokio::test]
    async fn writes_a_replayable_tlog_and_manifest() {
        let dir = temp_dir("tlog");
        let config = RecorderConfig {
            endpoint: "udpin:0.0.0.0:14550".to_string(),
            target_system_id: 1,
            target_component_id: 1,
            ..RecorderConfig::new(&dir)
        };
        // A well-formed MAVLink v2 frame: STX + 9 header + 2 payload + 2 CRC.
        let wire = vec![0xFD, 2, 0, 0, 0, 1, 1, 9, 0, 0, 0xAA, 0xBB, 0xCC, 0xDD];
        let recorder = Recorder::spawn(config).expect("spawn");
        recorder.record_frame(frame(&wire));
        recorder.record_event(RecordEvent::Note {
            text: "hello".to_string(),
        });
        recorder.close();
        let outcome = recorder.flush(Duration::from_secs(2)).await;
        assert!(
            matches!(outcome, StopOutcome::Stopped { .. }),
            "{outcome:?}"
        );

        let records = tlog::read_records(&dir.join(TLOG_FILE)).expect("read tlog");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].bytes, wire, "tlog replay is byte-identical");
        assert_eq!(records[0].utc_us, 1_234_567);

        let events = super::super::read_events(&dir.join(EVENTS_FILE)).expect("read events");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event,
            RecordEvent::Note {
                text: "hello".to_string()
            }
        );

        let manifest = super::super::session::read_manifest(&dir.join(super::super::SESSION_FILE))
            .expect("manifest");
        assert_eq!(manifest.frames, 1);
        assert_eq!(manifest.events, 1);
        assert_eq!(manifest.dropped, 0);
        assert!(manifest.closed_utc_us.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
