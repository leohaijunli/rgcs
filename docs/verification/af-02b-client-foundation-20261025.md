# AF-02b — Native client-lifecycle foundation checkpoint

- Date: 2026-10-25
- Scope: native-issued module/client registrations, actual-caller identity and
  captured instance expiry. Device operation adapters are the remaining AF-02b work.
- Implementation: `crates/app-tauri/src/runtime_clients.rs`,
  `core::runtime::operations::RuntimeClientSession` / `cancel_owner`.

## Delivered

`runtime_client_attach(module_id, target)` first verifies the exact available
target through the facade. It issues a client/module session tied to the actual
Tauri calling webview. The same window/module/target attachment is idempotent.
Registry budgets are 64 window clients and 128 module instances, with bounded
descriptor IDs. Default state creates no transport, worker or optional module.

`runtime_client_validate(session)` checks actual caller window, ClientId,
ModuleInstanceId, module/target identity and current target validity. A supplied
opaque owner ID alone is not authentication. Attach rechecks after registration
to avoid a late binding during target replacement or native destruction.

`runtime_client_detach(session)` removes only that module registration and signals
only its coordinator operations. Native Destroyed captures the old ClientId,
so a late callback cannot expire a reopened window with the same label. Cleanup
failure is published to the retained alert center; it is not silently reported
as successful physical stopping. None of these APIs enables motor permission.

## Verification

- Five app fixtures pass: foreign-window owner refusal; ten repeated attaches
  yield one module; late native-close identity does not expire reopened window;
  target context cannot be reassigned; A detach does not cancel B's operation.
- Core captured-owner expiry test leaves another instance's cancellation token
  untouched. The combined operation selection has 15 behavioral and 11 TS-export tests.
- Full current suite: **Core 998 passed / 1 ignored; App 198 passed / 8 ignored**.
- Core/app all-target clippy `-D warnings`, formatting and frontend strict
  typecheck pass. Client session bindings are generated from Rust in both mirrors.

The initial combined compile/clippy command exceeded a 180 s harness budget after
its tests passed. The unchanged standalone clippy run completed successfully;
no rejected check was counted as a pass before the retry.

```bash
cargo test -p maggcs-app --lib runtime_clients::tests
cargo test -p maggcs-core --lib runtime::operations
cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings
cargo fmt --all -- --check
npm --prefix frontend run typecheck
```

## Remaining AF-02b / G1 at this checkpoint

This is client/lifecycle groundwork, not completed device-adapter migration.
Parameters, Motor Test, Mission/Command, Shell/log effect/cancel callbacks must
use these owned operation contexts and actual target generations. Existing legacy
Parameters native-close/global cancel is still identified in the AF-00 inventory.
Do not close G1 or delete compatibility paths before their real protocol fixtures
and callers migrate. Live/packaged client RPC and active-output native close remain
additional integration evidence, independently of the earlier AF-01 idle RPC smoke.

**Subsequent checkpoint:** [AF-02b.2 / AF-02b.3 Parameters acceptance](af-02b-parameters-20261025.md)
records the completed owned pull/read/set adapter, cross-client cancellation and
read-back tests, rebuilt-native Parameters close, and UI screenshots. It replaces
the Parameters global-cancel path identified above. AF-02b.4's other effect
adapters and the G1 gate remain pending.
