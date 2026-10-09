# ADR-016: Signal Inspector runs in its own OS window, fed by a Tauri Channel

- Status: Accepted
- Date: 2026-10-08
- Supersedes: none. Related: ADR-012 (desktop CSP), ADR-015 (DSP in `core`),
  ADR-003 (QGC coexistence).

## Decision

- The Signal Inspector is a second, real OS window (`inspector.html`), opened
  from the main UI. It does not load Cesium. `capabilities` gains the
  `"inspector"` window label; `inspector_open` opens-or-focuses a singleton
  window and is created from Rust so the frontend is not granted
  `core:webview:allow-create-webview-window`.
- Live samples stream from the tap to the window over a `tauri::ipc::Channel`
  at about 30 Hz batch frames. The wire type is
  `SampleFrame { seq: u64, samples: Vec<TraceSample> }` with
  `TraceSample { id: SignalId, t_ms: f64, raw: f64, filtered: f64 }` — one row
  per processed sample (the plan §7 columnar layout
  `{ trace_id, t[], raw[], filtered[] }` is the S2 target once a `trace_id`
  exists). JSON first, binary only if load testing demands it.
- Each webview is an independent JS context, so the inspector subscribes to the
  channel directly and keeps its own ring buffers (not the shared Zustand
  stores).
- uPlot renders the plots (already a dependency); the CSP already allows
  `worker-src blob:`.

## Rationale

- A real window lets the operator keep inspecting while flying; a Radix dialog
  cannot leave the main window. Creating it from Rust keeps the webview API
  out of the frontend's least-privilege surface (ADR-012).
- The telemetry hub's 20 Hz aggregated snapshot cannot feed an FFT (Nyquist
  10 Hz) and does not carry arbitrary fields, so the inspector subscribes to
  `ConnectionHandle::subscribe_route(MessageRoute::all())` directly.
- Timestamps: prefer the FC's `time_boot_ms`/`time_usec` where a message
  carries one, mapped onto the host axis by the minimum `rx − fc` offset (plan
  §4); messages without a time field fall back to the receive time and the UI
  flags the trace. Per-source mappers (time_boot_ms and time_usec
  are not the same clock) plus rollback detection on FC reboot are planned
  (optimization plan S1) but not yet implemented.

## Consequences

- `crates/app-tauri` gains an `inspector_service` mirroring
  `mission_service`/`command_service`; `inspector_connect` registers the
  window's channel and attaches the tap to the current link;
  `inspector_disconnect` drops the channel, the trace set, and the
  subscriptions. The tap task itself runs for as long as the link stream is
  open — the subscription refcount gates *sample forwarding*, not the task —
  so closing the window does not currently stop the tap until the link closes
  (optimization plan P0-5; a window-`Destroyed` cleanup that cancels the tap is
  pending).
- `connect` does not re-attach an already-open inspector on a reconnect
  (optimization plan P0-6; pending), so reconnecting the link leaves the
  inspector window open but without data until the user toggles it.
- On attach, the filter state is reset but the trace configuration is kept; a
  reconnect therefore restarts the filtered curves with a gap (plan §4).
- `vite.config.ts` gains a second entry; `crates/app-tauri/tests/desktop_csp.rs`
  is updated for the new window label.

## Implementation notes (2026-10-09)

- Rust `TraceConfig.analyzer` and `Session::poll_spectra()` exist but the
  frontend always sends `analyzer: null` and the app-tauri layer never polls
  spectra: the FFT view currently runs on the TS mirror (`frontend/src/
  inspector/dsp.ts`) over the raw ring buffer only (optimization plan P0-4).
- A single bad frame no longer tears the link down: decode failures
  (`InvalidEnum`/`InvalidFlag`/`UnknownMessage`, e.g. PX4 1.17's
  `CURRENT_EVENT_SEQUENCE.flags=0`) are skipped and counted in the connection
  worker (connection.rs `is_recoverable_read_error`).