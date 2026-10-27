# AF-02b.4 — Owned Mission contract

- Date: 2026-10-26.
- Status: signatures and acceptance confirmed by the owner on 2026-10-26;
  implemented and accepted within software scope.
- Scope: existing mission upload, download, clear and set-current operations,
  exact cancellation/status, target-bound progress/current observation and their
  native/frontend callers.
- Parent: [Runtime contracts](runtime-contracts.md), following the accepted
  [flight-command slice](../verification/af-02b-flight-commands-20261026.md).

## Current implementation findings

- `MissionService` already owns one protocol/transport worker, with an eight-item
  command queue, 16-event observer queue and a 30 s protocol deadline. Its public
  methods currently acknowledge enqueueing; completion and downloaded plans go
  through unowned `mission` / `mission_plan` events.
- Enqueueing a new mission operation can replace protocol state. Writes currently
  use the generic transport send, and a full/closed event consumer can block/stop
  the service. No operation-specific cancellation exists.
- `MISSION_CURRENT` currently completes Set current even when the observed
  sequence differs from the requested sequence. That must not be interpreted as
  a successful owned set-current result.
- The frontend upload completion hashes the then-current editor rather than the
  clicked upload snapshot. Download, automatic read-back and passive-current
  handling also need exact request/target authority.

## Proposed Rust-derived wire signatures

All identity fields reuse the reviewed Runtime types. Mission items reuse the
existing frame-tagged `MissionItem` wire representation; any new absolute-height
domain helper must use `height::Height`.

```rust
pub struct MissionRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

pub struct MissionUploadRequest {
    pub context: MissionRequestContext,
    pub items: Vec<MissionItem>,
}

pub struct MissionSetCurrentRequest {
    pub context: MissionRequestContext,
    pub seq: u16,
}

pub enum MissionResult {
    Uploaded { count: u16 },
    Downloaded { items: Vec<MissionItem> },
    Cleared,
    Current { seq: u16 },
}

pub struct MissionReply {
    pub operation: OperationRef,
    pub result: MissionResult,
}

pub enum MissionOperationError {
    Operation { error: OperationError },
    Protocol { error: MissionProtocolError },
    ReconciliationRequired {
        operation: Box<OperationRef>,
        reason: String,
    },
}

pub enum MissionProtocolError {
    Stopped,
    Busy,
    Cancelled,
    TargetChanged,
    TimedOut,
    AlreadySubmitted,
    AmbiguousReply,
    InvalidRequest { message: String },
    Timeout { waiting_for: String },
    Unexpected { message: String },
    AckDenied { ack_type: u8 },
    SeqMismatch { expected: u16, got: u16 },
    CountMismatch { expected: u16, got: u16 },
    UnsupportedFrame { frame: u8 },
    NoItems,
    RetriesExhausted,
}

pub struct MissionProgressEnvelope {
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub sent: u16,
    pub total: u16,
}

pub struct MissionCurrentSnapshot {
    pub target: TargetRef,
    pub seq: u16,
    #[ts(type = "number")]
    pub received_at_ms: u64,
}
```

`MissionResult` is tagged by `kind`; operation/protocol errors are tagged by
`code`, with snake-case wire values. Protocol error context uses owned strings:
the existing `MissionError` contains `&'static str` and is not itself a suitable
deserializable cross-boundary DTO. All new types get canonical core exports and
the frontend mirror.

## Proposed protocol capability signatures

```rust
pub struct MissionTransaction { /* private ID, generation, deadline, cancellation */ }

impl MissionTransaction {
    pub fn new(id: OperationId, generation: TransportGeneration,
        timeout: Duration) -> Result<Self, MissionProtocolError>;
    pub fn with_owner_cancellation(self, cancellation: OperationCancellation) -> Self;
    pub fn id(&self) -> &OperationId;
    pub fn generation(&self) -> TransportGeneration;
    pub fn may_have_dispatched(&self) -> bool;
}

impl MissionService {
    pub async fn upload_owned(&self, transaction: MissionTransaction,
        items: Vec<MissionItem>) -> Result<u16, MissionProtocolError>;
    pub async fn download_owned(&self, transaction: MissionTransaction)
        -> Result<Vec<MissionItem>, MissionProtocolError>;
    pub async fn clear_owned(&self, transaction: MissionTransaction)
        -> Result<(), MissionProtocolError>;
    pub async fn set_current_owned(&self, transaction: MissionTransaction,
        seq: u16) -> Result<u16, MissionProtocolError>;
    pub async fn cancel_owned(&self, transaction: &MissionTransaction)
        -> Result<bool, MissionProtocolError>;
}
```

