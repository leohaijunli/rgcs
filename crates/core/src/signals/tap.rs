//! The signal tap: an async task that reads the routed connection stream,
//! extracts every numeric field, and forwards the samples the inspector is
//! subscribed to. Runs only while the inspector is open (subscription refcount),
//! so a closed inspector costs nothing.
//!
//! Timestamps prefer the flight controller's `time_boot_ms`/`time_usec` where a
//! message carries one, mapped onto the host axis by the minimum `rx − fc`
//! offset (the plan's approach); messages without a time field fall back to the
//! receive time. The UI flags traces whose timing is receive-based.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mavlink::Message;
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

/// The shared subscription set and reference count.
#[derive(Debug, Clone, Default)]
pub struct Subscriptions {
    ids: Arc<Mutex<HashSet<SignalId>>>,
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
        *self.ids.lock().expect("subscription lock") = ids;
    }

    /// Drop every subscription (the window closed; the tap id-checks nothing).
    pub fn clear(&self) {
        self.ids.lock().expect("subscription lock").clear();
    }

    pub fn contains(&self, id: &SignalId) -> bool {
        self.ids.lock().expect("subscription lock").contains(id)
    }

    /// The message ids that have at least one subscribed field (pre-filter).
    pub fn message_ids(&self) -> HashSet<u32> {
        self.ids
            .lock()
            .expect("subscription lock")
            .iter()
            .map(|s| s.message_id)
            .collect()
    }
}

/// Maps FC timestamps onto the host axis via the minimum `rx − fc` offset.
#[derive(Debug, Clone, Default)]
pub struct TimestampMapper {
    offset_ms: Option<f64>,
}

impl TimestampMapper {
    /// Map an FC timestamp (ms) to host time (ms), keeping the minimum offset.
    pub fn map(&mut self, fc_ms: f64, rx_ms: f64) -> f64 {
        let offset = rx_ms - fc_ms;
        match self.offset_ms {
            None => {
                self.offset_ms = Some(offset);
                rx_ms
            }
            Some(min) => {
                if offset < min {
                    self.offset_ms = Some(offset);
                }
                fc_ms + self.offset_ms.unwrap_or(offset)
            }
        }
    }
}

/// The tap task outcome, surfaced to the UI like the hub's dropped counter.
#[derive(Debug, Default, Clone)]
pub struct TapStats {
    pub messages: u64,
    pub samples: u64,
    pub dropped: u64,
}

/// Run the tap until the stream closes. Subscribed samples are forwarded on
/// `tx`; the catalog is updated for every seen field. Takes owned clones so a
/// service can spawn it without a `'static` fight.
pub async fn run_tap(
    mut events: RoutedEvents,
    subs: Subscriptions,
    catalog: Arc<Mutex<SignalCatalog>>,
    tx: mpsc::Sender<SignalSample>,
    stats: Arc<Mutex<TapStats>>,
) {
    let mut mapper = TimestampMapper::default();
    loop {
        match events.recv().await {
            Ok(ConnectionEvent::Message(env)) => {
                handle_message(&env, &subs, &catalog, &mut mapper, &tx, &stats);
            }
            Ok(_) => continue, // lifecycle events pass through but carry no samples
            Err(_) => break,   // lagged or closed; the caller re-subscribes
        }
    }
}

fn handle_message(
    env: &MessageEnvelope,
    subs: &Subscriptions,
    catalog: &Arc<Mutex<SignalCatalog>>,
    mapper: &mut TimestampMapper,
    tx: &mpsc::Sender<SignalSample>,
    stats: &Arc<Mutex<TapStats>>,
) {
    let message_id = env.message.message_id();
    let message_ids = subs.message_ids();
    let active = subs.is_active();
    if !message_ids.contains(&message_id) {
        return; // nothing subscribed on this message type: drop by id
    }
    let Ok(extraction) = extract::extract(&env.message) else {
        return;
    };
    let rx_ms = env.received_at.elapsed().as_secs_f64() * 1000.0;
    let t_ms = fc_ms(&extraction)
        .map(|fc| mapper.map(fc, rx_ms))
        .unwrap_or(rx_ms);
    {
        let mut cat = catalog.lock().expect("catalog lock");
        for field in &extraction.fields {
            let id = SignalId::new(
                env.system_id(),
                env.component_id(),
                message_id,
                field.field.clone(),
            );
            cat.observe(&id, field.value, env.received_at);
        }
    }
    if !active {
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
    stats.lock().expect("stats lock").messages += 1;
}

/// The FC timestamp in milliseconds, from `time_boot_ms` or `time_usec`.
fn fc_ms(extraction: &extract::Extraction) -> Option<f64> {
    for name in FC_MS_FIELDS {
        if let Some(f) = extraction.fields.iter().find(|f| f.field == *name) {
            let ms = if *name == "time_usec" {
                f.value / 1000.0
            } else {
                f.value
            };
            return Some(ms);
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
    use mavlink::common::{MavMessage, ATTITUDE_DATA};
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

        let tap = tokio::spawn(run_tap(
            events,
            subs.clone(),
            catalog.clone(),
            tx,
            stats.clone(),
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
    async fn tap_is_quiet_while_no_window_is_open() {
        let (bus_tx, bus_rx) = tokio::sync::broadcast::channel(64);
        let events = RoutedEvents::new(bus_rx, MessageRoute::all());
        let subs = Subscriptions::new(); // never acquired
        subs.set(HashSet::from([SignalId::new(1, 1, 30, "roll")]));
        let catalog = Arc::new(Mutex::new(SignalCatalog::new()));
        let stats = Arc::new(Mutex::new(TapStats::default()));
        let (tx, mut rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);

        let tap = tokio::spawn(run_tap(events, subs, catalog.clone(), tx, stats.clone()));
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
}
