# Tools → Logs: Assessment and Improvement Plan

- **Review date:** 2026-10-22
- **Status:** In progress — assessment recorded; primary-link log deletion
  delivered; reliability and layout work still pending.
- **Scope:** The main window's Tools → Logs dialog, including FC log retrieval,
  its frontend state, and the embedded MAVLink console.
- **Execution:** Resume this plan later, one small task at a time. Present the
  affected types, function signatures, and error types for confirmation before
  implementing each task.

## 1. Summary

The current dialog provides basic FC log retrieval and shell access, but its
operation states and layout do not yet form a complete operator workflow.

The recommended order is:

1. Fix cancellation, disconnect, and console state correctness.
2. Separate the log list and console into task-focused tabs.
3. Improve progress, completion actions, and multi-log workflows.

Reliability and information hierarchy should take priority over cosmetic changes.

## 2. Evidence and verification limits

The assessment is based on source review and browser previews at **1366 × 768**:

- Disconnected state with an empty log list and console.
- A synthetic connected state with 20 log entries.
- The map was omitted using the existing `?nomap` preview option.

The dialog measured approximately **960 × 722 px**. The console section occupied
about one third of its height even when unused, and the populated list showed
approximately nine rows at once. The initial console output height was about
80 px.

Behavioral defects below were identified from code paths. The automated
interaction walkthrough did not complete, so it is not an end-to-end acceptance
result. Native save-dialog behavior, real FC transfers, and full interaction
regression remain to be verified during implementation.

### Main source references

- [Dialog and panels](../../frontend/src/components/dialogs/LogsDialog.tsx)
- [Log store and event handling](../../frontend/src/stores/logs.ts)
- [Console store and output assembly](../../frontend/src/stores/console.ts)
- [Desktop log service](../../crates/app-tauri/src/logs_service.rs)
- [Desktop console service](../../crates/app-tauri/src/console_service.rs)
- [Core log protocol and available metadata](../../crates/core/src/logs/protocol.rs)
- [Link lifecycle bridge](../../frontend/src/desktop/bridge.ts)
- [Alert layer](../../frontend/src/components/AlertLayer.tsx)
- [Design tokens and field mode](../../frontend/src/design-system/index.css)
- [English UI strings](../../frontend/src/i18n/en.ts)

## 3. Existing capabilities

- Request the FC SD-card log list over the MAVLink LOG protocol.
- Display log ID, UTC timestamp, and file size.
- Download one file at a time, cancel an operation, and display a progress bar.
- Show a saved indicator and local path after completion.
- Send MAVLink console commands, including preset commands.
- Display console output and provide a draggable height control.
- Reuse the application's theme tokens and Radix dialog shell.

These provide a useful foundation for a more complete retrieval workflow.

## 4. Functional assessment

Priorities below are relative to this improvement plan.

| ID | Priority | Finding | Recommended change |
| --- | --- | --- | --- |
| F1 | P0 | Cancelling the native save picker can leave the frontend busy. `download()` sets `busy = true`, while `logs_download()` returns success without starting a transfer or emitting a terminal event when no path is selected. | Distinguish destination cancellation from transfer startup. Every operation path must resolve to a defined state. |
| F2 | P0 | Disconnects and target changes do not reset or invalidate the log store. Entries and progress remain in memory, and the row Download button checks only `busy`. | Bind state to the connection session and FC identity. End pending work on disconnect, mark cached entries stale, and prevent downloads from stale or disconnected targets. |
| F3 | P1 | Initial, loading, empty, and transfer states are poorly distinguished. The empty message combines “no logs” with “list not requested”; progress has no percentage or byte count. | Define explicit list and per-file states. Show percentage, transferred bytes, and clear cancellation/failure feedback; add speed and estimated time where meaningful. |
| F4 | P1 | Completion only exposes `Saved` and a truncated path. A successful list refresh clears the entire downloads map, including saved state. | Add Open folder, Copy path, and Download again. Keep save records separate from list refresh and scoped to the correct vehicle/file identity. |
| F5 | P1 | Console transcript assembly removes trailing empty lines, so a newline at the end of one message can be lost when the next message arrives. Auto-scroll always jumps to the bottom. Height persistence saves the drag-start value. | Preserve stream boundaries, follow new output only when appropriate, and persist the final dragged height. |
| F6 | P1 | The console hint names `MAV_CMD_DO_SEND_SCRIPT / COMMAND_LONG`, but the backend uses `SERIAL_CONTROL`. | Correct the hint to describe PX4 shell access and keep implementation details accurate. |
| F7 | P2 | There is no sorting, filtering, multi-selection, or download queue. | Default to newest first, add date/ID filtering, and support a serial multi-file queue. |

