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
| [Architecture-first delivery](docs/plans/architecture-first-delivery-plan.md) | AF execution order and engineering gates |
| [AF-00 ownership inventory](docs/architecture/af-00-current-system.md) | Current service owners, lifecycle paths, API directory and extraction constraints |
| [Runtime contracts](docs/architecture/runtime-contracts.md) | Minimum target/operation/subscription vocabulary and AF-01 acceptance fixtures |
| [AF-00 measured baseline](docs/verification/af-00-baseline-20261025.md) | Native startup/tool resources, browser cold/warm loading and reproducible idle profile |
| [AF-01 facade checkpoint](docs/verification/af-01-facade-20261025.md) | Explicit target read APIs, shared ownership, generated types and native/software verification |
| [AF-02 operation/client groundwork](docs/verification/af-02b-client-foundation-20261025.md) | Owner-bound records, native client lifecycle, current checks and remaining adapter integration |
| [AF-02b.2 / .3 Parameters acceptance](docs/verification/af-02b-parameters-20261025.md) | Owned parameter operations, cross-client/native-close regression, screenshots and reproduction steps |
| [AF-02b.4a Command/reboot acceptance](docs/verification/af-02b-command-reboot-20261025.md) | Owned command transactions, ACK quarantine, safe reboot admission and target retirement |
| [AF-02b.4b Flight-command acceptance](docs/verification/af-02b-flight-commands-20261026.md) | Typed owned flight commands, emergency/preflight isolation, 46 screenshots and reproducible build evidence |
| [AF-02b.4 Mission acceptance](docs/verification/af-02b-mission-20261026.md) | Owned mission transfers/current control, snapshot/read-back isolation, exact cancellation and reproducible evidence |
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
