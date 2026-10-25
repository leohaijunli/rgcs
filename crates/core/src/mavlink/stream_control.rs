//! Validated stream requests and bounded, target-agnostic arrival statistics.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::streams::{resolve_standard_stream, Px4StreamSupport, StreamLookupError};

const MICROS_PER_SECOND: f64 = 1_000_000.0;
const MIN_STALE: Duration = Duration::from_secs(2);
const MIN_WINDOW: Duration = Duration::from_secs(5);
const MAX_SAMPLES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "hz", rename_all = "snake_case")]
pub enum StreamRate {
    Hertz(f64),
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamRequest {
    pub message_id: u32,
    pub rate: StreamRate,
    pub allow_unknown_support: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamReceiveState {
    Waiting,
    Receiving,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamStatus {
    pub message_id: u32,
    pub requested_rate: StreamRate,
    pub observed_hz: Option<f64>,
    pub last_seen_age_ms: Option<u64>,
    pub receive_state: StreamReceiveState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum StreamRequestError {
    #[error(transparent)]
    Lookup(#[from] StreamLookupError),
    #[error("rate must be finite and positive with a representable MAVLink microsecond interval")]
    InvalidRate,
    #[error("PX4 support is unknown; explicitly confirm the selected standard message")]
    UnknownSupportNotConfirmed,
    #[error("waiting for the selected PX4 connection and target traffic")]
    NotConnected,
    #[error("stream target or transport generation changed")]
    TargetChanged,
    #[error("stream request failed: {message}")]
    Transport { message: String },
}

/// Round to a microsecond interval representable by COMMAND_LONG's f32 param2.
/// The upper i32 boundary itself rounds out of range as f32 and is refused.
pub fn stream_interval_us(rate: StreamRate) -> Result<i32, StreamRequestError> {
    let StreamRate::Hertz(hz) = rate else {
        return Ok(-1);
    };
    if !hz.is_finite() || hz <= 0.0 {
        return Err(StreamRequestError::InvalidRate);
    }
    let raw = MICROS_PER_SECOND / hz;
    if !(1.0..=f64::from(i32::MAX)).contains(&raw) {
        return Err(StreamRequestError::InvalidRate);
    }
    let wire = raw.round() as f32;
    if !(1.0..=f64::from(i32::MAX)).contains(&f64::from(wire)) {
        return Err(StreamRequestError::InvalidRate);
    }
    Ok(wire as i32)
}

pub fn validate_request(request: &StreamRequest) -> Result<i32, StreamRequestError> {
    let message = resolve_standard_stream(&request.message_id.to_string())?;
    let interval = stream_interval_us(request.rate)?;
    if message.px4_support == Px4StreamSupport::Unknown && !request.allow_unknown_support {
        return Err(StreamRequestError::UnknownSupportNotConfirmed);
    }
    Ok(interval)
}

struct Entry {
    request: StreamRequest,
    since: Instant,
    samples: VecDeque<Instant>,
    error: Option<String>,
    period: Duration,
}

impl Entry {
    fn snapshot(&self, now: Instant) -> StreamStatus {
        let stale_after = MIN_STALE.max(self.period.saturating_mul(3));
        let last = self.samples.back().copied();
        let age = last.map(|at| now.saturating_duration_since(at));
        let state = if now.saturating_duration_since(last.unwrap_or(self.since)) > stale_after {
            StreamReceiveState::Stale
        } else if last.is_some() {
            StreamReceiveState::Receiving
        } else {
            StreamReceiveState::Waiting
        };
        let window = MIN_WINDOW.max(self.period.saturating_mul(4));
        let samples: Vec<_> = self
            .samples
            .iter()
            .filter(|at| now.saturating_duration_since(**at) <= window)
            .collect();
        let observed = samples
            .first()
            .zip(samples.last())
            .and_then(|(first, last)| {
                let seconds = last.saturating_duration_since(**first).as_secs_f64();
                (samples.len() >= 2 && seconds > 0.0 && state == StreamReceiveState::Receiving)
                    .then(|| (samples.len() - 1) as f64 / seconds)
            });
        StreamStatus {
            message_id: self.request.message_id,
            requested_rate: self.request.rate,
            observed_hz: observed,
            last_seen_age_ms: age.map(|age| age.as_millis().min(u128::from(u64::MAX)) as u64),
            receive_state: state,
            error: self.error.clone(),
        }
    }
}

#[derive(Default)]
pub struct StreamTracker {
    entries: BTreeMap<u32, Entry>,
}

impl StreamTracker {
    pub fn set(&mut self, request: StreamRequest, now: Instant) -> Result<i32, StreamRequestError> {
        let interval = validate_request(&request)?;
        let period = if interval > 0 {
            Duration::from_micros(interval as u64)
        } else {
            Duration::ZERO
        };
        self.entries.insert(
            request.message_id,
            Entry {
                request,
                since: now,
                samples: VecDeque::new(),
                error: None,
                period,
            },
        );
        Ok(interval)
    }

    pub fn requests(&self) -> Vec<StreamRequest> {
        self.entries
            .values()
            .map(|entry| entry.request.clone())
            .collect()
    }

    pub fn reset_observations(&mut self, now: Instant) {
        for entry in self.entries.values_mut() {
            entry.samples.clear();
            entry.since = now;
        }
    }

    pub fn observe(&mut self, message_id: u32, at: Instant) {
        if let Some(entry) = self.entries.get_mut(&message_id) {
            if entry.samples.back().is_some_and(|last| at <= *last) || at < entry.since {
                return;
            }
            if entry.samples.back().is_some_and(|last| {
                at.duration_since(*last) > MIN_STALE.max(entry.period.saturating_mul(3))
            }) {
                entry.samples.clear();
            }
            if entry.samples.len() == MAX_SAMPLES {
                entry.samples.pop_front();
            }
            entry.samples.push_back(at);
        }
    }

    pub fn note_result(&mut self, message_id: u32, result: &Result<(), StreamRequestError>) {
        if let Some(entry) = self.entries.get_mut(&message_id) {
            entry.error = result.as_ref().err().map(ToString::to_string);
        }
    }

    pub fn status(&self, message_id: u32, now: Instant) -> Option<StreamStatus> {
        self.entries
            .get(&message_id)
            .map(|entry| entry.snapshot(now))
    }

    pub fn snapshot(&self, now: Instant) -> Vec<StreamStatus> {
        self.entries
            .values()
            .map(|entry| entry.snapshot(now))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(hz: f64) -> StreamRequest {
        StreamRequest {
            message_id: 147,
            rate: StreamRate::Hertz(hz),
            allow_unknown_support: false,
        }
    }

    #[test]
    fn rate_validation_respects_the_actual_wire_representation() {
        assert_eq!(stream_interval_us(StreamRate::Hertz(50.0)), Ok(20_000));
        assert_eq!(stream_interval_us(StreamRate::Hertz(0.1)), Ok(10_000_000));
        assert_eq!(stream_interval_us(StreamRate::Disabled), Ok(-1));
        for hz in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            0.0,
            1_000_001.0,
            1e-20,
            1_000_000.0 / f64::from(i32::MAX),
        ] {
            assert_eq!(
                stream_interval_us(StreamRate::Hertz(hz)),
                Err(StreamRequestError::InvalidRate)
            );
        }
        assert!(matches!(
            validate_request(&StreamRequest {
                message_id: 999_999,
                ..request(10.0)
            }),
            Err(StreamRequestError::Lookup(_))
        ));
        assert_eq!(
            validate_request(&StreamRequest {
                message_id: 43,
                ..request(10.0)
            }),
            Err(StreamRequestError::UnknownSupportNotConfirmed)
        );
        assert!(validate_request(&StreamRequest {
            message_id: 43,
            allow_unknown_support: true,
            ..request(10.0)
        })
        .is_ok());
    }

    #[test]
    fn measured_rate_needs_frames_and_recovers_from_stale_without_bridging_gaps() {
        let now = Instant::now();
        let mut tracker = StreamTracker::default();
        tracker.set(request(10.0), now).expect("request");
        assert_eq!(tracker.snapshot(now)[0].observed_hz, None);
        tracker.observe(147, now);
        assert_eq!(tracker.snapshot(now)[0].observed_hz, None);
        for i in 1..=10 {
            tracker.observe(147, now + Duration::from_millis(i * 100));
        }
        assert_eq!(
            tracker.snapshot(now + Duration::from_secs(1))[0].observed_hz,
            Some(10.0)
        );
        let stale = tracker.snapshot(now + Duration::from_secs(4)).remove(0);
        assert_eq!(stale.receive_state, StreamReceiveState::Stale);
        assert_eq!(stale.observed_hz, None);
        tracker.observe(147, now + Duration::from_secs(4));
        assert_eq!(
            tracker.snapshot(now + Duration::from_secs(4))[0].observed_hz,
            None
        );
        tracker.observe(147, now + Duration::from_millis(4100));
        assert_eq!(
            tracker.snapshot(now + Duration::from_millis(4100))[0].observed_hz,
            Some(10.0)
        );
        tracker.reset_observations(now + Duration::from_secs(5));
        assert_eq!(
            tracker.snapshot(now + Duration::from_secs(5))[0].last_seen_age_ms,
            None
        );
        tracker.observe(147, now); // old-generation timestamp
        assert_eq!(
            tracker.snapshot(now + Duration::from_secs(5))[0].observed_hz,
            None
        );
    }

    #[test]
    fn slow_streams_and_disabled_but_still_arriving_data_are_distinct() {
        let now = Instant::now();
        let mut tracker = StreamTracker::default();
        tracker.set(request(0.1), now).expect("slow request");
        tracker.observe(147, now);
        tracker.observe(147, now + Duration::from_secs(10));
        assert_eq!(
            tracker.snapshot(now + Duration::from_secs(10))[0].observed_hz,
            Some(0.1)
        );
        assert_eq!(
            tracker.snapshot(now + Duration::from_secs(20))[0].receive_state,
            StreamReceiveState::Receiving
        );
        tracker
            .set(
                StreamRequest {
                    rate: StreamRate::Disabled,
                    ..request(1.0)
                },
                now + Duration::from_secs(20),
            )
            .expect("disable");
        assert_eq!(
            tracker.snapshot(now + Duration::from_secs(20))[0].observed_hz,
            None
        );
        tracker.observe(147, now + Duration::from_secs(21));
        tracker.observe(147, now + Duration::from_secs(22));
        let live = tracker.snapshot(now + Duration::from_secs(22)).remove(0);
        assert_eq!(live.requested_rate, StreamRate::Disabled);
        assert_eq!(
            live.observed_hz,
            Some(1.0),
            "disable is not fabricated proof of silence"
        );
        tracker
            .set(request(5.0), now + Duration::from_secs(23))
            .expect("new rate");
        assert_eq!(
            tracker.snapshot(now + Duration::from_secs(23)).len(),
            1,
            "one row per ID"
        );
    }
}
