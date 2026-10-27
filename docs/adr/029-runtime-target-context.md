# ADR-029: Explicit Runtime target context over existing I/O owners

- Status: Accepted under owner standing implementation authorization, 2026-10-25.
- Related: ADR-002, ADR-025, ADR-026, ADR-028; AF-00/01.

## Decision

1. Keep protocol workers, serial inventory/leases and parameter caches in their
   existing owners. The same-process `runtime_facade` composes these services.
2. Separate application logical `runtime::VehicleId` (TS `RuntimeVehicleId`),
   `VehicleSessionId`, promoted `LinkId`, actual `TransportGeneration` and existing
   MAVLink node/routing identity. Equal addresses do not merge logical aircraft.
3. Issue opaque session identities with fallible OS-random allocation. Serialize
   full-width link/generation/revision counters as canonical decimal strings;
   allocation never wraps. No reference is a motor or client-authorization token.
4. A target handle is bound to an exact reference. It exposes copied telemetry
   and parameter observations and validates the target before and after reads.
   It exposes no mutable AppState/cache, current-global cancel or I/O shutdown.
5. Primary serial borrowing aliases the existing handle and parameter service.
   An alias does not register a second transport/cache or extend a retired primary
   context. Dedicated Motor Test remains an independently identified owner.
6. Available references require current worker identity and fresh matching PX4
   multirotor evidence. Fallback configured addresses alone remain unverified.
   Reopen changes the context session/generation and invalidates old references.
7. A reopened telemetry hub clears earlier aggregate values/ages. Parameter
   observation age uses actual frame receive time; scoped cache reads exclude
   values received before the current transport open without replacing the cache.
8. Existing side-effect adapters are migrated with AF-02/05–07. There is one
   active implementation of each device effect; facade delivery is not an
   independent Runtime process or the G-ARCH gate.

The current target metadata registry is bounded to the operational primary and
existing dedicated bench. Independent SITL/source and RTK/base sessions retain
their existing owners. A future fleet registry is a separate product delivery.

## Failure/routing analysis

| Failure | Handling |
| --- | --- |
| A stale caller binds the current selection instead of its old target | Exact vehicle/session/link/generation/node comparison; no implicit redirect |
| A borrowed facade closes primary serial or clears its cache | No shutdown/cache-mutation capability in TargetHandle; existing owner controls closure |
| IDs narrow in JavaScript or wrap into a previous session | String counters, checked parsing/allocation, independent random session IDs |
| Old queued telemetry/parameters look like a new generation | Aggregate reset on Connected; parameter cache retains actual receive time and generation-open filter |
| Serial/log request goes to a default or later-selected node | Capture the existing handle's discovered/configured route when creating the protocol; filter responses by that source; never use a later primary lookup inside the worker |
| Worker/session is unverified, stale or unsupported | Observe explicit unavailable state; binding does not restore an operation grant |
| A service is missing during attachment | Structured ServiceUnavailable; do not create another protocol/cache as a fallback |
| Physical FC reboot preserves an indistinguishable transport | No boot-identity claim; retain ADR-028 LINK-05's installed-topology investigation |

Software verification uses synthetic inputs, loopback and production serial via
PTYs. Physical serial writes, FC reboot continuity and electrical release remain
the declared bench/field acceptance layers. This slice's facade reads send no
device command; routing changes use isolated protocol fixtures before hardware.

## Verification / migration contract

See [Runtime contracts](../architecture/runtime-contracts.md) and
[AF-00 evidence](../verification/af-00-baseline-20261025.md). Core tests cover
full-width JSON/TS, identity exhaustion, exact context distinction, generation
telemetry reset and old-cache receive time. App/PTY tests cover stable borrowed
reference/cache, detach preserving primary I/O and rejection after reconnect.

AF-02 adds owned operations/deadlines/cancellation. AF-03/04 adds Module Host and
bounded data subscriptions. AF-09/10 freezes execution/monitor lifetime. AF-12
introduces process IPC/client authorization; it does not replay old start intent.
