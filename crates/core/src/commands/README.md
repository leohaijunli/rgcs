# Command protocol and owned flight operations

`CommandService` owns one `CommandSession` on the existing MAVLink transport.
`CommandTransaction` adds a one-use operation identity, captured transport
generation, deadline and owner cancellation to that worker. Queueing, writes,
retries and terminal replies use the same transaction; a dropped waiter retires
its local protocol work.

`send_owned` returns a terminal `CommandResult`. An accepted ACK is command
acceptance, rather than proof of completed flight or reboot. `cancel_owned`
confirms only this transaction's local retry retirement. Its 1 s receipt budget
does not wait behind another owner or an event observer.

Possibly dispatched failures quarantine the matching `MAV_CMD` for the transport
generation because MAVLink ACKs have no Runtime operation ID. Normal same-command
reuse returns `AmbiguousAck`. An emergency remains sendable, but quarantined ACKs
cannot establish success. A new generation clears quarantine and does not replay
the previous intent. The legacy notification queue is bounded and lossy;
owned replies and native retained operation statuses are authoritative.

## Native wire contracts

`operation_types` derives all Rust/TypeScript payloads:

- `CommandRequestContext`, `CommandReply`, `CommandOperationError` for the owned
  Parameters reboot and exact command status/cancel APIs.
- `FlightCommandRequest`, tagged `FlightCommandIntent`, `FlightCommandReply`,
  per-step results and structured `FlightCommandError` for the eight flight
  intents. Supplied absolute heights use `height::Height` with `AMSL_EGM96`.

The desktop flight adapter binds the real native caller to the existing target
worker. Takeoff and its optional follow-up arm share one operation. Preflight is
rechecked after the takeoff ACK, and forced-arm audit persistence precedes any
arm write. Flight protocol claims release after confirmed local retirement,
while the uncertain FC outcome and ACK quarantine remain retained. Reboot has
the stricter whole-target reconciliation policy.

Land, RTL and Arm/Disarm use the coordinator's standalone `EmergencyCommand`
marker and reserved capacity. Native emergency admission supersedes normal
command operations, including inter-step gaps. Ordinary clients still cannot
cancel another owner; the protocol worker arbitrates all actual command writes.

Budgets: 500 ms per transport write, 1 s retry interval and three retries;
the desktop selects 10 s operations or 20 s for a complete takeoff sequence.

```bash
cargo test -p maggcs-core --lib commands
cargo test -p maggcs-core --test command_service_integration
cargo test -p maggcs-app --lib flight_command_operations
```

See the reviewed [flight contract](../../../../docs/architecture/af-02b4b-flight-command-ownership.md)
and [acceptance matrix](../../../../docs/verification/af-02b-flight-commands-20261026.md).
