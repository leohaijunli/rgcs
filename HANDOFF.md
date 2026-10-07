# MagGCS — Session Handoff

Checkpoint for the **P0 milestone** (issues from `issues.md`).

## Status summary

- **#1–#4** (link-layer P0): **done and pushed** (`f3147fd`). Heartbeat
  watchdog on a fixed 500 ms tick, `Connected` event consumed by the hub, 1 Hz
  `MAV_TYPE_GCS` heartbeat, `connect` waits for the first bind attempt.
- **#5** (flight commands, P0): **done** — core layer pushed (`d5ec93f`),
  app-tauri adapter + frontend shipped in the commit that added
  `crates/app-tauri/src/command_service.rs`. See `issues.md` #5 for status.
- **#32** (UAV marker, P3): **done** — real glTF model, MAVLink-driven
  attitude, ground-clamped position, forward-only prediction. See below.
- #6–#31: not started.

Per-issue status lives in `issues.md` (status line under each heading plus the
P0 progress table at the top) — keep both in sync when a task lands.

## Issue #5 — flight commands are fire-and-forget

### Done (core)

New module `crates/core/src/commands/`:

- `mod.rs` — `CommandSession` state machine (mirrors `mission/protocol.rs`):
  - `begin(command, params)` sends `COMMAND_LONG` (confirmation 0) and awaits
    the `COMMAND_ACK`.
  - `handle()` accepts only acks **from the target FC** (`sys/comp` match) and
    only for the **pending command**; `MAV_RESULT_IN_PROGRESS` refreshes the
    deadline and keeps waiting.
  - `retransmit_due(now)` resends with `confirmation` incremented (1 s timeout,
    max 3 retries); `take_timeout_failure()` → `CommandError::NoAck`.
  - `cancel()` → `CommandError::LinkLost` (used on heartbeat loss / link fail).
  - `CommandResult::from_mav(MavResult)` maps all result codes.
  - Single-slot: a second `begin` fails with `CommandError::Busy`.
  - `self_sys/self_comp` deliberately dropped — `COMMAND_ACK` has no target
    field, so filtering is by sender only.
- `service.rs` — `CommandService`: spawned task owning one `CommandSession`,
  subscribes to `ConnectionHandle` events, ticks retransmit every 200 ms,
  emits `CommandEvent` on an `mpsc` stream. Cancels the session on
  `HeartbeatLost`/`ConnectionEvent::Failed`. Lives in `core` so the headless
  server can reuse it.
- `lib.rs` — registers and re-exports `commands`.

### Verification (done, all green)

- Unit tests in `mod.rs` (14): accepted, denied, IN_PROGRESS, wrong command /
  wrong sender ignored, confirmation increments, budget exhaustion, cancel.
- `tests/command_service_integration.rs` (4, real UDP, synthetic FC peer):
  `ack_accepted_completes`, `denied_reports_result`,
  `lost_ack_retransmits_with_confirmation`, `never_acked_fails_with_no_ack`.
  Run: `cargo test -p maggcs-core` — 78 unit + 4 integration pass.
  `cargo clippy -p maggcs-core --all-targets` clean.
  The harness retries on UDP port-reuse races; the FC heartbeat timeout is
  set to 30 s in these tests so silent FCs don't cancel sessions early.

### Done (app + frontend)

- `crates/app-tauri/src/command_service.rs` (new): maps core `CommandEvent` →
  `CommandEventPayload` (`{ command, kind: sent|completed|failed, result?,
  message? }`) and forwards `"command"` events to the webview. 4 unit tests.
- `state.rs` holds the `CommandService` (`set_command` / `take_command`);
  `commands.rs::connect` spawns it and `disconnect` drops it.
- `commands.rs::send_command` accepts only `"rtl"` (pause/resume removed — PX4
  v1.17 `DO_PAUSE_CONTINUE` unverified, see issues.md "待核实"), enqueues via
  `CommandService`, returns immediately.
- `frontend/src/stores/command.ts` (new) + `desktop/bridge.ts` listens for
  `"command"` and resets state when `link.fc_alive` goes false.
- `FlightCommands.tsx`: RTL two-click confirm (5 s window) and a status line
  for sent/completed/failed. Pause/resume removed; `i18n/en.ts` updated.

### Still to do for #5

- Manual acceptance: simulate lost ACK / lost command / denied and confirm the
  UI shows three distinct results.

## Other remaining work

- P0: #6 (telemetry source filtering), #7 (mission retransmit timer wiring),
  #8 (upload duplicate-request re-send), #9 (download target filtering).
- P1: #10–#17. P2: #18–#26. P3: #27–#31. See `issues.md`.

## Issue #32 — UAV marker

The map marker is a real drone model whose attitude comes straight from the
MAVLink `ATTITUDE` message.

- **Asset**: "animated drone with camera (FREE)" by ulunkwulunk, CC-BY-4.0.
  Source kept in `model/` (see `model/README.md` for attribution);
  `frontend/scripts/build-uav-model.mjs` (`npm run build:model`) emits the
  runtime copy into `frontend/public/model/`.
- **Why a build step**: Cesium builds draw commands for the asset's skinned
  primitives but never rasterizes them, so the raw export is invisible. The
  script strips skins/animations plus `JOINTS_0`/`WEIGHTS_0` and the unused UV
  sets/tangents, then repacks the attributes the materials use into a tight
  buffer — 8.4 MB → 2.8 MB, and it renders.
- **Attitude**: `uavOrientation()` composes ENU→ECEF with the body quaternion
  built from `attitude.yaw_deg/pitch_deg/roll_deg`. The shipped asset's nose is
  its local +Z, so `UAV_MODEL_NOSE_YAW_OFFSET_DEG = -90` aligns the nose with
  the reported heading. Verified by injecting known snapshots and screenshotting:
  level at roll=pitch=0, nose bearing == HDG, +pitch = nose up, +roll = right
  wing down.

## Verification commands

- `cargo clippy --all-targets -- -D warnings`, `cargo test -p maggcs-app`
- `cd frontend && npm run typecheck && npm run check:colors && npm run check:contrast && npm run build`
