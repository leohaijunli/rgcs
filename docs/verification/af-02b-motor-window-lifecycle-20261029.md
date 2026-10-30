# AF-02b Motor window-lifecycle ownership — 2026-10-29

## Scope

Native cleanup after the Motor window is destroyed used to run unscoped
`stop_all` / `revoke` / target teardown. A delayed `Destroyed` callback from a
closed window could therefore stop or revoke a replacement owner after the
window was reopened, and could tear down a target that the new window had just
attached.

This slice hardens that close path without changing any public IPC DTO:

- the Motor window's current native session is recorded when
  `runtime_client_attach` binds the `motor-test` module;
- `Destroyed` cleanup uses the captured `ClientId` for actuation, SD capture and
  log operations (`stop_all_for_client`, `revoke_client`);
- a per-window lifetime token rejects a delayed callback that belongs to a
  previous window instance;
- shared target/SITL/stream/telemetry teardown runs only when no
  `motor-test` module is attached any more;
- the lifecycle `MotorTest` stage is registered when capture or log work is
  active even if no actuation session exists.

## Evidence

| Check | Command | Result |
| --- | --- | --- |
| Old-window cleanup cannot finish or release a reopened Motor owner | `cargo test -p maggcs-app --lib late_cleanup_from_a_closed` | 2 passed |
| Reopened module attachment blocks shared teardown | `cargo test -p maggcs-app --lib runtime_clients::tests` | 6 passed |
| Native app library | `cargo test -p maggcs-app --lib` | 279 passed, 8 ignored |
| Workspace | `cargo test --workspace --all-targets` | 1529 passed, 12 ignored, 0 failed |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| Format | `cargo fmt --all -- --check` | clean |

The two focused regressions are:

- `motor_operations::tests::late_cleanup_from_a_closed_motor_window_cannot_stop_or_revoke_a_reopened_owner`
  starts output under a closed window's client, opens a replacement owner,
  replays the old client's cleanup, and asserts the replacement operation is
  still `Running`, its grant is intact, no extra release frame is sent, and the
  module stays attached.
- `capture_service::tests::late_cleanup_from_a_closed_window_cannot_stop_the_reopened_capture`
  does the same for SD capture: the replacement recording stays `Running` and
  no `logger off` is sent for its file (the fake PX4 now reports a distinct log
  path per logger start, matching a real restart).

## Residual

The final cross-adapter/client/fault matrix (AF-02b.4) is still pending, so G1
stays open. Physical bench qualification remains separate. This record closes
only the Motor window-lifecycle ownership defect.
