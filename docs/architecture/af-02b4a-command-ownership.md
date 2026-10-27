# AF-02b.4a — Owned commands and the Parameters reboot adapter

- Date: 2026-10-25.
- Owner confirmed the signatures and acceptance scope before implementation.
- Scope: the existing command worker's owned transactions, native command
  operation/status/cancel adapters, and the Parameters reboot caller.
- Parent: [Runtime contracts](runtime-contracts.md), AF-02b.4 in the
  [architecture-first plan](../plans/architecture-first-delivery-plan.md).

## Contracts

```rust
pub struct CommandRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

pub struct CommandReply {
    pub operation: OperationRef,
    pub result: CommandResult,
}

pub enum CommandOperationError {
    Operation { error: OperationError },
    Protocol { error: CommandError },
    ReconciliationRequired { operation: Box<OperationRef>, reason: String },
}

pub struct CommandTransaction { /* private identity, generation, deadline, cancellation */ }

impl CommandService {
    pub async fn send_owned(&self, transaction: CommandTransaction,
        command: MavCmd, params: [f32; 7]) -> Result<CommandResult, CommandError>;
    pub async fn cancel_owned(&self, transaction: &CommandTransaction)
        -> Result<bool, CommandError>;
}
```

Native IPC injects the actual Tauri app/window; supplied identities alone do not
authorize a request:

| Command | Explicit input | Result |
| --- | --- | --- |
| `params_reboot` | `CommandRequestContext` | `CommandReply` |
| `command_cancel` | `RuntimeClientSession`, exact `OperationRef` | `OperationStatus` |
| `command_operation_status` | Same session and operation | `OperationStatus` |

All three return structured `CommandOperationError`. `CommandResult` and the
protocol error are Rust-derived wire types. Native `command_operation` events
contain the original operation/request/target identity and retained cleanup state.

## Write/failure analysis

1. **Wrong window or replaced target:** authorize the real caller, bind the
   existing command service/transport together, and revalidate before dispatch.
   Every write/retry uses the captured transport generation and a bounded,
   cancellable write receipt. No request resolves a later primary service.
2. **Owner loss, timeout or missing ACK after dispatch:** cancelling stops local
   attempts; it cannot undo a command already received by the FC. Retain an
   uncertain outcome and resource claim. A terminal `accepted` reply means command
   acceptance, not proof of a completed FC reboot. Reconnect never replays intent.
3. **Late ACKs and preemption:** MAVLink ACKs carry no Runtime operation ID and
   `IN_PROGRESS` can produce later terminal updates. Any possibly dispatched
   owned or legacy failure quarantines that `MAV_CMD` for the transport generation.
   Normal same-command reuse fails `AmbiguousAck`. An emergency is still written,
   but matching ACKs remain untrusted; state read-back or a new generation must
   establish its outcome. Local reply ownership is retired before a preempting
   command installs its reply.
4. **Reboot conflicts:** claim the command/parameter/mission/shell/log resources,
   require fresh telemetry proving disarmed and `OnGround`, and respect the
   existing borrowed Motor Test/log exclusions. An accepted reboot immediately
   retires the exact Runtime generation, invalidates its active owned operations
   and clears the UI's reboot action. This coordinates migrated Runtime clients;
   external QGC/RC and the remaining legacy adapters are not represented as a
   global lock and migrate in later slices.
5. **Slow observation:** bounded legacy event delivery must not block an owned
   cancellation, deadline or command write. Dropped legacy notifications are
   counted; owned replies and retained native statuses remain authoritative.

Budgets: reboot operation 10 s including queueing; owned cancellation receipt 1 s;
individual transport write 500 ms, capped by the operation's remaining budget.
The command protocol retains its existing 1 s retry / three-retry policy.

## Acceptance

- Foreign caller and stale/prepared target requests produce zero command writes.
- Closing idle client A leaves B's command, primary I/O and parameter cache alive.
- Closing/cancelling the owner stops its retries within the receipt budget and
  retains uncertainty for possibly dispatched work. Idle/old cancellation does
  not cancel later work; dispatched uncertainty deliberately quarantines only
  its `MAV_CMD` until the transport generation changes.
- A preempting awaited emergency receives its own ACK; the displaced waiter gets
  `Superseded`. Foreign/stale ACKs cannot settle another local operation. A
  quarantined emergency is still sent but cannot report an ambiguous ACK as success.
- Queueing, a dropped waiter, a saturated observer and transport reopen cannot
  extend an owned command beyond its deadline or revive its intent.
- Parameters sends the clicked session/request, discards obsolete reboot results
  and never reports a timeout as an accepted reboot. An accepted ACK retires the
  old Runtime generation before another owned effect can start. Capture
  screenshots and record manual reproduction steps.

Use isolated UDP peers and test-created serial PTYs. These software fixtures do
not assert installed-FC reboot continuity or electrical output behavior. G1 stays
open for the remaining Command/Motor/Mission/Shell/Logs adapter work.

Implementation and measured results are in the
[AF-02b.4a acceptance record](../verification/af-02b-command-reboot-20261025.md).
