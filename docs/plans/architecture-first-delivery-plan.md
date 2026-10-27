# Architecture-First Delivery Plan

- **Language:** English | [简体中文](architecture-first-delivery-plan.zh-CN.md)
- **Date:** 2026-10-26
- **Status:** Execution authorized on 2026-10-25; AF-00/01 and AF-02 through the
  AF-02b.4b owned-flight-command slice are recorded.
  Continued small-slice implementation is authorized without repeated approval
  questions; engineering gates remain evidence-based.
  The owner reaffirmed autonomous decisions and continuous small-slice execution
  on 2026-10-26. Publish decisions/signatures and measurable acceptance before
  implementation, then proceed under that standing authorization.
- **Priority decision:** Architecture, demand loading, existing tools, execution and
  monitoring come first. **Flight Plan and Drape are the final feature phases.**
- **Near-term scope:** One operational flight aircraft, retaining the existing
  dedicated bench, SITL-source and independent base roles. Future multi-aircraft
  compatibility is an interface-design consideration, not a near-term feature package.
- **Sequence authority:** This document supersedes the implementation order in the
  earlier Survey/tool proposals. Their functional designs and BL/SW/TM IDs remain references.
- **Related designs:** [Survey workspace](ugcs-survey-workspace-plan.md),
  [tool/runtime architecture](tool-module-architecture-plan.md),
  [UgCS operating baseline](../ugcs-survey-operations-baseline.md).

**Review guide:** §4 phase order; §5 architecture tickets; §6 final Flight Plan/
Drape phases; §8 acceptance; §11 resources; §14 future multi-aircraft support.

**Execution record — 2026-10-25:** AF-00 ownership/lifecycle inventory,
minimum contracts and AF-01 fixtures are in
[the current-system inventory](../architecture/af-00-current-system.md) and
[Runtime contracts](../architecture/runtime-contracts.md). Native/browser
measurements and current regression results are in
[the baseline evidence](../verification/af-00-baseline-20261025.md).
The owner authorized continued execution and waived repeated approval questions.
Linux/WebKitGTK is the first reference desktop; PX4 multirotor remains the
aircraft scope. Implementation proceeds one small AF-owned slice at a time.

**AF-01 checkpoint:** Explicit target/context types and the same-process read
facade are implemented and verified. [Evidence](../verification/af-01-facade-20261025.md)
records core 972/app 193 passing tests, strict/clippy/format checks, generated TS
bindings and rebuilt-native RPC/close smoke. AF-02 adds operation ownership and
cancellation; G1/G-ARCH remain their declared engineering gates.

**AF-02a checkpoint:** The bounded core operation coordinator implements
owner/target cancellation, resource conflicts, deadline/drop uncertainty and
retained cleanup evidence. [Evidence](../verification/af-02a-operations-20261025.md)
records core 996/app 193 passing tests and strict/clippy checks. AF-02b native
effect/client integration remains the active work package; G1 is not closed.

**AF-02b client foundation:** Native caller/client/module registration and captured
expiry are implemented with bounded metadata and owner-only cancellation signals.
[Evidence](../verification/af-02b-client-foundation-20261025.md) records core 998/app
198 passing tests and strict checks. Device effect/cancel adapter wiring and G1
qualification remain in progress.

