# AF-02b.2 / AF-02b.3 — Parameters ownership acceptance

- Record date: 2026-10-25; native observation timestamp:
  `2026-10-26T00:45:42.537Z` (UTC).
- Source: `e0c1b45e8b435ebfb586375b7d1520762d8a762c` plus the current working-tree
  implementation. This verification uses the existing changes, a fresh frontend
  build and a rebuilt release application.
- Reference desktop: Linux / WebKitGTK, as recorded in the
  [AF-00 baseline](af-00-baseline-20261025.md).
- Retained machine-readable results and artifact identities:
  [verification summary](af-02b-parameters-20261025.json).

## 1. Checkpoint status

| Slice | Result | Scope |
| --- | --- | --- |
| AF-02b.2 | Complete, verified by this checkpoint | Parameters pull/read/set ownership, native caller and target checks, deadline/cancellation and interrupted-write read-back |
| AF-02b.3 | Complete | Cross-client and close regression, stale-result rejection, screenshots, reproduction steps and documentation |
| AF-02b.4 | Pending; G1 open | Motor / Command / Mission / Shell / Logs resource, deadline and cancellation migration |

AF-06 still owns the full Parameters Module Host/SDK migration and its batch
reconciliation acceptance. Command effects such as `params_reboot` belong to the
Command migration. G1 requires the remaining AF-02b.4 adapters and their evidence.

## 2. Verified implementation contract

