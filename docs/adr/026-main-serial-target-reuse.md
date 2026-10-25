# ADR-026: Motor Test borrowing the primary serial connection

- Status: Accepted, 2026-10-25 (MR-22b confirmed contract).
- Related: ADR-025, ADR-024, motor-test remediation plan.

## Decision

Offer **Use main serial link** when the primary transport is serial, a fresh
PX4 heartbeat identifies the configured system/component 1, and its parameter
service exists. IPC echoes `MainSerialTarget { target, transport_generation,
endpoint }`; an obsolete offer is refused. UDP, missing and incompatible main
connections retain the dedicated-target workflow.

The target clones the same `ConnectionHandle` and `ParamService`. Its
`AuxiliaryLinkRuntime::borrowed` owns only Motor Test tasks and never shuts down
the primary transport. `TargetLinkOrigin::{Dedicated, MainSerial}` makes this
ownership visible. Repeating the same attach is idempotent; attaching over a
different Motor Test owner is refused. Trying to open the main serial port as
a dedicated target is rejected, including canonical path aliases.

Motor control starts OFF. Main-link failure, heartbeat loss, reconnect,
replacement, tool detach or teardown revokes permission and clears the borrowed
attachment. Explicit replacement first confirms owned motor releases; a failed
release preserves the current link. Transport failure preserves the failed
session's diagnostics, never reports a successful release, and expires its
attachment. A failed old-generation session cannot issue commands through a
replacement transport.

## Failure analysis

| Failure | Handling |
| --- | --- |
| Tool cleanup closes the main port | Borrowed runtime has no transport shutdown ownership; primary runtime remains responsible |
| Duplicate parameter protocol/cache | Reuse the primary ParamService and its cache |
| A stale UI attaches a changed vehicle | Exact connection/node/generation/endpoint match is required under the lifecycle operation lock |
| Old actuator or shell writes cross reconnect | Bounded generation-guarded writes; capture requests bound to their worker generation |
| Concurrent shell or log protocols interfere | Finish an in-flight main shell exchange before attachment; refuse attachment during a main log transfer; route shell/log work through Motor Test while borrowed |
| Main log cancellation interferes with Motor Test | Idle cancellation sends no frame; a pending primary operation retains its busy reservation until handled |
| Main disconnect races tool teardown | Main replacement/lifecycle and attach serialize on AppState::ops; watcher cleanup rechecks the attachment identity |

Stream rates affect every subscriber on the shared connection. The primary
telemetry and Parameters views remain available. During borrowing, use Motor
Test's Capture/Logs controls; primary console/log actions return an actionable
ownership error. Motor STOP/E-STOP bypass shell/log operations.

## Verification

Unix PTY integration uses the production serial transport and primary parameter
worker, with a synthetic PX4 peer. It checks same connection identity and cache,
idempotent attach, default OFF, one configured motor's commands/releases,
detach preserving main I/O, reconnect revocation, obsolete-offer refusal and
primary log-operation exclusion. Core UDP integration rejects writes from an
old motor session after transport reconnect.

The React IPC check verifies offer eligibility, exact echoed generation, no
second connect command, visible shared ownership, detach and error handling.
Screenshots: `motor-main-serial-{offer,attached}-1366x768.png` in
`frontend/screenshots/motor-test/`.

PTY/browser tests do not prove electrical release, physical serial unplug
behavior, or packaged native-window close behavior; those remain bench checks.
