//! Signal Inspector service: attaches the `core::signals` tap to the live link,
//! batches samples into frames, and forwards them to the inspector window over a
//! Tauri channel (plan §4/§7, ADR-016). A thin adapter like
//! `mission_service`/`command_service`: the tap, extractor and catalog live in
//! `core`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use maggcs_core::inspector::{Session, TraceConfig, TraceSample};
use maggcs_core::mavlink::connection::ConnectionHandle;
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::signals::catalog::{CatalogEntry, SignalCatalog};
use maggcs_core::signals::tap::{run_tap, Subscriptions, TapStats, SAMPLE_CHANNEL_CAP};
use maggcs_core::signals::SignalSample;
use parking_lot::Mutex as PMutex;
use serde::Serialize;
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::mpsc;

use crate::state::AppState;

/// Batch window between channel frames (~30 Hz).
const FRAME_INTERVAL_MS: u64 = 33;

static FRAME_SEQ: AtomicU64 = AtomicU64::new(0);

/// One batched frame pushed to the inspector window at ~30 Hz.
#[derive(Debug, Clone, Serialize)]
pub struct SampleFrame {
    pub seq: u64,
    pub samples: Vec<TraceSample>,
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
    /// Set while a tap+batcher pair is running for the current link.
    tap_running: Arc<PMutex<bool>>,
}

impl AppState {
    pub fn inspector(&self) -> &InspectorState {
        &self.inspector
    }

    /// Attach the tap to a link and start the batcher (idempotent per link).
    pub fn attach_inspector(&self, handle: ConnectionHandle) {
        let inspector = self.inspector();
        if *inspector.tap_running.lock() {
            return;
        }
        *inspector.tap_running.lock() = true;

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

        let running = inspector.tap_running.clone();
        tauri::async_runtime::spawn(async move {
            run_tap(events, subs, catalog, tx, stats).await;
            // Link gone: allow a re-attach on the next connect.
            *running.lock() = false;
        });
        tauri::async_runtime::spawn(batch_loop(rx, session, frame));
    }

    /// Register the window's channel and start producing samples.
    pub fn inspector_connect(&self, handle: ConnectionHandle, channel: Channel<SampleFrame>) {
        let inspector = self.inspector();
        *inspector.frame.lock() = Some(channel);
        inspector.subs.acquire();
        self.attach_inspector(handle);
    }

    /// The window closed: stop producing samples and drop the trace set.
    pub fn inspector_disconnect(&self) {
        let inspector = self.inspector();
        inspector.subs.release();
        inspector.subs.clear();
        inspector.session.lock().clear();
        *inspector.frame.lock() = None;
    }

    /// Current catalog for the signal tree.
    pub fn inspector_catalog(&self) -> Vec<CatalogEntry> {
        self.inspector()
            .catalog
            .lock()
            .expect("catalog lock")
            .snapshot(std::time::Instant::now())
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
        let mut batch: Vec<TraceSample> = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(sample)) => batch.extend(session.lock().ingest(&sample)),
                Ok(None) => return, // tap ended: stop (don't spin on a closed channel)
                Err(_) => break,   // window elapsed: flush
            }
        }
        if batch.is_empty() {
            continue;
        }
        let Some(channel) = frame.lock().clone() else {
            continue;
        };
        let frame = SampleFrame {
            seq: FRAME_SEQ.fetch_add(1, Ordering::Relaxed),
            samples: batch,
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
    WebviewWindowBuilder::new(&app, "inspector", WebviewUrl::App("inspector.html".into()))
        .title("Signal Inspector")
        .inner_size(1280.0, 800.0)
        .min_inner_size(800.0, 480.0)
        .build()
        .map_err(|e| e.to_string())?;
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

/// Register the window's sample channel and start the tap.
#[tauri::command]
pub async fn inspector_connect(
    state: State<'_, AppState>,
    channel: Channel<SampleFrame>,
) -> Result<(), String> {
    let handle = state.connection().ok_or("no link")?;
    state.inspector_connect(handle, channel);
    Ok(())
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
    traces: Vec<TraceConfig>,
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
