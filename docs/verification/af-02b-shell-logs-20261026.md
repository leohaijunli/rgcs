# AF-02b.4 Shell/Logs software acceptance

- Campaign: 2026-10-26; final evidence generated 2026-10-27 UTC.
- Version: 1.1.0; verified source base `89f612f` plus this owned Shell/Logs slice.
- Result: **pass within software scope**. G1 remains open for the dedicated Motor
  adapters and the final cross-adapter/client/fault matrix.
- Contract: [owned Shell/Logs](../architecture/af-02b4-shell-logs-ownership.md).
- Decision: [ADR-030](../adr/030-owned-shell-logs-retirement.md).

## Implemented ownership and completion

Interactive shell now has native open/send/close/cancel/status, exact native-window
and Runtime session/target authority, generation-bound complete command framing,
bounded output, per-session request deduplication and priority close. Free-form
review captures the target and requires `RUN`; a compound read-only prefix cannot
bypass it. Write budgets include queue/lock waiting. Output sequences use lossless
wire counters; transcript gaps/limits are visible, history scroll is preserved and
the final resized height survives dialog reopen.

List/download/cancel reuse one owned core log worker. Cancellation and deadline
expiry send END with an independent cleanup deadline; quiet/transport failure
retains uncertainty and quarantine. Primary list/deletion reserve Shell and
LogTransfer; download reserves LogTransfer. PX4 silence returns an empty list only
after a complete owned directory observation. No second connection is created.

Destination selection precedes admission. A cancelled picker creates zero Runtime
operations/attachments/effects. Downloads recheck entry identity, write an exclusive
temporary sibling on blocking I/O, check owner/target/cancellation before rename,
sync the file/directory and return a durable typed result. The buffered adapter
rejects files above 256 MiB before admission. Failed/cancelled pre-rename writes
preserve the existing destination and remove their own partial files.

Delete/erase require native identity-bound confirmation, fresh disarmed/landed
telemetry, NuttShell, logger inactivity and safe canonical paths. Deletion is
verified by absence/list; erase by directory emptiness. A possibly dispatched
failure retains an explicit read-back barrier. Lifecycle cancels recorded actors
and local I/O, waits for their cleanup and then stops the worker before primary I/O.

## Measured checks

| Check | Result |
| --- | --- |
| Core Logs filter | 45 passing checks, including non-default UDP source rejection, END after expiry, full-queue deadline and drop/abort cleanup uncertainty |
| Native interactive Shell | 8 passing production PTY cases, including full 200-byte framing, A/B ownership, duplicate/unconfirmed input, native-close/reopen, replacement, Motor exclusion, contended cancel/priority close and lifecycle |
| Native Logs | 12 passing checks: production PTY ownership/list/download/cancel/quiet-failure/delete/read-back/lifecycle plus local I/O and input validation |
| Generated contracts | 293 complete Rust-derived frontend mirrors match |
| Browser | 9 suites, 330 passing assertions, zero failing assertions/exceptions |
| Screenshots | 68 retained and hashed, including 11 Shell/Logs screens |
| Workspace all-target tests | 1463 pass, 12 ignored opt-in cases |
| Strict/static/production/release | All 26 delivery gates pass |

The isolated native fixtures use the production serial transport/router/services;
only PX4 is simulated. Local-I/O tests verify destination preservation and partial
cleanup. Browser fixtures run the real React/store code with an isolated native
boundary. They do not touch hardware or claim physical-flight qualification.

## Reproduction and retained evidence

```bash
env PATH="$HOME/.cargo/bin:$PATH" cargo test -p maggcs-core --lib logs
env PATH="$HOME/.cargo/bin:$PATH" cargo test -p maggcs-app --lib console_service
env PATH="$HOME/.cargo/bin:$PATH" cargo test -p maggcs-app --lib logs_service
npm --prefix frontend run check:shell-logs-acceptance
npm --prefix frontend run check:shell-logs-delivery
npm --prefix frontend run check:shell-logs-delivery -- --verify-evidence
```

- [Browser record](af-02b-shell-logs-browser-20261026.json): exact commands,
  per-suite logs/counts and all screenshot hashes.
- [Build record](af-02b-shell-logs-build-20261026.json): 26 exact gates/log hashes,
  unchanged before/after source fingerprint over 766 inputs, browser evidence hash
  and three production artifact manifests.
- Logs: `/tmp/opencode/af02b-shell-logs-browser-*.log` and
  `/tmp/opencode/af02b-shell-logs-build-*.log`.
- Images: `frontend/screenshots/shell-logs/ownership/`.

| Artifact | Measured size |
| --- | --- |
| `frontend/dist` | 241 files, 23,815,849 bytes; 3476 transformed modules |
| `target/release/maggcs-app` | See final build record |
| `target/release/dem-prep` | 3,127,952 bytes |

Exact SHA-256/manifests are in the build record; `--verify-evidence` also verifies
the current source, log contents and rebuilt artifacts, not just the JSON verdict.

## Manual verification

1. Open Tools → Logs with an available FC. Refresh, inspect the list, cancel a
   download picker and confirm immediate idle with no transfer. Save a download;
   progress is provisional and Saved appears only after durable completion.
2. Cancel an active transfer. Confirm exact operation cleanup. Replace/disconnect
   the target while pending; old list/progress/output cannot complete new work.
3. Decline either delete confirmation: no destructive write. Accept both on an
   eligible NuttX target; check refreshed/renumbered list. Recording, armed,
   airborne, POSIX, unsafe/stale index or missing read-back must refuse or show
   explicit uncertainty. Refresh/read back before retrying an uncertain delete.
4. Send a read-only preset; verify complete line/output. Enter a free-form command,
   review the exact text and type RUN. Replace the target during review: no command
   follows it. Observe gap and 64 KiB transcript limits.
5. Scroll up while output continues, resize the console and close/reopen Logs.
   History position and final height persist. Close console before log actions;
   panel close cancels only its own work. Check desktop, 430px and field-mode
   screens, including actual Tools menu click/reopen without overlapping hit areas.

External QGC/RC exclusion, arbitrary shell execution/rollback and real aircraft
outputs remain outside these software evidence claims. Motor migration and the
remaining G1 matrix are the next architecture-first slice.
