# AF-02b.4a — Owned command and Parameters reboot acceptance

- Record date: 2026-10-25.
- Source: `e0c1b45e8b435ebfb586375b7d1520762d8a762c` plus the current
  working-tree implementation.
- Contract: [owned command/reboot](../architecture/af-02b4a-command-ownership.md).
- Scope: command-worker ownership primitives and the Parameters reboot caller.
  General flight Command, Motor, Mission, Shell and Logs adapters remain pending;
  G1 is open.

## Result

AF-02b.4a is complete within its software scope. The existing single command
worker now accepts owner/target/generation-bound `CommandTransaction` work without
opening another transport. Queueing, writes, retries, replies, deadlines and
cancellation are tied to one operation. Native caller identity and target binding
are checked before a Parameters reboot can enter that worker.

Possibly dispatched failures preserve an uncertain operation and quarantine that
`MAV_CMD`'s ACK for the transport generation. Normal same-command reuse is
refused because MAVLink ACKs carry no Runtime operation ID. Emergency commands
remain sendable, but matching ACKs are not trusted as success. A new transport
generation clears quarantine and never replays the old intent.

Reboot admission fails closed unless fresh telemetry proves disarmed and
`OnGround`. An accepted ACK means command acceptance, not completed physical
reboot. It retires the exact Runtime target generation immediately, invalidates
other active owned work for that target and removes the UI reboot action; a
reconnect must issue a fresh target/client session.

## Acceptance matrix

| Scenario | Evidence | Result |
| --- | --- | --- |
| Foreign caller cancels B; idle A closes | `foreign_cancel_and_native_close_a_leave_b_reboot_and_primary_services_alive` | A gets `NotOwner`; B receives its ACK; primary transport/cache remain alive |
| Owner closes after first reboot write | `closing_reboot_owner_stops_retries_and_retains_uncertainty_and_claims` | Retries stop within 1 s; outcome and cleanup remain uncertain; claims stay pinned |
| Wrong caller, expired owner or stale generation | Three native PTY cases | Zero reboot writes |
| Missing landed state, armed target or borrowed Motor target | Two native PTY cases | Admission fails before a write |
| Parameter claim conflicts with reboot | `parameter_claim_blocks_reboot_and_denied_ack_releases_its_own_claim` | Reboot is busy; denied terminal reply releases only its own claim |
| Accepted reboot while transport still open | Native PTY acceptance | Old Runtime target becomes unavailable immediately; no second owned effect can bind it |
| Legacy command is preempted | UDP command integration | Old waiter receives `Superseded`; late ACK cannot settle new owned work |
| Failed same-command then emergency | UDP command integration | Emergency frame is sent; all ambiguous ACKs remain untrusted |
| Dropped waiter / full event observer / full command queue | UDP and core fixtures | Retry retirement and deadline remain bounded; observer drops are counted |
| Wrong FC source or GCS destination | UDP command integration | ACK ignored; exact current peer's terminal verdict wins |
| React reboot denied / uncertain / old target / accepted | `check:params-ownership` | Distinct status; stale reply ignored; reconnect does not replay; accepted action clears |

## Checks run

| Check | Result |
| --- | --- |
| Core library | 1013 passed, 0 failed, 1 ignored |
| App library | 211 passed, 0 failed, 8 ignored |
| `command_service_integration` | 20 passed, 0 failed |
| `params_owned_integration` | 6 passed, 0 failed |
| `params_service_integration` | 4 passed, 0 failed, 1 physical-FC case ignored |
| Core/app all-target clippy with `-D warnings` | Passed |
| Workspace formatting and git whitespace | Passed |
| Frontend strict TypeScript and production build | Passed; 3470 modules |
| Release desktop build | Passed |
| Parameters ownership/connection/layout browser checks | Passed; zero page errors |

Twenty screenshots are retained under
[`frontend/screenshots/params/ownership/`](../../frontend/screenshots/params/ownership/):
seven client/operation states, seven connection states and six layout states.
The four new reboot captures show denied, reconciliation-required, stale-result
and accepted behavior. Visual review confirms the reboot banner remains available
after denial/uncertainty, disappears across target replacement, and is cleared on
the current target's accepted ACK. Full truncated status text remains available
through the native title tooltip.

## Reproduction

```bash
cargo test -p maggcs-core -p maggcs-app --lib --quiet
cargo test -p maggcs-core --test command_service_integration \
  --test params_owned_integration --test params_service_integration
cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings
cargo fmt --all -- --check
npm --prefix frontend run build
cargo build --release -p maggcs-app
```

Run Vite on port 5199, then:

```bash
npm --prefix frontend run check:params-ownership -- \
  http://127.0.0.1:5199/params.html screenshots/params/ownership/clients
npm --prefix frontend run check:params-connection -- \
  http://127.0.0.1:5199/params.html screenshots/params/ownership/connection
npm --prefix frontend run check:params -- \
  http://127.0.0.1:5199/params.html screenshots/params/ownership/layout
```

These tests use isolated UDP peers, test-created Unix PTYs and browser IPC mocks.
They do not claim installed-FC reboot continuity, external QGC/RC exclusion or
electrical behavior. Those remain explicit later integration/physical gates.
