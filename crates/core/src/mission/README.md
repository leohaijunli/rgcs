# Mission protocol and owned exchanges

`MissionService` drives one existing protocol/transport worker. Owned upload,
download, clear and set-current calls use a one-use `MissionTransaction` bound to
operation ID, transport generation, deadline and owner cancellation. Their
results are terminal protocol verdicts; queueing consumes the same budget.

The worker rejects concurrent exchanges rather than replacing another owner's
state. Every write/retry has a generation/deadline receipt (at most 500 ms), and
owned cancellation retires only its transaction within a 1 s receipt budget.
Cancellation stops local retries and does not restore or clear the FC mission.
Possibly dispatched failures quarantine mission-protocol reuse until generation
replacement/trusted reconciliation, because ACKs have no Runtime operation ID.

The eight-entry request queue and 16-entry observer queue remain bounded.
Notifications use nonblocking delivery with a dropped-event counter; owned
terminal replies do not depend on an observer. Current-sequence observation is
separate from command completion and carries generation/receive timing.

Source, destination and mission-type checks cover all protocol messages. Set
current completes only on fresh observation of the requested sequence. Upload
requires all items before an accepted ACK; duplicate COUNT preserves a download,
and an item before COUNT cannot establish completion.

Wire snapshots are validated before count narrowing/encoding. Mission item
parameters retain MAVLink's NaN/unspecified sentinel as JSON `null`, including
download -> upload. Absolute-height domain logic continues to use `height::Height`;
`MissionItem` remains the existing frame-tagged MAVLink wire representation.

```bash
cargo test -p maggcs-core --lib mission
cargo test -p maggcs-core --test mission_owned_integration \
  --test mission_service_integration --test mission_fake_fc
```

Nine owned UDP fixtures cover actual protocol verdicts, exact/idle/old
cancellation, queue admission, target generation replacement, full/closed
observers, wrong sources/destinations/types, deadline/drop retirement and
zero-write malformed/expired capabilities. The existing fake-FC and service
fixtures retain loss/round-trip coverage.

The native/UI migration follows the
[confirmed interface and acceptance](../../../../docs/architecture/af-02b4-mission-ownership-proposal.md).
