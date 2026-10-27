# AF-02b.4 Motor contract/data checkpoint

- Date: 2026-10-27.
- Source base: `66346a7` plus the owned Motor contract/data slice.
- Result: **pass for signatures/data validation**; native permission/actuation,
  capture/log migration and the complete G1 matrix remain active work.

## Delivered

- [Motor contract](../architecture/af-02b4-motor-ownership.md): native API mapping,
  owner/target/config/forward-source/revision identity, Release/observer/lifecycle
  risks and measurable sliced acceptance.
- `core::motor_test::operation_types`: 14 request/ref/result/error DTOs.
- Existing `SessionState`, `PresetKind`, `WaveformPreset` export their own Rust
  shapes as `MotorSessionState`, `MotorPresetKind`, `MotorWaveformPreset`.
- Validation rejects foreign/old permission or operation scope and invalid count,
  NaN/infinite/out-of-range demand and unrepresentable periodic preset requests.
  Full-width u64 generations/config/permission revisions round-trip as strings.
- The existing maximum session width is named `MAX_MOTOR_FUNCTIONS = 16` and used
  by both session resizing and input validation.

## Verification

| Command | Result |
| --- | --- |
| `cargo test -p maggcs-core --lib motor_test --quiet` | 136 pass |
| `cargo test -p maggcs-app --lib motor --quiet` | 24 pass, 2 opt-in cases ignored |
| core `export_bindings` with frontend `TS_RS_EXPORT_DIR` | 306 core export tests pass |
| `npm --prefix frontend run check:generated-types` | 310 complete mirrors match, including 4 native exports |
| `npm --prefix frontend run typecheck` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo fmt --all -- --check` | pass |

This slice changes no native motor IPC admission or transport behavior. The next
step binds the actual native window's issued permission and active operation to
the existing service and its priority STOP/Release path. Capture/log/configuration
and electrical/acoustic/FC timeout qualification retain their own evidence.
