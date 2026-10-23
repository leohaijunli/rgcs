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
