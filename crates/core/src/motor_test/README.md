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

## Owned actuation checkpoint

Core accepts an `OperationCancellation` and absolute native operation deadline
before Start. Owner loss/deadline expiry interrupts waits/writes and performs the
same full-bank Release without a renderer callback. Prepared owner loss writes
nothing. First-control and terminal Release evidence are retained separately from
bounded, droppable UI events.

Native permission is an exact `OperationOwner`/`TargetRef`/`Revision` grant.
Start owns `ActuatorOutput` through Release and replies only after a complete first
control bank. Updates require the exact active operation, increasing revision and
unique request ID. STOP/E-STOP name that operation; retained old STOP is idle and
cannot stop a replacement. Target-generation loss never redirects Release and
keeps uncertainty. Native close/lifecycle cancel recorded actors before cleaning
observers/dedicated I/O; borrowed primary I/O remains open.

Tone choices and mapping/parameter configuration now carry the actual Motor
Runtime session; physical configuration writes reuse the existing owned Parameter
worker and require the exact permission. The main preflight repair opens Motor
Test rather than invoking a global STOP.

See `docs/verification/af-02b-motor-actuation-20261027.md`. Dedicated capture/log
ownership remains the next slice; this checkpoint does not close G1.

## Software completion

Owned SD capture and Motor Logs are now complete. Capture has an independent
claim/file identity and owner-close finalization. Dedicated logs use one owned
`LogService`; borrowed logs reuse primary. Exact cancellation, END/quiet cleanup,
durable temporary-file commit, deletion read-back and lifecycle order are verified.
See `docs/verification/af-02b-motor-20261027.md`. G1 remains open only for the
cross-adapter matrix; electrical/acoustic behavior remains bench evidence.
