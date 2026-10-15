//! The instrument runner: an async task over the routed MAVLink stream (W1).
//!
//! Mirrors [`crate::signals::tap::run_tap`]: subscribe to a connection route,
//! decode every matching frame into an [`InstrumentSample`], feed a
//! [`StreamMonitor`], and forward samples *and* health transitions on one
//! channel. No writes to MAVLink/parameters/DroneCAN (ADR-020).

use std::sync::Arc;
use std::time::Instant;

use mavlink::common::{GLOBAL_POSITION_INT_DATA, GPS_RAW_INT_DATA};
use mavlink::MessageData;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use crate::mavlink::router::{MessageRoute, RoutedEvents};
use crate::signals::{HIGHRES_IMU_ID, RAW_IMU_ID};

use super::monitor::{StreamEvent, StreamMonitor};
use super::sample::InstrumentSample;
use super::source::InstrumentSource;

/// Capacity of the sample/health channel between the runner and its consumer.
pub const INSTRUMENT_CHANNEL_CAP: usize = 4096;

/// How often the runner polls the monitor for staleness even without traffic.
pub const POLL_INTERVAL_MS: u64 = 200;

/// Something the runner produced: a decoded sample, or a health transition.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum InstrumentEvent {
    Sample(InstrumentSample),
    Stream(StreamEvent),
}

/// The MAVLink route the instrument runner subscribes to.
///
/// Only the ids a current [`InstrumentSource`] can decode are routed:
/// `HIGHRES_IMU`/`RAW_IMU` for the magnetometer and `GPS_RAW_INT` for the RTK
/// GNSS, plus `GLOBAL_POSITION_INT` for a future position fallback.
pub fn route() -> MessageRoute {
    MessageRoute::messages(&[
        HIGHRES_IMU_ID,
        RAW_IMU_ID,
        GPS_RAW_INT_DATA::ID,
        GLOBAL_POSITION_INT_DATA::ID,
    ])
}

/// Subscribe to the instrument route on a live connection.
pub fn subscribe(handle: &ConnectionHandle) -> RoutedEvents {
    handle.subscribe_route(route())
}

/// Run until the stream closes or `stop` fires, forwarding decoded samples and
/// stream-health events on `tx`.
pub async fn run_instruments(
    mut events: RoutedEvents,
    sources: Vec<Box<dyn InstrumentSource>>,
    monitor: Arc<Mutex<StreamMonitor>>,
    tx: mpsc::Sender<InstrumentEvent>,
    mut stop: oneshot::Receiver<()>,
) {
    let epoch = Instant::now();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(POLL_INTERVAL_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = tick.tick() => {
                let now_ms = host_ms(epoch);
                let health = monitor.lock().await.poll(now_ms);
                for event in health {
                    if tx.try_send(InstrumentEvent::Stream(event)).is_err() {
                        break;
                    }
                }
            }
            ev = events.recv() => match ev {
                Ok(ConnectionEvent::Message(env)) => {
                    let now_ms = host_ms(epoch);
                    let mut mon = monitor.lock().await;
                    for source in &sources {
                        // Not this source's frame, or a malformed one to skip.
                        if let Ok(Some(sample)) = source.decode(&env) {
                            let _ = tx.try_send(InstrumentEvent::Sample(sample));
                            for event in mon.observe(&sample, now_ms) {
                                let _ = tx.try_send(InstrumentEvent::Stream(event));
                            }
                        }
                    }
                }
                Ok(_) => continue,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
        }
    }
}

