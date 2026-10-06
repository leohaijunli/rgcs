# MagGCS — Session Handoff

Checkpoint for the **P0 milestone** (issues from `issues.md`).

## Status summary

- **#1–#4** (link-layer P0): fixed in the working tree (uncommitted until this
  push). Heartbeat watchdog on a fixed 500 ms tick, `Connected` event consumed
  by the hub, 1 Hz `MAV_TYPE_GCS` heartbeat, `connect` waits for the first
  bind attempt.
- **#5** (flight commands, P0): **core layer complete and verified**; app-tauri
  adapter and frontend **not started yet**.
- #6–#31: not started.

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

### Remaining for #5

1. **app-tauri** (`crates/app-tauri/`):
   - New adapter module (e.g. `command_service.rs`): map core `CommandEvent` →
     `CommandEventPayload` (`{ command, kind: sent|completed|failed, result?,
     message? }`), forward `"command"` events to the webview.
   - `commands.rs::send_command`: only `"rtl"` (pause/resume removed — PX4
     v1.17 `DO_PAUSE_CONTINUE` unverified, see issues.md "待核实"); enqueue via
     `CommandService`, return immediately; results arrive as events.
   - `state.rs`: hold the `CommandService`; `lib.rs`: register module + spawn in
     `connect`.
2. **frontend** (`frontend/src/`):
   - `stores/command.ts` (zustand) + `desktop/bridge.ts` listener for `"command"`.
   - `components/FlightCommands.tsx`: remove pause/resume, RTL gets two-click
     confirm (arm "Confirm RTL?" for 5 s), status line for sent/completed/failed.
   - `i18n/en.ts`: strings.
3. Manual acceptance (issue #5): simulate lost ACK / lost command / denied and
   confirm the UI shows three distinct results.

## Other remaining work

- P0: #6 (telemetry source filtering), #7 (mission retransmit timer wiring),
  #8 (upload duplicate-request re-send), #9 (download target filtering).
- P1: #10–#17. P2: #18–#26. P3: #27–#31. See `issues.md`.

## Uncommitted work in the tree

- #1–#4 link-layer fixes + Phase-1 example tweaks (`git diff` for details).
- `crates/core/src/commands/` + `tests/command_service_integration.rs` (#5 core).
- `issues.md` (the task list), `package-lock.json` (4 KB npm lockfile).