Native composition adds a captured `MissionBinding { connection, service }` to
the existing target facade. Every owned mission uses the existing worker and the
`MissionTransfer` claim. It cannot replace another in-flight operation merely by
enqueueing, resolve a later primary worker, or close borrowed I/O.

## Proposed native IPC

The actual `AppHandle` / `WebviewWindow` is injected for every command.

| Command | Explicit input | Success inside `Result<_, MissionOperationError>` |
| --- | --- | --- |
| `mission_upload` | `MissionUploadRequest` | `MissionReply` |
| `mission_download` | `MissionRequestContext` | `MissionReply` |
| `mission_clear` | `MissionRequestContext` | `MissionReply` |
| `mission_set_current` | `MissionSetCurrentRequest` | `MissionReply` |
| `mission_cancel` | `RuntimeClientSession`, exact `OperationRef` | `OperationStatus` |
| `mission_operation_status` | Same session and operation | `OperationStatus` |

Retained `mission_operation` statuses and bounded `mission_progress` envelopes
carry the original operation/request/target. Typed invocation replies own terminal
completion/downloaded data. Passive `mission_current` snapshots carry only their
verified target/current sequence and cannot complete an unrelated operation.

## Write/failure and reconciliation policy

1. **Wrong caller/target:** authorize the actual native caller, capture the worker
   and generation, and revalidate before admission, writes/retries and publication.
   Validate item counts before `u16` conversion, sequence/frame/coordinate fields
   and command encodability; invalid input produces zero writes.
   Respect the native Motor/borrowed-target and reboot exclusion policies at
   admission and before dispatch.
2. **Cancelled writes:** upload, clear and set-current can already have changed the
   FC. Stop only local owned retries; do not implicitly clear/restore a mission or
   undo set-current. Retain an uncertain result and clear the UI's claim of FC sync.
   A confirmed local retirement releases the protocol claim; missing retirement
   retains it. A later read-back establishes the actual FC baseline.
3. **Late wire replies:** mission ACKs do not identify the Runtime operation or
   distinguish upload from clear. Possibly dispatched abandoned/failed exchanges
   quarantine ambiguous mission-protocol reuse for the generation. Normal reuse
   returns `AmbiguousReply` until a fresh verified generation or trusted
   reconciliation establishes reply authority; no timer alone claims success.
4. **Wrong source or current sequence:** verify FC source, GCS destination,
   mission type and receive context on every relevant frame. Set current requires
   a fresh post-dispatch observation of the exact requested sequence; a different
   passive sequence remains observation, not successful command completion.
5. **Observer pressure/owner loss:** bounded progress delivery cannot delay
   cancellation/deadlines/writes. A closed renderer/event receiver does not own
   the service's transport lifetime. Native close uses the captured client ID;
   reopening cannot inherit or cancel old work.
6. **UI editor/target changes:** capture the exact target and uploaded item snapshot
   before async work. Completion/read-back verifies that captured snapshot, not
   a newly edited plan. An obsolete download cannot overwrite newer local edits
   or a replacement target's baseline. Reconnect can request a fresh read-only
   observation, but never replay an old upload, clear or set-current intent.

Proposed native budgets: upload/download 30 s including queueing; clear/set-current
10 s; target observation 100 ms; each transport write at most 500 ms; local owned
cancellation receipt 1 s. Retain the existing 500 ms per-frame retry/five-retry
policy. Runtime emergency command admission remains independent of mission claims.

## Acceptance required before marking this slice complete

- Foreign/stale/prepared-owner-loss requests and invalid item/sequence inputs
  produce zero mission writes.
- All four operations return the exact owner/target/operation identity and a
  genuine terminal protocol verdict; wrong Set current read-back cannot succeed.
- A closes/cancels its own transfer within the receipt budget while idle A close
  or foreign cancellation leaves B's operation, primary I/O and parameter cache
  alive. Old cancellation never acts on later/reopened work.
- Target replacement/reopen during queued or dispatched work produces zero old
  retries on the new generation. Ambiguous old replies cannot settle later work.
- Cancelled/dispatched upload/clear/current retain uncertainty with no automatic
  rollback. Failed local retirement keeps claims; confirmed retirement preserves
  the FC uncertainty until read-back.
- Saturated/closed observers do not stall owned completion/cancellation.
- Browser checks cover progress, cancellation, denial, stale target/reply and
  editor-change isolation, with screenshots and manual steps. Existing `.plan`,
  mission sync and height/frame regression remains green.
- Core/native unit + UDP/PTY protocol fixtures, generated contract equality,
  clippy/format, production frontend and release builds pass.

The owner confirmed these signatures and acceptance on 2026-10-26 and authorized
continued autonomous implementation. Implementation/results are in the
[Mission acceptance record](../verification/af-02b-mission-20261026.md). G1
remains open for the remaining adapters and cross-adapter matrix.