The concrete [Runtime contract](../architecture/runtime-contracts.md#9-parameters-owned-operation-checkpoint)
and [Parameters guide](../../frontend/src/params/README.md#owned-operations)
describe the implemented inputs, results and UI behavior.

- `ParamTransaction` carries the operation ID, captured transport generation,
  deadline and cancellation token through the existing parameter worker.
- `params_pull`, `params_read` and `params_set` accept a native-issued
  `RuntimeClientSession` and a request ID. Admission validates the actual calling
  webview and the exact target; a cached read also validates its target context.
- `params_cancel` and `params_operation_status` require the session and exact
  `OperationRef`. Native `Destroyed` expires the captured client rather than
  cancelling the current primary parameter service.
- Pull/read use `ParameterTransfer`; writes use `ParameterWrite`. Budgets are
  60 s for a pull and 5 s for a read/write. Owned worker cancellation has a
  separate 1 s acknowledgement budget; interrupted-write read-back has a
  separate 1 s budget through the original binding.
- Cancellation after a dispatched write retains an `Uncertain` outcome even
  when read-back succeeds. Confirmed read-back permits resource release; missing
  read-back retains `CleanupOutcome::Uncertain`, the write claim and an alert.
- `ParameterCommandError` distinguishes `Operation`, `Protocol` and
  `ReconciliationRequired`. Errors such as `NotOwner`, `TargetChanged`,
  `ResourceBusy`, cancellation and timeout are not converted into write success.
- Parameter events carry target and optional operation identity. The UI rejects
  old-target events/results and retains a staged change after an obsolete write
  result. The observing client cannot use another client's Pull cancellation.

## 3. Acceptance matrix

| Scenario | Evidence | Observed result |
| --- | --- | --- |
| A attempts to cancel B's write, then idle A closes | `params_operations::tests::closing_idle_client_a_cannot_cancel_client_b_write_or_clear_primary_io` | `NotOwner`; B confirms value 22; primary transport stays connected |
| A closes after dispatching its own write | `client_loss_cancels_and_reconciles_only_its_dispatched_parameter_write` | Original worker reads back 17; outcome remains uncertain, cleanup confirmed; B can subsequently write |
| Read-back is unavailable | `unconfirmed_readback_preserves_evidence_and_the_write_resource_claim` | `ReconciliationRequired`; uncertain cleanup; B receives `ResourceBusy` and sends no parameter write |
| Prepared write outlives its target generation | `old_target_or_expired_owner_cannot_dispatch_a_prepared_write` | Request fails; zero `NO_SEND` writes |
| A repeats an old pull cancellation while B reads | `owned_pull_cancel_does_not_end_a_later_clients_read` | B reads 17 and finishes successfully |
| Owned protocol cancellation, legacy coexistence, reconnect, dropped waiter and scoped pull events | Six `params_owned_integration` cases | All pass over isolated UDP and the production parameter worker/transport |
| Actual native Parameters window closes while main-webview B awaits an echo | Rebuilt release Tauri / WebKitGTK / MAVLink 2 peer | Distinct clients; B confirms 77; post-close cache read returns 77; normal exit code 0 |
| Two real React pages observe one pull | `check:params-ownership` | Observer Cancel disabled; forged cancellation rejected; owner cancellation reaches its exact operation |
| A pending write crosses target generation 1 → 2 | `check:params-ownership` | Old result and event ignored; staged change retained; explicit retry uses generation 2 and a fresh request ID |
| Existing values, reconnect and layout behavior | `check:params-connection`, `check:params` | 1912-entry catalog, delayed drafts, long values, stale replies and geometry checks pass |

The app integration cases use production serial transport on test-created Unix
PTYs. The native desktop check uses an ephemeral loopback UDP peer with system 42,
component 1, and an isolated XDG profile with legacy, FC and RTK automatic
connections disabled. This keeps wrong-device writes out of the fixture; held
echo/read-back cases explicitly test uncertainty instead of assuming delivery.

## 4. Checks run

| Check | Result |
| --- | --- |
| Core library | 1007 passed, 0 failed, 1 ignored; 5.20 s |
| App library | 203 passed, 0 failed, 8 ignored; 7.09 s |
| `params_owned_integration` | 6 passed, 0 failed; 0.22 s |
| `params_service_integration` | 4 passed, 0 failed, 1 physical-FC test ignored; 14.20 s |
| Core/app all-target clippy with `-D warnings` | Passed |
| Workspace formatting | Passed |
| Frontend strict TypeScript + production build | Passed; 3470 modules, Vite build 26.58 s |
| Release native build | Passed; 2 m 17 s |
| Three Parameters browser scripts | Passed; zero page errors |
| Native close collector | Passed; destruction observed in 155 ms |

Ignored hardware/SITL/large-fixture tests retain their opt-in status and are not
included in the passing count. Vite still reports the existing mixed-import and
large-workspace-chunk warnings assigned to AF-03. The 155 ms sample measures
Parameters renderer disappearance after a native close request, not actuator
release or an electrical stopping deadline.

### Reproduce the automated regression

From the repository root, with Rust tooling on `PATH`:

```bash
cargo test -p maggcs-core -p maggcs-app --lib --quiet
cargo test -p maggcs-core --test params_owned_integration --test params_service_integration
cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings
cargo fmt --all -- --check
npm --prefix frontend run build
cargo build --release -p maggcs-app
```

Run Vite on port 5199, then from `frontend/`:

```bash
npm run check:params-ownership -- http://localhost:5199/params.html screenshots/params/ownership/clients
npm run check:params-connection -- http://localhost:5199/params.html screenshots/params/ownership/connection
npm run check:params -- http://localhost:5199/params.html screenshots/params/ownership/layout
```

The connection/layout checks use the dev-only store handle, so use Vite dev rather
than preview. `PLAYWRIGHT_MODULE=/path/to/playwright` can select an external install.

### Native close evidence and manual reproduction

The reference-machine collector was run with:

```bash
AF00_RUNS=0 node /tmp/opencode/af02b-params-native/check-close.mjs
```

It uses `peer.py` in that directory and `Protocol`, `targets`, `nativeClose` from
`/tmp/opencode/af00-native/measure.mjs`. `AF00_RUNS=0` disables that helper's baseline
launch loop. These are local desktop collector artifacts; their hashes and the
observed result are retained in the JSON summary. The repository's portable
automated coverage is the UDP/PTY and browser tests above.

To repeat the native scenario manually on an isolated synthetic target:

1. Build frontend then native. Use a fresh XDG profile based on
   [the idle fixture](../../testdata/architecture/af00-idle/), with automatic
   connections disabled. Start a loopback MAVLink 2 parameter peer that can hold
   a `PARAM_SET` echo while continuing heartbeats and parameter reads.
2. Open Parameters on the verified target. Via the two webviews' native IPC,
   attach module `parameters` in the Parameters window (A) and main window (B).
   Check that `runtime_client_attach` returns distinct `ClientId` values.
3. From B call `params_set` with its captured session, a fresh request ID,
   `name: "OWNED_B"`, `value: { int: 77 }`, `ptype: "i32"`. Wait until the peer
   has received the write, with its echo still held.
4. Close A with the native window close action. Verify A disappears and B's
   `runtime_client_validate` still succeeds. Then release the held echo within
   B's 5 s operation budget.
5. Verify B's reply is `confirmed`, actual value is 77, operation owner is B and
   `params_operation_status` is finished/succeeded. Read `OWNED_B` through B and
   confirm 77; `tools_status` must report Parameters closed.
6. Close the main window normally and verify exit code 0. Owner-write-loss and
   missing-read-back cases are reproduced by the named PTY tests in §3.

## 5. Screenshots and visual review

Sixteen images were regenerated under
[`frontend/screenshots/params/ownership/`](../../frontend/screenshots/params/ownership/).
Key captures reviewed with the automated assertions:

- [Observer cannot cancel B's pull](../../frontend/screenshots/params/ownership/clients/params-observer-cancel-disabled.png)
- [Owned pull cancellation finishes](../../frontend/screenshots/params/ownership/clients/params-owned-cancel-finished.png)
- [Stale write retains staging and Apply](../../frontend/screenshots/params/ownership/clients/params-stale-write-retains-staging.png)
- [Recovered current value and draft](../../frontend/screenshots/params/ownership/connection/params-reconnected.png)
- [Long current/default values at 900 × 600](../../frontend/screenshots/params/ownership/connection/params-long-values-900.png)
- [1366 × 768 table](../../frontend/screenshots/params/ownership/layout/params-1366x768.png)
- [1920 × 1080 table](../../frontend/screenshots/params/ownership/layout/params-1920x1080.png)
- [900 × 600 table](../../frontend/screenshots/params/ownership/layout/params-900x600.png)
- [Bounded Field-mode staging tray](../../frontend/screenshots/params/ownership/layout/params-staged-tray.png)

Manual UI review: compare the observer/owner controls, verify a cancelled pull
returns to **Pull all**, and confirm the retained staged entry remains reachable
after retargeting. Repeat the connection and long-value walkthrough in the
[Parameters guide](../../frontend/src/params/README.md#native-desktop-review).
These browser screenshots qualify the Parameters regression; AF-08 retains its
separate three-resolution navigation/G3 acceptance.
