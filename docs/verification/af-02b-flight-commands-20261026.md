# AF-02b.4b — Owned flight-command acceptance

- Record date: 2026-10-26.
- Source: current working tree on `main`, based on
  `e0c1b45e8b435ebfb586375b7d1520762d8a762c`.
- Contract: [owned flight commands](../architecture/af-02b4b-flight-command-ownership.md).
- Status: complete within the AF-02b.4b software scope; G1 remains open.

All eight general flight intents now use the existing single command worker with
native caller/owner/target/generation-bound operation authority. Final acceptance
includes the safety fixes, generated contracts, browser/manual evidence and the
full workspace/static/production/release gates below.

## Verified safety fixes

- Re-evaluate arm preflight after takeoff ACK waits and forced-arm audit
  persistence. Bound preparation by the original owner/target/deadline.
- Retain reconciliation after a partially accepted takeoff sequence; never arm
  after denial, cancellation, target replacement or emergency supersession.
- Capture the UI Runtime context before asynchronous terrain/checklist work.
  Revalidate native authority before dispatch and before applying terminal replies.
- Give each UI request independent pending/status/map-marker authority. Keep
  same-target displaced outcomes in the event log without settling newer work.
- Keep preflight open after a denied/uncertain arm; only this current request's
  accepted verdict closes confirmation. Retire forms on target-scope reset.
- Bound long native error text on the TopBar and retain its full tooltip, so
  status and the always-visible emergency controls remain reachable.
- Keep Arm/Disarm status labels bound to the captured intent when heartbeat
  read-back changes the vehicle's armed state before or after its ACK.

## Checks

| Check | Result |
| --- | --- |
| Native flight-command PTY fixtures | 16 passed, 0 failed |
| Complete Rust-derived TypeScript mirror | 264 core + 4 native contracts identical |
| Strict TypeScript / i18n | Passed; 855 keys, 0 missing |
| Command / Safety / Preflight browser checks | 66 / 26 / 43 assertions passed, 0 failed |
| TopBar / Heartbeat / Startup / Teardown browser checks | 69 / 24 / 14 / 22 assertions passed, 0 failed |
| Browser total | 264 assertions passed across 7 checks; 46 screenshots |
| Command / Safety / Preflight page-error checks | Zero page errors |
| Full workspace Rust all-target regression | 1378 passed, 0 failed, 12 ignored |
| Rust library breakdown | Core 1022 + app 227 + DEM helper 62 passed |
| Core integration / desktop CSP checks | 62 / 5 passed |
| Related command/parameter protocol fixtures | Command 20; owned Parameters 6; Parameters 4 passed, 1 ignored |
| Workspace doc-test check | Passed; 0 runnable doctests |
| Workspace all-target clippy with `-D warnings` | Passed |
| Workspace formatting / git whitespace | Passed |
| Frontend static gates | 17 passed, including the complete generated contract mirror |
| Compiled MAVLink stream catalog metadata | Passed |
| Production frontend | Passed; 3471 modules, 241 output files |
| Workspace release build | Passed; desktop and DEM-helper binaries rebuilt |

The 12 ignored cases are the existing physical-bench, isolated-SITL, GDAL/live
network, large-raster and generated-terrain opt-ins. This record covers the
default complete workspace regression plus isolated production-service fixtures.

## Ownership and failure matrix

