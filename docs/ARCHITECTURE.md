# MagGCS — System Architecture

2026-10-10 · Companion reading: ADR index at the bottom; feature-level
documentation in [FEATURES.md](FEATURES.md); open work in
[issues.md](../issues.md).

## 1. Stack

- **Rust** (workspace): core library + Tauri 2 desktop shell + (planned)
  headless server. Production code uses `Result`, never `unwrap`.
- **Frontend**: React + TypeScript strict, Zustand, Tailwind + Radix,
  uPlot for high-frequency curves. Colors/spacing/typography are design
  tokens (CSS variables) — no hardcoded colors (lint-enforced).
- **Types across the boundary**: ts-rs generates `frontend/src/generated-types/`
  from core structs; a CI check fails when the committed bindings drift
  (`cargo test -p maggcs-core` regenerates).
- **Cesium** for the 3D/2D map (only the main window loads it).

## 2. Crates and processes

```
crates/core          maggcs_core — all domain logic (ADR-001):
  mavlink/             connection worker + router (multi-link, primary
                       pointer), heartbeat monitor, link state model
  telemetry/           TelemetryHub: latest-snapshot + broadcast, per-field
                       staleness
  mission/             MISSION protocol state machine + service task
  commands/            single-slot COMMAND_LONG service (retransmit, ack)
  signals/             inspector tap: field extraction, mag_total synthesis,
                       ESC aggregation (ESC_STATUS index×4 → esc{k})
  dsp/                 biquad / moving average / detrend / FFT + registry
  inspector/           per-trace DSP session (filters + FFT run in Rust)
  motor_test/          motor-test session state machine, interlocks,
                       10 Hz ACTUATOR_TEST sender, mag alarm watchdog
  calib/               JSON calibration application (mag: A·(raw−b)+K·I)
  devices/             serial enumeration + hotplug watcher
  plan/ survey/ height/ rtk/ ulog/   plan compile, survey patterns,
                       height datums, RTCM, ULog reader/replay
crates/app-tauri     desktop shell: Tauri commands + thin service adapters
                     (mission/command/inspector/actuator), telemetry pump,
                     windows: main / inspector / actuator
crates/server        headless REST/WebSocket (planned)
frontend/            React app, three Vite entries (index / inspector /
                     actuator — separate webviews, no Cesium in the latter two)
tools/dem-prep       DEM preprocessing (planned)
helpers/udev-installer  pkexec-invoked udev rule installer (ADR-009)
```

Principle: **logic in core, adapters in app-tauri** — every service is a
tokio task driven by a command channel + routed inbound frames + a ticker,
so headless mode reuses them unchanged.

## 3. Windows and data flow

| Window | Content | Data path |
| --- | --- | --- |
| main | map + plan/fly task panel | Tauri events (`telemetry`, `link`, `mission`, `command`) + invokes |
| inspector | Signal Inspector (ADR-015/016) | one Tauri Channel, columnar frames (~30 Hz), DSP in Rust |
| actuator | motor / mag-interference test | invokes (commands) + `actuator_event`/`actuator_telemetry`/`actuator_alarm` events |

MAVLink in: UDP/TCP/serial endpoints (ADR-011 four-level link model).
MAVLink out: mission protocol, command service, and the motor-test sender
(direct `ConnectionHandle::send`, fire-and-forget + ack counting — high-rate
traffic bypasses the single-slot command service).

## 4. Height conventions (ADR-006/013)

- `H` CGVD2013 orthometric (BC LiDAR) · `h` WGS84 ellipsoid (Cesium) ·
  AMSL (`PX4 MAV_FRAME_GLOBAL`).
- `H + N(CGG2013) = h`; `h − N(EGM96) = AMSL`.
- Never represent an absolute height as a bare `f64` in core — use the
  datum-tagged `height::Height`. Conversions live in `core::height` only.
- AGL is relative to DTM by default (DEM pending, WS-D).

## 5. PX4 interface facts (verified against PX4 v1.17 source)

- `MAV_CMD_ACTUATOR_TEST`: param1 = value, param2 = timeout ≤ 3 s (0 = release
  control), param5 = 1-based motor function. Denied while armed or with
  `COM_MOT_TEST_EN != 1`.
- `MAV_CMD_DO_PAUSE_CONTINUE`: **no handler** in v1.17 (NACK) — the UI uses
  NAV_TAKEOFF / NAV_LAND / NAV_RETURN_TO_LAUNCH instead.
- `ESC_STATUS` (291): `{index, rpm[4], voltage[4], current[4]}` — index×4
  maps to `esc{k}`.
- `SERVO_OUTPUT_RAW` (36) main outputs 1–4 feed the realtime SITL→FC
  forward source (normalized PWM 1000–2000 µs → 0..1).
- Mission read-back hash (`itemsHash`) narrows `z`/`params` and
  MAV_FRAME_MISSION `x`/`y` to f32 — PX4 parks command-item coordinates in
  f32 storage, so exact comparison false-fails otherwise.

## 6. Architecture decision records

| ADR | Decision |
| --- | --- |
| 001 | Logic in core, thin app adapters |
| 002 | Frontend types via ts-rs codegen |
| 003 | QGC coexistence (independent sys/comp ids) |
| 004 | Terrain sources |
| 005 | Terrain following (GCS-side, not PX4 terrain frames) |
| 006 | Height datums |
| 007 | License (Apache-2.0) |
| 008 | RTCM forwarding |
| 009 | udev privilege strategy (polkit + strict helper) |
| 010 | Frontend stack + design tokens |
| 011 | Link state model (transport vs FC heartbeat) |
| 012 | Desktop webview CSP |
| 013 | Plan height policy (AMSL storage, compile-time frame) |
| 014 | Map tools |
| 015 | DSP in core |
| 016 | Inspector window + channel |
| 017 | Flight mode switching and arming |
