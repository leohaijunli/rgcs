# Minimum Runtime contracts and AF-01 fixtures

- Contract revision: AF-00 / v0, 2026-10-25.
- Concrete implementation checkpoints: through AF-02b.4b, 2026-10-26.
- Implementation authorization: owner standing authorization, 2026-10-25.
- Scope: one operational flight target and the existing dedicated bench, SITL
  source and independent RTK base roles.
- Related: [current ownership](af-00-current-system.md), ADR-025/026/028 and the
  [architecture-first delivery plan](../plans/architecture-first-delivery-plan.md).

Publish these contracts before implementing each slice. Wire/schema and Runtime
API revisions are separate concerns; concrete IPC/process packaging is AF-12.

## 1. Identity vocabulary

| Type | Meaning / issuance | Compatibility rule |
| --- | --- | --- |
| `runtime::VehicleId` | Opaque application logical aircraft identity | Distinct from existing `telemetry::VehicleId`; generated TS name `RuntimeVehicleId`. An equal route/address does not merge aircraft |
| `VehicleSessionId` | Opaque Runtime flight/target context | Replace at relevant target/transport context boundary. Not a physical FC boot-epoch claim |
| `LinkId` | Identity of an existing Runtime I/O-owner entry | Promote/reuse the current app type. A surviving handle can have a newer transport generation |
| `TransportGeneration` | Actual successful-open generation of the existing worker | Zero is not an opened/verified connection; never resurrect old grants after reconnect |
| `MavlinkTarget` | Validated system/component routing address | Compatibility alias/view of current telemetry node identity; component/system match checked separately from aircraft identity |
| `ClientId` | Runtime-issued client session | Ephemeral; a persisted string is not client authorization |
| `ModuleInstanceId` | One module/workspace instance, separate from module descriptor ID | Bind owner cleanup to this instance; a reopened window is a new operational owner |
| `OperationId` | Runtime-issued operation record | Bind resource claims, cancellation, terminal result and cleanup evidence |
| `RequestId` | Correlation identity of one request | Does not grant authority and is not a replay instruction |
| `Revision` | Monotonic revision inside a specified snapshot/input scope | Compare only within matching target/session and resource scope |
| `SubscriptionId` | One owner-bound data subscription | Closing it withdraws only that owner's demand |

Opaque references and full-width counters are string-encoded across JSON/TS,
following serial inventory/base-session conventions. Reject malformed, zero or
out-of-range numeric identities. Do not cast `u64`/`usize` identities to JavaScript
numbers. Client/session/operation IDs need non-reuse across relevant restarts;
the later IPC adapter checks live client registration before accepting effects.

The independent base continues to use `BaseSessionId`; automatic serial intent
continues to use `AutoConnectIntentRef`. A base/source is not represented as the
primary aircraft. `FlightSessionId`/`ExecutionId` arrive with AF-09 rather than a
premature Survey schema.

## 2. AF-01 target facade signatures

The following is the minimum signature-level contract. Private fields denote
native composition capabilities, rather than an exposed mutable application store.

```rust
pub struct TargetRef {
    pub vehicle_id: VehicleId,
    pub session_id: VehicleSessionId,
    pub link_id: LinkId,
    pub generation: TransportGeneration,
    pub mavlink_target: MavlinkTarget,
}

pub enum TargetAvailability {
    Unverified,
    Available,
    Disconnected,
    Stale,
    Unsupported,
}

pub struct TargetSnapshot {
    pub link_id: LinkId,
    pub target: Option<TargetRef>,
    pub availability: TargetAvailability,
    pub heartbeat_age_ms: Option<u64>,
    pub parameter_service_available: bool,
}

pub struct TargetHandle { /* exact existing services + private validity lease */ }

pub trait TargetRuntime {
    fn snapshot(&self, link: LinkId) -> Result<TargetSnapshot, RuntimeError>;
    fn bind(&self, expected: &TargetRef) -> Result<TargetHandle, RuntimeError>;
}

impl TargetHandle {
    pub fn target(&self) -> &TargetRef;
    pub fn telemetry_snapshot(&self)
        -> Result<Option<TelemetrySnapshot>, RuntimeError>;
    pub fn parameter_observation(&self, name: &str)
        -> Result<Option<ParamObservation>, RuntimeError>;
}

pub enum RuntimeError {
    TargetUnavailable,
    TargetChanged,
    InvalidIdentity,
    ServiceUnavailable,
}
```

