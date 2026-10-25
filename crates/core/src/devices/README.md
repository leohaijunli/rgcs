# Serial inventory and ownership

`DeviceManager` enumerates operator-pluggable ports; `DeviceDatabase` supplies
role hints. `discovery` owns a revisioned inventory with retained watch snapshots.

## Shared discovery — AUTO-02a-1

`spawn_serial_discovery(source, config)` returns a `SerialDiscoveryHandle` and
joinable task. The handle provides `snapshot`, `subscribe`, `refresh` and
deadline-bounded `shutdown`. `DeviceManager` implements the injectable
`SerialInventorySource` trait.

- The initial scan starts immediately. Defaults: 500 ms polling, 2 s enumeration
  deadline including blocking-pool queueing. Only one scan runs at a time.
- Refresh waits for a scan started after its request. Concurrent requests
  coalesce into one follow-up scan; a cached unavailable list is not success.
- Failure retains previous ports with Unavailable health. Timed-out results are
  discarded, and the blocking call is drained before another scan starts.
- Subscribe before fetching the snapshot. Slow consumers see the latest complete
  state; enumeration order alone does not change revisions.
- Removal, replacement and renumbering allocate new attachment IDs. Display-label
  changes retain the attachment. Exact VID/PID/nonblank USB serial supplies
  stable matching evidence; duplicate keys remain visible for ambiguity handling.
- IDs/revisions are string-encoded to preserve integers above 2^53.
- Shutdown cancels scheduling and waits for actual enumeration exit. A timeout
  can be retried. Last-handle drop cancels; snapshot subscribers do not retain
  the worker.

Inventory performs no serial opens/writes. `reserve(LeaseRequest)` atomically
claims a current attachment and canonical/stable USB identity. The non-cloneable
`SerialLease` moves into the actual RTK or MAVLink worker; it survives cancellation
timeouts until that owner closes. Phase/release changes publish revised inventory.
`None` alone does not establish external OS port availability.

Manual and automatic desktop paths use this same registry. `revalidate()` checks
the selected attachment before opening and after open. A changed attachment needs
a new lease. `shutdown()` also waits for outstanding leases to be released.

## Verification

```bash
cargo test -p maggcs-core --lib devices
```

The discovery/lease checkpoint has **47 passing device tests**, including nine
lease tests. Scripted blocking enumeration checks recovery, coalescing, identity,
late-result rejection, joined shutdown and worker loss. Controlled-clock tests
account for Tokio timer-wheel rounding. No physical device is required.

See the [confirmed design](../../../../docs/plans/serial-auto-connect-design.md),
[ADR-028](../../../../docs/adr/028-shared-serial-discovery.md) and
[backlog](../../../../docs/plans/rtk-base-and-air-ground-todos.md).
