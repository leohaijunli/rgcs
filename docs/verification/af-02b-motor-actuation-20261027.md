# AF-02b.4 owned Motor actuation checkpoint

- Date: 2026-10-27.
- Source base: `e8d6151` plus this native actuation slice.
- Result: **pass within software actuation scope**. Dedicated capture/log ownership,
  final Motor delivery evidence and G1 cross-adapter qualification remain open.
- Contract: [owned Motor](../architecture/af-02b4-motor-ownership.md).

## Delivered

- Permission is native-window/Runtime-session/target/revision bound and defaults
  OFF. Foreign ON/OFF and stale grants fail. Permission-only native close becomes
  inactive; active owner close directly cancels the core service.
- Manual/preset/forward START captures permission, target generation, output
  configuration and exact forward-source identity. It owns `ActuatorOutput` and
  returns `MotorStartReply` only after the first complete output bank has a bounded
  transport receipt. Prepared owner loss writes zero output.
- Manual updates require the exact running operation, request ID and monotonically
  increasing revision. Count remains tied to the captured mapping. STOP/E-STOP
  use the exact session/operation and the priority Release path. Retained repeated/
  old STOP sends zero Release and cannot affect a later operation.
- Core observes owner cancellation and operation deadline directly. It retains
  first-write, Release and stop-reason evidence independently of observer pressure.
  A generation replacement cannot redirect old Release; failed original-generation
  cleanup yields `Uncertain` and keeps its claim/diagnostic.
- Native close and lifecycle first cancel/wait owned operations. Tool observers,
  SITL/dedicated target and scope tasks are cleaned only after confirmed Release;
  uncertainty preserves the link for retry. Borrowed primary transport/cache is
  never shut down by Motor cleanup.
- Tone settings require exact permission/target/config revision before affecting
  the next owned start. Mapping refresh and parameter read carry exact Motor
  context; parameter writes additionally require permission and use the existing
  owned Parameter operation/worker. The unused global `COM_MOT_TEST_EN` write
  command was removed. Preflight no longer has a global no-owner STOP action.

## Automated evidence

| Check | Result |
| --- | --- |
| Core Motor filter | 146 pass, including owner-loss/deadline Release fixtures |
| Native app library | 263 pass, 8 opt-in ignored |
| Native owned Motor PTY | 3 pass: A/B/old OFF-start-update-STOP, prepared/native-close, target replacement uncertainty |
| Full workspace all targets | pass, including all protocol integrations |
| Generated contracts | 315 complete Rust-derived frontend mirrors match |
| Strict/static | workspace clippy `-D warnings`, rustfmt, TypeScript and production Vite build pass |
| Browser Motor regression | 12 scenario groups pass against the real React UI and isolated native boundary |
| Screenshots | 24 retained under `frontend/screenshots/motor-test-owned/` |

The PTY fixture uses production serial transport, Runtime facade, coordinator,
parameter/cache, Motor adapter/service and main-link borrowing; only PX4 is
simulated. Browser checks include exact session/permission/config START, exact
operation STOP, pending tone STOP/OFF/Escape, responsive 1024/1366/1920 layouts,
field mode, mixed output units, source/configuration and observer pressure UI.

## Reproduction

```bash
env PATH="$HOME/.cargo/bin:$PATH" cargo test -p maggcs-core --lib motor_test
env PATH="$HOME/.cargo/bin:$PATH" cargo test -p maggcs-app --lib motor_operations
env PATH="$HOME/.cargo/bin:$PATH" cargo test --workspace --all-targets
env PATH="$HOME/.cargo/bin:$PATH" cargo clippy --workspace --all-targets -- -D warnings
env PATH="$HOME/.cargo/bin:$PATH" cargo fmt --all -- --check
npm --prefix frontend run build
npm --prefix frontend run check:generated-types
npm --prefix frontend run check:motor-scope
node frontend/scripts/check-motor-controls.mjs http://127.0.0.1:5199/actuator.html frontend/screenshots/motor-test-owned
```

## Manual verification

1. Connect a dedicated target and a borrowed-main target separately. Enabling
   control must not write output. Start manual, adjust sliders and STOP; confirm
   the operation/session identities remain constant and the target link survives.
2. Begin START tone, then use header STOP, sidebar STOP, Escape and OFF. No late
   tone/config reply may start output. Native-close must Release before dedicated
   link cleanup; borrowed cleanup must leave main telemetry/parameters usable.
3. Replace/reconnect the target during a run. No Release/control reaches the new
   generation; show reconciliation instead of idle success.
4. Verify normal, 1024×640, 1366×768, 1920×1080, field and light screenshots.
   STOP/E-STOP remain reachable; mapping/protocol/FC range and predicted-versus-
   measured output stay explicit.

Transport receipts do not prove zero PWM, stationary rotors, acoustic tone
completion or external QGC/RC exclusion. Those remain bench evidence after the
capture/log software slice.
