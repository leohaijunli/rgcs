# Tool Modules and the Shared Vehicle Runtime

- **Language:** English | [简体中文](tool-module-architecture-plan.zh-CN.md)
- **Date:** 2026-10-25
- **Status:** Detailed architecture proposal; implementation is on hold for owner review.
- **Execution order:** The [architecture-first master plan](architecture-first-delivery-plan.md)
  assigns TM/SW scope to AF packages. Near-term delivery is single-flight-target;
  future multi-aircraft compatibility is reserved at interface level only. Use the
  master plan's order and combined estimates for scheduling.
- **Related:** [Survey workspace](ugcs-survey-workspace-plan.md),
  [current architecture](../ARCHITECTURE.md), ADR-025/026/028.

## 1. Recommendation

Use **one authoritative vehicle/device runtime with independently developed tool
modules and independently openable workspaces**. Keep the main application as the
project/workflow shell. Tools access shared services through a typed runtime API.

Use three explicit independence boundaries:

1. **Module boundary:** each tool owns its domain logic, UI state, tests, and result
   schema; shared services have narrow interfaces.
2. **Workspace boundary:** Inspector, Motor Test, Parameters, and future setup/
   calibration can open, close, and recover independently of the main view.
3. **Process boundary:** heavy decoders and extensions can run in workers; a later
   separate runtime process keeps device/session services alive through shell failure.

Separate webviews already provide useful UI separation, but a fatal failure of
their shared native host still affects them together. A separate runtime process
is a distinct delivery step, not a property that existing windows automatically have.

## 2. Existing foundations and migration constraints

| Existing foundation | Source / implication |
| --- | --- |
| Main, Inspector, Actuator, and Parameters have separate frontend entries/webviews. | `docs/ARCHITECTURE.md`; tool `*_open` commands focus existing windows. |
| Process-wide state and per-link services already exist. | `app-tauri/src/state.rs`: `AppState`, `ActiveLink`, `LinkId`, lifecycle serialization. |
| Inspector uses a tap and batched Tauri Channel; DSP is in Rust. | `inspector_service.rs`, `core/signals`, `core/inspector`, `core/dsp`. |
| Motor control permission is target/generation-bound, default OFF, and revoked on close/change. | [ADR-025](../adr/025-motor-control-permission.md). Reuse this policy. |
| Motor Test can borrow the primary serial connection and parameter service without owning their shutdown. | [ADR-026](../adr/026-main-serial-target-reuse.md). Dedicated bench targets remain separately identified. |
| Serial inventory and leases are being consolidated. | [ADR-028](../adr/028-shared-serial-discovery.md), `core/devices/lease.rs`; respect the staged wiring work. |
| Headless runtime reuse is already an architectural direction. | `crates/server` and the current architecture describe a planned server; this is not an already deployed runtime daemon. |
| Several tool close handlers currently stop/cancel service work. | Inspector detaches its tap; Motor Test revokes/stops its session; Parameters cancels the current primary parameter service operation. Cancellation needs explicit operation ownership as services gain more clients. |

This proposal evolves those mechanisms. In particular, it should not create a
second port registry, parameter cache, motor permission switch, or MAVLink stack.

## 3. Target relationship

```text
Main shell                         Independently openable tools
Project / Survey / workflow        Inspector  Motor Test  Configuration  Calibration
Menus / resources / summaries      local UI state, charts, forms, procedure guidance
             \                     /       |          |          /
              Typed Tool SDK / runtime facade / revisioned subscriptions
                                      |
                  Shared Runtime (one authoritative owner)
       +-----------------------------------------------------------+
       | target identity + transport registry + serial leases      |
       | telemetry / stream-rate arbitration / parameter service  |
       | mission / command / operation sessions + resource claims |
       | recorder / projects / alerts / lifecycle / recovery      |
       +-----------------------------------------------------------+
                 |                         |
           Aircraft / devices        Bounded worker interfaces
                                     ULog / DEM / costly analysis
```

Initially, the runtime facade can live inside the existing Tauri native process.
Once the interfaces and lifetime rules are stable, its composition layer can move
to a dedicated runtime process. Both placements expose the same logical API.

