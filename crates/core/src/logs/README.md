# Owned FC log service

`LogService` adds one bounded worker to an existing `ConnectionHandle`, reusing
`LogProtocol`. `LogTransaction::new(OperationId, TransportGeneration, Duration)`
captures the exact owner operation/generation/deadline; an optional owner
cancellation token retires its queue, writes and retries.

- Queue 4, progress queue 16; progress publishing is nonblocking and dropped
  notifications are counted. The native adapter correlates events with retained
  operation/request/entry identities.
- List/download writes bind the original generation and FC system/component.
  Foreign sources, pre-start replies and earlier connection output are ignored.
- `cancel_owned` returns a bounded **cleanup receipt**, not simply waiter exit.
  Cancel/timeout sends END under a separate 750 ms cleanup deadline and requires
  200 ms route quiet. Missing evidence quarantines that generation and preserves
  uncertainty; shutdown never opens another connection.
- Downloads above 256 MiB are rejected before buffer allocation. Existing 18,000
  byte windows, contiguous-tail retry and four 1.2 s retry windows are retained.
  The streaming file adapter is a later bounded-data work package.
- PX4 silence is not an empty list. The native primary-list adapter owns Shell
  and LogTransfer and can verify emptiness with a complete target-bound directory
  observation. Delete additionally rechecks NuttShell, fresh landed/disarmed state,
  logger inactivity, id/time/size and safe canonical SD paths.
- The native adapter selects the destination before admission, writes an owned
  temporary sibling on blocking I/O, checks cancellation/owner/target before
  rename, syncs the file/directory and publishes a durable typed result. A possible
  rename or destructive FC write followed by failure remains uncertain.

Verification: `cargo test -p maggcs-core --lib logs` includes queue deadline,
non-default UDP source rejection and END after expiry. Native production PTY and
local-I/O fixtures live in `crates/app-tauri/src/logs_operation_tests.rs`.

See `docs/architecture/af-02b4-shell-logs-ownership.md` for wire contracts, budgets,
risks and acceptance. Dedicated Motor Test log/capture adapters are the next slice.