| Scenario | Measured acceptance |
| --- | --- |
| Eight typed flight intents | Exact owner/target replies and production `COMMAND_LONG` parameters; datum-tagged height inputs |
| Foreign caller / wrong height / prepared generation replacement | Zero flight-command writes |
| Prepared owner loss / duplicate request | Zero stale writes and one original dispatch; no intent replay |
| Denied takeoff / initially blocked arm or takeoff | Zero follow-up arm / zero command writes |
| Preflight becomes blocked after takeoff ACK wait | One takeoff frame, zero arm frames; retained partial-step uncertainty |
| RTL arrives between takeoff steps | Takeoff + RTL only; normal operation is superseded and cannot arm later |
| Forced-arm audit failure | Zero arm writes; PX4 force/kill parameter remains zero in accepted arm fixtures |
| Foreign cancel and idle native client close | B's command ACK, primary I/O and original parameter cache remain available |
| Owner cancel / emergency owner closes | Retry retirement within the 1 s receipt budget; FC uncertainty retained |
| Old cancel / captured old native-close callback | Later/reopened work remains active and receives its own ACK |
| Missing ACK / repeated normal command | Uncertain outcome; confirmed local protocol retirement; same-command reuse is quarantined with zero new writes |
| Transport reopen after dispatch | No old retry on the replacement generation; fresh command uses a new target context |
| Displaced same-target UI reply | Own event log/marker only; newer RTL pending state is preserved |
| Delayed terrain/checklist and hidden native target loss | Zero stale dispatch; obsolete acceptance/refusal cannot revive the UI scope |
| Arm/Disarm read-back and long native errors | Exact requested action retained; four viewport widths keep status/emergency controls visible with full tooltip text |

The browser runner writes exact commands, assertion counts, exit codes, full-log
hashes and screenshot hashes to
[`af-02b-flight-command-browser-20261026.json`](af-02b-flight-command-browser-20261026.json).
Screenshots are retained under
[`frontend/screenshots/commands/ownership/`](../../frontend/screenshots/commands/ownership/).
Final checks and build artifacts are recorded in
[`af-02b-flight-command-build-20261026.json`](af-02b-flight-command-build-20261026.json).
It contains all 26 gate commands, exit codes/timings, complete-log hashes, the
verification-time source fingerprint and the linked browser-record hash.
The `--verify-evidence` mode rechecks every retained command/log, screenshot and
production/release artifact hash.

Key screenshots were visually reviewed after correction: current RTL awaiting
ACK after an old reply, denied/uncertain/accepted preflight, exact Arm/Disarm
labels, obsolete target scope and long errors at 1024/1280/1440/1920 px.

### Release artifacts

| Artifact | Bytes | SHA-256 / manifest SHA-256 |
| --- | ---: | --- |
| `target/release/maggcs-app` | 46,076,864 | `59a3b0fa97f41941fd9e80cf6b7b1b69c76f8806c49fcabc60fed83446da71d9` |
| `target/release/dem-prep` | 3,105,432 | `0da987db0b5f851fd64a733ae5df090b29eea82141f5aa92e0601999918667a8` |
| `frontend/dist` (241 files) | 23,793,698 | `0b9c61a1e6987f81788d8ec55b3c35f77bc9ab4201e9d8e20adc4a260551049c` |

## Reproduction

From the repository root:

```bash
# Browser prerequisites for a fresh development checkout (Node 20+).
npm --prefix frontend install --no-save --package-lock=false playwright@1.63.0
npm --prefix frontend exec -- playwright install chromium
cargo test -p maggcs-core -p maggcs-app --lib export_bindings
TS_RS_EXPORT_DIR="$PWD/frontend/src/generated-types" \
  cargo test -p maggcs-core -p maggcs-app --lib export_bindings
npm --prefix frontend run check:generated-types
npm --prefix frontend run check:flight-acceptance
npm --prefix frontend run check:flight-delivery
npm --prefix frontend run check:flight-delivery -- --verify-evidence
```

The delivery runner verifies the complete passing browser record and screenshot
hashes, then runs frontend production build, helper build, all Rust targets/doc
tests, clippy/format, the static gates and workspace release build. The verified
Rust/TypeScript code is followed by this documentation-only delivery finalization.

The browser runner owns a loopback Vite process on port 4179, stops it after the
checks and retains full logs in `/tmp/opencode/af02b4b-browser-*.log`. Set
`AF02B4B_PORT` to another free loopback port if needed. Individual scripts also
accept a running Vite URL and screenshot directory:

