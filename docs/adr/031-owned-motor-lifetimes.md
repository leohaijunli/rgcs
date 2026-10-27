# ADR-031: Owned Motor actuation, capture and log lifetimes

- Date: 2026-10-27
- Status: Accepted

## Decision

Keep the existing generation-bound Motor service as the sole actuator effect path,
but bind it before START to a native-issued Runtime owner/target/operation/deadline.
Actuation owns `ActuatorOutput` until full-bank Release. STOP has a priority channel;
event pressure cannot block it. First-bank and Release receipts are retained outside
the renderer.

SD capture has a separate persistent `Capture` resource and exact test/path identity,
so recording and actuation can overlap but owner loss can finalize the owned file.
Changed external logger identity is never stopped under an old operation.

Motor logs reuse one existing `LogService`: dedicated targets own one worker;
borrowed targets use the primary worker. Consequential requests carry exact Motor
session/operation identity. Local durable completion and FC protocol completion are
separate evidence; destructive ambiguity requires read-back before another delete.

Native close/lifecycle order is Motor Release → capture finalization → log cleanup →
observer/tool link cleanup. Unconfirmed cleanup preserves diagnostics and the link
when it can still support retry. Borrowed cleanup never closes primary I/O.

## Consequences

No global renderer STOP/cancel remains in production Motor IPC. Preflight opens the
tool for operator-owned cleanup. Transport receipts do not prove rotor stop, acoustic
tone completion or exclusion of QGC/RC; those remain explicit bench qualification.

See [contract](../architecture/af-02b4-motor-ownership.md) and
[acceptance](../verification/af-02b-motor-20261027.md).