Concrete errors may attach structured field/reason context while preserving those
categories. Identity allocation can fail; production propagates `Result`.

### Required behavior

- Resolve an explicit link/target reference, never redirect an existing request
  using a later `primary` or UI selection.
- Only native verification creates an available reference. An open transport,
  configured fallback IDs or stale heartbeat cannot supply write eligibility.
- A handle pins existing services and checks session validity/actual generation
  before returning data. A replaced target invalidates old requests and results.
- Expose copied snapshots/observations. Never expose cache mutation, transport
  shutdown, the complete `AppState` or another tool's UI store.
- Borrowing retains the primary worker and parameter-cache identity. Detaching
  the Motor Test owner retires only its attachment/resources.
- Logical vehicle, session and transport identities stay separately typed even
  where the current supported topology uses one control route.
- Existing motor permission remains an additional admission gate. Binding a
  target or enabling observation does not enable motors or send a stream request.

## 3. AF-02 operation vocabulary (subsequent slice)

```rust
pub struct OperationOwner {
    pub client_id: ClientId,
    pub instance_id: ModuleInstanceId,
}

pub struct RequestContext {
    pub request_id: RequestId,
    pub owner: OperationOwner,
    pub target: TargetRef,
    pub input_revision: Option<Revision>,
}

pub struct OperationRef {
    pub id: OperationId,
    pub owner: OperationOwner,
    pub target: TargetRef,
}

pub enum OperationResource {
    ParameterTransfer,
    ParameterWrite,
    ActuatorOutput,
    MissionTransfer,
    Command,
    EmergencyCommand,
    Shell,
    LogTransfer,
}

pub enum CleanupOutcome {
    NotRequired,
    Confirmed,
    Pending,
    Failed,
    Uncertain,
}

pub enum OperationOutcome<T> {
    Completed(T),
    Cancelled { cleanup: CleanupOutcome },
    Failed { error: OperationError, cleanup: CleanupOutcome },
    Uncertain { evidence: ReconciliationRef },
}

pub enum OperationError {
    Runtime(RuntimeError),
    NotOwner,
    NotFound,
    ResourceBusy,
    PermissionRequired,
    DeadlineExceeded,
    InvalidRequest,
}
```

`begin(context, resources, requested_budget)` returns an owned operation reference;
Runtime limits the budget with the existing configured deadline. `cancel(context,
operation)` requests cancellation of that operation and returns a receipt/status,
rather than claiming successful physical stopping. `status` identifies the same
owner/target and retained terminal/cleanup evidence. A cancelled caller may still
need write/read-back reconciliation. Missing read-back is never `Completed`.

Resource coordination covers this Runtime's clients. External QGC/RC changes
remain observable state changes requiring verification, not a global lock claim.
Normal work cannot hold the release path's long-running lock or queue slot.

## 4. AF-03/04 subscription vocabulary (subsequent slices)

```rust
pub struct SubscriptionRef {
    pub id: SubscriptionId,
    pub owner: OperationOwner,
    pub target: TargetRef,
}

pub struct Snapshot<T> {
    pub target: TargetRef,
    pub revision: Revision,
    pub data: T,
}

pub struct DataBatch<T> {
    pub subscription: SubscriptionRef,
    pub revision: Revision,
    pub sequence: u64,
    pub published_at_ms: u64,
    pub source_age_ms: Option<u64>,
    pub gap_samples: u64,
    pub columns: T,
}
```

Sequence/timestamp/counter fields use lossless wire types. Actual sample receive
timestamps and per-field ages remain in reused telemetry types. A publication
timestamp cannot make old telemetry fresh. Subscription registration and starting
snapshot/revision form one consistent handoff; overload has an explicit bounded
drop/coalesce/failure policy. Display decimation and recording/analysis demand are
separate. No UI load/detach implicitly ends an execution pin.

## 5. AF-01 acceptance fixtures

All software fixtures use loopback UDP, synthetic input or Unix PTYs; existing
ignored physical bench/SITL cases keep their opt-in boundary.

