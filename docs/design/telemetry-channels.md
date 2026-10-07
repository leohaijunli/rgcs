# Telemetry channels: low-rate state vs. high-rate series

Status: design frozen (issues.md #28, before Phase 4).

## Problem

The desktop shell pushes one throttled `TelemetrySnapshot` JSON document on the
`telemetry` Tauri event at 20 Hz (`DEFAULT_PUSH_HZ`). That is fine for the six
low-rate state messages it carries today (heartbeat, position, attitude,
battery, GPS, link), but it does not scale to the Phase 4 magnetic data layer:

- JSON-encoding the full snapshot for every sample wastes CPU and IPC budget.
- A single `watch` value collapses bursts; a 100 Hz+ series cannot ride it.
- `uPlot` needs an append-only buffer, not a whole-document replace.

## Decision

Split telemetry into two surfaces with clearly different rates and encodings:

1. **State channel — keep as is.** `telemetry` stays the JSON
   `TelemetrySnapshot` at 20 Hz. It is the source of truth for the HUD, the
   map marker, and discrete status. Producers of high-rate data must **not**
   fold samples into it.

2. **Series channel — new, for high-rate samples (mag, and any future raw
   waveform).** A Tauri `Channel<T>` (not a global event) carries batches of
   numeric samples as binary frames rather than JSON:
   - One channel per series, created per subscription so it can be closed when
     the view unmounts.
   - Each batch carries a small header (series id, first timestamp, sample
     rate, channel masks, sample count) followed by little-endian `f32`/`i32`
     payloads the frontend reads via `ArrayBuffer`/`Float32Array`.
   - Batching amortizes IPC: target one message per ~50–100 ms, not per sample.

3. **Ring buffer on the frontend.** The consumer writes batches into a
   fixed-size ring buffer sized to the QC window (e.g. 100k points), and uPlot
   draws directly from it. Capacity and window length are config, not magic
   numbers.

4. **Backpressure = drop-oldest with accounting.** If the consumer (or IPC)
   lags, the oldest samples are overwritten and a monotonically increasing
   `dropped_samples` counter is surfaced next to the curve — the same
   never-drop-silently rule as the link error channel (issues.md #17).

## Why not reuse the `telemetry` event

- Different cardinality (one batch vs. one snapshot), different encoding
  (binary vs. JSON), and different lifetime (per-view subscription vs. global).
- Mixing them would force every snapshot consumer to pay for the series and
  would re-introduce the lossy single-slot semantics.

## Consequences

- `core` gains a series aggregation type (ring buffer + batching) that the
  headless server can reuse; the Tauri layer only adapts it onto a `Channel`.
- The `telemetry` event contract is unchanged, so existing HUD/map code is
  untouched.
- Implemented in Phase 4; this document is the frozen interface.
