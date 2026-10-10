//! Signal Inspector service: attaches the `core::signals` tap to the live link,
//! batches samples into frames, and forwards them to the inspector window over a
//! Tauri channel (plan §4/§7, ADR-016). A thin adapter like
//! `mission_service`/`command_service`: the tap, extractor and catalog live in
//! `core`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use std::collections::HashMap;
use std::path::Path;

use maggcs_core::dsp::SpectrumFrame;
use maggcs_core::inspector::{Session, Trace};
use maggcs_core::mavlink::connection::ConnectionHandle;
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::signals::catalog::{CatalogEntry, SignalCatalog};
use maggcs_core::signals::tap::{run_tap, Subscriptions, TapStats, SAMPLE_CHANNEL_CAP};
use maggcs_core::signals::SignalSample;
use maggcs_core::ulog::replay::{run_replay, ReplayControl, UlogSource};
use parking_lot::Mutex as PMutex;
use serde::Serialize;
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::WindowEvent;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::{mpsc, oneshot};

use crate::state::AppState;

/// Batch window between channel frames (~30 Hz).
const FRAME_INTERVAL_MS: u64 = 33;

static FRAME_SEQ: AtomicU64 = AtomicU64::new(0);

/// One trace's samples within a frame, columnar so the `SignalId` is not
/// repeated on every sample (plan §S2: `{ trace_id, t[], raw[], filtered[] }`).
#[derive(Debug, Clone, Serialize)]
pub struct TraceFrame {
    pub trace_id: String,
    pub t: Vec<f64>,
    pub raw: Vec<f64>,
    pub filtered: Vec<f64>,
}

/// One batched frame pushed to the inspector window at ~30 Hz.
#[derive(Debug, Clone, Serialize)]
pub struct SampleFrame {
    pub seq: u64,
    pub traces: Vec<TraceFrame>,
    /// Any analyzer windows that completed since the last frame (S3: the FFT
    /// runs in Rust, decoupled from the frontend's view buffer).
    pub spectra: Vec<TraceSpectrum>,
}

/// One trace's spectrum frame.
#[derive(Debug, Clone, Serialize)]
pub struct TraceSpectrum {
    pub trace_id: String,
    pub frame: SpectrumFrame,
}

/// Shared inspector state held in [`AppState`] (not on the link, so the plot
/// configuration survives reconnects, ADR-016).
#[derive(Default)]
pub struct InspectorState {
    pub subs: Subscriptions,
    pub catalog: Arc<Mutex<SignalCatalog>>,
    pub stats: Arc<Mutex<TapStats>>,
    /// The trace set and its running DSP state (plan §5, ADR-015).
    pub session: Arc<PMutex<Session>>,
    /// The window's channel, registered by `inspector_connect`.
    frame: Arc<PMutex<Option<Channel<SampleFrame>>>>,
    /// The running tap's stop signal, one per attached link. Re-attaching
    /// (reconnect) or disconnecting (window close) fires it so the tap exits
    /// promptly instead of draining until the link closes (plan P0-5).
    tap: Arc<PMutex<Option<oneshot::Sender<()>>>>,
    /// The running ULog replay, if any. Opening a log stops the live tap so
    /// only one source feeds the session (plan decision B).
    replay: Arc<PMutex<Option<ReplayHandle>>>,
}

/// A running ULog replay: its identity plus the stop signal and playback
/// control shared with [`run_replay`].
struct ReplayHandle {
    name: String,
    duration_ms: f64,
    control: Arc<ReplayControl>,
    stop: oneshot::Sender<()>,
}

/// A snapshot of the tap and link for the inspector window's status bar.
#[derive(Debug, Clone, Serialize)]
pub struct InspectorStatus {
    pub messages: u64,
    pub samples: u64,
    pub dropped: u64,
    /// True when a primary link exists (the tap can stream).
    pub connected: bool,
    /// True while a tap task is attached to a link.
    pub tap_running: bool,
    /// True while a ULog replay (not the live tap) is the active source.
    pub replay_active: bool,
}

/// A ULog topic summary for the open dialog (plan decision B).
#[derive(Debug, Clone, Serialize)]
pub struct UlogTopicDto {
    pub name: String,
    pub signals: usize,
}

/// Result of opening a ULog: what replay will stream.
#[derive(Debug, Clone, Serialize)]
pub struct UlogInfo {
    pub name: String,
    pub duration_ms: f64,
    pub topics: Vec<UlogTopicDto>,
}

/// Playback state of the replay source, polled by the toolbar.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayStatus {
    /// True when a ULog is open (even if paused or finished).
    pub active: bool,
    pub name: Option<String>,
    pub duration_ms: f64,
    pub position_ms: f64,
    pub playing: bool,
    pub speed: f64,
}

