# ADR-017: Flight mode switching and arming over MAVLink

- Status: Accepted
- Date: 2026-10-11
- Supersedes: none. Related: ADR-003 (QGC coexistence, component id),
  comprehensive plan W3 (`comprehensive-implementation-plan.md` §4.1).

## Decision

- Mode switches go through `MAV_CMD_DO_SET_MODE` on the existing single-slot
  `CommandService`; arming/disarming through `MAV_CMD_COMPONENT_ARM_DISARM`.
  Both are low-frequency, acked commands — `CommandService::send_await` is the
  primitive (the UI must see the FC's verdict, not an optimistic "sent").
- The UI never pre-validates exhaustively: PX4's own state machine decides,
  the ack result plus the heartbeat readback is the truth. No optimistic mode
  display — the `MODE` pill follows `HEARTBEAT.custom_mode` (readback wins
  over the ack when they disagree, plan §4.1 acceptance).
- Arm/disarm and the risky modes (Return, Land) sit behind the same
  two-click confirm pattern as Takeoff/Land today; switching is disabled
  while the motor test runs.
- The mode table lives in `core::telemetry::px4_mode` (ts-rs exported); the
  frontend's hand-verified `util/px4mode.ts` decode is replaced by generated
  bindings so there is exactly one implementation.
- Offered modes: Hold (Auto/Loiter), Mission (Auto/Mission), Return
  (Auto/RTL), Land (Auto/Land), Position, Altitude. Manual/Acro/Offboard are
  "advanced modes" behind Settings (operator decision 2026-10-11). Arming and
  disarming are first-class commands in the Fly panel.

## Verified PX4 v1.17 facts (source: `src/modules/commander/Commander.cpp`,
`px4_custom_mode.h` — checked against `/home/dev/PX4-Autopilot-v1.17`)

### `MAV_CMD_DO_SET_MODE` (Commander.cpp:800)

- `param1` = base-mode bits; `MAV_MODE_FLAG_CUSTOM_MODE_ENABLED` (bit 0,
  value 1) must be set for PX4 custom modes. `param2` = main mode,
  `param3` = sub mode (both per `px4_custom_mode.h`):
  - Main: Manual=1, Altctl=2, Posctl=3, Auto=4, Acro=5, Offboard=6,
    Stabilized=7, AltitudeCruise=11.
  - Auto sub: Ready=1, Takeoff=2, Loiter=3, Mission=4, RTL=5, Land=6,
    FollowTarget=8, PrecLand=9, External1..8=15..22. Sub 0 defaults to
    Mission. Posctl sub: Posctl=0 (default), Orbit=1, Slow=2.
- **Rejections come back as `MAV_RESULT_TEMPORARILY_REJECTED`, not
  `DENIED`** — both for state-machine denial (`_user_mode_intention.change`)
  and for an unsupported auto sub mode ("Unsupported auto mode" critical
  log). The UI must treat `temporarily_rejected` as the normal "no" and show
  the current mode unchanged.
- `NAVIGATION_STATE_AUTO_LAND` is force-allowed (`force = true`): Land stays
  reachable as an emergency mode even when other transitions are denied.
- SITL supports mode switching fully (unlike `PREFLIGHT_REBOOT_SHUTDOWN`,
  which SITL denies — ADR-018 territory), so the whole line is
  SITL-testable.

### `MAV_CMD_COMPONENT_ARM_DISARM` (Commander.cpp:948)

- `param1`: 1 = arm, 0 = disarm. `param2 = 21196` = forced (kill):
  - **Arming from an external GCS always runs the preflight checks**
    (`arm(reason, from_external || !forced)`) — the magic cannot bypass
    checks for us, by design. A denied arm is
    `TEMPORARILY_REJECTED` plus a statustext/event reason.
  - Disarm honors forced: `disarm(reason, forced)` — a forced disarm works
    in air. We do **not** expose the kill code in the UI by default;
    the emergency path is Land (force-allowed) and RTL.
- Denial acks `TEMPORARILY_REJECTED`; the arming refusal reasons arrive as
  events/statustext (PX4 `COM_ARM_*` checks) and should be surfaced inline.

### Readback

- `HEARTBEAT.base_mode & MAV_MODE_FLAG_SAFETY_ARMED` (bit 7, value 128) is
  the armed truth; `HEARTBEAT.custom_mode` packs main+sub mode
  (`px4_custom_mode.h` union). Mode confirmations wait up to ~3 s for the
  readback and then fall back to the ack verdict with a warning.

## Consequences

- `available_modes(state)` in `core::telemetry::px4_mode` decides which
  entries are greyed out and why (link, mission sync, armed, home) — the
  table is data, the reasons are operator-readable English strings.
- Integration tests drive a fake FC through ack/deny/timeout paths
  (mirroring `command_service_integration.rs`).
- `parameters-window-plan.md` PM9 note unchanged: motor-test interlock keeps
  its own disarm-only gate.
