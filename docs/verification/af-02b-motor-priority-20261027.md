# AF-02b.4 Motor priority control-path checkpoint

- Date: 2026-10-27.
- Source base: `aa69c19` plus priority/reliable-state changes.
- Result: **pass for the control-path prerequisite**. Native permission/operation
  ownership and capture/log migration remain active; G1 stays open.

## Implemented

- Ordinary commands use a bounded eight-slot channel; STOP/Emergency use a
  separate two-slot priority channel. A blocked control write can be interrupted
  by STOP, and the existing full-Release deadline still includes queuing/writes.
- A non-wrapping admission epoch invalidates buffered and blocked old producers.
  A pre-STOP Start waiting outside a full queue cannot wake later and restart.
  Ordinary telemetry/tick selection remains fair under update pressure.
- State/Stopped events use bounded nonblocking publication and count drops.
  `MotorTestService::session_state()` retains state independently of event loss;
  native reads retain its startup reservation until the first transition.
- An event receiver closing initiates Release before service retirement.
  Failed release still tries the remaining motors, stays retryable and cannot
  produce a false transport receipt or idle/zero-output observation.

## Measured acceptance

Three new controlled-transport tests use the real session/service loop, replacing
only the transport's confirmation boundary:

1. Fill the 32-slot observer queue across repeated starts/stops without consuming
   it; observe a counted gap, complete all four Release receipts, retain Idle.
2. Block a control write, queue eight Starts and hold a ninth producer outside
   the queue; priority STOP completes the four Release writes, and none of those
   old intents sends output or restarts after the blocked writer is retired.
3. Close the observer; every Release is confirmed before service termination.

| Check | Result |
| --- | --- |
| `cargo test -p maggcs-core --lib motor_test --quiet` | 139 pass |
| `cargo test -p maggcs-app --lib --quiet` | 260 pass, 8 opt-in ignores |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo fmt --all -- --check` | pass |
| generated frontend mirrors / TypeScript | 310 mirrors match / pass |

This is software transport/control evidence. It does not establish zero PWM,
stationary rotors, acoustic tone completion or exclusion of external QGC/RC.
