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
release mode. PX4 SITL: `scripts/sitl/run_sitl_docker.sh`.

## AI collaboration rules

See [AGENTS.md](AGENTS.md).
