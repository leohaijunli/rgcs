//! The recorder itself: a bounded, drop-oldest queue drained by a dedicated
//! writer thread (plan §3 A4).

use std::collections::VecDeque;
use std::fs::File;
use std::future::Future;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::events::{write_event, LoggedEvent, RecordEvent};
use super::session::{write_manifest, ForceArmAudit, SessionManifest};
use super::{now_utc_us, tlog, RecorderError, EVENTS_FILE, TLOG_FILE};
use crate::lifecycle::{ManagedService, Stage, StopOutcome};
use crate::mavlink::connection::RawFrame;
use crate::safety::{ChecklistItemId, ForceArmReason};

/// Default bounded-queue capacity. At ~100 Hz mag + telemetry, this is several
/// seconds of headroom before the drop-oldest policy engages.
pub const DEFAULT_QUEUE_CAP: usize = 8192;

/// How often the writer flushes. Bounds how much a crash can lose.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// How long [`Recorder::record_force_arm`] waits for the writer thread to
/// confirm the audit is durable before refusing the forced arm. The writer
/// normally acknowledges within microseconds; the bound only matters when the
/// disk or the writer stalls, and the forced arm is then refused — never
/// authorized without a durable record.
const AUDIT_ACK_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// GCS identity used for synthetic tlog markers (ADR-021); never the FC's.
    pub gcs_system_id: u8,
    pub gcs_component_id: u8,
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
            gcs_system_id: 250,
            gcs_component_id: 250,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// A snapshot of the recorder's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RecorderStats {
    /// Frames written to the tlog.
    pub frames: u64,
    /// Drape monitor samples written to `drape.jsonl` (DR7).
    pub drape: u64,
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
    drape: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> RecorderStats {
        RecorderStats {
            frames: self.frames.load(Ordering::Relaxed),
            drape: self.drape.load(Ordering::Relaxed),
            events: self.events.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
        }
    }
}

/// The plan identity the `drape.jsonl` header carries (set once per session).
#[derive(Debug, Clone)]
pub struct DrapeMetaShared {
    pub spec_hash: String,
    pub dem_hash: String,
}

enum Item {
    Frame(RawFrame),
    Event(LoggedEvent),
    /// One drape monitor sample (DR7), appended to `drape.jsonl`.
    Drape(super::drape_log::DrapeSampleRecord),
    /// The durable part of a forced-arm audit. Handled by the writer thread
    /// so `events.jsonl` keeps strict queue order; the ack reports when the
    /// audit is on disk (or why it is not).
    Audit(Box<AuditWork>),
}

/// Work for one forced-arm audit, executed by the writer thread.
struct AuditWork {
    event: LoggedEvent,
    audit: ForceArmAudit,
    ack: std::sync::mpsc::Sender<Result<(), RecorderError>>,
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
    manifest: Arc<Mutex<SessionManifest>>,
    manifest_event_lock: Mutex<()>,
    audit_closed: AtomicBool,
    /// The writer thread's events-file handle, kept here as the
    /// failure-injection point for tests (poisoning the lock makes the audit
    /// write fail). Production code never reads it: the writer thread owns
    /// the only write path.
    #[allow(dead_code)]
    events_writer: Arc<Mutex<BufWriter<File>>>,
    marker_header: crate::mavlink::MavHeader,
    /// The plan identity for the drape log header (DR7): set once, before
    /// the first `record_drape`.
    drape_meta: Arc<std::sync::Mutex<Option<DrapeMetaShared>>>,
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

