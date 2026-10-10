//! ULog replay source for the Signal Inspector (plan decision B): turn a
//! recorded PX4 `.ulg` into the same [`SignalSample`] stream the live tap
//! produces, so `batch_loop`, the DSP session and the frontend stay unchanged.
//!
//! [`UlogSource`] owns the [`UlogReader`] and seeks on the log's own time axis
//! (ms from the first data record). [`run_replay`] mirrors
//! `core::signals::tap::run_tap`'s signature and honours a [`ReplayControl`]
//! (play/pause, speed, seek), so a service can swap the two without touching
//! the rest of the pipeline.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use crate::signals::catalog::SignalCatalog;
use crate::signals::tap::{Subscriptions, TapStats};
use crate::signals::{SignalId, SignalSample};

use super::{UlogError, UlogReader, UlogRecord};

/// The boxed source type: `Read + Seek + Send` cannot be written as one trait
/// object (only auto traits may be added), so wrap them in a supertrait.
trait ReadSeekSend: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeekSend for T {}

/// Synthetic MAVLink node ids for replayed ULog topics (a log has no link).
const SYSTEM_ID: u8 = 1;
const COMPONENT_ID: u8 = 1;

/// Longest single pacing sleep, so pause/seek stay responsive during a long gap.
const MAX_SLEEP_MS: f64 = 50.0;
/// Poll interval while paused, at the end of the log, or after a read error.
const IDLE_POLL_MS: u64 = 30;
/// After a seek, stream this much log time (capped) without pacing so the plot
/// shows data at the new position immediately — even while paused.
const SCRUB_MAX_MS: f64 = 30_000.0;
/// Upper bound on the records read in one scrub burst (a dense log's 30 s).
const SCRUB_MAX_STEPS: usize = 120_000;
/// Yield to the runtime every this many scrub records so a burst cannot starve
/// the batch loop (which drains the sample channel).
const SCRUB_YIELD_EVERY: usize = 512;

/// One topic's identity for the catalog: a stable synthetic `message_id` plus
/// the uORB name.
#[derive(Debug, Clone, PartialEq)]
struct TopicMeta {
    message_id: u32,
    name: String,
}

/// A topic summary returned by [`UlogSource::topics`], for the open dialog's
/// confirmation.
#[derive(Debug, Clone, PartialEq)]
pub struct UlogTopicSummary {
    pub name: String,
    pub message_id: u32,
    pub signals: usize,
}

/// One data record decoded into samples that share a relative timestamp.
#[derive(Debug, Clone)]
pub struct ReplayStep {
    /// Relative log time, ms from the first data record.
    pub t_ms: f64,
    /// Synthetic MAVLink message id (see [`topic_message_id`]).
    pub message_id: u32,
    /// uORB topic name, e.g. `vehicle_attitude`.
    pub message_name: String,
    /// One sample per numeric field of the topic.
    pub samples: Vec<SignalSample>,
}

/// A recorded ULog opened for replay, with seeking on the log's time axis.
pub struct UlogSource {
    reader: UlogReader<Box<dyn ReadSeekSend>>,
    /// Byte offset of the first data record (rewind here before scanning).
    data_start: u64,
    /// `timestamp` of the first data record, µs.
    start_us: u64,
    /// `timestamp` of the last data record, µs.
    end_us: u64,
    /// Last emitted timestamp, for topics without a `timestamp` field.
    last_us: u64,
    /// Topic id → synthetic message identity.
    meta: HashMap<u16, TopicMeta>,
}

impl UlogSource {
    /// Open `path` and index it: one pass builds the topic table and finds the
    /// first/last data timestamps, then the file is rewound to the first data
    /// record so [`Self::next_step`] starts at the beginning.
    pub fn open(path: &Path) -> Result<Self, UlogError> {
        Self::from_reader(File::open(path)?)
    }

    /// Open any seekable source (used by the tests with an in-memory buffer).
    pub fn from_reader<R: Read + Seek + Send + 'static>(src: R) -> Result<Self, UlogError> {
        let boxed: Box<dyn ReadSeekSend> = Box::new(src);
        let mut reader = UlogReader::open(boxed)?;