impl AppState {
    pub fn inspector(&self) -> &InspectorState {
        &self.inspector
    }

    /// Attach the tap to a link and start the batcher. Stops a previous tap
    /// from an earlier link (it may still be draining a stream being torn
    /// down) and starts a fresh one for this link (plan P0-6).
    pub fn attach_inspector(&self, handle: ConnectionHandle) {
        let inspector = self.inspector();
        if let Some(old) = inspector.tap.lock().take() {
            let _ = old.send(());
        }
        // A live link supersedes a ULog replay (plan decision B): only one
        // source may feed the session.
        if let Some(old) = inspector.replay.lock().take() {
            let _ = old.stop.send(());
        }

        // A (re)connected link restarts the filter state but keeps the trace
        // configuration (plan §4: "滤波状态重置并在曲线留断点").
        inspector.session.lock().reset();

        let events = handle.subscribe_route(MessageRoute::all());
        let subs = inspector.subs.clone();
        let catalog = inspector.catalog.clone();
        let stats = inspector.stats.clone();
        let session = inspector.session.clone();
        let frame = inspector.frame.clone();
        let (tx, rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);
        let (stop_tx, stop_rx) = oneshot::channel();
        *inspector.tap.lock() = Some(stop_tx);

        tauri::async_runtime::spawn(run_tap(events, subs, catalog, tx, stats, stop_rx));
        tauri::async_runtime::spawn(batch_loop(rx, session, frame));
    }

    /// Register the window's channel and start producing samples. Idempotent:
    /// a window may (re)register without double-acquiring the subscription
    /// refcount. A missing link is fine — the tap attaches when a link comes
    /// up (via [`attach_inspector`](Self::attach_inspector)).
    pub fn inspector_connect(&self, handle: Option<ConnectionHandle>, channel: Channel<SampleFrame>) {
        let inspector = self.inspector();
        let first = inspector.frame.lock().is_none();
        *inspector.frame.lock() = Some(channel);
        if first {
            inspector.subs.acquire();
        }
        if let Some(handle) = handle {
            self.attach_inspector(handle);
        }
    }

    /// The window closed: stop producing samples, cancel the tap, and drop the
    /// trace set. Idempotent (frontend cleanup and the window `Destroyed`
    /// handler may both call it).
    pub fn inspector_disconnect(&self) {
        let inspector = self.inspector();
        if inspector.frame.lock().is_none() {
            return;
        }
        if let Some(stop) = inspector.tap.lock().take() {
            let _ = stop.send(());
        }
        if let Some(replay) = inspector.replay.lock().take() {
            let _ = replay.stop.send(());
        }
        inspector.subs.release();
        inspector.subs.clear();
        inspector.session.lock().clear();
        *inspector.frame.lock() = None;
    }

    /// Whether a tap is attached and a link is present (for the status bar).
    pub fn inspector_status(&self) -> InspectorStatus {
        let inspector = self.inspector();
        let stats = inspector.stats.lock().expect("stats lock");
        InspectorStatus {
            messages: stats.messages,
            samples: stats.samples,
            dropped: stats.dropped,
            connected: self.connection().is_some(),
            tap_running: inspector.tap.lock().is_some(),
            replay_active: inspector.replay.lock().is_some(),
        }
    }

    /// Current catalog for the signal tree.
    pub fn inspector_catalog(&self) -> Vec<CatalogEntry> {
        self.inspector()
            .catalog
            .lock()
            .expect("catalog lock")
            .snapshot(std::time::Instant::now())
    }

