# Q-Mag

Open-source ground control station running side-by-side with QGroundControl,
for survey / geophysics drone operations (multirotor first).

- Flight-controller setup, sensor calibration, airframe and radio
  configuration stay in QGC.
- Three differentiating pillars: survey-grade planning, realtime data & QC,
  open integration (QGC `.plan` compatibility, headless mode).

## Documentation

| Document | Content |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System architecture: crates, processes/windows, MAVLink data flow, height conventions, ADR index |
| [docs/FEATURES.md](docs/FEATURES.md) | Feature guide: what each view/window does and how to use it |
| [docs/RELEASES.md](docs/RELEASES.md) | Release checks, installer formats, download warnings, and Windows tool-window verification |
| [issues.md](issues.md) | Open issues, deferred work and items awaiting bench verification |

## Build & run

```bash
# frontend (dist/ must exist before cargo build — see crates/app-tauri/build.rs)
npm --prefix frontend install
npm --prefix frontend run build

# desktop app
cargo build --release -p maggcs-app
./target/release/maggcs-app
```

Development: `npm --prefix frontend run dev` (Vite on :5173/5199) plus
`cargo run -p maggcs-app`; the desktop shell serves the built `dist/` in
release mode. PX4 SITL: `scripts/sitl/setup_px4_sitl.sh sim` (or
`scripts/sitl/run_sitl_docker.sh` for headless integration tests).

### Windows tool-window verification

After rebuilding and installing the Windows package, verify without a vehicle
connected:

1. Open Motor test from the main window. Its controls should render within
   5 seconds, and the main window should remain responsive.
2. Close Motor test using the native X or Alt-F4. The window should close
   within 2 seconds. Reopen and close it three times.
3. Repeat for Signal Inspector and Parameters. Opening an already-open tool
   should focus that window rather than create a duplicate.
4. Close the main window and confirm that the application exits.

Tool windows use asynchronous Tauri commands to avoid Windows/WebView2's
synchronous window-creation deadlock; see [the window architecture notes](docs/ARCHITECTURE.md#3-windows-and-data-flow).

## AI collaboration rules

See [AGENTS.md](AGENTS.md).
