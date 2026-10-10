//! The signal tap: an async task that reads the routed connection stream,
//! extracts every numeric field, and forwards the samples the inspector is
//! subscribed to. Runs only while the inspector is open (subscription refcount),
//! so a closed inspector costs nothing.
//!
//! Timestamps prefer the flight controller's `time_boot_ms`/`time_usec` where a
//! message carries one, mapped onto the host axis by the minimum `rx − fc`
//! offset (the plan's approach); messages without a time field fall back to the
//! receive time. The UI flags traces whose timing is receive-based.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mavlink::Message;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::mavlink::connection::ConnectionEvent;
use crate::mavlink::router::{MessageRoute, RoutedEvents};
use crate::mavlink::MessageEnvelope;

use super::catalog::SignalCatalog;
use super::extract;
use super::{SignalId, SignalSample};

/// Capacity of the sample channel between tap and service.
pub const SAMPLE_CHANNEL_CAP: usize = 4096;

/// The FC timestamp fields, in preference order (value in milliseconds).
const FC_MS_FIELDS: &[&str] = &["time_boot_ms", "time_usec"];

/// The subscription table plus the derived set of message ids (precomputed so
/// the hot path never allocates).
#[derive(Debug, Default)]
struct SubsState {
    ids: HashSet<SignalId>,
    message_ids: HashSet<u32>,
}

/// The shared subscription set and reference count.
#[derive(Debug, Clone, Default)]
pub struct Subscriptions {
    state: Arc<Mutex<SubsState>>,
    refs: Arc<AtomicUsize>,
}

impl Subscriptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// The inspector window opened: start producing samples.
    pub fn acquire(&self) {
        self.refs.fetch_add(1, Ordering::SeqCst);
    }

    /// The inspector window closed: stop producing samples.
    pub fn release(&self) {
        self.refs.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn is_active(&self) -> bool {
        self.refs.load(Ordering::SeqCst) > 0
    }

    /// The union of all signals the open plots use.
    pub fn set(&self, ids: HashSet<SignalId>) {
        let message_ids = ids.iter().map(|s| s.message_id).collect();
        *self.state.lock().expect("subscription lock") = SubsState { ids, message_ids };
    }

    /// Drop every subscription (the window closed; the tap id-checks nothing).
    pub fn clear(&self) {
        let mut state = self.state.lock().expect("subscription lock");
        state.ids.clear();
        state.message_ids.clear();
    }

    pub fn contains(&self, id: &SignalId) -> bool {
        self.state
            .lock()
            .expect("subscription lock")
            .ids
            .contains(id)
    }

    /// Whether any subscribed field belongs to this message (pre-filter, no
    /// allocation — P2).
    pub fn contains_message(&self, message_id: u32) -> bool {
        self.state
            .lock()
            .expect("subscription lock")
            .message_ids
            .contains(&message_id)
    }
}

/// An FC timestamp that drops by at least this much (ms) means the FC rebooted
/// (e.g. `time_usec` restarted) or the clock wrapped; re-baseline so new data
/// lands on the host axis instead of in the past.
const ROLLBACK_MS: f64 = 1000.0;

/// Maps one FC time source onto the host axis via the minimum `rx − fc`
/// offset, detecting FC reboots/clock wraps and re-baselining.
#[derive(Debug, Clone, Default)]
pub struct TimestampMapper {
    offset_ms: Option<f64>,
    last_fc_ms: Option<f64>,
    last_out_ms: Option<f64>,
}

impl TimestampMapper {
    /// Map an FC timestamp (ms) to host time (ms), keeping the minimum offset.
    pub fn map(&mut self, fc_ms: f64, rx_ms: f64) -> f64 {
        // FC reboot / clock wrap: the raw timestamp fell far below the previous
        // sample's, so the old minimum offset is meaningless — drop it.
        let rebooted = self
            .last_fc_ms
            .is_some_and(|last| fc_ms < last - ROLLBACK_MS);
        if rebooted {
            self.offset_ms = None;
        }
        self.last_fc_ms = Some(fc_ms);
        let offset = rx_ms - fc_ms;
        let offset = match self.offset_ms {
            None => {
                self.offset_ms = Some(offset);
                offset
            }
            Some(min) if offset < min => {
                self.offset_ms = Some(offset);
                offset
            }
            Some(min) => min,
        };
        let out = fc_ms + offset;
        // Host-axis safety net: if the mapped time still went backward by a
        // large margin, drop the stale baseline so the next sample re-baselines.
        if self
            .last_out_ms
            .is_some_and(|last| out < last - ROLLBACK_MS)
        {
            self.offset_ms = None;
        }
        self.last_out_ms = Some(out);
        out
    }
}