    /// Open a ULog and start replaying it (plan decision B). Stops the live tap
    /// and any previous replay so exactly one source feeds the session, then
    /// spawns `run_replay` into the same sample channel `batch_loop` reads.
    pub fn start_ulog_replay(&self, path: &str) -> Result<UlogInfo, String> {
        let source = UlogSource::open(Path::new(path)).map_err(|e| e.to_string())?;
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        let duration_ms = source.duration_ms();
        let seeds = source.signal_entries();
        let topics = source
            .topics()
            .into_iter()
            .map(|t| UlogTopicDto {
                name: t.name,
                signals: t.signals,
            })
            .collect();

        let inspector = self.inspector();
        if let Some(old) = inspector.replay.lock().take() {
            let _ = old.stop.send(());
        }
        if let Some(old) = inspector.tap.lock().take() {
            let _ = old.send(());
        }
        inspector.session.lock().reset();
        // Refresh the signal tree for this recording: drop the live catalog and
        // pre-register every topic field so the list updates immediately, before
        // the first record streams.
        {
            let mut catalog = inspector.catalog.lock().expect("catalog lock");
            catalog.clear();
            let now = std::time::Instant::now();
            for (id, name) in &seeds {
                catalog.observe(id, name, f64::NAN, now);
            }
        }

        let subs = inspector.subs.clone();
        let catalog = inspector.catalog.clone();
        let stats = inspector.stats.clone();
        let session = inspector.session.clone();
        let frame = inspector.frame.clone();
        let (tx, rx) = mpsc::channel(SAMPLE_CHANNEL_CAP);
        let (stop_tx, stop_rx) = oneshot::channel();
        let control = Arc::new(ReplayControl::default());
        *inspector.replay.lock() = Some(ReplayHandle {
            name: name.clone(),
            duration_ms,
            control: control.clone(),
            stop: stop_tx,
        });
        tauri::async_runtime::spawn(run_replay(source, subs, catalog, tx, stats, control, stop_rx));
        tauri::async_runtime::spawn(batch_loop(rx, session, frame));

        Ok(UlogInfo {
            name,
            duration_ms,
            topics,
        })
    }

    /// Update the replay control (play/pause, speed, seek) and report the
    /// resulting state. With no log open this is a no-op.
    pub fn inspector_replay(
        &self,
        play: Option<bool>,
        speed: Option<f64>,
        seek_ms: Option<f64>,
    ) -> ReplayStatus {
        let inspector = self.inspector();
        let guard = inspector.replay.lock();
        if let Some(handle) = guard.as_ref() {
            if let Some(play) = play {
                handle.control.set_playing(play);
            }
            if let Some(speed) = speed {
                handle.control.set_speed(speed);
            }
            if let Some(seek_ms) = seek_ms {
                handle.control.request_seek(seek_ms);
            }
        }
        replay_status(guard.as_ref())
    }

    /// Stop replaying and hand the session back to the live tap (if a link is
    /// present). Idempotent.
    pub fn inspector_use_live(&self) {
        let inspector = self.inspector();
        if let Some(replay) = inspector.replay.lock().take() {
            let _ = replay.stop.send(());
            inspector.session.lock().reset();
            // Drop the recording's topics so the live link repopulates the tree.
            inspector.catalog.lock().expect("catalog lock").clear();
        }
        if let Some(handle) = self.connection() {
            self.attach_inspector(handle);
        }
    }

    /// The active source label: `live` or `ulog:<file>`.
    pub fn inspector_source(&self) -> String {
        match self.inspector().replay.lock().as_ref() {
            Some(handle) => format!("ulog:{}", handle.name),
            None => "live".to_string(),
        }
    }

    /// The replay toolbar state (position, speed, playing).
    pub fn inspector_replay_status(&self) -> ReplayStatus {
        replay_status(self.inspector().replay.lock().as_ref())
    }
}

/// Snapshot the replay handle for the toolbar (or an inert state).
fn replay_status(handle: Option<&ReplayHandle>) -> ReplayStatus {
    match handle {
        Some(handle) => ReplayStatus {
            active: true,
            name: Some(handle.name.clone()),
            duration_ms: handle.duration_ms,
            position_ms: handle.control.position_ms(),
            playing: handle.control.is_playing(),
            speed: handle.control.speed(),
        },
        None => ReplayStatus {
            active: false,
            name: None,
            duration_ms: 0.0,
            position_ms: 0.0,
            playing: false,
            speed: 1.0,
        },
    }
}

/// Collect samples for ~33 ms, route them through the session's pipelines, and
/// push one [`SampleFrame`].
async fn batch_loop(
    mut rx: mpsc::Receiver<SignalSample>,
    session: Arc<PMutex<Session>>,
    frame: Arc<PMutex<Option<Channel<SampleFrame>>>>,
) {
    loop {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(FRAME_INTERVAL_MS);
        // Columnar accumulation, grouped by trace id, preserving first-seen
        // order so the frontend renders traces stably.
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut traces: Vec<TraceFrame> = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(sample)) => {
                    for s in session.lock().ingest(&sample) {
                        let i = match index.get(&s.trace_id) {
                            Some(&i) => i,
                            None => {
                                let i = traces.len();
                                index.insert(s.trace_id.clone(), i);
                                traces.push(TraceFrame {
                                    trace_id: s.trace_id.clone(),
                                    t: Vec::new(),
                                    raw: Vec::new(),
                                    filtered: Vec::new(),
                                });
                                i
                            }
                        };
                        let f = &mut traces[i];
                        f.t.push(s.t_ms);
                        f.raw.push(s.raw);
                        f.filtered.push(s.filtered);
                    }
                }
                Ok(None) => return, // tap ended: stop (don't spin on a closed channel)
                Err(_) => break,   // window elapsed: flush
            }
        }
        if traces.is_empty() {
            continue;
        }
        let spectra: Vec<TraceSpectrum> = session
            .lock()
            .poll_spectra()
            .into_iter()
            .map(|(trace_id, frame)| TraceSpectrum { trace_id, frame })
            .collect();
        let Some(channel) = frame.lock().clone() else {
            continue;
        };
        let frame = SampleFrame {
            seq: FRAME_SEQ.fetch_add(1, Ordering::Relaxed),
            traces,
            spectra,
        };
        let _ = channel.send(frame);
    }
}

