# Owner-bound operation coordination — AF-02a

`OperationCoordinator` owns bounded metadata/resource claims and cancellation
identity. Native composition validates the live `TargetRef`/client, selects the
configured control deadline and runs the existing protocol service. The coordinator
creates no I/O worker and cannot claim physical release from a cancelled future.

## API and behavior

- `prepare` returns one non-cloneable `OperationLease`, with owner/target/request
  identity and nonempty resource claims. Duplicate requests are reported rather
  than replayed. Random identity allocation stays outside the metadata lock.
- `mark_running` records possible dispatch; `check` refuses cancellation,
  expired deadlines, retired targets and already-finished operations.
- `cancel(owner, operation)` acts only on the exact owning client/instance and
  target. It signals the shared atomic/Notify cancellation primitive without
  waiting for a device lock or a slow observer's watch borrow.
- Prepared cancellation/drop can release a claim because nothing dispatched.
  Dispatched cancellation/drop retains Pending/Uncertain cleanup and resource
  claims until a real owned cleanup/read-back receipt.
- `finish` records a terminal outcome. Late success after cancellation/target
  invalidation cannot turn into success. Succeeded requires confirmed/no-needed
  cleanup; outcome text is limited to 2048 bytes.
- `confirm_cleanup` is a trusted native composition method, not an unauthenticated
  UI "clear busy" endpoint. It requires the original operation ref and explicit
  evidence, preserves the outcome and releases only its claims.
- `supersede_commands` is native arbitration for an already running, exact
  `EmergencyCommand` operation. It stops normal command operations on that target,
  including gaps between steps; their original cleanup receipts govern release.

Claims are per logical vehicle. Equal-resource claims conflict; actuator driving
also conflicts with parameter writes, mission replacement and ordinary command
work. Shell/log transfer conflict, while existing capture and driving can remain
independent. Stop/release requests operate on an existing owned operation rather
than acquiring a new normal command/actuator claim. External QGC/RC is not locked.

`EmergencyCommand` is a standalone nonblocking marker. It does not conflict with
ordinary claims; the single command worker still arbitrates the physical protocol.
Two active slots are reserved so normal metadata cannot consume all emergency
admission capacity. Caller-scoped cancellation remains distinct from this trusted
native safety arbitration.

Defaults: 32 active/pinned operations, 128 retained records, at most eight distinct
resource kinds and a 24 h general ceiling (native services select shorter actual
deadlines). Config validation caps active at 256, retained at 4096 and ceiling at
24 h; these are metadata/number-representation bounds, not hardware timing claims.
Only finished, unpinned history is evicted. Uncertain pinned work is never removed
to fabricate resource availability.

## Verification and remaining integration

```bash
cargo test -p maggcs-core --lib runtime::operations
```

Tests cover cross-client/instance cancellation, idle repeated cancel, dispatched
cleanup retention, resource conflicts, drop/expiry uncertainty, duplicate intent,
old-target/late success, capacity/history bounds and outcome validation.
Emergency fixtures also cover nonblocking claims and reserved active capacity.

`RuntimeClientSession` and `cancel_owner` support native client expiry; owner scope
includes both client and module instance. Native attach/validate/detach APIs are
documented in the [client foundation checkpoint](../../../../docs/verification/af-02b-client-foundation-20261025.md).

AF-02b now connects Parameters, reboot, general flight commands, Mission and
primary Shell/Logs to this
primitive; see the [flight-command acceptance](../../../../docs/verification/af-02b-flight-commands-20261026.md).
The 24 h ceiling permits a long-lived shell lease; write/list/download/delete
deadlines remain independently bounded by their native adapters.
G1 cannot close until the remaining Motor adapters and
cross-client protocol fixtures exercise the actual effect/cancel paths.