/// One [`TimestampMapper`] per FC time field, because `time_boot_ms` and
/// `time_usec` are not the same clock (different magnitude/rate) and each needs
/// its own rollback baseline.
#[derive(Debug, Clone, Default)]
pub struct TimestampMappers {
    by_field: HashMap<&'static str, TimestampMapper>,
}

impl TimestampMappers {
    pub fn map(&mut self, field: &'static str, fc_ms: f64, rx_ms: f64) -> f64 {
        self.by_field.entry(field).or_default().map(fc_ms, rx_ms)
    }
}

/// The tap task outcome, surfaced to the UI like the hub's dropped counter.
#[derive(Debug, Default, Clone, Serialize)]
pub struct TapStats {
    pub messages: u64,
    pub samples: u64,
    pub dropped: u64,
}

/// Run the tap until the stream closes or `stop` fires. Subscribed samples are
/// forwarded on `tx`; the catalog is updated for every seen field. Takes owned
/// clones so a service can spawn it without a `'static` fight.
pub async fn run_tap(
    mut events: RoutedEvents,
    subs: Subscriptions,
    catalog: Arc<Mutex<SignalCatalog>>,
    tx: mpsc::Sender<SignalSample>,
    stats: Arc<Mutex<TapStats>>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) {
    let mut mappers = TimestampMappers::default();
    // Host time axis: milliseconds since the tap started. (`received_at` is an
    // `Instant`, so `.elapsed()` would be ~0 for every message.)
    let epoch = std::time::Instant::now();
    loop {
        tokio::select! {
            // Window closed: cancel the tap instead of draining until the link
            // closes (plan P0-5).
            _ = &mut stop => break,
            ev = events.recv() => match ev {
                Ok(ConnectionEvent::Message(env)) => {
                    handle_message(&env, &subs, &catalog, &mut mappers, epoch, &tx, &stats);
                }
                Ok(_) => continue, // lifecycle events pass through but carry no samples
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    // The shared bus overflowed (PX4 streams a lot): count the
                    // loss and keep going instead of killing the tap for good.
                    stats.lock().expect("stats lock").dropped += n;
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

fn handle_message(
    env: &MessageEnvelope,
    subs: &Subscriptions,
    catalog: &Arc<Mutex<SignalCatalog>>,
    mapper: &mut TimestampMappers,
    epoch: std::time::Instant,
    tx: &mpsc::Sender<SignalSample>,
    stats: &Arc<Mutex<TapStats>>,
) {
    let message_id = env.message.message_id();
    let message_name = env.message.message_name();
    let Ok(extraction) = extract::extract(&env.message) else {
        return;
    };
    let rx_ms = env
        .received_at
        .saturating_duration_since(epoch)
        .as_secs_f64()
        * 1000.0;
    let t_ms = fc_ms(&extraction)
        .map(|(field, fc)| mapper.map(field, fc, rx_ms))
        .unwrap_or(rx_ms);
    // The catalog fills for every seen message, subscribed or not, so the
    // signal tree is populated before anything is checked (plan §4).
    {
        let mut cat = catalog.lock().expect("catalog lock");
        for field in &extraction.fields {
            let id = SignalId::new(
                env.system_id(),
                env.component_id(),
                message_id,
                field.field.clone(),
            );
            cat.observe(&id, message_name, field.value, env.received_at);
        }
    }
    stats.lock().expect("stats lock").messages += 1;
    if !subs.is_active() {
        return;
    }
    if !subs.contains_message(message_id) {
        return;
    }
    for signal in extraction.signals(env.system_id(), env.component_id(), message_id) {
        if subs.contains(&signal) {
            let value = extraction
                .fields
                .iter()
                .find(|f| {
                    let field = match (&extraction.name, f.field.as_str()) {
                        (Some(name), "value") => name == &signal.field,
                        _ => f.field == signal.field,
                    };
                    field
                })
                .map(|f| f.value);
            if let Some(value) = value {
                let sample = SignalSample {
                    id: signal,
                    t_ms,
                    value,
                };
                if tx.try_send(sample).is_err() {
                    stats.lock().expect("stats lock").dropped += 1;
                } else {
                    stats.lock().expect("stats lock").samples += 1;
                }
            }
        }
    }
}

/// The FC timestamp in milliseconds, from `time_boot_ms` or `time_usec`, plus
/// which field it came from (each field gets its own host-axis mapper).
fn fc_ms(extraction: &extract::Extraction) -> Option<(&'static str, f64)> {
    for name in FC_MS_FIELDS {
        if let Some(f) = extraction.fields.iter().find(|f| f.field == *name) {
            let ms = if *name == "time_usec" {
                f.value / 1000.0
            } else {
                f.value
            };
            return Some((name, ms));
        }
    }
    None
}

/// Construct the routed event stream for a connection handle.
pub fn subscribe(handle: &crate::mavlink::connection::ConnectionHandle) -> RoutedEvents {
    handle.subscribe_route(MessageRoute::all())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::connection::ConnectionEvent;
    use crate::mavlink::MavHeader;
    use mavlink::common::{
        MavAutopilot, MavMessage, MavModeFlag, MavState, MavType, ATTITUDE_DATA, HEARTBEAT_DATA,
    };
    use std::time::{Duration, Instant};

    fn attitude(time_boot_ms: u32, roll: f32) -> MavMessage {
        MavMessage::ATTITUDE(ATTITUDE_DATA {
            time_boot_ms,
            roll,
            pitch: 0.0,
            yaw: 0.0,
            rollspeed: 0.0,
            pitchspeed: 0.0,
            yawspeed: 0.0,
        })
    }

    fn envelope(seq: u8, msg: MavMessage) -> MessageEnvelope {
        MessageEnvelope {
            header: MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: seq,
            },
            message: msg,
            received_at: Instant::now(),
        }
    }

    /// A HEARTBEAT: a numeric message with no FC time field, so its samples
    /// must fall back to the host clock.
    fn heartbeat(custom_mode: u32) -> MavMessage {
        MavMessage::HEARTBEAT(HEARTBEAT_DATA {
            custom_mode,
            mavtype: MavType::MAV_TYPE_QUADROTOR,
            autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: MavModeFlag::empty(),
            system_status: MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        })
    }

    #[tokio::test]
    async fn tap_forwards_subscribed_samples_at_the_fc_rate_without_loss() {
        let (bus_tx, bus_rx) = tokio::sync::broadcast::channel(4096);
        let events = RoutedEvents::new(bus_rx, MessageRoute::all());
        let subs = Subscriptions::new();
        subs.acquire();
        subs.set(HashSet::from([SignalId::new(1, 1, 30, "roll")]));
        let catalog = Arc::new(Mutex::new(SignalCatalog::new()));
        let stats = Arc::new(Mutex::new(TapStats::default()));
        let (tx, mut rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);
        let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel();

        let tap = tokio::spawn(run_tap(
            events,
            subs.clone(),
            catalog.clone(),
            tx,
            stats.clone(),
            stop_rx,
        ));

        // Fake FC: 100 ATTITUDE messages at 100 Hz.
        for i in 0..100u32 {
            bus_tx
                .send(ConnectionEvent::Message(Box::new(envelope(
                    i as u8,
                    attitude(i, i as f32 * 0.1),
                ))))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(bus_tx);

        let mut samples = 0usize;
        while rx.recv().await.is_some() {
            samples += 1;
        }
        tap.await.unwrap();

        assert_eq!(samples, 100, "zero-loss samples");
        let s = stats.lock().unwrap();
        assert_eq!(s.messages, 100, "messages seen");
        assert_eq!(s.dropped, 0, "no channel drops");
        drop(s);

        let rows = catalog.lock().unwrap().snapshot(Instant::now());
        assert_eq!(rows.len(), 7, "all ATTITUDE fields in the tree");
        let roll = rows
            .iter()
            .find(|r| r.signal.field == "roll")
            .expect("roll row");
        assert!(
            roll.rate_hz > 50.0 && roll.rate_hz < 150.0,
            "rate {}",
            roll.rate_hz
        );
    }

    #[tokio::test]
    async fn catalog_fills_for_unsubscribed_messages_while_window_open() {
        // The window is open but nothing is checked yet: the signal tree must
        // still populate so the user can pick a signal (plan §4).
        let (bus_tx, bus_rx) = tokio::sync::broadcast::channel(64);
        let events = RoutedEvents::new(bus_rx, MessageRoute::all());
        let subs = Subscriptions::new();
        subs.acquire(); // window open
        let catalog = Arc::new(Mutex::new(SignalCatalog::new()));
        let stats = Arc::new(Mutex::new(TapStats::default()));
        let (tx, mut rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);
        let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel();

        let tap = tokio::spawn(run_tap(
            events,
            subs.clone(),
            catalog.clone(),
            tx,
            stats.clone(),
            stop_rx,
        ));

        bus_tx
            .send(ConnectionEvent::Message(Box::new(envelope(
                0,
                attitude(0, 0.1),
            ))))
            .unwrap();
        bus_tx
            .send(ConnectionEvent::Message(Box::new(envelope(
                1,
                attitude(10, 0.2),
            ))))
            .unwrap();
        drop(bus_tx);
        tokio::time::sleep(Duration::from_millis(30)).await;

        assert_eq!(
            catalog.lock().unwrap().len(),
            7,
            "all ATTITUDE fields are visible before any subscription"
        );
        assert!(rx.try_recv().is_err(), "no samples without a subscription");
        drop(rx);
        tap.await.unwrap();
    }

    #[tokio::test]
    async fn tap_is_quiet_while_no_window_is_open() {
        let (bus_tx, bus_rx) = tokio::sync::broadcast::channel(64);
        let events = RoutedEvents::new(bus_rx, MessageRoute::all());
        let subs = Subscriptions::new(); // never acquired
        subs.set(HashSet::from([SignalId::new(1, 1, 30, "roll")]));
        let catalog = Arc::new(Mutex::new(SignalCatalog::new()));
        let stats = Arc::new(Mutex::new(TapStats::default()));
        let (tx, mut rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);
        let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel();

        let tap = tokio::spawn(run_tap(
            events,
            subs,
            catalog.clone(),
            tx,
            stats.clone(),
            stop_rx,
        ));
        bus_tx
            .send(ConnectionEvent::Message(Box::new(envelope(
                0,
                attitude(0, 0.1),
            ))))
            .unwrap();
        drop(bus_tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The catalog fills (the tree needs it) but no samples leave the tap.
        assert!(!catalog.lock().unwrap().is_empty(), "catalog filled");
        assert!(rx.try_recv().is_err(), "no samples without an open window");
        tap.await.unwrap();
    }

    #[test]
    fn timestamp_mapper_rebaselines_on_fc_reboot() {
        let mut m = TimestampMapper::default();
        // Baseline at 100 Hz: fc time 100_000..100_020 ms, host 1000..1020 ms.
        assert_eq!(m.map(100_000.0, 1000.0), 1000.0);
        assert!(m.map(100_010.0, 1010.0) > 1005.0);
        assert!(m.map(100_020.0, 1020.0) > 1015.0);
        // FC reboot: time_usec restarts near zero.
        let t = m.map(5.0, 1030.0);
        // Mapped time must land on the host axis, not in the past.
        assert!(t > 1020.0, "re-baselined to host axis, got {t}");
        assert!(t < 1040.0, "within the current window, got {t}");
    }

    #[test]
    fn timestamp_mappers_keep_per_field_baselines() {
        // time_boot_ms and time_usec are different clocks; each re-baselines
        // independently, so a reboot of one must not disturb the other.
        let mut m = TimestampMappers::default();
        let boot = m.map("time_boot_ms", 10.0, 1000.0);
        assert_eq!(boot, 1000.0);
        assert!(m.map("time_boot_ms", 20.0, 1010.0) > 1005.0);
        // time_usec (ms) has a large magnitude; its baseline is independent.
        let usec = m.map("time_usec", 5_000_000.0, 1010.0);
        assert!(usec > 1000.0, "usec baseline independent, got {usec}");
        assert!(usec < 1020.0, "usec on host axis, got {usec}");
        // Mapping one field does not disturb the other's baseline.
        assert!(m.map("time_usec", 5_000_010.0, 1020.0) > 1010.0);
        assert!(m.map("time_boot_ms", 30.0, 1030.0) >= 1020.0);
    }

    #[test]
    fn subscriptions_precompute_message_ids() {
        let subs = Subscriptions::new();
        subs.set(HashSet::from([SignalId::new(1, 1, 30, "roll")]));
        assert!(subs.contains_message(30));
        assert!(!subs.contains_message(105));
        subs.clear();
        assert!(
            !subs.contains_message(30),
            "cleared subscriptions match none"
        );
    }

    #[tokio::test]
    async fn timestamps_without_a_time_field_are_monotonic() {
        // HEARTBEAT carries no time field, so `t_ms` falls back to the host
        // clock; consecutive samples must still be strictly increasing (plan S5).
        let (bus_tx, bus_rx) = tokio::sync::broadcast::channel(64);
        let events = RoutedEvents::new(bus_rx, MessageRoute::all());
        let subs = Subscriptions::new();
        subs.acquire();
        subs.set(HashSet::from([SignalId::new(1, 1, 0, "custom_mode")]));
        let catalog = Arc::new(Mutex::new(SignalCatalog::new()));
        let stats = Arc::new(Mutex::new(TapStats::default()));
        let (tx, mut rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);
        let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let tap = tokio::spawn(run_tap(events, subs, catalog, tx, stats, stop_rx));

        for i in 0..20u32 {
            bus_tx
                .send(ConnectionEvent::Message(Box::new(envelope(
                    i as u8,
                    heartbeat(i),
                ))))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(bus_tx);

        let mut ts = Vec::new();
        while let Some(s) = rx.recv().await {
            ts.push(s.t_ms);
        }
        tap.await.unwrap();
        assert_eq!(ts.len(), 20, "one sample per heartbeat");
        assert!(
            ts.windows(2).all(|w| w[1] > w[0]),
            "monotonic host timestamps: {ts:?}"
        );
    }

    #[tokio::test]
    async fn tap_survives_a_lagged_bus_and_keeps_forwarding() {
        // The shared MAVLink bus is a bounded broadcast channel; under PX4's
        // high-rate streams a slow reader overflows it. The tap must count the
        // loss and keep running instead of exiting for good (plan S5).
        let (bus_tx, bus_rx) = tokio::sync::broadcast::channel(2);
        let events = RoutedEvents::new(bus_rx, MessageRoute::all());
        let subs = Subscriptions::new();
        subs.acquire();
        subs.set(HashSet::from([SignalId::new(1, 1, 30, "roll")]));
        let catalog = Arc::new(Mutex::new(SignalCatalog::new()));
        let stats = Arc::new(Mutex::new(TapStats::default()));
        let (tx, mut rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);
        let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel();

        let tap = tokio::spawn(run_tap(events, subs, catalog, tx, stats.clone(), stop_rx));

        // Burst more than the bus holds without yielding, so the tap's first
        // `recv` is guaranteed to observe `Lagged`.
        for i in 0..20u32 {
            bus_tx
                .send(ConnectionEvent::Message(Box::new(envelope(
                    i as u8,
                    attitude(i, i as f32),
                ))))
                .unwrap();
        }
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }

        // The bus still carries data after the lag.
        bus_tx
            .send(ConnectionEvent::Message(Box::new(envelope(
                42,
                attitude(42, 5.0),
            ))))
            .unwrap();
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        drop(bus_tx);

        let mut samples = Vec::new();
        while let Some(s) = rx.recv().await {
            samples.push(s);
        }
        tap.await.unwrap();

        let dropped = stats.lock().unwrap().dropped;
        assert!(
            dropped > 0,
            "the overflow is counted as dropped, got {dropped}"
        );
        assert!(
            samples.len() >= 2,
            "tap keeps forwarding after Lagged, got {}",
            samples.len()
        );
        assert_eq!(
            samples.last().unwrap().value,
            5.0,
            "the post-lag message is delivered"
        );
    }

    #[tokio::test]
    async fn tap_stops_when_the_window_closes() {
        // The link stays open; closing the window must cancel the tap instead
        // of leaving it draining until the link closes (plan P0-5).
        let (bus_tx, bus_rx) = tokio::sync::broadcast::channel(64);
        let events = RoutedEvents::new(bus_rx, MessageRoute::all());
        let subs = Subscriptions::new();
        subs.acquire();
        let catalog = Arc::new(Mutex::new(SignalCatalog::new()));
        let stats = Arc::new(Mutex::new(TapStats::default()));
        let (tx, mut rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();

        let tap = tokio::spawn(run_tap(
            events,
            subs,
            catalog.clone(),
            tx,
            stats.clone(),
            stop_rx,
        ));
        stop_tx.send(()).unwrap();

        tokio::time::timeout(Duration::from_secs(2), tap)
            .await
            .expect("tap cancelled on window close")
            .unwrap();
        // The tap dropped its sender, so the receiver closes.
        assert!(rx.recv().await.is_none());
        drop(bus_tx);
    }
}