| ID | Fixture / action | Numerically or automatically checked result |
| --- | --- | --- |
| CTX-01 | Valid primary PX4 heartbeat; capture binding; replace primary with another installed handle | Old bind/read fails `TargetChanged`; new session/ref differs; old events cannot be labelled as new target |
| CTX-02 | Retained handle reopens transport from generation n to n+1 | Old handle/ref invalid; no operation/grant inherited; missing fresh verification reports unavailable |
| CTX-03 | Echo exact primary serial offer in Motor Test, then detach | Same `handle_id` and same parameter cache/service; zero second serial opens; primary reads continue after detach |
| CTX-04 | Change endpoint/node/generation while an offer is pending | Old offer refused before attachment/effect; exact current offer is accepted idempotently |
| CTX-05 | Open transport with absent/stale/foreign/GCS/unsupported heartbeat or configured fallback IDs | No available control reference; age/absence explicit; zero motor/start/configuration writes |
| CTX-06 | Flight target retires while independent base and selected SITL/source roles exist | Only flight-owned context expires; base/source retain their own identity and accurately scoped status |
| CTX-07 | IDs/revisions above 2^53, malformed/zero strings and serialization round trip | Exact full-width identity retained; bad references rejected; TS bindings have no numeric identity narrowing |
| CTX-08 | Obtain facade target handle; inspect supported consumer API and drop it | No mutable AppState/cache or transport-close API; dropping borrowed facade leaves owner alive |

Existing starting fixtures:

- `main_link_service_tests.rs`: borrowed worker/cache identity, detach, reconnect,
  stale offer, primary shell/log exclusion.
- `capture_service/tests.rs` and `core/motor_test/service.rs`: default OFF, passive
  enable, generation revocation, pending-start cancellation, owned release failure.
- `core/mavlink/connection.rs`: queued/inflight generation-bound writes.
- `serial_service/tests.rs`, `core/devices` and `rtk_service/tests.rs`: stale
  intent, worker lease lifetime, target replacement and independent base.
- Inspector/Parameters connection IPC scripts: source freshness, delayed data,
  cache/draft coherence and reconnect behavior.

AF-02 adds cross-client cancellation/conflict fixtures. AF-09 adds frozen actual
mission items/read-back identity. Neither requires the new Flight Plan/Drape model.

## 6. Measured scope and remaining qualification

Use the clean [idle profile](../../testdata/architecture/af00-idle/) for startup
measurements. Counters must identify whether they count OS processes/threads,
native tasks, UI references, subscriptions or jobs. Only the first two currently
have process-wide measured values; optional service status is separately recorded.

The reference host is under memory/swap pressure. Retain raw samples and cold-cache
definition before choosing a performance budget. True OS-cold startup, FC reboot
identity continuity and electrical release remain named physical/platform
qualification fixtures. They are not inferred from browser or PTY success.

## 7. AF-01 same-process facade delivery

`core::runtime` implements the identity/target types. The native adapter in
`crates/app-tauri/src/runtime_facade.rs` supplies `RuntimeFacade<R>`,
`TargetRuntime<R>` and target-bound read handles. It registers these read-only
Tauri commands:

| Command | Explicit input / result |
| --- | --- |
| `runtime_targets` | Current supported target descriptors; none creates a transport or grants control |
| `runtime_target_snapshot` | Lossless `LinkId` → `TargetSnapshot` |
| `runtime_telemetry_snapshot` | Exact `TargetRef` → optional copied `TelemetrySnapshot` |
| `runtime_parameter_observation` | Exact `TargetRef` + name → optional fresh-generation `ParamObservation` |

The registry contains metadata only and aliases borrowed primary services. Target
handles revalidate before/after reading; the facade never supplies a mutable
parameter cache or an unscoped cancel. Existing effect adapters remain until their
owner-scoped migration slice. G1 includes AF-02, rather than being closed by this
read-only facade.

Concrete `RuntimeError` also reports identity allocation/exhaustion, invalid
request and unavailable service context. Counters reject noncanonical JSON;
opaque IDs use bounded ASCII parsing. Generated TS separates `RuntimeVehicleId`
from the existing node `VehicleId` and represents `LinkId`/generation as strings.

AF-02a's concrete operation types/coordinator are implemented in
`core::runtime::operations`; see [its contract](../../crates/core/src/runtime/operations.md)
and [checkpoint evidence](../verification/af-02a-operations-20261025.md).
Native adapter/client integration remains AF-02b.

## 8. Native client-lifecycle foundation

