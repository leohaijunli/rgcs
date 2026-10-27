# ADR-030: Owned Shell/Logs retirement and durable completion

- Date: 2026-10-26
- Status: Accepted under the standing autonomous execution authorization
- Scope: AF-02b.4 primary Shell/Logs; dedicated Motor adapters follow separately

## Decision

Reuse the existing primary connection and log protocol, with native-window,
Runtime-session, target/generation, operation and request identities. Shell has
bounded output/command queues and a separate priority-close channel. List/deletion
reserve Shell and LogTransfer; download reserves LogTransfer. No global cancel
current or second transport is introduced.

A cancelled or expired log transfer uses an independent bounded END/route-quiet
receipt. Waiter retirement alone cannot confirm cleanup. Missing evidence pins
uncertainty/quarantines the generation. A possibly dispatched delete requires
fresh directory/list proof before another destructive operation. PX4 silence
cannot stand in for an empty directory.

Download admission follows destination selection. Temporary-file creation is
exclusive, owner/target/cancellation is checked before rename, and Unix directory
sync follows file sync. An existing destination is never removed as a non-atomic
fallback. The current single-buffer service caps downloads at 256 MiB, pending
the streaming/bounded-data adapter. Possible rename followed by failure is an
uncertain outcome, not a fabricated durable download.

## Consequences

The coordinator permits a 24 h shell lease while native write/close/log budgets
remain short and independent. Output/progress pressure cannot hold control locks.
Lifecycle signals recorded actors, waits for cleanup and then stops the log worker
before primary I/O. Local arbitration and bounded quiet observations do not prove
exclusion of external QGC/RC, arbitrary shell rollback or physical-flight safety.

See the [contract](../architecture/af-02b4-shell-logs-ownership.md) and
[acceptance record](../verification/af-02b-shell-logs-20261026.md).
