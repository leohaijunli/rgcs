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