### Console details to preserve in follow-up work

- `loadConsoleHeight()` converts a missing storage value using `Number(null)`,
  yielding zero and then the 80 px minimum rather than the intended 160 px
  fallback.
- The scroll effect's implementation does not honor its comment about retaining
  the operator's position when scrolled up.
- The pointer-up handler closes over the height from the start of the drag.
- The console store exposes `close()`, but the dialog does not call it. The
  eventual console lifecycle should explicitly define when the shell session is
  released, separately from whether a log download continues in the background.

## 5. Layout and UI assessment

### 5.1 Space allocation

The almost full-height dialog permanently stacks two different tasks. With no
logs, the upper panel is mostly blank; with many logs, the unused console still
reduces the visible list. The console's fixed 600 px maximum also needs a
viewport-aware bound so resizing cannot consume the useful list area.

**Recommendation:** give Flight logs and MAVLink console separate tabs, with an
operation summary that remains visible while switching tabs.

### 5.2 List structure

The list uses individual bordered cards without column headers. Important
shortcomings are:

- Timestamps are formatted as UTC but do not identify the timezone.
- File size and its Download action are separated by a large empty span.
- Repeated borders are more prominent than operation state.
- Saved paths use small, truncated text without a full-path action.
- The toolbar scrolls with the list rather than remaining available.

**Recommendation:** use a compact table with fixed headers and toolbar:

`ID / Recorded at (UTC) / Size / Status / Actions`

Use accurate size units: the current calculation uses powers of 1024, so values
should be labelled KiB/MiB rather than KB/MB.

### 5.3 Hierarchy, readability, and accessibility

Refresh, Download, and console presets have similar low-emphasis treatments.
Some meaningful text is only 10–11 px, and row actions/presets are small in
normal mode. Field mode increases button targets globally, but the complete
dialog still needs layout and typography verification in that mode.

**Recommendations:**

- Give primary actions a clear emphasis.
- Use approximately 12–14 px for normal content; avoid 10 px for essential state.
- Verify at least 44 × 44 px interactive targets in Field mode.
- Use text and icons alongside state colors.
- Add progress semantics, accessible control names, and keyboard reachability.

### 5.4 Main-window integration

A long-running download currently lives behind a large modal surface that
obscures the flight view. The existing critical banner uses `z-40`, while the
Logs content uses `z-50`; critical-alert visibility and interaction need explicit
verification and correction while the dialog is open.

**Recommendation:** keep transfer state independent of panel visibility, expose
a compact task entry in the main UI, and ensure critical alerts remain visible
and actionable. Define and communicate what closing the panel does.

## 6. Recommended information architecture

Use a vehicle/connection header, two task tabs, a main work area, and a download
task summary. The following is illustrative layout and data, not a final design:

```text
Logs                         PX4 · SYS 1 · Connected       ×

[ Flight logs ]  [ MAVLink console ]

[ Refresh ]  [ Search by ID… ]  [ Date ▼ ]          24 logs

ID    Recorded at (UTC)     Size       Status       Actions
23    2026-10-22 10:35      48.2 MiB    Ready        Download
22    2026-10-22 09:10      32.8 MiB    Saved        Open folder
21    2026-10-21 16:40      18.6 MiB    Downloading  Cancel
──────────────────────────────────────────────────────────
Log #21   46% · 8.6 / 18.6 MiB · 120 KiB/s · 01:25 left
```

### Flight logs

- Make this the default tab and give the list the available work area.
- Keep the toolbar and table header fixed; scroll only the data region.
- Sort newest first and identify unknown timestamps explicitly.
- Surface transfer progress, errors, and completion actions clearly.
- Add filtering and multi-selection in the later workflow increment.

### MAVLink console

- Give output the main work area and keep command entry at the bottom.
- Add Clear, Copy, Auto-scroll, and command history.
- Group preset commands in a compact toolbar or menu.
- Retain the operator's reading position when reviewing older output.

### Metadata constraint

`LOG_ENTRY` provides only **ID, UTC timestamp, and size**. Original filename,
flight duration, and mission name require additional parsing or association.
Do not display inferred values as FC-provided metadata. A richer metadata view
can be considered after retrieval and appropriate ULog/project integration.

