# AF-02b.4 — Owned Shell and Logs contract

- Date: 2026-10-27.
- Status: implemented and accepted within software scope under the owner's
  standing autonomous-execution authorization; [evidence](../verification/af-02b-shell-logs-20261026.md).
- Scope: primary-link interactive MAVLink shell, FC log list/download/cancel,
  single/all deletion, panel/native close and target replacement. Motor Test's
  dedicated capture/log adapters remain AF-02b.4 Motor.

## Current findings

- `console_send` holds only a process-global mutex, resolves after enqueueing a
  raw write and truncates command+newline to 70 bytes. A truncated shell command
  can execute a different operation. Output is an unowned global event; closing
  one renderer can detach another's watcher or leave EXCLUSIVE shell state open.
- Main logs use one global `busy` bit and unscoped events. Save-picker cancellation
  can leave the UI busy; disconnect/replacement leaves stale list/progress. A
  cancel acts on whichever primary transfer is current.
- Download writes synchronously/directly to the final local path from the async
  worker. Terminal protocol completion and durable local-file completion are not
  separate evidence.
- Single/all deletion has strong path/logger checks but resolves through global
  primary state. `LOG_ERASE` has no ACK; cancellation or target loss after any
  delete write cannot undo files already removed.

## Rust-derived wire contracts

```rust
pub struct ShellRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

pub struct ShellCommandRequest {
    pub session: RuntimeClientSession,
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub command: String,
    pub confirmation: Option<String>,
}

pub struct ShellSessionReply { pub operation: OperationRef }
pub struct ShellWriteReceipt { pub operation: OperationRef, pub bytes: u16 }

pub struct ShellOutputEnvelope {
    pub operation: OperationRef,
    pub target: TargetRef,
    pub sequence: u64,       // string on TypeScript wire
    pub received_at_ms: u64, // number, like telemetry ages
    pub chunk: String,
}

pub enum ShellOperationError {
    Operation { error: OperationError },
    Protocol { error: ShellProtocolError },
    ReconciliationRequired { operation: Box<OperationRef>, reason: String },
}

pub enum ShellProtocolError {
    Stopped, Busy, Cancelled, TargetChanged, TimedOut,
    InvalidCommand { message: String }, ConfirmationRequired,
    OutputGap, OutputLimit, UnexpectedResponse { message: String },
}

pub struct LogRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

pub struct LogDownloadRequest {
    pub context: LogRequestContext,
    pub entry: LogEntryInfo,
    pub save_path: String,
}

pub struct LogDeleteRequest {
    pub context: LogRequestContext,
    pub entry: LogEntryInfo,
    pub confirmation: String,
}

pub struct LogEraseRequest {
    pub context: LogRequestContext,
    pub confirmation: String,
}

pub enum LogResult {
    Listed { entries: Vec<LogEntryInfo> },
    Downloaded { entry: LogEntryInfo, path: String },
    Deleted { entries: Vec<LogEntryInfo> },
    Erased,
}

pub struct LogReply { pub operation: OperationRef, pub result: LogResult }

pub struct LogProgressEnvelope {
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub entry: LogEntryInfo,
    pub fetched: u32,
    pub total: u32,
}

pub enum LogOperationError {
    Operation { error: OperationError },
    Protocol { error: LogProtocolError },
    LocalIo { message: String },
    ReconciliationRequired { operation: Box<OperationRef>, reason: String },
}
```

Errors/results use tagged snake-case wire enums. `LogProtocolError` includes
stopped/busy/cancelled/target/deadline/invalid-entry/no-ack/malformed-chunk/
cleanup-timeout variants and owned strings instead of borrowing protocol text.

## Native APIs

| Command | Input | Result |
| --- | --- | --- |
| `console_open` | `ShellRequestContext` | `ShellSessionReply` |
| `console_send` | `ShellCommandRequest` | `ShellWriteReceipt` |
| `console_close` / `console_cancel` | exact session + operation | `OperationStatus` |
| `console_operation_status` | exact session + operation | `OperationStatus` |
| `logs_request_list` | `LogRequestContext` | `LogReply::Listed` |
| `logs_download` | `LogDownloadRequest` | `LogReply::Downloaded` |
| `logs_delete` | `LogDeleteRequest` | `LogReply::Deleted` |
| `logs_erase_all` | `LogEraseRequest` | `LogReply::Erased` |
| `logs_cancel` / `logs_operation_status` | exact session + operation | `OperationStatus` |

Interactive shell owns `Shell` from open until confirmed close. Download owns
`LogTransfer`; list/deletion own both `Shell` and `LogTransfer`. List may need an
owned `ls -R` fallback because PX4 can remain silent on an empty SD card. Silence
alone never returns an empty list. Existing resource
conflicts prevent shell/log cross-talk and coordinate with reboot. All paths reuse
the primary transport and existing protocol logic; none opens a second connection.

## Serial-write and deletion risk analysis