/// Host time since the runner started, in milliseconds.
fn host_ms(epoch: Instant) -> f64 {
    epoch.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruments::monitor::MonitorConfig;
    use crate::instruments::sim::{SimConfig, Simulator};
    use crate::instruments::source::default_sources;
    use mavlink::common::{GpsFixType, MavMessage, GPS_RAW_INT_DATA, HIGHRES_IMU_DATA};
    use std::time::Duration;

    struct Harness {
        tx: tokio::sync::broadcast::Sender<ConnectionEvent>,
        events_rx: mpsc::Receiver<InstrumentEvent>,
        monitor: Arc<Mutex<StreamMonitor>>,
        stop: oneshot::Sender<()>,
        task: tokio::task::JoinHandle<()>,
    }

    fn start(config: MonitorConfig) -> Harness {
        let (tx, rx) = tokio::sync::broadcast::channel(4096);
        let events = RoutedEvents::new(rx, route());
        let monitor = Arc::new(Mutex::new(StreamMonitor::new(config)));
        let (sample_tx, events_rx) = mpsc::channel(INSTRUMENT_CHANNEL_CAP);
        let (stop, stop_rx) = oneshot::channel();
        let task = tokio::spawn(run_instruments(
            events,
            default_sources(),
            monitor.clone(),
            sample_tx,
            stop_rx,
        ));
        Harness {
            tx,
            events_rx,
            monitor,
            stop,
            task,
        }
    }

    async fn drain(rx: &mut mpsc::Receiver<InstrumentEvent>) -> Vec<InstrumentEvent> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    #[tokio::test]
    async fn runner_decodes_a_simulated_stream() {
        let mut h = start(MonitorConfig::default());
        let mut sim = Simulator::new(SimConfig::default());
        for _ in 0..10 {
            for env in sim.step_envelopes(20.0) {
                h.tx.send(ConnectionEvent::Message(Box::new(env))).unwrap();
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let events = drain(&mut h.events_rx).await;

        let mags = events
            .iter()
            .filter(|e| matches!(e, InstrumentEvent::Sample(InstrumentSample::Mag(_))))
            .count();
        let gnss = events
            .iter()
            .filter(|e| matches!(e, InstrumentEvent::Sample(InstrumentSample::Gnss(_))))
            .count();
        assert!(mags > 0, "mag samples decoded");
        assert!(gnss > 0, "gnss samples decoded");
        assert_eq!(
            h.monitor.lock().await.health(),
            crate::instruments::StreamHealth::Ok,
            "healthy simulated stream"
        );
        let _ = h.stop.send(());
        let _ = h.task.await;
    }

    #[tokio::test]
    async fn runner_reports_a_stale_magnetometer() {
        let mut h = start(MonitorConfig {
            mag_stale_after_ms: 200.0,
            ..MonitorConfig::default()
        });
        h.tx.send(ConnectionEvent::Message(Box::new(envelope(
            MavMessage::HIGHRES_IMU(HIGHRES_IMU_DATA {
                xmag: 0.3,
                ymag: 0.4,
                zmag: 0.0,
                ..Default::default()
            }),
        ))))
        .unwrap();
        // Keep the link open but send no more mag data; the poll must notice.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let events = drain(&mut h.events_rx).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, InstrumentEvent::Stream(StreamEvent::MagStale { .. }))),
            "expected MagStale, got {events:?}"
        );
        let _ = h.stop.send(());
        let _ = h.task.await;
    }

    #[tokio::test]
    async fn runner_ignores_frames_it_cannot_decode() {
        let mut h = start(MonitorConfig::default());
        // A GPS frame with no fix and no satellites: decodes, but is a valid
        // sample (the monitor, not the runner, judges quality).
        h.tx.send(ConnectionEvent::Message(Box::new(envelope(
            MavMessage::GPS_RAW_INT(GPS_RAW_INT_DATA {
                fix_type: GpsFixType::GPS_FIX_TYPE_NO_GPS,
                ..Default::default()
            }),
        ))))
        .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let events = drain(&mut h.events_rx).await;
        assert_eq!(events.len(), 1, "one decoded GNSS sample, got {events:?}");
        let _ = h.stop.send(());
        let _ = h.task.await;
    }

    #[tokio::test]
    async fn stop_cancels_the_runner() {
        let mut h = start(MonitorConfig::default());
        h.stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), h.task)
            .await
            .expect("runner cancelled on stop")
            .unwrap();
        assert!(h.events_rx.recv().await.is_none(), "channel closed");
    }

    fn envelope(message: MavMessage) -> crate::mavlink::MessageEnvelope {
        crate::mavlink::MessageEnvelope {
            header: crate::mavlink::MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            message,
            received_at: Instant::now(),
        }
    }
}