        let initial_manifest = Arc::new(Mutex::new(SessionManifest::new(&config, started_utc_us)));
        let marker_header = crate::mavlink::MavHeader {
            system_id: config.gcs_system_id,
            component_id: config.gcs_component_id,
            sequence: 0,
        };
        let queue = Arc::new(Queue::new(config.queue_cap));
        let counters = Arc::new(Counters::default());
        let drape_meta: Arc<std::sync::Mutex<Option<DrapeMetaShared>>> =
            Arc::new(std::sync::Mutex::new(None));
        let writer_queue = queue.clone();
        let writer_counters = counters.clone();
        let events_writer = Arc::new(Mutex::new(BufWriter::new(events_file)));
        let writer_events = events_writer.clone();
        let writer_manifest = initial_manifest.clone();
        let writer_dir = config.dir.clone();
        let writer_drape_meta = drape_meta.clone();
        let join = std::thread::Builder::new()
            .name("maggcs-recorder".to_string())
            .spawn(move || {
                writer_loop(
                    writer_queue,
                    writer_counters,
                    tlog_file,
                    writer_events,
                    writer_manifest,
                    writer_drape_meta,
                    writer_dir,
                )
            })
            .map_err(RecorderError::Thread)?;

        Ok(Arc::new(Recorder {
            queue,
            counters,
            join: Mutex::new(Some(join)),
            dir: config.dir.clone(),
            manifest: initial_manifest,
            manifest_event_lock: Mutex::new(()),
            audit_closed: AtomicBool::new(false),
            events_writer,
            marker_header,
            drape_meta,
        }))
    }

    /// Set the plan identity the `drape.jsonl` header carries (DR7): call
    /// once when the drape monitor starts, before the first sample.
    pub fn set_drape_meta(&self, spec_hash: String, dem_hash: String) {
        *self
            .drape_meta
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(DrapeMetaShared {
            spec_hash,
            dem_hash,
        });
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

    /// Record one drape monitor sample (DR7): the step's scalars land in
    /// `drape.jsonl` next to the tlog/events. Never blocks (queue overflow
    /// counts as dropped like any other item).
    pub fn record_drape(&self, sample: super::drape_log::DrapeSampleRecord) {
        self.queue.push(Item::Drape(sample), &self.counters);
    }

    /// Record a forced-arm authorization in both the event log and manifest.
    ///
    /// The audit is serialized through the writer thread's queue so
    /// `events.jsonl` stays in enqueue order, and the call returns only after
    /// the audit is durable (fsynced) — a recorder that cannot confirm the
    /// record refuses the forced arm. Ordinary producers never block; this
    /// call may block up to [`AUDIT_ACK_TIMEOUT`] and callers should use a
    /// blocking-capable context.
    pub fn record_force_arm(
        &self,
        items: Vec<ChecklistItemId>,
        reason: ForceArmReason,
        note: Option<String>,
    ) -> Result<u64, RecorderError> {
        let time_utc_us = now_utc_us();
        let marker = self.force_arm_marker(time_utc_us, reason, items.len())?;
        if self.audit_closed.load(Ordering::Acquire) {
            return Err(RecorderError::WriterLost);
        }
        let audit = ForceArmAudit {
            items: items.clone(),
            reason,
            note: note.clone(),
            time_utc_us,
        };
        let event = LoggedEvent {
            ts_us: time_utc_us,
            event: RecordEvent::ForceArmAudit {
                items,
                reason,
                note,
                time_utc_us,
            },
        };
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        self.queue.push(
            Item::Audit(Box::new(AuditWork {
                event,
                audit,
                ack: ack_tx,
            })),
            &self.counters,
        );
        match ack_rx.recv_timeout(AUDIT_ACK_TIMEOUT) {
            Ok(Ok(())) => {}
            // The writer dropped the ack without confirming: never recorded.
            Ok(Err(error)) => return Err(error),
            // No answer in time (writer stalled, dead, or the item was
            // evicted after the queue overflowed): treat as unrecorded. A
            // late confirmation may still reach disk, which is safe — the
            // authorization is refused either way.
            Err(_) => return Err(RecorderError::AuditUnconfirmed),
        }
        // The tlog marker makes the audit visible in a QGC-style replay. It is
        // queued after the durable event/manifest write: a crash may lose the
        // marker, but never the authorization itself.
        self.record_frame(marker);
        Ok(time_utc_us)
    }

    fn force_arm_marker(
        &self,
        utc_us: u64,
        reason: ForceArmReason,
        item_count: usize,
    ) -> Result<RawFrame, RecorderError> {
        let reason = match reason {
            ForceArmReason::SafetyOverride => "safety_override",
            ForceArmReason::Weather => "weather",
            ForceArmReason::SensorTimeout => "sensor_timeout",
            ForceArmReason::TimePressure => "time_pressure",
        };
        let text = format!(
            "MAGGCS FORCE ARM: {reason} ({item_count} item{})",
            if item_count == 1 { "" } else { "s" }
        );
        let message = crate::mavlink::MavMessage::STATUSTEXT(::mavlink::common::STATUSTEXT_DATA {
            severity: ::mavlink::common::MavSeverity::MAV_SEVERITY_NOTICE,
            text: text.as_str().into(),
            id: 0,
            chunk_seq: 0,
        });
        let mut bytes = Vec::new();
        let mut header = self.marker_header;
        header.sequence = (utc_us & 0xff) as u8;
        ::mavlink::write_v2_msg(&mut bytes, header, &message)
            .map_err(|e| RecorderError::Audit(format!("marker encode failed: {e}")))?;
        Ok(RawFrame {
            header,
            bytes,
            received_at: Instant::now(),
            utc_us,
        })
    }
    /// Count frames the raw tap lost to a lagged subscriber.
    pub fn note_lagged(&self, n: u64) {
        self.queue.note_dropped(&self.counters, n);
    }

    /// Stop accepting work and wake the writer.
    pub fn close(&self) {
        if let Ok(_guard) = self.manifest_event_lock.lock() {
            self.audit_closed.store(true, Ordering::Release);
            self.queue.close();
        }
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
        match self.manifest_event_lock.lock() {
            Ok(_guard) => {
                self.audit_closed.store(true, Ordering::Release);
                self.queue.close();
            }
            Err(_) => {
                return StopOutcome::Failed {
                    message: "forced-arm audit lock was poisoned".to_string(),
                }
            }
        }
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
                let mut manifest = match self.manifest.lock() {
                    Ok(manifest) => manifest,
                    Err(_) => {
                        return StopOutcome::Failed {
                            message: "session manifest lock was poisoned".to_string(),
                        }
                    }
                };
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
fn writer_loop(
    queue: Arc<Queue>,
    counters: Arc<Counters>,
    tlog_file: File,
    events: Arc<Mutex<BufWriter<File>>>,
    manifest: Arc<Mutex<SessionManifest>>,
    drape_meta: Arc<std::sync::Mutex<Option<DrapeMetaShared>>>,
    dir: PathBuf,
) {
    let mut tlog = BufWriter::new(tlog_file);
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
                if let Ok(mut writer) = events.lock() {
                    if write_event(&mut *writer, &event).is_ok() {
                        counters.events.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Pop::Item(Item::Drape(sample)) => {
                // drape.jsonl: the meta header rides the first write (DR7);
                // without a meta yet the sample queues back off by skipping
                // (the caller sets the meta before the first sample arrives).
                let meta = drape_meta
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if let Some(meta) = meta {
                    if let Ok(line) = super::drape_log::sample_to_line(&sample) {
                        let meta = super::drape_log::DrapeMeta {
                            spec_hash: meta.spec_hash,
                            dem_hash: meta.dem_hash,
                            started_utc_us: sample.utc_us,
                        };
                        if super::drape_log::write_line(&dir, &meta, &line).is_ok() {
                            counters.drape.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            Pop::Item(Item::Audit(work)) => {
                let outcome = write_audit(&dir, &manifest, &events, &work);
                if outcome.is_ok() {
                    counters.events.fetch_add(1, Ordering::Relaxed);
                }
                // A dropped ack is fine: the caller treats a missing answer as
                // a refusal and never authorizes an unrecorded forced arm.
                let _ = work.ack.send(outcome);
            }
            Pop::Timeout => {}
            Pop::Drained => break,
        }
        if last_flush.elapsed() >= FLUSH_INTERVAL {
            let _ = tlog.flush();
            if let Ok(mut writer) = events.lock() {
                let _ = writer.flush();
            }
            last_flush = Instant::now();
        }
    }
    let _ = tlog.flush();
    if let Ok(mut writer) = events.lock() {
        let _ = writer.flush();
        let _ = writer.get_ref().sync_all();
    }
    let _ = tlog.get_ref().sync_all();
}

/// Persist one forced-arm audit: the manifest first, then the event line,
/// both durable (fsynced) before the writer acks the caller. Any failure
/// rolls the manifest back, so a failed audit never leaves a trace in
/// `session.json`. Runs on the writer thread, which is the only writer of
/// `events.jsonl`, keeping the file in strict enqueue order.
fn write_audit(
    dir: &Path,
    manifest: &Mutex<SessionManifest>,
    events: &Mutex<BufWriter<File>>,
    work: &AuditWork,
) -> Result<(), RecorderError> {
    let mut manifest_guard = manifest.lock().map_err(|_| RecorderError::WriterLost)?;
    let previous = manifest_guard.forced_arm.clone();
    let mut next = manifest_guard.clone();
    next.forced_arm = Some(work.audit.clone());
    write_manifest(dir, &next)?;
    *manifest_guard = next;
    drop(manifest_guard);

    let mut events_guard = match events.lock() {
        Ok(events) => events,
        Err(_) => {
            let rollback = restore_forced_arm(dir, manifest, previous);
            return match rollback {
                Err(error) => Err(error),
                Ok(()) => Err(RecorderError::WriterLost),
            };
        }
    };
    let event_start = events_guard.get_ref().metadata()?.len();
    if let Err(error) = write_event(&mut *events_guard, &work.event)
        .and_then(|()| events_guard.flush().map_err(RecorderError::from))
        .and_then(|()| {
            events_guard
                .get_ref()
                .sync_all()
                .map_err(RecorderError::from)
        })
    {
        let rollback = events_guard
            .get_mut()
            .set_len(event_start)
            .and_then(|()| events_guard.get_ref().sync_all());
        drop(events_guard);
        let manifest_rollback = restore_forced_arm(dir, manifest, previous);
        if let Err(rollback_error) = rollback {
            return Err(rollback_error.into());
        }
        manifest_rollback?;
        return Err(error);
    }
    Ok(())
}

/// Roll the manifest's `forced_arm` record back to `previous` after a failed
/// audit write.
fn restore_forced_arm(
    dir: &Path,
    manifest: &Mutex<SessionManifest>,
    previous: Option<ForceArmAudit>,
) -> Result<(), RecorderError> {
    let mut manifest_guard = manifest.lock().map_err(|_| RecorderError::WriterLost)?;
    manifest_guard.forced_arm = previous;
    write_manifest(dir, &manifest_guard)
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
    async fn force_arm_audit_failure_is_reported_and_not_added_to_manifest() {
        let dir = temp_dir("audit-failure");
        let recorder = Recorder::spawn(RecorderConfig::new(&dir)).expect("spawn");
        let events_writer = recorder.events_writer.clone();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = events_writer.lock().expect("event writer lock");
            panic!("poison event writer lock for failure injection");
        }))
        .is_err());

        let result = recorder.record_force_arm(
            vec![ChecklistItemId::Battery],
            ForceArmReason::SafetyOverride,
            None,
        );
        assert!(
            result.is_err(),
            "failed event write must block forced arming"
        );
        let manifest = super::super::session::read_manifest(&dir.join(super::super::SESSION_FILE))
            .expect("manifest remains valid");
        assert!(manifest.forced_arm.is_none());

        let outcome = recorder.flush(Duration::from_secs(2)).await;
        assert!(
            matches!(outcome, StopOutcome::Stopped { .. }),
            "{outcome:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn force_arm_after_close_is_rejected() {
        let dir = temp_dir("closed-audit");
        let recorder = Recorder::spawn(RecorderConfig::new(&dir)).expect("spawn");
        recorder.close();
        let result = recorder.record_force_arm(
            vec![ChecklistItemId::Battery],
            ForceArmReason::SafetyOverride,
            None,
        );
        assert!(
            result.is_err(),
            "closed recorder cannot authorize forced arming"
        );
        let outcome = recorder.flush(Duration::from_secs(2)).await;
        assert!(
            matches!(outcome, StopOutcome::Stopped { .. }),
            "{outcome:?}"
        );
        let manifest = super::super::session::read_manifest(&dir.join(super::super::SESSION_FILE))
            .expect("manifest");
        assert!(manifest.forced_arm.is_none());
        let _ = std::fs::remove_dir_all(&dir);
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
        let audit_us = recorder
            .record_force_arm(
                vec![ChecklistItemId::Battery, ChecklistItemId::MissionReady],
                ForceArmReason::Weather,
                Some("crosswind increased".to_string()),
            )
            .expect("force-arm audit");
        recorder.record_event(RecordEvent::Note {
            text: "after the audit".to_string(),
        });
        recorder.close();
        let outcome = recorder.flush(Duration::from_secs(2)).await;
        assert!(
            matches!(outcome, StopOutcome::Stopped { .. }),
            "{outcome:?}"
        );

        let records = tlog::read_records(&dir.join(TLOG_FILE)).expect("read tlog");
        assert_eq!(records.len(), 2, "one source frame plus the audit marker");
        assert_eq!(records[0].bytes, wire, "tlog replay is byte-identical");
        assert_eq!(records[0].utc_us, 1_234_567);

        let marker = &records[1];
        assert_eq!(
            marker.utc_us, audit_us,
            "marker matches the audit timestamp"
        );
        let mut reader =
            ::mavlink::peek_reader::PeekReader::new(std::io::Cursor::new(marker.bytes.as_slice()));
        let (header, message) =
            ::mavlink::read_v2_msg::<crate::mavlink::MavMessage, _>(&mut reader)
                .expect("marker decodes as MAVLink");
        assert_eq!(header.system_id, 250, "marker is GCS-sourced");
        assert_eq!(header.component_id, 250);
        match message {
            crate::mavlink::MavMessage::STATUSTEXT(text) => {
                let text = text.text.to_str().expect("ASCII marker text");
                assert!(text.starts_with("MAGGCS FORCE ARM: weather"), "{text}");
            }
            other => panic!("expected a STATUSTEXT marker, got {other:?}"),
        }

        let events = super::super::read_events(&dir.join(EVENTS_FILE)).expect("read events");
        // events.jsonl is in strict enqueue order: the note queued before the
        // audit is persisted before it, and the note queued after comes last.
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0].event,
            RecordEvent::Note {
                text: "hello".to_string()
            }
        );
        match &events[1].event {
            RecordEvent::ForceArmAudit {
                items,
                reason,
                note,
                time_utc_us,
            } => {
                assert_eq!(
                    items,
                    &vec![ChecklistItemId::Battery, ChecklistItemId::MissionReady]
                );
                assert_eq!(*reason, ForceArmReason::Weather);
                assert_eq!(note.as_deref(), Some("crosswind increased"));
                assert_eq!(*time_utc_us, events[1].ts_us);
            }

            other => panic!("expected forced-arm audit event, got {other:?}"),
        }
        assert_eq!(
            events[2].event,
            RecordEvent::Note {
                text: "after the audit".to_string()
            }
        );

        let manifest = super::super::session::read_manifest(&dir.join(super::super::SESSION_FILE))
            .expect("manifest");
        assert_eq!(manifest.frames, 2, "source frame plus marker");
        assert_eq!(manifest.events, 3);
        assert_eq!(manifest.dropped, 0);
        let force = manifest.forced_arm.expect("forced-arm audit");
        assert_eq!(
            force.items,
            vec![ChecklistItemId::Battery, ChecklistItemId::MissionReady]
        );
        assert_eq!(force.reason, ForceArmReason::Weather);
        assert_eq!(force.note.as_deref(), Some("crosswind increased"));
        assert!(force.time_utc_us > 0);
        assert_eq!(force.time_utc_us, events[1].ts_us);
        assert!(manifest.closed_utc_us.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