        let mut data_start = None;
        let mut start_us = 0u64;
        let mut end_us = 0u64;
        loop {
            let rec_start = reader.position;
            match reader.next_record()? {
                None => break,
                Some(UlogRecord::Data { topic_id }) => {
                    let body = reader.data_body();
                    let Some(topic) = reader.topics.get(&topic_id) else {
                        continue;
                    };
                    let Some(t_us) = topic.timestamp_us(body) else {
                        continue;
                    };
                    if data_start.is_none() {
                        data_start = Some(rec_start);
                        start_us = t_us;
                    }
                    end_us = t_us;
                }
                Some(UlogRecord::Skipped(_)) => {}
            }
        }
        let data_start =
            data_start.ok_or_else(|| UlogError::BadFormat("no data records".into()))?;

        let meta = reader
            .topics
            .iter()
            .map(|(id, topic)| {
                (
                    *id,
                    TopicMeta {
                        message_id: topic_message_id(&topic.name, topic.multi_id),
                        name: topic.name.clone(),
                    },
                )
            })
            .collect();

        reader.seek_to(data_start)?;
        Ok(Self {
            reader,
            data_start,
            start_us,
            end_us,
            last_us: start_us,
            meta,
        })
    }

    /// Length of the log's data span, ms.
    pub fn duration_ms(&self) -> f64 {
        self.end_us.saturating_sub(self.start_us) as f64 / 1000.0
    }

    /// Every signal paired with its uORB topic name, so the caller can seed the
    /// signal catalog (the tree) before any sample has streamed.
    pub fn signal_entries(&self) -> Vec<(SignalId, String)> {
        let mut out = Vec::new();
        for (id, topic) in &self.reader.topics {
            let Some(meta) = self.meta.get(id) else {
                continue;
            };
            for field in &topic.fields {
                out.push((
                    SignalId::new(SYSTEM_ID, COMPONENT_ID, meta.message_id, field.name.clone()),
                    meta.name.clone(),
                ));
            }
        }
        out.sort_by(|a, b| (a.0.message_id, &a.0.field).cmp(&(b.0.message_id, &b.0.field)));
        out
    }

    /// Every numeric signal the log carries, sorted for stable output.
    pub fn signals(&self) -> Vec<SignalId> {
        self.signal_entries()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// One summary per topic, sorted by name.
    pub fn topics(&self) -> Vec<UlogTopicSummary> {
        let mut out: Vec<UlogTopicSummary> = self
            .reader
            .topics
            .iter()
            .filter_map(|(id, topic)| {
                let meta = self.meta.get(id)?;
                Some(UlogTopicSummary {
                    name: topic.name.clone(),
                    message_id: meta.message_id,
                    signals: topic.fields.len(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Seek so that the next [`Self::next_step`] returns the first data record
    /// at or after `t_ms` (relative log time, clamped to the log's span).
    pub fn seek_ms(&mut self, t_ms: f64) -> Result<(), UlogError> {
        let target = t_ms.clamp(0.0, self.duration_ms());
        let target_us = self.start_us + (target * 1000.0).round() as u64;
        self.reader.seek_to(self.data_start)?;
        self.last_us = self.start_us;
        loop {
            let rec_start = self.reader.position;
            match self.reader.next_record()? {
                None => {
                    self.last_us = self.end_us;
                    break;
                }
                Some(UlogRecord::Data { topic_id }) => {
                    let body = self.reader.data_body();
                    let Some(topic) = self.reader.topics.get(&topic_id) else {
                        continue;
                    };
                    let Some(t_us) = topic.timestamp_us(body) else {
                        continue;
                    };
                    if t_us >= target_us {
                        self.reader.seek_to(rec_start)?;
                        self.last_us = t_us;
                        break;
                    }
                    self.last_us = t_us;
                }
                Some(UlogRecord::Skipped(_)) => {}
            }
        }
        Ok(())
    }

    /// Decode the next data record into samples. `Ok(None)` is the end of the
    /// log (or a truncated tail, which the reader treats as EOF).
    pub fn next_step(&mut self) -> Result<Option<ReplayStep>, UlogError> {
        loop {
            match self.reader.next_record()? {
                None => return Ok(None),
                Some(UlogRecord::Skipped(_)) => continue,
                Some(UlogRecord::Data { topic_id }) => {
                    let Some((message_id, message_name)) = self
                        .meta
                        .get(&topic_id)
                        .map(|m| (m.message_id, m.name.clone()))
                    else {
                        continue;
                    };
                    let (t_us, t_ms, samples) = {
                        let body = self.reader.data_body();
                        let Some(topic) = self.reader.topics.get(&topic_id) else {
                            continue;
                        };
                        let t_us = topic.timestamp_us(body).unwrap_or(self.last_us);
                        let t_ms = t_us.saturating_sub(self.start_us) as f64 / 1000.0;
                        let mut values = Vec::new();
                        topic.decode(body, &mut values);
                        let samples = topic
                            .fields
                            .iter()
                            .zip(values)
                            .map(|(field, value)| SignalSample {
                                id: SignalId::new(
                                    SYSTEM_ID,
                                    COMPONENT_ID,
                                    message_id,
                                    field.name.clone(),
                                ),
                                t_ms,
                                value,
                            })
                            .collect();
                        (t_us, t_ms, samples)
                    };
                    self.last_us = t_us.max(self.last_us);
                    return Ok(Some(ReplayStep {
                        t_ms,
                        message_id,
                        message_name,
                        samples,
                    }));
                }
            }
        }
    }
}

/// Stable synthetic MAVLink message id for a ULog topic, so the inspector's
/// `(message, field)` keys survive across opens. The instance is mixed in, so
/// several instances of one topic get distinct ids. `fnv1a` is used instead of
/// the default hasher because it must not change between runs.
fn topic_message_id(name: &str, multi_id: u8) -> u32 {
    fnv1a(format!("ulog:{name}#{multi_id}").as_bytes())
}

/// 32-bit FNV-1a.
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Playback control shared between the command thread and [`run_replay`].
///
/// Speed is stored as an integer `×1000` because there is no `AtomicF64` in
/// std; a clamped `0.1..=100` range keeps the conversion lossless enough.
pub struct ReplayControl {
    playing: AtomicBool,
    speed_milli: AtomicU64,
    /// Position (relative ms) the replay has reached, for the seek slider.
    position_ms: AtomicU64,
    /// Pending seek request `(target ms, span ms to scrub forward to show)`.
    seek_ms: Mutex<Option<(f64, f64)>>,
}

impl Default for ReplayControl {
    fn default() -> Self {
        Self {
            playing: AtomicBool::new(true),
            speed_milli: AtomicU64::new(1000),
            position_ms: AtomicU64::new(0),
            seek_ms: Mutex::new(None),
        }
    }
}

impl ReplayControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::Relaxed);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn set_speed(&self, speed: f64) {
        let milli = (speed.clamp(0.1, 100.0) * 1000.0).round() as u64;
        self.speed_milli.store(milli, Ordering::Relaxed);
    }

    pub fn speed(&self) -> f64 {
        self.speed_milli.load(Ordering::Relaxed) as f64 / 1000.0
    }

    pub fn position_ms(&self) -> f64 {
        self.position_ms.load(Ordering::Relaxed) as f64
    }

    fn set_position_ms(&self, t_ms: f64) {
        self.position_ms
            .store(t_ms.max(0.0).round() as u64, Ordering::Relaxed);
    }

    /// Ask the replay to jump to `t_ms`; `span_ms` is how much log time to
    /// stream immediately so the plot shows data there (the visible window).
    pub fn request_seek(&self, t_ms: f64, span_ms: f64) {
        *self.seek_ms.lock().expect("replay seek lock") = Some((t_ms, span_ms));
    }

    pub fn take_seek(&self) -> Option<(f64, f64)> {
        self.seek_ms.lock().expect("replay seek lock").take()
    }

    pub fn has_seek(&self) -> bool {
        self.seek_ms.lock().expect("replay seek lock").is_some()
    }
}

/// Replay a log into `tx` until it is stopped or the receiver closes. Mirrors
/// `run_tap`: subscribed samples are forwarded, but the catalog is updated for
/// every field so the signal tree fills.
///
/// At the end of the log playback pauses (rather than exiting) so the user can
/// seek back; only `stop` (window close / source switch) ends the task.
pub async fn run_replay(
    mut source: UlogSource,
    subs: Subscriptions,
    catalog: Arc<Mutex<SignalCatalog>>,
    tx: mpsc::Sender<SignalSample>,
    stats: Arc<Mutex<TapStats>>,
    control: Arc<ReplayControl>,
    mut stop: oneshot::Receiver<()>,
) {
    let epoch = Instant::now();
    let mut prev_rel_ms = 0.0f64;
    // Log time to scrub up to without pacing after a seek (`NEG_INFINITY` when
    // not scrubbing).
    let mut scrub_until = f64::NEG_INFINITY;
    let mut scrub_steps = 0usize;
    loop {
        // Stop promptly even in the middle of an unpaced scrub burst.
        if matches!(
            stop.try_recv(),
            Ok(()) | Err(oneshot::error::TryRecvError::Closed)
        ) {
            return;
        }
        if let Some((seek, span)) = control.take_seek() {
            if source.seek_ms(seek).is_ok() {
                prev_rel_ms = seek;
                control.set_position_ms(seek);
                scrub_until = seek + span.clamp(0.0, SCRUB_MAX_MS);
            }
        }
        // While paused (and not filling a fresh seek) there is nothing to do.
        if !control.is_playing() && prev_rel_ms >= scrub_until {
            if wait_or_stop(&mut stop, Duration::from_millis(IDLE_POLL_MS)).await {
                break;
            }
            continue;
        }
        if tx.is_closed() {
            break;
        }
        let step = match source.next_step() {
            Ok(Some(step)) => step,
            // End of log: park in the paused state so the UI can seek back.
            Ok(None) => {
                control.set_playing(false);
                scrub_until = f64::NEG_INFINITY;
                if wait_or_stop(&mut stop, Duration::from_millis(IDLE_POLL_MS)).await {
                    break;
                }
                continue;
            }
            // A corrupt record: pause instead of spinning on the same offset.
            Err(_) => {
                control.set_playing(false);
                scrub_until = f64::NEG_INFINITY;
                if wait_or_stop(&mut stop, Duration::from_millis(IDLE_POLL_MS * 4)).await {
                    break;
                }
                continue;
            }
        };
        let gap_ms = (step.t_ms - prev_rel_ms).max(0.0);
        let t_ms = step.t_ms;
        if !deliver(&step, &subs, &catalog, &tx, &stats, epoch).await {
            break;
        }
        stats.lock().expect("stats lock").messages += 1;
        control.set_position_ms(t_ms);
        prev_rel_ms = t_ms;

        // Scrubbing past a seek: read on without pacing so the plot fills, but
        // yield so the batch loop can drain, and stop after a bounded burst.
        if prev_rel_ms < scrub_until {
            scrub_steps += 1;
            if scrub_steps >= SCRUB_MAX_STEPS {
                scrub_until = f64::NEG_INFINITY;
            } else if scrub_steps.is_multiple_of(SCRUB_YIELD_EVERY) {
                tokio::task::yield_now().await;
            }
            continue;
        }
        scrub_steps = 0;

        let mut remaining = gap_ms / control.speed();
        while remaining > 0.0 {
            if !control.is_playing() || control.has_seek() {
                break;
            }
            let chunk = remaining.min(MAX_SLEEP_MS);
            if wait_or_stop(&mut stop, Duration::from_secs_f64(chunk / 1000.0)).await {
                return;
            }
            remaining -= chunk;
        }
    }
}

/// Update the catalog for every field and forward the subscribed samples. Uses
/// a blocking send (backpressure): replay is not real-time, so it must not drop
/// samples the way the live tap's `try_send` deliberately can.
async fn deliver(
    step: &ReplayStep,
    subs: &Subscriptions,
    catalog: &Arc<Mutex<SignalCatalog>>,
    tx: &mpsc::Sender<SignalSample>,
    stats: &Arc<Mutex<TapStats>>,
    epoch: Instant,
) -> bool {
    // A synthetic clock on the log's time axis (not wall time): the catalog's
    // EMA rate then reflects the log's native sample rate regardless of speed.
    let now = epoch + Duration::from_secs_f64((step.t_ms / 1000.0).max(0.0));
    {
        let mut cat = catalog.lock().expect("catalog lock");
        for sample in &step.samples {
            cat.observe(&sample.id, &step.message_name, sample.value, now);
        }
    }
    if !subs.is_active() {
        return true;
    }
    for sample in &step.samples {
        if !subs.contains(&sample.id) {
            continue;
        }
        if tx.send(sample.clone()).await.is_err() {
            return false; // receiver dropped: stop the task
        }
        stats.lock().expect("stats lock").samples += 1;
    }
    true
}

/// Wait for `dur`, returning `true` if the stop signal fired first.
async fn wait_or_stop(stop: &mut oneshot::Receiver<()>, dur: Duration) -> bool {
    tokio::select! {
        _ = &mut *stop => true,
        _ = tokio::time::sleep(dur) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::tests_util::replay_log;
    use super::*;
    use std::collections::HashSet;
    use std::io::Cursor;

    fn source(bytes: Vec<u8>) -> UlogSource {
        UlogSource::from_reader(Cursor::new(bytes)).expect("open replay source")
    }

    #[test]
    fn indexes_duration_signals_and_topics() {
        let src = source(replay_log(0));
        assert!((src.duration_ms() - 8.0).abs() < 1e-9, "8 ms span");

        let signals = src.signals();
        let fields: Vec<&str> = signals.iter().map(|s| s.field.as_str()).collect();
        assert_eq!(fields, ["timestamp", "x"]);
        assert_eq!(
            signals[0].message_id, signals[1].message_id,
            "one topic, one id"
        );
        assert_eq!(signals[0].system_id, SYSTEM_ID);
        assert_eq!(signals[0].component_id, COMPONENT_ID);

        let topics = src.topics();
        assert_eq!(topics.len(), 1);
        assert_eq!(topics[0].name, "sensor");
        assert_eq!(topics[0].signals, 2);
    }

    #[test]
    fn signal_entries_pair_each_signal_with_its_topic_name() {
        let src = source(replay_log(0));
        let entries = src.signal_entries();
        let fields: Vec<&str> = entries.iter().map(|(id, _)| id.field.as_str()).collect();
        let names: Vec<&str> = entries.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(fields, ["timestamp", "x"]);
        assert_eq!(names, ["sensor", "sensor"]);
        assert_eq!(entries[0].0, src.signals()[0]);
    }

    #[test]
    fn message_ids_are_stable_across_opens() {
        let a = source(replay_log(0));
        let b = source(replay_log(500_000));
        assert_eq!(a.signals()[0].message_id, b.signals()[0].message_id);
    }

    #[test]
    fn seek_positions_the_next_step_at_the_target() {
        let mut src = source(replay_log(0));
        src.seek_ms(4.0).expect("seek");
        let step = src.next_step().expect("read").expect("a record");
        assert!((step.t_ms - 4.0).abs() < 1e-9, "t={}", step.t_ms);
        // Resume continues forward from the seek point.
        let step = src.next_step().expect("read").expect("a record");
        assert!((step.t_ms - 6.0).abs() < 1e-9, "t={}", step.t_ms);
    }

    #[test]
    fn seek_is_clamped_to_the_log_span() {
        let mut src = source(replay_log(0));
        src.seek_ms(1000.0).expect("seek past the end");
        let step = src.next_step().expect("read").expect("last record");
        assert!((step.t_ms - 8.0).abs() < 1e-9, "t={}", step.t_ms);
        assert!(src.next_step().expect("read").is_none(), "then EOF");
    }

    #[test]
    fn control_toggles_speed_and_reports_seeks() {
        let control = ReplayControl::new();
        assert!(control.is_playing());
        control.set_playing(false);
        assert!(!control.is_playing());
        control.set_speed(2.5);
        assert!((control.speed() - 2.5).abs() < 1e-9);
        assert!(!control.has_seek());
        control.request_seek(123.0, 5000.0);
        assert!(control.has_seek());
        assert_eq!(control.take_seek(), Some((123.0, 5000.0)));
        assert!(!control.has_seek(), "a seek is delivered once");
    }

    #[tokio::test]
    async fn replay_emits_only_subscribed_signals_but_fills_the_catalog() {
        let subs = Subscriptions::new();
        subs.acquire();
        let message_id = topic_message_id("sensor", 0);
        let x_id = SignalId::new(SYSTEM_ID, COMPONENT_ID, message_id, "x");
        subs.set(HashSet::from([x_id.clone()]));

        let catalog = Arc::new(Mutex::new(SignalCatalog::new()));
        let stats = Arc::new(Mutex::new(TapStats::default()));
        let control = Arc::new(ReplayControl::new());
        control.set_speed(100.0);
        let (tx, mut rx) = mpsc::channel(64);
        let (stop_tx, stop_rx) = oneshot::channel();

        let task = tokio::spawn(run_replay(
            source(replay_log(0)),
            subs,
            catalog.clone(),
            tx,
            stats.clone(),
            control,
            stop_rx,
        ));

        let mut got = Vec::new();
        while let Ok(Some(sample)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            got.push(sample);
            if got.len() >= 5 {
                break;
            }
        }
        let _ = stop_tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;

        assert_eq!(got.len(), 5, "one sample per record");
        assert!(
            got.iter().all(|s| s.id == x_id),
            "only the subscribed field is forwarded"
        );
        assert_eq!(
            catalog.lock().expect("catalog lock").len(),
            2,
            "the catalog still learns every field"
        );
        assert_eq!(stats.lock().expect("stats lock").samples, 5);
    }

    #[tokio::test]
    async fn replay_pauses_and_resumes_at_a_seek() {
        let subs = Subscriptions::new();
        subs.acquire();
        let x_id = SignalId::new(SYSTEM_ID, COMPONENT_ID, topic_message_id("sensor", 0), "x");
        subs.set(HashSet::from([x_id]));

        let control = Arc::new(ReplayControl::new());
        control.set_speed(100.0);
        control.set_playing(false); // start paused
        let (tx, mut rx) = mpsc::channel(64);
        let (stop_tx, stop_rx) = oneshot::channel();

        let task = tokio::spawn(run_replay(
            source(replay_log(0)),
            subs,
            Arc::new(Mutex::new(SignalCatalog::new())),
            tx,
            Arc::new(Mutex::new(TapStats::default())),
            control.clone(),
            stop_rx,
        ));

        // While paused nothing is emitted, even though the log has data.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), rx.recv())
                .await
                .is_err(),
            "paused replay emits nothing"
        );

        // Seek to 4 ms and play: the first sample is the record at 4 ms.
        control.request_seek(4.0, 1000.0);
        control.set_playing(true);
        let sample = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a sample")
            .expect("channel open");
        assert!((sample.t_ms - 4.0).abs() < 1e-9, "t={}", sample.t_ms);

        let _ = stop_tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
    }

    #[tokio::test]
    async fn a_seek_while_paused_streams_the_new_region() {
        // The user drags the slider while paused: the plot must show data at the
        // new position without having to press play.
        let subs = Subscriptions::new();
        subs.acquire();
        let x_id = SignalId::new(SYSTEM_ID, COMPONENT_ID, topic_message_id("sensor", 0), "x");
        subs.set(HashSet::from([x_id]));

        let control = Arc::new(ReplayControl::new());
        control.set_speed(100.0);
        control.set_playing(false);
        let (tx, mut rx) = mpsc::channel(64);
        let (stop_tx, stop_rx) = oneshot::channel();

        let task = tokio::spawn(run_replay(
            source(replay_log(0)),
            subs,
            Arc::new(Mutex::new(SignalCatalog::new())),
            tx,
            Arc::new(Mutex::new(TapStats::default())),
            control.clone(),
            stop_rx,
        ));

        assert!(
            tokio::time::timeout(Duration::from_millis(150), rx.recv())
                .await
                .is_err(),
            "paused replay emits nothing on its own"
        );

        // A 2 ms span covers the records at 4 ms and 6 ms.
        control.request_seek(4.0, 2000.0);
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a sample")
            .expect("channel open");
        assert!((first.t_ms - 4.0).abs() < 1e-9, "t={}", first.t_ms);
        let second = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a second sample")
            .expect("channel open");
        assert!((second.t_ms - 6.0).abs() < 1e-9, "t={}", second.t_ms);

        let _ = stop_tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
    }
}

/// In-memory ULog fixtures, reusing the writers in `ulog::tests`.
#[cfg(test)]
mod tests_util {
    use super::super::tests::{add_record, data_record, format_record, header};

    /// `sensor: uint64 timestamp; float x;` with five records 2 ms apart.
    pub(super) fn replay_log(start_us: u64) -> Vec<u8> {
        let mut v = header(start_us);
        v.extend(format_record("sensor:uint64_t timestamp;float x;"));
        v.extend(add_record(0, 7, "sensor"));
        for i in 0..5u64 {
            let mut body = (start_us + i * 2000).to_le_bytes().to_vec();
            body.extend_from_slice(&(i as f32).to_le_bytes());
            v.extend(data_record(7, &body));
        }
        v
    }
}