1. **Arbitrary shell effects:** shell text can arm/reboot, change persistent
   parameters, alter files or stop services. Native rejects empty/control/newline
   input, bounds UTF-8 bytes, and sends the complete command in multiple
   `SERIAL_CONTROL` frames; it never truncates. A reviewed read-only preset
   allowlist needs no keyword. Every other command requires exact `RUN` UI
   confirmation and renders the exact text before dispatch. This is accidental-
   action mitigation, not a hostile-renderer security boundary.
2. **Wrong device/target:** native window authorization, original session,
   `TargetRef`, MAVLink destination and transport generation are checked before
   each frame and output publication. Target loss cannot redirect a command,
   close or deletion to a later primary FC.
3. **Exclusive shell cleanup:** owner close/cancel sends a zero-count release in
   the captured generation and waits for a bounded transport receipt. Missing
   receipt retains uncertain cleanup/claim and warns the operator. Closing idle A
   never releases B's shell.
4. **Deletion permanence:** single delete rechecks list id/time/size against
   `logdata.txt`, accepts only a canonical path under `/fs/microsd/log`, and
   verifies absence. Erase-all confirms the directory is empty. Both require
   fresh disarmed/OnGround telemetry, fresh logger-not-recording observation,
   NuttShell capability and native confirmation (`DELETE LOG <id>` or
   `DELETE ALL LOGS`) immediately before the first destructive write.
5. **Cancellation after delete:** `rm`/`LOG_ERASE` cannot be undone. Owner loss,
   deadline or target change after dispatch records uncertain outcome; it never
   fabricates the old list. A fresh target-bound list/directory observation is
   required before another destructive action.
6. **Download cancellation:** local cancellation sends `LOG_REQUEST_END`, then
   drains the exact route until a bounded quiet receipt. Confirmed cleanup may
   admit later work; missing quiet/transport evidence retains cleanup uncertainty.
   Late data cannot complete a new transfer.
7. **Local files:** destination selection occurs before operation start. Cancelled
   picker creates no Runtime operation. Download writes to a bounded temporary
   sibling on blocking I/O, flushes, then renames to the selected path; failure
    removes the temporary file and never reports `Downloaded`.
    The current single-buffer downloader rejects files above 256 MiB before
    operation admission. It checks owner/target/deadline again before atomic
    rename, never unlinks an existing destination as a fallback, and syncs the
    containing directory on Unix. Failure after possible rename retains uncertainty.
8. **Output/observer pressure:** shell transcript and log progress are bounded,
   operation/target/sequence scoped and nonblocking. Slow/closed renderers cannot
   stall serial writes, cancellation, close or lifecycle teardown.

Budgets: shell session maximum 24 h, individual command write 2 s, close receipt
1 s; list 10 s; download 10 min with progress resetting protocol retry windows;
delete one 20 s; erase all 20 s; log cleanup quiet receipt 1 s. Existing 1.2 s/
four-retry log windows remain. Native teardown closes owned shell/log work before
the link stage; it preserves uncertainty on failed cleanup.

Command deadlines include queue/lock waiting; a separate priority-close channel
retires queued writes. Session request history retains at most 4096 identities,
without evicting IDs inside a live session. Read-only prefix arguments exclude
shell compound/substitution characters. Free-form review captures the exact target
and cannot follow a target replacement. Unconfirmed FC deletion retains a bounded
primary-role read-back barrier; another delete requires a fresh verified list.
Trusted lifecycle composition first cancels the exact recorded Shell/Logs
operations and waits for their actor/file cleanup, then shuts down the existing
log worker. It does not infer cleanup merely from an event-forwarder exit.

## Acceptance

- Foreign/stale/prepared-owner-loss and invalid/unconfirmed shell/delete requests
  produce zero writes. Commands over one frame are complete, not truncated.
- A/B shell and log ownership, exact/idle/old cancellation, native close/reopen,
  target replacement and reboot/Motor conflicts pass production PTY/UDP fixtures.
- Closing A cannot detach/release B or close primary I/O. Owner close sends one
  release in the original generation; old close/cancel cannot affect later work.
- Save-picker cancellation immediately returns idle with zero operation/effect.
  Download cancellation/timeout/local-write failure has explicit terminal cleanup.
- Destructive operations require two UI interactions plus native identity-bound
  confirmation; logger-running, armed/airborne, Pxh, unsafe/stale paths and changed
  entry identity write zero deletes. Post-write uncertainty is visible/read back.
- Shell chunks preserve line boundaries, output gaps and limits are explicit,
  scroll-follow respects history reading, final resize persists, and dialog close
  behavior is documented/tested.
- Browser checks retain connected/disconnected, list/empty/progress/cancel/save,
  delete/erase, shell free-form confirmation/output/close, stale target/reply,
  native-close and responsive/field-mode screenshots.
- Generated contracts, full Rust/protocol regression, clippy/format, frontend
  static/production checks and workspace release build pass before completion.

This contract deliberately excludes Motor Test's dedicated shell/capture/log
paths; AF-02b.4 Motor migrates those while preserving STOP/Release priority.
