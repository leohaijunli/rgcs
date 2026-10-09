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
  at about 30 Hz batch frames (`Frame { seq, t[], signals: [{ idx, traces:
  [{ id, y[] }] }] }`); spectrum frames are pushed separately at ≤ 10 Hz. JSON
  first, binary only if load testing demands it.
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

## Consequences

- `crates/app-tauri` gains an `inspector_service` mirroring
  `mission_service`/`command_service`; `connect` attaches the tap, `disconnect`
  detaches it, and the tap runs only while the window is open (subscription
  refcount).
- `vite.config.ts` gains a second entry; `crates/app-tauri/tests/desktop_csp.rs`
  is updated for the new window label.
- Plot configuration persists across reconnects; filter state resets and the
  curve gets a break on a reconnect.