```bash
node frontend/scripts/check-command-ux.mjs http://127.0.0.1:4179/ /tmp/opencode/flight-ux
node frontend/scripts/check-safety.mjs http://127.0.0.1:4179/ /tmp/opencode/flight-safety
node frontend/scripts/check-preflight.mjs http://127.0.0.1:4179/ /tmp/opencode/flight-preflight
```

## Manual verification steps

The browser scripts model the native Runtime and FC replies explicitly; inspect
their retained screenshots alongside these operator actions. Backend write
counts, exact identities and bounded cancellation are checked by isolated UDP
and test-created PTY fixtures.

1. **Command grading:** open Flight, click Takeoff/Land/RTL/Arm once, then hold
   Land briefly and release. No request or status appears. Hold for the complete
   1 s window: exactly one typed command leaves the UI, the strip shows awaiting
   ACK, and the current accepted verdict freezes elapsed time. Compare
   `ux/command-menu.png`, `command-holding.png` and `command-result.png`.
2. **Emergency interaction:** select Plan and use the always-visible RTL hold.
   Repeat while a normal command awaits ACK. RTL remains reachable within two
   interactions. Deliver the displaced normal reply: it is logged, while RTL
   stays pending. Deliver the current denied RTL verdict: it remains visible
   until acknowledged. Compare `safety/safety-plan.png`, `safety-in-flight.png`
   and `safety-old-request.png`, plus `ux/command-request-isolation.png`.
3. **Preflight confirmation:** attempt an arm against the blocked checklist.
   Review the failing items and repair entries. `FORCE` cannot confirm;
   `FORCE ARM` enables the chosen reason/note. A denied attempt keeps the panel
   and restores confirmation. An uncertain attempt keeps the FC reconciliation
   reason. Only the current accepted attempt closes it. With a warn-only
   checklist, confirm every warning and verify the acknowledged IDs travel in
   the typed Arm intent. Compare `preflight/preflight-force-*.png` and
   `preflight-warnings-accepted.png`.
4. **Map-request isolation:** prepare two Set home or Go to requests with
   different points. Deliver the older reply first: it cannot install or clear
   the newer request's marker or pending state. The newer accepted Set home
   installs only its own coordinates. `ux/command-home-owner.png` records the
   command state; the no-map browser fixture asserts exact marker coordinates.
   For Arm/Disarm, inspect `preflight/preflight-force-accepted.png` and
   `ux/command-disarm-accepted.png`: the label names the requested action even
   after heartbeat read-back reports the new armed state.
5. **Slow preparation and target replacement:** defer terrain sampling during
   Land, replace the Runtime target, then resolve the sample. Zero Land writes
   occur. Repeat with a delayed Takeoff checklist: no command or obsolete
   refusal appears. Compare `ux/command-stale-terrain.png` and
   `command-stale-checklist.png`.
6. **Old native reply and reconnect:** replace the native generation without a
   UI link event, then deliver the old accepted reply. Native reply validation
   retires its display/map authority. A click that discovers another replacement
   produces zero writes and asks for a fresh command. The next explicit command
   names the new generation and accepts only its own operation. Compare
   `ux/command-native-target-loss.png` and `command-replaced-target.png`.
7. **Long error layout:** inspect `ux/command-long-error-{1024,1280,1440,1920}.png`.
   Armed/Mode/Link, RTL/Hold and application context must remain inside the
   viewport. Hover the command strip to read the complete native error; elapsed
   time and dismissal remain available.
8. **Lifecycle regression:** confirm/cancel the startup auto-connect window,
   accept then reject a changed FC target, and inspect Prepare's metadata rows.
   Refuse armed/airborne disconnect/quit, acknowledge native-close refusal, then
   check that unsaved waypoint work requires the teardown keyword. Compare the
   `startup/`, `teardown/` and `heartbeat/` captures.

G1 remains open for Mission, Shell/Logs, Motor and their complete cross-adapter
matrix. Physical flight/electrical behavior and external QGC/RC interaction have
separate acceptance records.
