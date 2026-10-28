# Signal Inspector

The selected source (live MAVLink or ULog replay) is distinct from primary FC
availability. `inspector_status.connected` reads the primary hub's current
heartbeat/transport status. A connection handle retained for reconnect must
not keep the header labelled **Live link** after FC loss.

After loss, the header shows **FC disconnected** and zero live message rate.
It restores **Live link** when the target heartbeat returns. A ULog replay
remains labelled **Replay** independently of FC availability.

## Verification — 2026-10-23

```bash
# Repository root
cargo test -p maggcs-app inspector_service --lib

# frontend/, with Vite and Playwright installed
PLAYWRIGHT_MODULE=/path/to/playwright npm run check:inspector-connection -- \
  http://127.0.0.1:1420/inspector.html screenshots/inspector/connection
```

The backend and real-React/isolated-IPC checks pass live → loss with the tap
and handle retained → restoration. The UI check also covers replay while
disconnected. The operator confirmed Inspector operation after testing.

- [Disconnected header](../../screenshots/inspector/connection/inspector-disconnected.png)
- [Recovered header](../../screenshots/inspector/connection/inspector-reconnected.png)

Native review: connect the FC, open Inspector, unplug USB and confirm that
data stops and the header changes. Reconnect and confirm recovery. Open a ULog
to verify the independent replay source label.

## Magnetometer source isolation — 2026-10-28

Magnetometer axes and totals are partitioned by the complete source identity
`(system_id, component_id, message_id)`. `SCALED_IMU`, `SCALED_IMU2`, and
`SCALED_IMU3` cannot complete one another's x/y/z triples or share a derived
calibrated-total ring.

ULog `sensor_mag` instances retain separate synthetic message IDs and appear as
`sensor_mag · instance 1`, `instance 2`, and so on. Each group exposes its own
nT `xmag`, `ymag`, `zmag`, and `mag_total`; plot legends and trace chips keep
the instance label.

Automated review:

```bash
cargo test -p maggcs-core inspector::session::mag_cal_tests --lib
cargo test -p maggcs-core ulog::replay --lib
cd frontend
npm run check:inspector-mag
node scripts/check-inspector.mjs \
  http://127.0.0.1:5199/inspector.html /tmp/opencode/n9-inspector
```

The browser fixture mounts four complete MAVLink triples in real React/uPlot
and asserts four separately labelled totals. [Four-source capture](../../screenshots/n9/inspector-mag-sources-1366x768.png).

Manual ULog review: open a recording with at least three `sensor_mag`
instances, expand each one, add each `mag_total`, and verify that changing or
hiding instance 3 does not change instances 1/2. Confirm each instance name is
still visible in the tree, legend, and trace chip.

## ULog history default — 2026-10-28

Opening a `.ulg` selects **Full log** and shows selected traces across
`0 … log duration`, including signals selected after opening. The native
`inspector_ulog_history` command scans an independent reader through a fresh
core DSP session, then returns bounded time-bucket extrema. The UI requests up
to 16,384 points per trace; the native per-trace/global budgets bound memory
for dense and long recordings. First/last samples and raw/filtered peaks are
retained across the whole time range.

Static history has its own buffers. Playback's rolling ring and the live
settling gate do not trim it. Drag-zoom, Shift-drag pan, fit and measurement
remain available; seeking moves the replay playhead while preserving the full
history and zoom. Trace edits preserve a zoomed x range. **Follow** restores the
full range. The time-window selector offers 10/30/60 s rolling views, and
**Back to live** restores the 30 s live window.

A new recording clears old curves while its history loads. Late replies from
old trace/source requests are ignored; read/configuration failures use the
Inspector error notice. CSV exports the retained view samples, including the
bounded overview samples in Full log mode.

### Full-history verification

With Vite running:

```bash
npm run check:inspector-history -- http://localhost:1420/inspector.html screenshots/inspector/history
npm run check:inspector-connection -- http://localhost:1420/inspector.html
npm run check:replay-seek
npm run check:inspector-mag
```

Set `PLAYWRIGHT_MODULE` if Playwright is installed outside the frontend.
The native fixture scans a 180 s / 180,001-record ULog into at most 256 plot
points while retaining both ends and early/late peaks. Browser checks cover
full-range defaults, added signals, zoom/seek/trace edits, explicit rolling
views, late replies, read errors, short/single-point logs and 800×480 layout.

See [screenshots and manual steps](../../../docs/verification/inspector-full-history-20261028.md).