`RuntimeClientSession { owner, target, module_id }` is issued by the native
`runtime_client_attach` command after exact target binding. `validate`/`detach`
check the actual Tauri caller and original session. The native close handler
captures ClientId instead of acting on a later module with the same window label.

See [the native client checkpoint](../verification/af-02b-client-foundation-20261025.md).
Client/lifecycle primitives and the Parameters adapter below are implemented.
The remaining device operation/cancellation adapters are AF-02b.4; G1 remains open.

## 9. Parameters owned-operation checkpoint

AF-02b.2's Parameters transfer/write implementation and AF-02b.3's regression
acceptance use these concrete existing types:

```rust
pub struct ParameterRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

pub struct ParameterReadRequest {
    pub context: ParameterRequestContext,
    pub name: String,
}

pub struct ParameterSetRequest {
    pub context: ParameterRequestContext,
    pub name: String,
    pub value: ParamValue,
    pub ptype: ParamType,
}

pub enum ParameterCommandError {
    Operation { error: OperationError },
    Protocol { error: ParamError },
    ReconciliationRequired { operation: Box<OperationRef>, reason: String },
}
```

Definitions and generated wire types originate in
`core::params::operation_types`; `ParamError` exports as `ParamProtocolError`.

| Command | Input, in addition to the native-injected app/webview | Result inside `Result<_, ParameterCommandError>` |
| --- | --- | --- |
| `params_pull` | `ParameterRequestContext` | `OperationRef`; native background work publishes `params_operation` |
| `params_read` | `ParameterReadRequest` | `Option<ParamEntry>`; fresh-generation cache hit or owned protocol read |
| `params_set` | `ParameterSetRequest` | `ParameterSetEcho { operation, status, actual, requested }` |
| `params_cancel` | `RuntimeClientSession`, exact `OperationRef` | `OperationStatus` cancellation receipt |
| `params_operation_status` | `RuntimeClientSession`, exact `OperationRef` | `OperationStatus` |
| `params_fc_version` | `RuntimeClientSession` | `Option<String>` from the captured parameter binding |

`ParameterSetStatus` is `Confirmed` or `Clamped`; timeout/cancellation is an error.
`ParameterEventEnvelope` contains `TargetRef`, optional `OperationId` and the
flattened parameter event. Native caller authorization is separate from identity
serialization. The operation driver runs independently of the renderer's reply
future and reconciles interrupted writes through the original binding.

`ParamService::{pull_owned, read_owned, set_owned, cancel_owned}` reuse the existing
worker/cache/transport. Their `ParamTransaction` binds ID, generation, deadline
and cancellation; stale/idle cancellation never cancels another owned or legacy
transaction. A confirmed read-back releases a write claim while retaining the
uncertain outcome. Missing read-back keeps both uncertainty and the claim.

See [the acceptance matrix and budgets](../verification/af-02b-parameters-20261025.md).
The full Parameters SDK migration remains AF-06.

## 10. Owned-command / Parameters reboot checkpoint

AF-02b.4a implements `CommandTransaction`, `CommandService::{send_owned,
cancel_owned}` and the reviewed
[command contract](af-02b4a-command-ownership.md). `params_reboot` now accepts a
`CommandRequestContext { session, request_id }` and returns
`CommandReply { operation, result }`; `command_cancel` and
`command_operation_status` require the exact native caller/session/operation.

Writes and retries use the captured transport generation, operation deadline and
owner cancellation. Possibly dispatched failures preserve uncertainty and
quarantine that `MAV_CMD`'s ACK for the generation. Normal reuse is refused;
emergency effects remain sendable but cannot claim an ambiguous ACK as success.
Reboot requires fresh disarmed/OnGround telemetry. An accepted ACK means command
acceptance rather than completed reboot and immediately retires the old Runtime
target generation so reconnect must issue a new session.

See the [acceptance record](../verification/af-02b-command-reboot-20261025.md).
The general flight caller checkpoint follows below. Motor, Mission, Shell and
Logs remain AF-02b.4; G1 stays open for their effect/cancellation matrix.

## 11. Owned flight-command checkpoint

AF-02b.4b implements the reviewed
[flight-command contract](af-02b4b-flight-command-ownership.md). Native IPC injects
the actual app/window and accepts these Rust-derived types:

