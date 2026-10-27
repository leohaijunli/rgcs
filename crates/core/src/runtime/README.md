# Runtime identity and target contracts

AF-00/01 introduces a small `runtime` vocabulary. Existing MAVLink, telemetry,
parameter, command and mission modules remain protocol authorities. This module
opens no transport and initializes no optional worker.

- `runtime::VehicleId` is an opaque logical aircraft ID; its TypeScript name is
  `RuntimeVehicleId`. Existing `telemetry::VehicleId` remains the MAVLink route
  used through the `MavlinkTarget` compatibility alias.
- `VehicleSessionId` is a Runtime context, not proof of physical FC boot identity.
  Opaque IDs use OS-random issuance and checked bounded ASCII deserialization.
- `LinkId` is the promoted desktop I/O-owner identity. `next()` is fallible and
  never wraps. Link IDs and transport generations serialize as decimal strings,
  retaining values above 2^53. Revision zero is a valid initial revision.
- `TargetRef` identifies vehicle/session/link/generation/node separately.
  `TargetSnapshot` is observation/capability state, not motor or operation authority.
- `ClientId`, `ModuleInstanceId`, `OperationId`, `RequestId` and `SubscriptionId`
  reserve the minimum typed vocabulary for subsequent owner-bound services.

Verification:

```bash
cargo test -p maggcs-core --lib runtime
# From the workspace root, regenerate the frontend mirror from the same derives:
TS_RS_EXPORT_DIR="$PWD/frontend/src/generated-types" cargo test -p maggcs-core --lib runtime
```

Tests check lossless full-width JSON/TS bindings, malformed/broadcast identities,
counter exhaustion, opaque ID validation and exact target-context distinctions.
See [Runtime contracts](../../../../docs/architecture/runtime-contracts.md) and
[AF-00 inventory](../../../../docs/architecture/af-00-current-system.md).

The same-process desktop adapter is `app-tauri/src/runtime_facade.rs`. It reuses
primary/dedicated services, refuses expired references, and keeps a borrowed
primary/cache alive. [ADR-029](../../../../docs/adr/029-runtime-target-context.md)
records the ownership and migration decisions.

AF-02a's [operation coordinator](operations.md) adds bounded owner/target resource
records, atomic cancellation, deadlines and retained cleanup uncertainty. Native
effect/cancellation integration is AF-02b. Parameters, reboot and flight commands
are connected and verified. `EmergencyCommand` supplies nonblocking admission and
reserved capacity; native supersession retires normal commands without exposing
a foreign-owner cancel API. The remaining adapter matrix keeps G1 open.
See the [flight-command contract](../../../../docs/architecture/af-02b4b-flight-command-ownership.md).
