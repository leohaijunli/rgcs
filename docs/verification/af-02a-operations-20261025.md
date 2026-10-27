# AF-02a — Core operation coordination checkpoint

- Date: 2026-10-25
- Scope: bounded owner/target operation records and cancellation/resource semantics.
- Implementation: `crates/core/src/runtime/operations.rs`.
- API/behavior: [operation coordination](../../crates/core/src/runtime/operations.md).

## Delivered / measured acceptance

- Owner includes both Runtime client and module-instance identity. An unrelated
  owner/instance cannot cancel a record; original operation/target context is checked.
- Non-cloneable operation leases distinguish Prepared, possible dispatch,
  cancellation request and terminal outcome. An old target or late cancelled
  success cannot complete another operation.
- Cancellation reuses the atomic/Notify primitive without exposing cancellation
  authority to observers or queuing it behind device/DSP work.
- Drop/timeout after dispatch retains uncertainty and resource claims. Prepared
  cancellation/drop releases only that operation's undispatched claim. Trusted
  cleanup confirmation requires original identity and does not erase outcome evidence.
- Records/resources are bounded. Default 32 active/pinned and 128 retained;
  only finished unpinned history is evicted. Request duplicate is reported, not replayed.
- Outcome text is bounded; success cannot skip the recorded dispatch boundary or
  report unconfirmed cleanup. Native composition selects actual service deadlines.

Verification after this slice: **Core 996 passed / 1 ignored; App 193 passed /
8 ignored; clippy core/app all targets `-D warnings`, frontend strict typecheck
and formatting passed.** The operation selection has 14 behavioral tests and
10 generated binding tests; derived operation types are in both core and frontend
binding directories.

The coordinator opens no transport, starts no worker and sends no hardware command.
It is a core primitive, not proof that every current adapter has owner-specific
cancellation. AF-02b connects live native clients/targets and existing device
effects/cancel paths; **G1 remains open** until those integration fixtures pass.

```bash
cargo test -p maggcs-core --lib runtime::operations
TS_RS_EXPORT_DIR="$PWD/frontend/src/generated-types" cargo test -p maggcs-core --lib runtime::operations
cargo test -p maggcs-core --lib
cargo test -p maggcs-app --lib
cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings
npm --prefix frontend run typecheck
```
