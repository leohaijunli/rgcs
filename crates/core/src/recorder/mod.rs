//! Flight recorder (plan §3 A4): a lossless MAVLink tlog plus a JSONL event log
//! and a `session.json` manifest, written by a dedicated thread that never
//! blocks the telemetry hub.
//!
//! ```text
//! connection worker ─ subscribe_raw() ─▶ pump_raw ─┐
//! app events (alerts…)               record_event ─┤── bounded queue ──▶ writer thread ──▶ <session>/
//! forced-arm audit ────────────────────────────────────┘   (drop-oldest)      tlog + JSONL + JSON
//! ```
//!
//! The tlog stores each frame's exact wire bytes (see [`tlog`]), so a replay is
//! byte-identical to what the flight controller sent. The queue is bounded and
//! drop-oldest: a stalled disk costs recorded frames (counted in
//! [`RecorderStats::dropped`]) but never the live link. Stopping flushes and
//! `fsync`s both files, and both readers ignore a truncated tail, so a crash
//! leaves a file that still parses to the last flush.

mod drape_bias;
mod drape_log;
mod drape_report;
mod events;
mod service;
mod session;
pub mod tlog;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, watch};

use crate::mavlink::connection::RawFrame;

pub use drape_bias::{estimate_bias, DemBiasEstimate, DemBiasOutcome, MIN_SAMPLES};
pub use drape_log::{read_drape_log, DrapeLine, DrapeMeta, DrapeSampleRecord, DRAPE_FILE};
pub use drape_report::{
    build_report, to_csv, to_markdown, DrapeFlightLineSummary, DrapeFlightReport,
};
pub use events::{read_events, LoggedEvent, RecordEvent};
pub use service::{Recorder, RecorderConfig, RecorderStats, DEFAULT_QUEUE_CAP};
pub use session::{read_manifest, ForceArmAudit, SessionManifest, SESSION_MANIFEST_VERSION};

/// Directory (under the app data dir) that holds one sub-directory per session.
pub const RECORDINGS_DIR: &str = "recordings";
/// Session directory layout.
pub const TLOG_FILE: &str = "raw/session.tlog";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const SESSION_FILE: &str = "session.json";

/// Microseconds since the Unix epoch (`0` before the epoch).
pub fn now_utc_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// Everything that can stop the recorder from writing.
#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("could not create recordings directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("could not start the recorder thread: {0}")]
    Thread(std::io::Error),
    #[error("the recorder writer thread panicked")]
    WriterLost,
    #[error("the writer thread did not confirm the forced-arm audit in time")]
    AuditUnconfirmed,
    #[error("could not persist the forced-arm audit: {0}")]
    Audit(String),
}

/// Pump the connection's raw-frame tap into `recorder` until `stop` flips to
/// `true` or the link is torn down.
///
/// A subscriber that falls behind (the writer stalled) loses frames — counted
/// by [`Recorder::note_lagged`] — instead of ever slowing the hub.
pub async fn pump_raw(
    recorder: Arc<Recorder>,
    mut frames: broadcast::Receiver<RawFrame>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            return;
        }
        tokio::select! {
            _ = stop.changed() => {
                if *stop.borrow() {
                    return;
                }
            }
            res = frames.recv() => match res {
                Ok(frame) => recorder.record_frame(frame),
                Err(broadcast::error::RecvError::Lagged(n)) => recorder.note_lagged(n),
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}
