# Motor Test ownership migration

The existing motor session/service remains the sole output effect path. It uses
the original target generation, source-filtered safety observations, conditioned
output values and transport-confirmed Release. Release returns to configured
disarmed outputs; it is not proof of zero PWM or mechanically stopped motors.

`operation_types` is the first AF-02b.4 Motor slice: Rust-derived request/permission/
operation/revision/source/result/error contracts and input/scope validation.
Manual demands are bounded to the existing 16-function session, finite and in
`[0, 1]`; periodic presets respect the existing ticker's Nyquist frequency. Opaque
and full-width generation/revision identities stay strings on the wire.

The data validator is not native authorization. Native adapters must verify the
actual window, issued session/permission, exact active operation, configuration
and forward-source revision before admitting writes. That adapter migration is
the next slice; the legacy IPC entry points have not yet adopted these DTOs.

```bash
cargo test -p maggcs-core --lib motor_test
npm --prefix frontend run check:generated-types
```

The contract/data checkpoint passes 136 filtered core checks, including three
meaningful input/scope/serde fixtures, and 310 complete generated frontend mirrors.
See `docs/architecture/af-02b4-motor-ownership.md` for signatures, serial-write/
Release risks and the remaining native/core/browser/capture/log acceptance.

## Priority control-path checkpoint

STOP/Emergency use a separate bounded priority channel, interrupt an in-flight
control write and retain the existing 2 s full-Release receipt. A non-wrapping
command epoch rejects both buffered and blocked pre-STOP producers; clearing a
queue cannot re-admit a delayed old Start. Telemetry/tick handling remains fair
under continuous ordinary updates.

State/Stopped notifications publish nonblocking, with `dropped_events()` accounting.
`session_state() -> Option<SessionState>` supplies the retained transition independently
of event delivery; native state uses it after the initial startup reservation.
Closing the event receiver triggers owned-bank Release before service retirement.

Three controlled-transport fault tests cover full observer queues, blocked output
plus eight queued/one waiting Start, and observer close. The complete Motor filter
now passes 139 checks; native library regression passes 260 with 8 opt-in ignores.