/// Open the inspector window, or focus it if it already exists.
#[tauri::command]
pub fn inspector_open(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("inspector") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, "inspector", WebviewUrl::App("inspector.html".into()))
        .title("Signal Inspector")
        .inner_size(1280.0, 800.0)
        .min_inner_size(800.0, 480.0)
        .build()
        .map_err(|e| e.to_string())?;
    // Belt-and-suspenders on top of the React effect cleanup: closing the
    // window with the X cancels the tap even if the webview's cleanup never
    // runs (plan P0-5). `inspector_disconnect` is idempotent.
    let app_for_cleanup = app.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            app_for_cleanup.state::<AppState>().inspector_disconnect();
        }
    });
    Ok(())
}

/// Close the inspector window and detach the tap.
#[tauri::command]
pub fn inspector_close(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.inspector_disconnect();
    if let Some(win) = app.get_webview_window("inspector") {
        let _ = win.close();
    }
    Ok(())
}

/// Register the window's sample channel and start the tap. A missing link is
/// not an error: the tap attaches when a link comes up (plan P0-6).
#[tauri::command]
pub fn inspector_connect(
    state: State<'_, AppState>,
    channel: Channel<SampleFrame>,
) -> Result<(), String> {
    state.inspector_connect(state.connection(), channel);
    Ok(())
}

/// Snapshot of the tap stats and link state, for the window's status bar.
#[tauri::command]
pub fn inspector_status(state: State<'_, AppState>) -> Result<InspectorStatus, String> {
    Ok(state.inspector_status())
}

/// Stop streaming into this window (the window itself stays open).
#[tauri::command]
pub fn inspector_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    state.inspector_disconnect();
    Ok(())
}

/// The trace set: binds signals to their filter pipelines and updates the tap's
/// subscription set to the union of the traces' signals.
#[tauri::command]
pub fn inspector_set_traces(
    state: State<'_, AppState>,
    traces: Vec<Trace>,
) -> Result<(), String> {
    let inspector = state.inspector();
    let ids = inspector
        .session
        .lock()
        .set_traces(traces)
        .map_err(|e| e.to_string())?;
    inspector.subs.set(ids);
    Ok(())
}

/// The current catalog, for the signal tree.
#[tauri::command]
pub fn inspector_catalog(state: State<'_, AppState>) -> Result<Vec<CatalogEntry>, String> {
    Ok(state.inspector_catalog())
}

/// Every algorithm the frontend can attach to a trace, with its parameter form.
#[tauri::command]
pub fn inspector_list_algorithms() -> Result<Vec<maggcs_core::dsp::AlgorithmInfo>, String> {
    Ok(maggcs_core::dsp::list_algorithms())
}

/// Open a ULog file and start replaying it. Stops the live tap so the replay is
/// the active source (plan decision B).
#[tauri::command]
pub fn inspector_open_ulog(state: State<'_, AppState>, path: String) -> Result<UlogInfo, String> {
    state.start_ulog_replay(&path)
}

/// Control the ULog replay (play/pause, speed, seek) and return its state.
#[tauri::command]
pub fn inspector_replay(
    state: State<'_, AppState>,
    play: Option<bool>,
    speed: Option<f64>,
    seek_ms: Option<f64>,
) -> Result<ReplayStatus, String> {
    Ok(state.inspector_replay(play, speed, seek_ms))
}

/// Poll the replay state (position, speed, playing) for the toolbar.
#[tauri::command]
pub fn inspector_replay_status(state: State<'_, AppState>) -> Result<ReplayStatus, String> {
    Ok(state.inspector_replay_status())
}

/// Stop replaying and hand the session back to the live tap.
#[tauri::command]
pub fn inspector_use_live(state: State<'_, AppState>) -> Result<(), String> {
    state.inspector_use_live();
    Ok(())
}

/// The active source label: `live` or `ulog:<file>`.
#[tauri::command]
pub fn inspector_source(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.inspector_source())
}