**AF-02b.2 / AF-02b.3 Parameters checkpoint:** Owner/target-bound parameter
pull/read/set, exact cancellation and interrupted-write read-back are implemented
and verified. [Acceptance evidence](../verification/af-02b-parameters-20261025.md)
records core 1007/app 203 library tests, ten parameter protocol integration tests,
three browser regressions with 16 screenshots, strict checks and a rebuilt-native
cross-client close test (155 ms; B's write and post-close cached read both return 77).

**AF-02b.4a Command/reboot checkpoint:** The existing command worker now supports
owner/target/generation-bound transactions, cancellation retirement, bounded
observer behavior and generation-wide ambiguous-ACK quarantine. Parameters reboot
requires fresh disarmed/OnGround telemetry and retires the accepted target
generation. [Acceptance evidence](../verification/af-02b-command-reboot-20261025.md)
records core 1013/app 211 library tests, 20 UDP command and 7 native PTY ownership
fixtures, three browser regressions with 20 retained screenshots, strict checks
and rebuilt release artifacts. G1 remains open for the other adapters.

**AF-02b.4b Flight-command checkpoint — 2026-10-26:** All eight general flight
intents use typed owner/target/generation-bound command operations. Emergency
admission has reserved/nonblocking capacity and native normal-command supersession.
Takeoff/preflight/audit rechecks, partial-step uncertainty, local retry retirement,
old UI request/reply isolation and exact Arm/Disarm labels are verified.
[Acceptance evidence](../verification/af-02b-flight-commands-20261026.md) records
16 native flight PTY fixtures, 268 synchronized Rust/TypeScript contracts,
7 browser regressions with 264 passing assertions and 46 screenshots,
1378 passing workspace tests (12 opt-in/environment-dependent cases ignored),
all-target clippy, formatting, 17 frontend static gates, production frontend and
workspace release builds. G1 remains open for Mission/Shell/Logs/Motor and the
complete cross-adapter matrix.

**AF-02b.4 Mission checkpoint — 2026-10-26:** Existing upload, download, clear
and set-current now return typed terminal replies from the captured single Mission
worker. Exact cancellation/status, generation-bound writes, ambiguous-reply
quarantine, bounded observation, target/current events and sent-snapshot read-back
are implemented. The UI preserves newer edits/local clear, requires read-back
before reporting sync, coalesces passive current validation, and never redirects
old intent to a replacement target. [Acceptance evidence](../verification/af-02b-mission-20261026.md)
records 9 owned UDP and 9 native PTY cases, 24 store scenarios, 301 browser
assertions with 57 screenshots, 277 synchronized generated contracts, 1428
passing workspace tests (12 ignored opt-ins), strict/static checks and rebuilt
production/release artifacts.
G1 remains open for Shell/Logs/Motor and the final cross-adapter matrix.

| Active AF-02b slices | Status |
| --- | --- |
| AF-02b.2 — Parameters owned transfer/write implementation | Complete |
| AF-02b.3 — Cross-client/native-close regression, screenshots and documentation | Complete |
| AF-02b.4a — Command primitive and Parameters reboot | Complete |
| AF-02b.4b — General flight Command migration and acceptance | Complete within software scope |
| AF-02b.4 — Mission adapter and acceptance | Complete within software scope |
| AF-02b.4 — Primary Shell/Logs adapter and acceptance | Complete within software scope |
| AF-02b.4 — Motor adapters and cross-adapter matrix | Pending; G1 open |

AF-06 retains the full Parameters Module Host/SDK and batch-reconciliation
acceptance. The Command checkpoints do not imply the remaining legacy effect
callers are migrated. The [Mission contract](../architecture/af-02b4-mission-ownership-proposal.md)
is implemented; primary Shell/Logs is also implemented and accepted.
Its [owned Shell/Logs contract](../architecture/af-02b4-shell-logs-ownership.md)
records the approved signatures, serial-write/deletion risk controls and
measurable acceptance. [Shell/Logs evidence](../verification/af-02b-shell-logs-20261026.md)
records exact actor/cancel/status, complete shell framing/priority close, durable
local files, delete/empty-directory read-back, 293 synchronized contracts,
330 browser assertions/68 screenshots, 1463 workspace tests (12 opt-in ignores)
and all 26 production/release gates. Dedicated Motor ownership and the final
cross-adapter/native-client/fault matrix are next; G1 stays open.

**Motor contract/data checkpoint — 2026-10-27:** The
[owned Motor contract](../architecture/af-02b4-motor-ownership.md) and Rust-derived
permission/start/update/Release/source/error DTOs are published. Scope/numeric/
lossless-wire fixtures, 136 filtered Motor tests, 310 generated mirrors and strict
checks pass ([evidence](../verification/af-02b-motor-contract-20261027.md)). Native
window/permission/operation admission is the next small implementation slice;
Motor migration and G1 are not yet complete.

The [Motor priority prerequisite](../verification/af-02b-motor-priority-20261027.md)
is also verified: urgent STOP interrupts control writes, command epochs fence old
blocked producers, event pressure is nonblocking and native state is retained
independently. Three new fault fixtures, 139 core Motor checks and 260 native
library checks pass; native permission/operation binding remains active.

## 1. Outcome and ordering decision

The first deliverable is a reliable, efficient application platform that can host
independent tools. It must be possible to prove its ownership, lifecycle, loading,
and failure behavior before replacing the Survey planner or terrain algorithms.

The recommended order is:

```text
Contracts and current-state baseline
  → shared Runtime ownership and operation coordination
  → lazy module host / bounded data services
  → migrate Inspector, Parameters, Motor Test and menus
  → execution snapshots / navigation-independent monitoring
  → worker and Runtime process isolation / architecture qualification
  → prove the extension contract for future Configuration / Calibration
  → FLIGHT PLAN: semantic task/editor and survey generators
  → DRAPE: height design, complete-route terrain behavior and survey closure
  → integrated release acceptance
```

Flight Plan/Drape interfaces are reserved early only where other modules need a
small data contract. Their new editor, generators, terrain solver integration,
full-route behavior, and line-aware survey business logic are delivered at the end.

## 2. Architectural rules

1. **One physical-I/O owner.** Reuse the existing device inventory/leases and
   connection services. A borrowed connection cannot shut down its owner.
2. **Explicit operation ownership.** Start, cancel, stop and cleanup identify the
   owning tool/session and target generation. They do not act on whichever global
   service happens to be current.
3. **Views do not own flights.** Changing stage or closing a chart must not stop
   active flight monitoring/recording or replace its executed baseline.
4. **Authority remains native.** Device writes and admission checks go through the
   runtime, with the existing Motor Test permission/release semantics.
5. **All asynchronous work is identified.** Use request, operation, instance,
   target-generation and input-revision references as appropriate.
6. **Bounded resource use.** Subscriptions, queues, caches and workers have declared
   limits. Analysis cannot block actuator release or command scheduling.
7. **Demand loading.** Register small descriptors at startup; initialize UI, data,
   DSP and worker state only when requested or required by an active runtime role.
8. **Recovery preserves uncertainty.** A timeout or crash does not become a
   successful stop/write. Reconnect does not restore motor authority or replay
   start/calibration intent automatically.
9. **Small shared contracts.** Tools receive scoped service interfaces, not the
   complete `AppState` or another tool's UI store.
10. **Stage migrations are reversible.** Keep one active implementation of each
    effect while adapters are replaced; never shadow-send device commands.
11. **Existing native roles retain their policy.** RTK/base and configured automatic
    connections remain independent of window visibility.
12. **The early platform has an honest capability boundary.** Architecture
    qualification does not claim that the legacy Flight Plan/Drape defects are fixed.

## 3. Starting point and extraction boundaries

| Existing code / design | Reuse or extraction |
| --- | --- |
| `app-tauri/src/state.rs`: `AppState`, `ActiveLink`, `LinkId`, lifecycle lock | Runtime composition, target registry and scoped service handles |
| `core/devices`, `mavlink`, `telemetry`, `params`, `commands`, `mission` | Existing protocol/device services remain authoritative |
| ADR-025 / ADR-026 | Motor permission, generation revocation, owned output release, primary-link borrowing |
| ADR-028 and ongoing serial work | Shared inventory/lease implementation; preserve its staged integration |
| `inspector_service.rs`, `actuator_service.rs`, `params_service.rs` | Tool adapters migrated one at a time |
| `capture_service`, `run_service`, `stream_service`, logs/RTK services | Reusable service boundaries; avoid duplicating their protocols/caches |
| `TopBar.tsx`, `WorkflowNav.tsx`, tool HTML entries, `RightInspector.tsx` | Lazy module loading, tool routing, menu consolidation, independent UI lifetimes |
| Existing mission upload/read-back and drape-monitor types | Compatibility inputs for early execution/monitor modules |
| `core/plan`, `core/survey`, `core/terrain` | Retain existing functionality behind adapters until the final feature phases |

Paths beginning `core/` or `app-tauri/` refer to `crates/.../src/` where appropriate.
The working tree contains ongoing user work: extraction must reconcile that work,
not replace whole services from an old snapshot.

## 4. Phase map and acceptance gates

| Phase | Packages | Deliverable | Exit gate |
| --- | --- | --- | --- |
| 0 — Baseline and contracts | AF-00 | Ownership/lifetime map, measured startup/resource baseline, reviewed API vocabulary | G0: contracts and acceptance fixtures agreed |
| 1 — Shared Runtime core | AF-01–02 | Target-bound facade, operation coordination, owner-specific cancellation | G1: one owner, no stale writes or cross-owner cancellation |
| 2 — Demand loading and data plane | AF-03–04 | Lazy host, single-flight startup, bounded subscriptions and workers' interface | G2: optional work is dormant and resource bounds are observable |
| 3 — Existing tools and navigation | AF-05–08 | Inspector/Parameters/Motor Test migrated; engineering menu/top-bar shell | G3: tools operate independently with preserved behavior |
| 4 — Execution and monitoring | AF-09–10 | Immutable executed baseline and monitoring independent of navigation | G4: existing validated missions can be observed without a new planner |
| 5 — Isolation and architecture release | AF-11–14 | Isolated jobs/runtime, recovery evidence, extension proof | **G-ARCH: architecture qualified before new Flight Plan/Drape work** |
| 6 — Flight Plan, late | AF-15–16 | Semantic Survey task/editor, generators, payloads, interchange and estimates | G-PLAN: corresponding BL scenarios pass |
| 7 — Drape and Survey closure, last | AF-17 | Height/drape correctness, complete-flight terrain and dependent survey workflows | G-DRAPE: numerical/vehicle acceptance and reviewed dependencies pass |
| 8 — Integrated acceptance | AF-18 | Native/bench/field evidence and release documentation | G-RELEASE: approved scope is demonstrably complete |

Gates are engineering acceptance evidence. No phase begins until implementation
has been authorized after the owner's review of this plan.

## 5. Detailed early work packages

### AF-00 — Inventory and contract baseline

**Deliver:** a service/owner/lifetime inventory, module dependency graph, event and
command catalog, and reference-machine/target fixture definition.

- Trace every connect, borrow, detach, target-change, window-close and quit path.
- Identify eager frontend imports, native task starts, background subscriptions,
  raster/file opens and global mutable caches.
- Record cold/warm startup, time-to-tool, CPU, memory, worker count and tool-close
  cleanup before choosing improvement budgets.
- Review minimal types: `VehicleId`, `VehicleSessionId`, `LinkId`, `ClientId`,
  `ModuleInstanceId`, `TargetRef`, `OperationId`, `RequestId`, `Revision`, subscription
  and operation result contracts.
- Document configured control deadlines and how software/bench evidence verifies them.

**Acceptance:** every current device/service has an identified owner and shutdown
path; all fixtures and performance measurements are reproducible. No full Survey
schema is required to pass G0.

### AF-01 — Runtime facade and target context

**Touch:** `AppState`/`ActiveLink`, primary/dedicated-link adapters and shared types.

- Introduce a narrow, initially in-process facade over existing services.
- Bind calls to the verified target and existing handle/generation identity.
- Expose snapshots/service capabilities without exposing a mutable global store.
- Make main-shell and tool callers use the same target-resolution rules.
- Preserve primary-link borrowing and independent bench/base roles.
- Separate logical vehicle identity from transport identity; primary/selected
  vehicle is a UI/default-selection pointer, not implicit global command authority.

**Acceptance:** target replacement invalidates old requests; a borrowed tool detach
preserves primary transport/cache identity; ambiguous targets cannot inherit authority.

**Forward-compatible design only:** AF-00–02 retain explicit vehicle/target context,
session-scoped mutable state and target-bound events/operations. The near-term
runtime serves one operational flight-aircraft context plus its existing device
roles. A multi-aircraft registry implementation, fleet UI, shared-link multiplexing,
concurrent mission service and multi-aircraft integration campaign are deferred.
Interface preparation is included in AF-00–02's ordinary contract work.

### AF-02 — Operation coordination and cancellation ownership

**Touch:** command/parameter/motor/mission adapters, lifecycle and operation records.

- Associate each consequential operation with its owner, target, resources,
  deadline, cancellation handle and terminal outcome.
- Coordinate conflicting parameter, actuator, calibration, mission and shared
  shell/log operations using existing policies.
- Replace global “cancel current” call sites with owned operation cancellation.
- Preserve the Motor Test default-OFF/generation-bound permission and owned release.
- Prioritize stop/release paths; retain Failed/Uncertain cleanup evidence.

**Acceptance:** cancelling one client's work cannot cancel another's; idle STOP
does not release another owner's outputs; reconnect revokes authority; an old
result cannot complete a replacement operation. Local arbitration is not claimed
to lock external QGC/RC control out of the aircraft.

### AF-03 — Lazy Module Host and Tool SDK

**Touch:** frontend tool entries, `App.tsx`, `RightInspector.tsx`, tool routing and descriptors.

- Register lightweight descriptors: ID, version, loader, required services,
  target capabilities, entry view and state schema.
- Keep separate tool entries; dynamically load heavy main-workspace panels/map.
- Implement single-flight initialization per module/target and loading/error/retry states.
- Distinguish registered code, loaded UI, running service, view reference, operation
  ownership and execution pins.
- Preserve lightweight draft/preferences on close while releasing owned working buffers.

**Acceptance:** no optional tool windows/DSP/DEM jobs at startup without a relevant
configured role; ten concurrent opens yield one instance; cancelling startup or
changing target prevents late attachment; import failure leaves the shell usable.

### AF-04 — Bounded subscriptions and shared data services

**Touch:** telemetry/router, Inspector batching, stream service and runtime subscriptions.

- Provide latest snapshots and bounded columnar batches with sequence/time/age/gap data.
- Keep display decimation independent of required recording/analysis sampling.
- Combine stream-rate demands in one owner-aware service.
- Define queue/cache/job budgets and visible overload behavior.
- Keep CPU-heavy tasks outside control I/O and release-critical locks.

**Acceptance:** saturated consumers cannot create unbounded queues or delay release
beyond the recorded deadline; closing a consumer removes only its stream demand;
resubscription begins from a consistent snapshot/revision.

### AF-05 — Migrate Signal Inspector first

- Move attachment/subscription/analysis lifecycle behind the SDK/facade.
- Retain existing traces, filters, FFT, channel batching and independent window.
- Show explicit target/data age/drop counts; keep per-tool UI preferences local.
- Detach only its tap/pipelines when closed; a main-view change has no effect.

**Acceptance:** existing Inspector checks remain valid; repeated close/reopen does
not grow active subscriptions; analysis overload leaves shared control responsive.

### AF-06 — Migrate Parameters and configuration primitives

- Reuse the current `ParamService`, cache, metadata, staging and echo/read-back.
- Scope pulls/writes/cancellation to operation ownership.
- Preserve confirmed/clamped/rejected/uncertain outcomes and external-change handling.
- Expose these primitives to a future Configuration module without building that
  module's aircraft-specific forms yet.

**Acceptance:** two clients can read; conflicting writes are coordinated; closing
one window does not cancel another client's transaction; interrupted batches are
reconciled rather than reported as atomic success.

### AF-07 — Migrate Motor Test with its existing safety contract

- Use the shared target context and existing dedicated/borrowed runtime semantics.
- Route start/set/stop/tone/configuration through owned operations and existing permission.
- Separate observation, capture and actuation lifetimes.
- Preserve failed release diagnostics and retry state across window loss.
- Keep UI failure handling and FC output-timeout verification as separate evidence.

**Acceptance:** default OFF is passive; no obsolete-generation control reaches a
new target; close/failure revokes authority and releases only owned outputs;
borrowed cleanup preserves main I/O. Native-close and electrical behavior receive
bench evidence after software fixtures pass.

### AF-08 — Navigation and menu shell

**Use:** the existing [navigation concept](ugcs-survey-navigation-concept.svg).

- Introduce global/status and task/stage rows with stable health/Hold/RTL positions.
- Generate Tools entries from descriptors and reuse focus/availability behavior.
- Consolidate application, task and selected-object commands with explicit scopes.
- Keep the current planner accessible through its compatibility entry; defer the
  new Survey outline/editor and Drape controls to the late phases.
- Reuse design tokens, keyboard navigation, field density and existing command confirmation.

**Acceptance:** 1920×1080, 1366×768 and 1024×640 layouts have no horizontal overflow;
battery/Hold/RTL remain visible/reachable; Fly navigation sends no vehicle command;
menu actions have one implementation and focus existing tool windows.

### AF-09 — Mission Execution and immutable baseline adapter

**Touch:** existing mission/command services and transfer adapters; no new plan generator.

- Add `ExecutionBaseline`/snapshot references containing actual wire items, target,
  transfer identity, required height reference and optional trusted source-line mapping.
- Adapt existing explicit missions, imported fixtures or aircraft read-back into this
  contract. Missing Home/datum/source-line information stays explicit.
- Snapshot the sent items; verify read-back against that snapshot, not mutable editor state.
- Coordinate mission intents through the existing aircraft mode/command adapter.
- Record transfer and execution outcomes independently of the displayed stage.

**Acceptance:** edit/retarget during upload cannot change the sent identity; no
verified state precedes matching read-back; transfer failure is recoverable; validated
simple missions work without loading the new Flight Plan or Drape implementation.

### AF-10 — Flight Monitor and recorder lifetime

- Extract monitor ownership from `RightInspector`/React view effects.
- Attach to the frozen executed baseline and timestamped telemetry.
- Reuse existing deviation/recording code; publish metrics through bounded subscriptions.
- Support waypoint/path observation without requiring new semantic Survey generators.
- Report unavailable terrain/line identity honestly; derive richer metrics only
  when the required inputs are present and trusted.

**Acceptance:** switching through all stages and closing charts does not stop the
monitor/recorder or replace its baseline; changing a planning draft cannot change
replay results; stale telemetry is not reported as zero deviation.

AF-09/10 normalize existing protocol execution and observation. The deferred M1
measurement-session state machine, line-aware continuation and M3/QC business
features remain in AF-17e, after their required scope review.

### AF-11 — Worker isolation and resource accounting

- Run heavy analysis/parsing in bounded jobs/workers with input hashes and job IDs.
- Start workers on demand; implement cancellation, cleanup, idle retirement and budget reporting.
- Pin resources required by active operations/monitoring; evict only unowned idle data.
- Use existing analysis/log fixtures to prove the boundary. New Drape algorithms
  and broad raster redesign remain in AF-17.

**Acceptance:** a worker crash or oversized request returns an identified failure;
control/cleanup remains responsive; stale results cannot replace newer work;
resource counts return to their warmed baseline after cleanup.

### AF-12 — Independent Runtime process

- Extract native service composition behind the already tested facade.
- Deliver a local IPC adapter with version handshake, authorized local-client boundary,
  request IDs, bounded data transport and snapshot-based reconnection.
- Enforce one runtime I/O authority; multiple shells/tools cannot create duplicate port owners.
- Separate shell-close, tool-close, runtime shutdown and runtime-crash behavior.
- Restore evidence/current state after restart; never auto-replay actuator/start/calibration intent.
- Package and verify on the selected primary desktop target before expanding platforms.

**Acceptance:** terminate a shell/worker without losing the independent runtime;
sensitive client-held authority expires correctly; runtime restart is reconciled;
API-version mismatch is clear; normal quit obeys the existing operation policy.

### AF-13 — Architecture qualification

- Run the ownership, lifecycle, loading, concurrency, performance and recovery matrix in §8.
- Verify the three existing tools together, including native-window and primary-link borrowing cases.
- Audit dependencies: optional UI/tool code is lazy; tool modules use scoped runtime APIs.
- Remove obsolete adapter paths only after their callers and acceptance cases have migrated.
- Record known legacy Flight Plan/Drape limitations separately from platform results.

**Acceptance:** G-ARCH has reproducible evidence and no unresolved architecture
failure in owner/generation, cancellation, bounded loading, or critical cleanup.

### AF-14 — Prove future extension independence

- Register a test-only configuration/calibration-like module through the descriptor/SDK.
- Exercise read, owned write simulation, progress, cancel, failure and result reporting.
- Prove it needs no new transport, parameter cache, menu-specific dispatcher or
  changes to unrelated tools.
- Review the real Configuration/Calibration procedure scope as a separate product decision.

**Acceptance:** the test module passes against a synthetic target and can be
removed without changing the runtime's unrelated modules.

AF-13's final G-ARCH sign-off includes AF-14's extension proof. Qualification can
run throughout tool migration, but the gate closes only after AF-11/12 isolation
and AF-14 evidence have passed; package numbering is not a reason to sign off early.

**Optional follow-on before the late Survey phases, if approved:** AF-14C implements
one aircraft Configuration workspace; AF-14K implements a defined PX4 calibration
procedure set. Current `AGENTS.md` scope delegates these operations to QGC, so
product scope must be updated before their implementation. The extension proof,
not delivery of every future calibration feature, is required for G-ARCH.

## 6. Final feature phases

### AF-15 — Flight Plan model and editor

Begin only after G-ARCH. Implement persistent semantic tasks/segments, stable line
identity, recipes, variants, overrides and semantic history. Build the new outline,
context inspector and linked selection using the established lazy host/SDK.

**Acceptance:** multi-segment tasks survive save/reopen/undo; insertion and duplicate
operations preserve source identity; normal planning works without Drape loaded.
Reuses SW-06/SW-08 and the Flight Plan design in the Survey proposal.

### AF-16 — Flight Plan tools, payloads and interchange

Add the remaining baseline generators/recipes, double grid/oblique capture, ordered
actions, AOI interchange, route conversion reports and whole-flight estimate inputs.
Camera/LiDAR-specific controls must have implemented effects and identified target contracts.

**Acceptance:** corresponding BL scenarios pass with deterministic geometry/action
fixtures; unsupported capabilities and incomplete data are explicit. Reuses SW-09–11.

### AF-17 — Drape and dependent Survey closure

Split this final package into reviewable slices:

1. **AF-17a Height/command correctness:** close the documented speed, datum/anchor,
   source-line and coordinate-less command issues; keep encoding in the common compiler.
2. **AF-17b Pure Drape contract:** source-versioned DTM/DSM inputs, tagged profiles,
   numerical constraints, cancellation/revision behavior and independent tests.
3. **AF-17c Complete-flight treatment:** approach, turns, overshoots, return and
   landing context; validated simplification and full mission-item accounting.
4. **AF-17d Truthful UI integration:** configured bands/floors, real intermediate
   terrain samples, whole-route/per-line profiles and located issues.
5. **AF-17e Survey-specific execution closure:** line-aware battery continuation,
   QC/reflight and reproducible task manifests, once their existing deferred dependencies
   are authorized. Reuse the already delivered Execution/Monitor runtime.

**Acceptance:** equivalent-datum, ridge/NoData, stale-result, item-limit, actual
trajectory and line-continuation scenarios pass. This package closes the deferred
Flight Plan/Drape findings; architecture qualification alone did not close them.

### AF-18 — Integrated release evidence

Run all agreed BL scenarios and the native/SITL/bench/field campaign against the
final modules. Verify no architecture/lazy-loading regressions, reproducible
snapshots/reports, supported platform packaging and documented remaining scope.
This phase adds acceptance evidence rather than a new feature family after Drape.

## 7. Compatibility and migration strategy

- **Facade before movement:** first route existing commands through scoped service
  interfaces; then relocate implementation. Avoid simultaneous public-API and
  algorithm rewrites in the same slice.
- **One side-effect path:** an operation uses either the old adapter delegating to
  the runtime or its replacement. Read-only comparisons are possible; device writes
  are never duplicated for comparison.
- **Legacy mission adapter:** early Execution/Monitor accept known valid existing
  wire missions and explicit references. New segment generation and terrain design
  are not prerequisites. Optional source metadata remains optional and honest.
- **Versioned persistence:** migrate stored data only when that model's phase lands;
  keep old project/recipe inputs readable and distinguish schema from runtime API versions.
- **Rollback:** restore the prior compatible adapter/UI only after owned operations
  reach a reconciled state. A failed release is not erased to make rollback appear successful.
- **Crash/reconnect:** retain journals, derive current FC state again, revoke old
  client authority, and require a new valid operation context for consequential writes.
- **Adapter retirement:** remove obsolete paths only when callers, types, tests and
  documented user behavior have migrated. Preserve unrelated ongoing serial/tool work.

## 8. Architecture acceptance matrix

| Case | Required result |
| --- | --- |
| Ten concurrent opens of one tool/target | One module instance and required attachment; idempotent focus |
| Close/reload Inspector with other clients active | Its subscriptions stop; shared transport and other clients continue |
| Cancel Parameters operation A while B is active | Only A is cancelled; B's state/echo remains correctly correlated |
| Close Motor Test during pending/running/failed-release states | Permission revoked; only owned cleanup; failure/uncertainty retained |
| Main link borrowed by Motor Test, then tool detach | Same primary handle/cache remains operational |
| Replace target during initialization or a write | Old-generation completion/write cannot affect the new target |
| External QGC parameter/mode change | Revalidation/reconciliation; no false global-exclusivity claim |
| Startup with automatic roles disabled | No optional tool windows, DSP, DEM reads or calibration/motor sessions |
| Startup with a configured native role enabled | Only that role's documented connection behavior starts |
| Repeated warm open/close cycles | Owned subscription/task/buffer counts return to baseline without growth |
| FFT/log/analysis load at configured capacity | Bounded memory/queue behavior; control/release meets recorded deadlines |
| Workflow navigation during active execution | Monitor/recorder and their frozen baseline remain active |
| Delayed job result after input/target replacement | Result discarded or reconciled by its identity, never applied blindly |
| Worker/shell/runtime failure | Each stated isolation boundary demonstrated separately |
| Reopen after interrupted operation | Restore evidence/current state, not actuator/start/calibration intent |
| Test-only extension module | No modifications to unrelated tools or transport protocols |
| Primary flight-target disconnect with an independent base/source active | Preserve that role's owned session and report flight-side injection/attachment state accurately |

Record fixture, target, deadline, before/after resource counts, result and evidence
path for each case. Unit/PTY/browser checks do not substitute for native process,
FC reboot or electrical-output evidence where those behaviors are claimed.

## 9. Existing issue allocation under the new order

| Earlier finding | New placement |
| --- | --- |
| F01 Drape speed parameter and fixed modeled speed | AF-17a; early runtime tests use explicit validated mission fixtures |
| F02 Drape datum/anchor merge | Minimal typed-reference contract early; actual Drape path correction in AF-17a |
| F03 Drape line re-anchoring | Define optional stable source IDs early; rewrite generator/drape mapping in AF-15/17 |
| F04 Drape calculation revision/cache loop | Generic job lifecycle in AF-03/11; Drape-specific correction in AF-17b |
| F05 Mutable upload/read-back identity | AF-09, because it is an execution-runtime responsibility |
| F06 Incorrect profile constants/endpoint interpolation | AF-17d |
| F07 Target limits and incomplete facts | Target capability/availability contracts in AF-01/09; full-route estimates and Drape facts in AF-16/17 |
| F08 Incomplete flight-path terrain treatment | AF-17c |

Until the late phases pass, legacy terrain-generated missions retain their known
validation limitations. Early architecture acceptance uses explicit validated
missions/fixtures and does not serve as flight clearance for those legacy paths.

## 10. Dependency and staffing plan

The critical path is:

`AF-00 → AF-01/02 → AF-03/04 → existing-tool migration → AF-09/10 → AF-11–14 → AF-15/16 → AF-17 → AF-18`

After contracts are reviewed, frontend loader/menu work can overlap native facade
implementation. Inspector and parameter migration can overlap once ownership
contracts stabilize. Motor Test follows those foundations. Isolation packaging
can be prepared early, but moving I/O authority follows verified service boundaries.

Recommended roles:

- Rust/runtime lead: ownership, protocol semantics, operations, process boundary.
- React/Tauri engineer: lazy UI, Tool SDK consumers, menus, window and layout behavior.
- Integration/GIS engineer: adapters, bounded data/worker work, fixtures, later Survey/Drape.
- Part-time QA/native/bench support, interaction design, and aircraft/geophysics review.
- Owner/operator: scope, target-platform and acceptance decisions.

## 11. Updated effort and resource assessment

One engineering person-week is five focused days and includes implementation-level
tests/review/docs. Dedicated QA/design/domain effort is listed separately.

| Package | Engineering person-weeks |
| --- | --- |
| AF-00 | 0.5–1 |
| AF-01 | 1–2 |
| AF-02 | 1.5–3 |
| AF-03 | 1–2 |
| AF-04 | 1–2 |
| AF-05 | 0.5–1 |
| AF-06 | 1–2 |
| AF-07 | 1.5–3 |
| AF-08 | 1–2 |
| AF-09 | 1.5–3 |
| AF-10 | 1–2 |
| AF-11 | 1–2 |
| AF-12 | 3–6 |
| AF-13 | 1–2 |
| AF-14 | 0.5–1 |
| **Architecture subtotal, AF-00–14** | **17–34** |
| AF-15 | 6–10 |
| AF-16 | 8–15 |
| AF-17 | 13–25 |
| AF-18 | 2–4 |
| **Late feature/release subtotal** | **29–54** |
| **Complete engineering total** | **46–88** |

### 11.1 Architecture-first approval envelope

For AF-00–14, allow another **8–14 person-weeks** for dedicated QA/native/bench
work, design and domain/owner review. Base effort is **25–48 person-weeks**;
with 20–30% integration/rework contingency, budget approximately **30–63 person-weeks**.
With the three engineering roles plus part-time support, plan approximately
**3–5 months** for the architecture-qualified milestone; menu/loading improvements
arrive as earlier slices rather than waiting for the final feature phases.

### 11.2 Full program and optional scope

- Full engineering plus dedicated supporting work of **17–31 person-weeks** is
  **63–119 person-weeks** before contingency.
- If the deferred M7/M8 dependencies are included for full QC closure, allow **+4–8
  engineering person-weeks**. Including them and 20–30% contingency gives a planning
  envelope of approximately **81–166 person-weeks**.
- Indicative full calendar duration is **8–12 months** for the recommended team,
  assuming timely decisions, available target equipment and eventual authorization
  of deferred Survey work. It is not a fixed-date commitment.
- Optional AF-14C/14K real Configuration/Calibration delivery adds approximately
  **6–12 engineering person-weeks**, plus its dedicated bench/QA effort. It is not
  included in the architecture-only gate or the totals above.

This is a new roll-up of the same SW/TM scope plus explicit architecture/release
work. **Do not add the earlier Survey and tool totals to these numbers.** Recalibrate
after AF-00 measurements and the first facade/tool migrations.

### 11.3 Equipment and data timing

- **Architecture phases:** developer workstations, CI/SITL, the primary native desktop
  platform, a representative PX4 bench target, USB/telemetry paths, and fixtures for
  protocol, ownership, window and process failures. Reuse existing motor/RTK assets.
- **Monitor/execution qualification:** validated simple missions, read-back snapshots,
  timestamped telemetry/replay and known coordinate/height references; no new Drape
  generator is required.
- **Late Flight Plan/Drape:** representative DTM/DSM/control-point fixtures, geoid
  grids, camera/LiDAR contracts/data, and planned field sessions over relevant terrain.
- **Optional Configuration/Calibration:** agreed aircraft/procedure list and matching
  hardware/bench observation. Hardware-specific confirmation follows software fixtures.

Use actual loaded personnel rates and the missing-equipment inventory for monetary
budgeting. The main technical uncertainties are native process packaging, target
reboot/identity behavior, shared-protocol ownership and late terrain/flight fidelity.

## 12. Mapping to prior work packages

| New ownership | Prior scope absorbed |
| --- | --- |
| AF-00–04 | TM-01–04 foundations, generic revision/ownership portions of SW-04/05 |
| AF-05–07 | Existing-tool migration aspects of TM-02–04 |
| AF-08 | SW-14A and module-driven menus |
| AF-09–10 | Execution snapshot/monitor-lifetime portions of SW-04/05/12/14B |
| AF-11–14 | TM-04/05, extension proof; TM-06/07 only through optional AF-14C/K |
| AF-15 | SW-06/08 and semantic source-ID application |
| AF-16 | SW-09–11 |
| AF-17 | SW-01–03, Drape portions of SW-04/07, remaining SW-12/13 and reviewed M dependencies |
| AF-18 | Cross-module release qualification; reuses all accumulated fixtures |

The split of a prior package is intentional: for example, immutable transfer
identity is early, while the Drape calculation cache is late. Every detailed
implementation ticket must name one AF owner to prevent double-counted work.

## 13. Owner review and first authorized slice

Review:

1. The architecture-first order and G-ARCH boundary before Flight Plan/Drape.
2. The primary desktop/aircraft target and separate-Runtime process milestone.
3. Mandatory extension proof versus optional real Configuration/Calibration scope.
4. Runtime operation/permission/lazy-loading contracts and existing ADR compatibility.
5. Single-flight-target delivery now, with only interface-level future fleet compatibility.
6. The architecture-only staffing/effort envelope and late feature budget.

After authorization, begin with **AF-00 only**: inventory, baseline measurements,
minimal contract/error signatures and a concrete AF-01 acceptance fixture. Follow
the repository rule of one small reviewed task at a time.

This document is a planning artifact. No AF implementation, application migration,
mission operation or hardware test has been executed by creating it.

## 14. Future multi-aircraft extension

### 14.1 Feasibility and the boundary to reserve now

**Yes: the architecture can later support multiple aircraft by retaining explicit
vehicle/target context and avoiding unqualified global state.** The current owner
decision is to prepare the architecture only and not develop multi-aircraft support
in the near term. The `ActiveLink` map is a useful starting point, but it is a link
collection rather than a complete fleet session model.

Separate these identities:

| Identity | Meaning |
| --- | --- |
| `VehicleId` | Logical aircraft identity registered by the application |
| `VehicleSessionId` | Current runtime/flight context; does not by itself prove a physical FC boot identity |
| `LinkId` + transport generation | A specific I/O connection instance and its replacement boundary |
| MAVLink system/component IDs | Routing addresses within a verified link/network context, not globally unique physical identities |
| `FlightSessionId` / `ExecutionId` | One aircraft's actual mission execution and records |
| `SelectedVehicleId` | The UI focus/default selection; no authority to retarget existing operations |

Use verified identity evidence and explicit bindings. A missing/duplicate UID or
equal system IDs must not silently merge two aircraft. Preserve the existing
reboot/identity investigation requirements from ADR-028.

### 14.2 Runtime model

The following is a **future topology sketch**, not a requirement to instantiate
multiple operational aircraft in this delivery. The current implementation
contract may serve a single flight-aircraft context behind the same scoped APIs.

```text
Runtime
  TransportRegistry: LinkId → I/O owner / generation
  VehicleRegistry: VehicleId → VehicleContext
    identity and verified routes
    telemetry and parameter values
    command/mission operations and resource claims
    execution baseline and monitor
    recorder/session references
    height-anchor and sensor-calibration bindings
  RouteTable: VehicleId ↔ verified LinkId + MAVLink target + generation
  Shared immutable resources: DEM tiles / geoid grids / profile definitions

Shell: SelectedVehicleId
Tools: explicit pinned VehicleContext
```

Avoid baking a permanent one-aircraft-equals-one-port assumption into public types.
Implementing one-aircraft/multiple-link or shared-link/multiple-aircraft routing is
future work with its own acceptance; the short-term system remains within its
supported single-flight-target topology.

Mutable parameter values, the upload hash, command progress, recorder, drape
monitor, estimator-height anchor and active calibration binding must not be one
unqualified global value. Immutable DEM/geoid/profile resources may be shared,
with per-execution pins and budgets.

Shared independent devices, such as an RTK base, retain their own source-session
ownership. Multiple vehicles may consume that source while keeping separate
injection state, target routing, freshness and bandwidth accounting.

### 14.3 Commands, events and tools

- Every operation names `VehicleId`, verified target/route, session/generation and
  operation/request ID. ACK and mission-current correlation includes vehicle context.
- Each vehicle has independent protocol/operation state; short registry locks do
  not become long global locks around one aircraft's configuration or cleanup.
- A tool opened for aircraft A stays pinned to A when the main map selects B.
  Any follow-selection behavior is explicit and appropriate to that tool's authority.
- Telemetry/events include vehicle identity, source link, timestamps and revision.
  Boot-time/UTC mappings are per aircraft; log streams cannot be merged by bare sequence number.
- Read-only link selection and control-link replacement have different policies.
  Motor/write authority is not automatically inherited across a new connection.
- A future group operation contains explicit child targets and per-aircraft
  outcomes. Partial failure is visible; it is not represented as an atomic all-aircraft success.

### 14.4 Extension stages

| Stage | Scope | Relationship to the current plan |
| --- | --- | --- |
| MV0 — design readiness | Explicit target/vehicle context, session ownership and extensible interfaces | Included in ordinary AF-00–02 design; no separate multi-aircraft development package |
| MV1 — multi-aircraft observation | Multiple connections, aircraft list, independent health/tracks/logs | Later product extension on the qualified runtime |
| MV2 — independent missions | Separate plans/uploads/executions/monitors per aircraft; explicit targeted commands | Reuse late Flight Plan/Drape and per-vehicle Execution |
| MV3 — coordinated surveying | Task/line allocation, sortie scheduling, airspace-conflict handling and coordinated interruption policy | Separate Fleet Coordinator and dedicated acceptance scope |

Multi-aircraft observation, independent missions and coordinated flight have
different acceptance requirements. MV1/MV2 alone do not establish swarm control
or automatic collision avoidance. The current Flight Plan/Drape-last order remains;
MV1–MV3 are future product phases, not silently inserted ahead of it.

### 14.5 Deferred multi-aircraft acceptance and capacity

Current gates test the supported single flight target, target replacement, stale
requests and existing independent device-role ownership. Two-aircraft end-to-end
tests and simultaneous mission validation are not part of the near-term gate.

When fleet work is authorized later, add two-aircraft fixtures with different
battery/mode/parameter values, interleaved equal command/mission sequence values,
selection changes during operations and independent cancellation/disconnect.
Shared-network identity ambiguity and routing topology then receive dedicated tests.

Later capacity targets must state aircraft count, telemetry rates, link topology,
active monitors and recording load. Use per-aircraft quotas plus a global budget;
background display can decimate without suppressing required monitoring/recording
or critical health events. Do not claim an unlimited aircraft count from the map type.

Minimal interface preparation is included in AF-00–02. Fleet implementation,
simultaneous field validation and coordination will be separately planned after
the owner authorizes that scope and selects an aircraft count/topology.