```rust
pub struct FlightCommandRequest {
    pub context: CommandRequestContext,
    pub intent: FlightCommandIntent,
}

pub struct FlightCommandReply {
    pub operation: OperationRef,
    pub command: FlightCommandKind,
    pub steps: Vec<FlightCommandStepResult>,
}

pub enum FlightCommandError {
    Command { error: CommandOperationError },
    Preflight { refusal: PreflightRefusal },
}
```

`flight_command_execute` replaces the general flight intent IPC callers with
Takeoff, Land, RTL, Set mode, Arm/Disarm, Go to and Set home. Coordinates are
validated WGS84 values; supplied absolute heights are `Height` with the
`AMSL_EGM96` datum. The original session/operation also authorizes exact
`command_cancel` and `command_operation_status` requests.

The coordinator's `EmergencyCommand` wire value is `emergency_command`. It is a
standalone nonblocking marker, with two reserved slots in the default 32-active
ledger. Native emergency admission supersedes normal command operations on that
exact target, including takeoff's inter-step gap. The existing single worker
arbitrates writes; ordinary clients cannot use foreign-owner cancellation.

Takeoff uses one operation for `NAV_TAKEOFF` and its optional arm, with preflight
rechecked after the ACK. Denial, cancellation, target replacement or supersession
prevents the follow-up arm. Forced arm persists an audit on the captured recorder
and rechecks its physical gate before writing. Preparation/queueing use the same
10 s operation budget, or 20 s for the complete takeoff sequence.

Confirmed local retry retirement releases flight protocol claims while retaining
an uncertain FC outcome and generation-wide same-command ACK quarantine. Reboot
keeps its stricter whole-target claims. Cancellation does not undo an accepted
flight command; telemetry/read-back remains the physical state authority.

The UI captures context before asynchronous preparation and revalidates native
authority before dispatch and terminal publication. A local request token keeps
displaced same-target outcomes from settling a newer pending command. Old-target
replies/preflight data cannot revive a retired scope. Arm/Disarm status labels keep
the actual requested action across heartbeat changes, and preflight closes only
for the current request's accepted verdict.

All 264 core and four native TypeScript exports match the complete frontend
mirror. [Acceptance evidence](../verification/af-02b-flight-commands-20261026.md)
records protocol/native ownership cases, browser screenshots/manual steps and
complete workspace/release checks. Shell/Logs/Motor retain the remaining
AF-02b.4 adapter ownership and G1 qualification.

## 12. Owned Mission checkpoint

The confirmed [Mission contract](af-02b4-mission-ownership-proposal.md) is
implemented by `MissionTransaction`, the existing single `MissionService` worker,
native `mission_operations` and the target-scoped frontend adapter.

`mission_upload`, `mission_download`, `mission_clear` and `mission_set_current`
return `MissionReply { operation, result }`. Their input contexts contain the
native-issued `RuntimeClientSession` and fresh `RequestId`. Exact
`mission_cancel`/`mission_operation_status` calls return retained
`OperationStatus`; no global cancel-current endpoint remains in the main UI.

Core rejects concurrent exchanges instead of replacing protocol state. It checks
all source/destination/mission-type contexts, bounds queue/event delivery, writes
in the captured generation/deadline and quarantines ambiguous reuse after a
possibly dispatched failure. Set current completes only when the exact requested
sequence is observed. Upload cannot complete from an early ACK or out-of-range
request. Download retains QGC/MAVLink null parameters as the NaN sentinel and
rejects non-finite altitude read-back.

`mission_progress` identifies operation/request/target. `mission_current` is a
separate target/timestamp observation and has no terminal authority. Invocation
replies carry terminal/downloaded data. Native forwarding and the UI validate the
original generation/session before publication/application.

The editor captures the sent wire snapshot. An ACK is not FC-sync proof; matching
owned read-back establishes it. Later edits, explicit local clear, old downloads,
old target replies and passive-current races cannot overwrite or verify a newer
editor/target. Reconnect revokes the old baseline and reads current state without
replaying consequential intent.

Confirmed local retry retirement releases `MissionTransfer` while retaining any
uncertain FC result and read-back requirement; failed retirement keeps uncertain
cleanup. Reboot and borrowed Motor policies are rechecked at admission/dispatch.
[Acceptance evidence](../verification/af-02b-mission-20261026.md) records the
core/native/browser matrices, generated contracts and complete build gates.
Shell/Logs/Motor and the final cross-adapter matrix keep G1 open.