### 3.1 Main shell responsibilities

- Project/task selection, Survey workflow, resources, window routing, and shared appearance.
- Display aggregate status and the active target/session for every tool.
- Request operations through the same service APIs used by the tools.
- Coordinate normal application shutdown through the existing lifecycle policy.

### 3.2 Shared runtime responsibilities

- Own physical I/O, target verification, connection generations, protocol services,
  command coordination, and operation cleanup.
- Supply authoritative state and per-field age; arbitrate shared changes such as
  stream rates and parameter writes.
- Keep recording and active flight monitoring independent of which view is selected.
- Apply operation-specific admission rules and retain uncertain/failed outcomes.
- Publish bounded, revisioned events and retain snapshots for client resynchronization.

### 3.3 Tool responsibilities

- Own its workspace, interaction model, calculations specific to its domain, and report.
- Declare required services and operation capabilities.
- Attach to an explicit target; obtain snapshots/subscriptions or an operation session.
- Show target, connection origin, mode, ownership, progress, and recoverable errors.
- Release its own subscriptions/operations when appropriate, without closing
  borrowed transport or cancelling another module's work.

### 3.4 Survey domain modules

Flight Plan, Drape, Mission Execution and Flight Monitor are separate domain
modules composed by the Survey workspace, using the same shared runtime services
as the independent tools. Their responsibilities, input/output contracts and
one-directional dependencies are specified in [Survey plan section 12](ugcs-survey-workspace-plan.md#12-flight-plan-drape-execution-and-monitor-modules).
Module, window and process boundaries remain separate decisions.

## 4. Tool-specific contracts

| Tool | Backend contract | Independence / coupling rule |
| --- | --- | --- |
| Signal Inspector | Read-only telemetry subscriptions; DSP/analysis session; separate stream-rate requests if needed. | Closing a chart removes that subscriber. Slow plotting never back-pressures flight control. Stream-rate changes are shared device operations, even when the chart itself is read-only. |
| Motor Test | Existing motor-control permission plus an owned actuator operation and deadline/watchdog. | Share verified connection/services when borrowed. Stop/release applies only to owned outputs. UI loss/reconnect revokes authority; reopening starts monitor-only. |
| Aircraft Configuration | Parameter snapshot, staged changes, capability validation, owned write transaction, echo/read-back and reboot reconciliation. | Reuse the parameter service. Independent UI; coordinated writes. A partially applied batch is reported as partial, not as an atomic transaction. |
| Aircraft Calibration | Firmware-specific procedure driver, exclusive operation context, progress observation, cancellation and terminal-state reconciliation. | Runtime owns procedure state. The wizard renders actual state; closing a page cannot imply the aircraft calibration stopped successfully. |
| Logs / DEM / analysis | Identified data jobs with progress/cancellation and versioned outputs. | Expensive parsing can move to workers. Its memory/CPU failures must not block device release or mission commands. |

**Scope note:** The current `AGENTS.md` delegates initial aircraft configuration and
flight-controller sensor calibration to QGroundControl. The owner's request adds
these as future product candidates. Their implementation requires a reviewed scope
update. Existing geophysical calibration data and Survey calibration maneuvers
are separate from flight-controller setup/calibration.

## 5. Ownership, conflicts, and concurrency

### 5.1 Identity

Bind attachments and operations to the existing verified target reference and
`(LinkId/handle identity, transport generation)`. A port name is not a vehicle
identity. Borrowing a link preserves its actual I/O owner.

Detecting a flight-controller reboot may require additional evidence even when a
transport remains open. Follow ADR-028's LINK-05 investigation before introducing
a new boot-epoch contract. An unresolved target must not inherit write authority.

For later multiple-transport support, arbitrate control by verified target, not
just by port. Two connections to the same aircraft must not create two independent
local actuator/configuration owners. Do not infer equivalence from an ambiguous ID.

### 5.2 Resource claims

Use operation-scoped claims, initially with conservative per-target exclusivity:

| Resource | Sharing rule |
| --- | --- |
| Telemetry read | Many subscribers; bounded buffers and independent decimation. |
| Physical serial transport | One I/O owner; borrowed handles do not own shutdown. |
| Stream-rate configuration | One arbitration service combines subscriber demands and link budgets. |
| Parameter writes | One coordinated writer for conflicting sets/batches; reads/cache remain usable. |
| Actuator control | Existing Motor Test ownership/permission rules, coordinated with other target operations. |
| Calibration / setup procedure | Exclusive where the procedure changes mode, calibration state, or related parameters. |
| Mission replacement/start | Coordinated against the active target/session and conflicting calibration/actuator operations. |
| Shell / FC log transfer | Reuse existing shared-protocol exclusions; cancellation belongs to the operation owner. |

A “busy” response names the owner, operation, and possible next action. Stop and
release paths are not queued behind ordinary configuration, log download, or DSP.

These claims coordinate RGCS clients. They cannot lock an external QGC or RC out
of the aircraft. External mode/parameter changes must cause the affected operation
to revalidate, reconcile, or stop according to its contract.

### 5.3 Lifecycle rules

| Event | Expected effect |
| --- | --- |
| Switch Survey stage | Change the view only; active recording/monitoring keeps its frozen flight baseline. |
| Close Inspector | Cancel only its subscriptions/analysis job; preserve transport and other consumers. |
| Close Motor Test | Revoke its permission immediately; run owned cancellation/release; retain any release failure for retry/diagnosis. |
| Close Configuration UI | Preserve draft changes as data; cancel only owned pending work; reconcile any writes already sent. |
| Close Calibration UI | Follow the procedure's detach/cancel policy; continue showing its actual runtime state elsewhere. |
| Change/reconnect target | Invalidate old operation context; no old-generation write can reach the replacement target. |
| UI renderer failure | Expire sensitive client-held operation leases; keep independent subscriptions/services functioning where the native host survives. |
| Separate shell process failure | A delivered independent runtime can preserve flight observation/recording; actuator client authority still expires. |
| Runtime failure | The FC's own configured timeouts/failsafes remain the final boundary. On restart, reconcile state and logs; do not replay motor/start/calibration intent automatically. |
| Normal Quit | Use the explicit lifecycle policy and known operation state; distinguish quitting the application from closing a tool. |

## 6. Extension and API design

### 6.1 Tool descriptor

A registered tool declares:

- Stable tool ID, version, label/icon, entry view and window preferences.
- Required device roles and services.
- Read/operation capabilities and supported target profiles.
- State/report schema versions and available migrations.
- Isolation level: workspace-only, analysis worker, or independently launched client.

The Tools menu is generated from those descriptors. Tool launch is idempotent:
focus the existing workspace for its instance/target rather than opening duplicates.
Main menus and tools use the same command availability descriptions.

Start with compiled-in trusted modules and stable service boundaries. A third-party
extension interface should use versioned process IPC when introduced; module
independence does not require a dynamically loaded Rust ABI.

### 6.2 Narrow runtime API

Proposed interface sketch for review, not implemented code:

```rust
pub struct ToolAttachment { /* tool instance + verified target reference */ }
pub struct OperationRequest { /* kind + target + expected revision + resources */ }
pub struct OperationHandle { /* operation ID + generation + cancellation authority */ }

pub enum RuntimeError {
    TargetUnavailable,
    StaleTarget,
    UnsupportedCapability,
    ResourceBusy,
    PermissionRequired,
    Cancelled,
    OutcomeUncertain,
    IncompatibleApiVersion,
}

pub trait ToolRuntime {
    // Signatures are illustrative; async/event transport is finalized per adapter.
    fn attach(&self, request: AttachRequest) -> Result<ToolAttachment, RuntimeError>;
    fn begin(&self, request: OperationRequest) -> Result<OperationHandle, RuntimeError>;
    fn cancel(&self, operation: OperationHandle) -> Result<(), RuntimeError>;
    fn snapshot(&self, attachment: &ToolAttachment) -> Result<ToolSnapshot, RuntimeError>;
}
```

`AttachRequest` and `ToolSnapshot` are proposed contract types too. Production
commands use typed payloads and explicit target/session IDs, not arbitrary raw
serial writes from a tool UI. Keep request IDs, revisions, terminal outcomes, and
cancellation ownership in the protocol. Expected revisions are local concurrency
checks; they do not make a multi-parameter FC write atomic.

### 6.3 Data and command planes

- **Commands:** bounded, acknowledged requests with identity, ownership, deadlines,
  and terminal result. Reuse existing control services and release semantics.
- **Data:** bounded columnar batches or latest snapshots, with sequence, timestamps,
  dropped-sample counts, and field age. Consumers can independently decimate.
- **Resynchronization:** reconnect with a snapshot/revision, then resume events;
  consumers do not infer state from a partially missed event stream.
- **Workers:** request/result envelopes include input hashes and job IDs; stale
  results cannot replace a current plan or operation.

Initially use Tauri invokes/Channels behind the facade. A later process adapter
can use local IPC with a version handshake and an explicit authorized local client
boundary. Protocol/API isolation and process isolation must be tested independently.

## 7. Configuration and calibration details

### 7.1 Configuration

1. Attach to an explicit target and read its firmware/capabilities/current parameters.
2. Edit a local draft; show baseline/requested values and parameters needing reboot.
3. Validate the batch, acquire its write context, and send through `ParamService`.
4. Distinguish confirmed, clamped, rejected, pending, and uncertain values.
5. On interruption, reread/reconcile. Roll back only values whose restoration is
   supported and verified; otherwise preserve a clear partial-result report.
6. Reboot is an explicit operation, followed by target verification and read-back.

The existing Parameters window becomes the reusable low-level editor. A future
configuration workspace adds aircraft-specific forms and procedures above it,
without creating another parameter protocol/cache.

### 7.2 Calibration

Use a procedure driver per supported firmware/procedure family:

`Prepare → Acquire operation → Start → Observe progress → Verify → Complete`

Every active state also supports the documented cancellation/failure branch.
Progress comes from FC feedback correlated to the operation, not from pressing
Next in the UI. If the firmware cannot confirm cancellation/completion, the state
is Uncertain until reconciled. Store procedure type, target/firmware, relevant
before/after values, timestamps, and result in its report.

The runtime checks fresh target state and conflicts before each consequential
step. Reuse Motor Test's permission model for actuator-related operations rather
than inventing another independent “unlock” path.

## 8. Migration slices and resources

| Package | Scope | Engineering effort, person-weeks |
| --- | --- | --- |
| TM-01 | Runtime facade, target/session contracts, ownership inventory | 1.5–3 |
| TM-02 | Tool descriptors, narrow frontend SDK, existing-window adapters | 1–2 |
| TM-03 | Operation-scoped conflicts/cancellation, permission reuse, watchdog contracts | 2–4 |
| TM-04 | Bounded subscriptions, navigation-independent lifetimes, fault-injection suite | 1.5–3 |
| TM-05 | Separate runtime / worker process adapter, supervision, recovery and packaging | 4–8 |
| TM-06 | Configuration workspace above existing parameter services, one target family | 2–4 |
| TM-07 | Calibration drivers/UI/reports for a defined PX4 procedure set | 4–8 |

- **Existing-tool modularization (TM-01–04): 6–12 engineering person-weeks.**
- **Stronger process boundary (TM-05): an additional 4–8.**
- **Future configuration/calibration (TM-06–07): an additional 6–12.**
- The complete architecture/tool expansion is **16–32 engineering person-weeks**
  before dedicated QA/field work and contingency. Approximately 2–4 person-weeks
  may overlap Survey SW-04/SW-05/SW-14B and must be assigned once during detailed planning.

These are early estimates for the existing stack and one target-firmware family,
not a quote for all aircraft or calibration procedures. Define the calibration
procedure list before turning TM-07 into a delivery commitment. Reuse the Survey
resource estimate's core/Frontend/QA roles; add native IPC/packaging expertise for
TM-05 and an aircraft-domain engineer/bench operator for TM-06/07.

## 9. Required robustness evidence

1. Open/close/reopen each tool while other tools subscribe; transport and unrelated
   operations retain identity and continue correctly.
2. Saturate Inspector data/FFT workload; measure bounded memory, dropped-data
   reporting, and unaffected command/release scheduling.
3. Kill/reload a Motor Test UI; verify permission revocation and owned-release
   behavior against the configured deadline, then verify electrical behavior on a bench.
4. Cancel a parameter pull/write from one client; another client's operation is
   not cancelled. Partial writes remain explicitly partial/uncertain until read-back.
5. Reconnect/replace the target during every operation state; stale requests are refused.
6. Inject external QGC/FC parameter or mode changes; active procedures reconcile
   rather than silently assuming exclusive control of the aircraft.
7. Switch every main workflow stage during an active drape monitor/recording;
   neither the service nor its frozen baseline is replaced by navigation.
8. Crash an analysis worker; the runtime returns a failed job and continues serving
   flight/actuator cleanup. Separately crash the shell/runtime to test actual process boundaries.
9. Restart with an interrupted operation journal; restore evidence and current
   state, never automatically replay an actuator/start/calibration command.
10. Register a test-only tool against the SDK without changing transport,
    parameter, lifecycle, or unrelated tool implementations.

Physical output release, FC reboot behavior, and native window/process failures
require their own bench/packaged-app evidence. Existing tests and ADR records are
foundations; no such experiments were run by this documentation task.

## 10. Review decisions

Before implementation, review the runtime ownership/API boundaries, the first
process-isolation milestone, the configuration/calibration scope extension, and
the resource-conflict matrix. Then split one package into concrete signatures and
acceptance cases. This document does not authorize starting those packages.

## 11. Lazy loading, efficiency, and robust lifetimes

### 11.1 Small resident core, demand-started features

Keep a lightweight shell and the necessary runtime coordinators resident. Register
tool metadata at startup; initialize implementations, buffers, data files and
workers when their capability is needed.

| Component | Start trigger | Retention / stop condition |
| --- | --- | --- |
| Shell, tool catalog, project metadata, alert/lifecycle coordinators | Application startup | Application/runtime lifetime; small state only. |
| Device inventory and configured automatic connections | Enabled native connection policy | Owned by the configured role; independent of whether a tool window is visible. |
| Per-link telemetry/command services | A verified connection | Link/session lifetime; preserve required heartbeat/command behavior. |
| Flight Plan UI and generators | Open Plan/create/edit a task | Draft persists as data; idle UI/work buffers can be released. |
| Cesium/map implementation | Open a workspace that needs the map | A map view/resource lease; StartPage does not require the full map bundle. |
| Drape/DEM work | Terrain mode or explicit terrain/profile calculation | Versioned cancellable job; retain referenced profiles/tiles under a bounded cache. |
| Flight Monitor and recorder | Active execution policy / prepared executed baseline | Execution-owned pin; navigation and panel closure do not stop them. |
| Inspector UI/DSP | Open Inspector and request signals/analysis | Subscribe only to selected signals; release its tap/pipelines when unused. |
| Motor Test | Open/attach for observation; explicit control operation for actuation | Observation and actuation lifetimes remain separate; actuation obeys existing permission/deadline rules. |
| Configuration | Open relevant parameter/form sections | Load necessary metadata/values; preserve draft data; an active write has its own operation lifetime. |
| Calibration | Open the selected procedure / explicitly begin it | Procedure owns its runtime state until a verified terminal outcome. |
| ULog/DEM/large analysis workers | Submit a job | Bounded pool; idle worker termination and cache eviction follow resource policy. |

This preserves existing independent RTK/base and auto-connection policies: hiding
their UI does not revoke an intentionally enabled runtime role.

### 11.2 Frontend loading boundary

- Retain separate Vite/webview entries for tools; do not create hidden tool windows
  at startup just to make them look available in the menu.
- Use dynamic imports for heavy workspaces/components, with loading/error/retry
  boundaries. The tool descriptor contains a loader reference rather than an
  eager import of every tool implementation.
- `App.tsx` currently imports MapView, and `RightInspector.tsx` imports its stage
  panels statically. Inspect the generated bundle before claiming a startup gain;
  split heavy imports where the dependency graph actually permits it.
- Keep shared design tokens and the small SDK inexpensive. Loading an Inspector
  client should not pull in Cesium or every Survey editing component.
- A failed import/render leaves the shell and other tools usable and reports a
  retryable module error. A React error boundary does not claim to contain a native
  WebView or shared-host crash; that is the process-isolation milestone.

Unmounting a React view releases its instances/subscriptions, not necessarily its
cached JavaScript module. Native Rust modules compiled into one binary are not
started services, nor are they dynamically unloadable just because a panel closes.
Efficiency comes first from deferring initialization, data loading, computation,
subscriptions and worker creation; actual code/process reclamation follows the
appropriate webview/worker/process boundary.

### 11.3 Runtime lifecycle and single-flight initialization

Use an explicit service lifecycle:

`Dormant → Starting → Ready/Active → Idle → Stopping → Dormant`

Starting may fail with a retryable error. Lifecycle changes carry an instance ID,
target generation and revision.

- Concurrent requests for the same service/target share one initialization attempt.
- Cancelling/closing during initialization prevents a late result from resurrecting
  an obsolete tool or attaching to a changed target.
- Separate **view references**, **operation owners**, and **execution pins**. Releasing
  the last view is not sufficient to stop an active calibration or flight monitor.
- Idle eviction requires no active owner/pin, completed cleanup, and a valid resource
  policy. A failed release retains diagnostic/ownership state for reconciliation.
- Reopen from a runtime snapshot, then subscribe. Do not start a second protocol
  reader, parameter cache, stream supervisor, or device probe for the same owner.

### 11.4 CPU, I/O, and memory budgets

- Put CPU-heavy work on a bounded worker pool, outside the async I/O/control path.
  Stop/release scheduling must not wait behind FFT, DEM parsing, file compression,
  or a long-held UI/analysis lock.
- Route telemetry once, then provide bounded per-consumer batches/snapshots.
  Decimate display independently of the required recording/analysis rate; report
  dropped/gapped data rather than silently treating it as complete.
- Combine stream-rate requests in the existing shared service; remove only the
  closing consumer's demand and preserve other owners' requirements.
- Cache terrain by source/version/area under a declared memory budget. Prefer
  windowed/tile access for large rasters. An actively flown baseline and its needed
  resources remain pinned; a large draft job cannot silently evict them.
- Keep worker/job queues bounded and expose queued/running/cancelled/failed status.
  Reject or defer excess work explicitly instead of allocating without limit.
- Store lightweight recipes/history references separately from large derived buffers.
  Closing a graph releases its history buffers; reopening can request bounded replay.

### 11.5 Efficiency acceptance

1. With automatic roles disabled, startup creates no optional tool windows, DSP
   pipelines, DEM reads, calibration sessions or motor-control sessions. With an
   automatic role enabled, only that role's documented behavior is started.
2. Inspect startup network/bundle traces: Inspector/Motor Test/Calibration UI chunks
   are requested only on use; StartPage can render without requesting the heavy map chunk.
3. Open the same tool concurrently ten times: one instance and one required runtime
   attachment are created, with idempotent focus/attach behavior.
4. Cancel initialization and replace the target before it finishes: no obsolete
   subscription, window, operation permission or device write survives.
5. Repeat open/close cycles and record live subscription/task/buffer counts; after
   cleanup, owned resources return to the warmed baseline without monotonic growth.
6. Saturate analysis inputs: queues remain within configured bounds; measure
   command/release scheduling against its existing validated deadline.
7. Hide/close every monitoring panel during an active execution: the pinned monitor,
   recorder, target identity and frozen baseline continue correctly.
8. Measure cold/warm startup, time-to-tool, idle CPU, RSS/heap and worker count on the
   declared reference machine before and after changes. Set absolute budgets after
   that baseline rather than inventing current performance numbers.

Basic lazy registration/loading is included in TM-02 and lifecycle/budget behavior
in TM-04. Larger COG/windowed-I/O and process-memory work overlaps SW-07/SW-10 and
TM-05 and must be sized once. All work remains on hold for owner review.