## 6a. Delivered: primary-link SD-log deletion

Parity with the motor-test window: the Tools → Logs dialog can now delete SD
logs on the primary link's FC, each behind **two** confirmations.

- **Delete all** sends MAVLink `LOG_ERASE` and confirms an empty
  `/fs/microsd/log` over NuttShell (`ls -R`) before reporting success.
- **Delete one** maps the MAVLink log id to a path via `logdata.txt`
  (`time_utc`/`size` identity re-checked), removes it, and verifies the file is
  gone. The list is then re-requested so ids renumber exactly as PX4 assigns
  them.
- Both refuse while the logger is recording, and both return the refreshed list.

Implementation notes:

- New shared [`px4_shell`](../../crates/app-tauri/src/px4_shell.rs) module owns
  the `SERIAL_CONTROL` framing, the marker-delimited request/response, the safe
  `logdata.txt` path mapping, and the SD-directory parser (extracted from the
  motor-test capture service and reused by both callers).
- The primary link's NuttShell is shared with the MAVLink console. `ConsoleState`
  now holds a shell lock and a suppression flag: a deletion exchange takes the
  lock, the console waits, and the watcher stops echoing while the exchange reads
  its reply.
- Commands: `logs_erase_all`, `logs_delete` (both hold the ops lock);
  `core::logs::parse_logger_status` is exported for the recording check.
- Regression: [`check-logs-delete.mjs`](../../frontend/scripts/check-logs-delete.mjs)
  asserts two confirmations, that declining sends no command, and the exact
  identity passed to the backend. Screenshot: `screenshots/logs/logs-delete.png`.

## 7. Deferred implementation plan

All steps are pending. Split each step into independently reviewable small
tasks when execution resumes.

| Step | Work | Acceptance criteria |
| --- | --- | --- |
| 1. Confirm design and contracts | Confirm tabs, table fields, task summary, list/file states, connection ownership, and close behavior. Present affected type/function signatures and error types. | Normal, not-yet-loaded, loading, empty, downloading, cancelled, failed, and disconnected cases each have a defined UI and terminal outcome. The design is confirmed before implementation. |
| 2. Fix reliability | Address F1/F2 and console transcript, scroll, and height defects in separate small tasks. Define shell release behavior. | Cancelling the save picker permits an immediate retry; disconnect leaves no pending busy state; changing FC cannot reuse another FC's entries; chunked console output preserves line breaks; new output does not interrupt history reading; reopening restores the final resized height. |
| 3. Improve layout and feedback | Introduce tabs, table, fixed toolbar, detailed progress, completion actions, and alert-layer integration. | Capture screenshots at 1366 × 768 and 1920 × 1080, including Field mode. Header, input, and active controls remain reachable without clipping. Keyboard-only refresh → download → cancel works. Critical alerts remain visible and actionable. Same-target refresh preserves saved records. |
| 4. Improve retrieval efficiency | Add filtering/sorting, serial multi-file queue, persistent save records, and a main-window background-task entry. | Multiple selected logs download sequentially with identifiable states; failed items can be retried; switching tabs and refreshing do not discard task records; panel closure follows the agreed background-transfer behavior. |

### Verification scenarios for implementation

- [ ] First open with no FC connection.
- [ ] Connected, before the list has been requested.
- [ ] List request in progress, successful empty list, and list failure.
- [ ] Populated lists at both target desktop sizes and in Field mode.
- [ ] Save picker cancelled before a transfer starts, followed by a retry.
- [ ] Download progress, cancellation, timeout, and local write failure.
- [ ] Completed download, full-path access, and same-target list refresh.
- [ ] Disconnect during an operation, reconnect, and switch to a different FC.
- [ ] Console messages split inside a line and immediately after a newline.
- [ ] New console output while scrolled to the bottom and while reading history.
- [ ] Console resize, close, reopen, and viewport-size changes.
- [ ] Critical alert arriving while Logs is open.
- [ ] Keyboard-only operation and accessible progress reporting.
- [ ] Multi-file queue with one failure and a targeted retry, when Step 4 lands.

Use meaningful store/protocol tests for state transitions, browser interaction
checks and screenshots for layout, and native desktop/SITL verification for
save dialogs, shell lifecycle, and actual FC transfer behavior. Run the checks
appropriate to each completed increment and record the results here.

## 8. Resumption point

Start with **Step 1**, then implement reliability fixes before the layout
refactor. Recheck the referenced code and current working-tree state before
resuming, since the application may evolve after this assessment.
