# Signal Inspector full-history default — 2026-10-28

## Behavior

Opening a `.ulg` selects **Full log** and plots selected signals across the
recording's entire time range, including signals added afterward. The history
load is independent of playback, so data at the end is available while the
playhead is still at the beginning.

An independent reader and core DSP session scan the whole log into bounded
time-bucket extrema. Static history buffers retain both ends and peaks instead
of being trimmed to the rolling 30 s window or overwritten by the 65,536-sample
live ring. Zoom/seek/trace edits preserve the plotted range; Follow restores
the full extent. Explicit rolling windows and Back to live remain available.

## Automated evidence

- Native: a valid 180 s ULog with 180,001 records produces at most 256 output
  points while retaining `t = 0`, `t = 180,000 ms`, an early +9,999 peak and a
  late −9,999 peak. Core filter-chain, short-log and replaced-request fixtures
  pass.
- Core: all 17 ULog reader/replay tests pass, including maximum-timestamp
  duration when a lagging topic is the final record.
- Browser: a saved 10 s window and paused/zoomed live view become Full log on
  open; plots cover exactly 0–180 s before playback advances. Added signals,
  early/late peaks, zoom/seek/trace edits, explicit 30 s rolling view, late source
  replies, read errors, 5 s/single-point logs and 800×480 layout are covered.
  The run completes with zero page errors.
- Production frontend build, generated-type/i18n checks, Rust format/Clippy and
  Inspector live-connection/layout regressions pass. The native Inspector suite
  passes all four tests.

Reproduce with Vite running:

```bash
cargo test -p maggcs-app --lib inspector
cargo test -p maggcs-core --lib ulog::
PLAYWRIGHT_MODULE=/path/to/playwright npm --prefix frontend run check:inspector-history -- http://localhost:1420/inspector.html screenshots/inspector/history
```

## Screenshots

- [Full 180 s history](../../frontend/screenshots/inspector/history/inspector-full-history-1366x768.png)
- [Zoomed history after seeking](../../frontend/screenshots/inspector/history/inspector-history-zoom.png)
- [Full 5 s log](../../frontend/screenshots/inspector/history/inspector-short-log.png)
- [Minimum 800×480 window](../../frontend/screenshots/inspector/history/inspector-full-history-800x480.png)

## Manual verification

1. Open Tools → Signal Inspector, select a signal and choose a rolling window.
   Pause/zoom the live view, then open a `.ulg` longer than 60 s.
2. Confirm **Full log** is selected, the x axis spans the whole duration, and
   early and late data appear without waiting for playback. Select another
   signal and confirm it receives the same whole-history coverage.
3. Drag-zoom and Shift-drag pan. Move the replay seek slider and add/remove a
   trace; confirm the viewed range remains stable. Use Follow to restore it.
4. Choose a 30 s rolling view, switch back to Full log, then Back to live.
   Confirm the source chip, plots and selector agree with the chosen source.
5. Repeat with a short log and at 800×480. Load a second recording and verify
   old curves clear while it loads and read failures appear as an error notice.

The browser evidence uses isolated IPC; packaged-app review uses a real ULog.
