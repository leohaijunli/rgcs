# Motor-Test Design Review and Improvement Plan

- **Date:** 2026-10-22
- **Reviewed commit:** `b8759be`
- **Status:** Design review completed; implementation planned in separately verified tasks.
- **Firmware reference:** Official PX4 v1.17 sources. The installed firmware release/hash still needs identification; “6X” identifies the hardware.

## Overall assessment

The review found **confirmed software defects behind the parameter problem**, and several mapping/telemetry weaknesses that can explain the motor-output discrepancy.

The agreed workflow is:

- **6X flight-controller hardware with PWM motor outputs.**
- **SITL pulse-width matching:** approximately 1400 µs from SITL should produce approximately 1400 µs on each mapped hardware output.
- **One onboard SD-card ULog per test**, started before motor commands and finalized afterward.

The priorities are **reliable stopping, correct target parameter transactions, explicit output mapping, PWM compensation, and real onboard logger control**. The UI should expose that complete pipeline.

This review covers commit `b8759be`. PX4 behavior below was checked against official **v1.17** sources; the installed firmware release/hash still needs identification. “6X” identifies the hardware.

---

## 1. Confirmed problems in the current design

| Finding | Consequence | Evidence |
|---|---|---|
| Target parameter reads return the encoded MAVLink float without decoding its declared type. | Integer `1` can appear as approximately `1.4e-45`; the UI reports an enabled parameter as disabled. | `crates/app-tauri/src/actuator_service.rs:747–778` |
| Parameter writes can return success after receiving a different value. The set is sent once; retries only repeat reads. | A request to enable testing can “succeed” while the FC remains disabled. | `crates/app-tauri/src/actuator_service.rs:786–824` |
| The motor window falls back to the primary parameter cache and opens the primary Parameters window. | With primary SITL plus serial hardware, the repair action can read or change SITL instead of the hardware. | `frontend/src/actuator/ActuatorApp.tsx:291–309,1432–1464` |
| `SDLOG_MODE` values are mapped incorrectly, and restart requirements are absent. | “Disabled”, “always”, and “armed” select different behavior from their labels. | `crates/app-tauri/src/actuator_service.rs:827–868` |
| “Logger on/off” starts/stops the PC received-message recorder. | Clicking it does not start/stop an SD-card ULog on PX4. | `frontend/src/actuator/ActuatorApp.tsx:554–563,1342–1355` |
| SITL forwarding accepts message 36 from any node and ignores its output port. Target feedback also ignores the port. | Another output bank can overwrite the displayed/input values; MAIN and AUX can be confused. | `crates/app-tauri/src/actuator_service.rs:687–711,1653–1671` |
| Output addressing is fixed to Motor functions 1–4; physical output assignments are not checked. | Commands and displayed channels can refer to different outputs. | `crates/core/src/motor_test/mod.rs:180–193` |
| Emergency stop clears local state without generating release frames. Normal stop changes state before selecting confirmed writes. | Emergency stopping relies on timeout; normal stop can acknowledge queueing rather than completed transport writes. | `crates/core/src/motor_test/mod.rs:237–261`, `crates/core/src/motor_test/service.rs:219–239` |
| Load limits are not reliably updated when a service is reused; preset/forward starts default to “no props”. | The selected load and backend output envelope can disagree. | `crates/app-tauri/src/actuator_service.rs:169–227,613–638` |

The SD-logging findings are resolved: the motor-test panel no longer writes
`SDLOG_MODE` (it reads and displays the hardware values), and the PC-side
received-message recorder was removed; "Check logger"/"Start capture" now
control the PX4 file logger. The other findings remain the tasks below.

Two additional reliability problems matter for the logging workflow:

- Healthy downloads can exhaust the retry budget after approximately **six seconds**, because progress does not reset the download timeout/retry state.
- The log runtime treats ordinary lifecycle events and lag notifications as termination.

These need correction before SD-log retrieval can be considered dependable.

---

## 2. Why 1400 µs can become approximately 1000 µs

### What the application currently sends

The current conversion is:

```text
1400 µs → (1400 − 1000) / 1000 → ACTUATOR_TEST value 0.4
```

That conversion itself does **not** turn 1400 into 1000.

However, `ACTUATOR_TEST` controls a **logical actuator function using a normalized value**. PX4 subsequently applies its motor model and physical output configuration.

### Most likely explanations for the reported symptoms

These remain hypotheses about the hardware incident:

1. **The displayed value comes from the wrong output bank.**
   The target observer merges all `SERVO_OUTPUT_RAW.port` values into one readout labelled MAIN.
2. **Only Motor 1 is assigned on the observed physical channels.**
   Motor 2–4 may be assigned elsewhere, disabled, or configured as Servo functions.
3. **Commands are rejected, delayed, or expire.**
   Firmware support, hardware safety state, test permission, and delivery timing need visible evidence.
4. **The app briefly supplies zero demand.**
   The startup grace and source-switching paths can produce zero-valued active commands.

**Motor function number and connector number must be distinguished.** For example:

```text
SITL port 0, channel 2
        ↓
ACTUATOR_TEST Motor 2
        ↓
PWM_MAIN_FUNC5 = Motor 2
        ↓
Physical MAIN output 5
```

In that configuration, feedback must come from MAIN channel 5.

### Required PWM-matching conversion

For a supported **non-reversible Motor function**, let:

- `P` = requested pulse width;
- `min`, `max` = target channel’s actual PWM endpoints;
- `a` = target `THR_MDL_FAC`.

Calculate:

```text
q = (P − min) / (max − min)
x = q                       // normal output
x = 1 − q                   // PWM range reversal enabled

ACTUATOR_TEST value = a × x² + (1 − a) × x
```

This compensates for PX4’s subsequent inverse thrust-model conversion.

Example for **1400 µs**:

| Target configuration | Required command |
|---|---:|
| 1000–2000 µs, factor 0 | 0.400000 |
| 1100–1900 µs, factor 0 | 0.375000 |
| 1100–1900 µs, factor 0.5 | 0.2578125 |

### Forwarding improvements

- Select the source by **connection, system/component, port, and channel**.
- Store raw PWM, timestamp, identity, and validity in one atomic snapshot.
- Read target function assignments, MIN/MAX/DIS, reversal, and thrust factor.
- Apply load limits and slew limits **in PWM space before compensation**.
- Show when limits prevent exact tracking.
- Distinguish `Pulse`, `Disarmed`, and `Release`. **Command value zero means minimum motor thrust, not necessarily off.**
- Use expiring latest-value batches; prevent old commands replaying after reconnect.
- Measure delivery rate and gaps for every function.

Four commands per tick are feasible, but they are not an atomic hardware vector update.

---

## 3. Parameter read/write redesign

### Parameter naming

For the inspected PX4 v1.16/v1.17 releases, the enable parameter is:

**`COM_MOT_TEST_EN`**, type `INT32`, values `0` and `1`.

The UI can label it “Actuator testing enabled” while displaying the actual firmware parameter name. Availability should be firmware-qualified.

### Correct transaction sequence

**Read**

1. Bind the transaction to the selected target and connection generation.
2. Subscribe before sending.
3. Match reply source, parameter name, and type.
4. Decode the value.
5. Return a typed observation with age and target identity.

**Write**

1. Freshly establish the parameter’s type/current value.
2. Encode using the target’s parameter protocol.
3. Subscribe before sending `PARAM_SET`.
4. Observe its echo and perform bounded read-back verification.
5. Retry the set when appropriate.
6. Report success only when the decoded value matches.
7. Return a mismatch, timeout, cancellation, or target-change error otherwise.

**Keep PX4’s bytewise integer encoding.** Sending numeric float `1.0` would be incorrect for bytewise `INT32`.

### UI behavior

Provide direct controls in **Target Parameters**:

| Parameter | Observed value | State | Controls |
|---|---|---|---|
| `COM_MOT_TEST_EN` | Enabled — `1` | Fresh | Read / Enable / Disable |
| `SDLOG_MODE` | Armed until disarm — `0` | Confirmed | Read (read-only in the motor-test panel) |
| `SDLOG_BACKEND` | SD + MAVLink — `3` | Restart pending | Read (read-only in the motor-test panel) |

States should include **Reading, Pending, Confirmed, Mismatch, Unknown, Stale, and Unsupported**.

Connecting should establish identity and read settings. Enabling motor testing should be an explicit, confirmed operation. Parameter observations must be invalidated after target replacement or reboot.

---

## 4. Actual onboard SD logging

### Correct configuration semantics

For PX4 v1.17:

| `SDLOG_MODE` | Meaning |
|---:|---|
| 0 | Armed until disarm |
| 1 | Boot until disarm |
| 2 | Boot until shutdown |
| 3 | While manual-input AUX1 exceeds 30% |
| 4 | First arm until shutdown |

`SDLOG_MODE` is **restart-required**. Changing it does not reconfigure the running logger.

`SDLOG_BACKEND` controls backend selection:

- Bit 0: SD/file logging.
- Bit 1: MAVLink logging.

Older releases have different disable semantics, so the UI should use version-matched metadata.

### Runtime capture control

For stock v1.17, a compatible logger running in armed-only mode with a file backend supports:

```text
logger on
logger off
logger status
```

These can be accessed through a **dedicated-target MAVLink shell service**, while the FC remains disarmed.

`MAV_CMD_LOGGING_START/STOP` controls **MAVLink ULog streaming**. It does not provide this SD-file capture control.

The service must verify the running logger’s configuration. `SDLOG_MODE=0` alone is insufficient evidence if the module was started with different arguments.

### Per-test lifecycle

A backend coordinator should own:

```text
Verify prerequisites
  → Start SD capture
  → Confirm file recording and unique pathname
  → Pre-roll
  → Start motor commands
  → Stop/release motor commands
  → Post-roll
  → Stop SD capture
  → Observe stopped state
  → Verify file stability and artifact integrity
  → Complete
```

Suggested initial configuration:

- **1 second pre-roll**
- **1 second post-roll**
- **10-second bounded finalization window**, followed by an explicit unconfirmed state if necessary

Emergency motor release must remain independent of shell polling and downloads.

PX4’s capture stop is asynchronous: it normally includes approximately one second of postflight processing plus file draining. “Not logging” is therefore not an immediate file-close acknowledgement.

### Artifact and recovery handling

Store a manifest containing:

- Test ID and FC identity.
- Firmware/build and connection/boot generation.
- Remote ULog pathname.
- Mapping and parameter snapshot.
- Capture and motor-test boundaries.
- Stop reason and finalization evidence.

Require a unique pathname for every test. Preserve failed/aborted captures.

On disconnect, the SD logger can continue recording. Reconnection must reconcile capture state on the same FC before another test starts.

### Logging data requirements

Verify that the onboard ULog contains:

- `vehicle_command`, `vehicle_command_ack`, `actuator_test`.
- The relevant `actuator_outputs` instances.
- Actual magnetometer instances at the required rates.
- Vehicle/actuator state.
- ESC telemetry where available.

Default v1.17 logging can be too slow for magnetic interference analysis. Also, the target ULog does not inherently contain the original SITL PWM or the GCS’s limiting/conversion decisions; record those in the correlated PC test record.

Custom logger-topic configuration needs explicit preparation: a custom topic file can **replace** the profile-selected topic set and requires logger reinitialization.

---

## 5. UI layout optimization

### Recommended workspace

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Motor Bench   Target: 6X / ID 1   DISARMED   SD: RECORDING             │
│ State: RUNNING      Measured rate: 10 Hz       STOP   EMERGENCY STOP   │
├───────────────────────────────┬──────────────────────────────────────┤
│ SOURCE — SITL                 │ TARGET — PX4                         │
│ Endpoint · ID · port · age    │ Serial device · ID · firmware · age  │
├─────────────────────┬─────────┴──────────────────────────────────────┤
│ TEST SETUP          │ MOTOR TRACKING                                 │
│ Manual/Preset/Forward│ Motor  Source   Expected  Reported  Δ   State  │
│ Load and PWM limits │ M1     1400 µs  1400 µs   1399 µs  -1  Live   │
│ Mapping summary     │ M2     1450 µs  1450 µs   1451 µs  +1  Live   │
│ Target parameters   │ M3     …                                       │
│ Readiness checks    ├────────────────────────────────────────────────┤
│ Start controls      │ PWM tracking / Magnetic field / ESC plots     │
├─────────────────────┴────────────────────────────────────────────────┤
│ SD capture: test-… / path / elapsed / finalization / download         │
│ Diagnostics: parameter transactions, command results, stop reason    │
└──────────────────────────────────────────────────────────────────────┘
```

### Specific changes

1. **Consolidate connections.**
   Remove the duplicated SITL connection controls. Show one Source card and one Target card.
2. **Make the header target-specific.**
   Display the serial target’s heartbeat, armed state, firmware and measured command rate.
3. **Use a motor comparison table.**
   Keep source PWM, conditioned/expected PWM, target-reported PWM, error and age together. Expand a row for function-to-pin mapping and encoded command value.
4. **Separate logging controls clearly.**
    - **PX4 SD capture**
    - **PC test record** — optional, when an existing correlated record is available
    - **Log retrieval**
5. **Keep Stop and Emergency Stop fixed and visible.**
   Distinguish Starting, Running, Releasing, Finalizing and Recovery Required.
6. **Make errors durable and actionable.**
   Connection polling currently clears start errors. Retain failures until resolved or dismissed, and show parameter mismatch/command rejection details.
7. **Make the layout responsive.**
   The current fixed sidebars and 250 px scope signal list leave too little plotting space at the supported minimum width. Use:
   - Main comparison/plots area as the flexible region.
   - Collapsible signal legend.
   - Logging details docked at wide sizes and expandable at smaller sizes.
   - One primary body-scroll region.
8. **Align motor-bench IPC types with Rust.**
    Prefer generation scoped to the existing motor, capture and run contracts.
    If generation requires a broader refactor, a small set of aligned TypeScript
    declarations is an acceptable simplified delivery. Verify the declarations
    against Rust-serialized payloads and TypeScript strict checks. Stop events
    must retain the named `reason` field added by Task 1.

While running, freeze load, routing and mapping changes. Changes made while idle should reset the start confirmation.

---

## 6. Staged implementation plan

Each row should be a separately approved, verified change.

### Availability and simplified scope

Missing optional information may be skipped or represented explicitly. It must
not block the remaining UI work or be displayed as confirmed data.

| Unavailable item | Handling |
|---|---|
| Firmware release/hash | Display `Unknown`; retain any observed target identity. Firmware-qualified hardware acceptance remains pending. |
| Reliable command rate, ESC telemetry or optional plot signals | Display `—` or omit the optional metric/plot. Use the existing backend observations without adding a new acquisition pipeline for Task 9. |
| Mapping, PWM model or fresh output feedback | Show the affected value as unknown/stale. Show expected PWM and tracking error only when their inputs are known and fresh; an identity-model fallback is explicitly unverified for pulse matching. |
| Correlated PC record or custom logger-topic configuration | Omit the unavailable record/detail from the workspace. Topic/rate validation can wait until a real ULog is available. |
| Connected 6X, pulse-measurement equipment or real ULog artifacts | Mark the affected bench checks `Deferred — hardware/artifact unavailable`. Software verification and UI work can proceed with clearly labelled fixtures; physical acceptance remains pending. |

The simplified workspace uses existing Source/Target observations, motor
readouts, capture/run states and stop controls. Advanced plots, expanded
diagnostics and application-wide type generation are optional follow-ups.
Existing target/readiness checks, load limits, confirmed motor releases and
owned-capture confirmation remain part of the motor-test flow.

### Remaining TODOs

- [x] **Task 9a — Minimal responsive workspace:** Source/Target summary, test
  setup, motor comparison, capture/run status, fixed Stop/ESTOP and durable
  errors. Freeze load/routing/mapping during a run; idle edits reset start
  confirmation. Optional fields follow the availability rules above.
- [ ] **Task 9b — Existing event/status contract alignment:** deliberately
  deferred by the project owner; the hand-written declarations in
  `frontend/src/actuator/benchTypes.ts` remain the working contract for now.
- [x] **Task 9c — UI verification:** screenshots and manual checks at
  1024×640, 1366×768 and 1920×1080, plus TypeScript strict checking and build.
  Fixtures must be labelled; unknown/stale values must remain visibly distinct
  from confirmed values, and stop controls must stay visible.
- [ ] **Conditional bench verification — Tasks 4/5/6/8:** mapping, ±5 µs physical
  PWM tracking, SD capture, 20 distinct test artifacts and interruption/recovery.
  Perform these when the required hardware/artifacts are available and record
  deferred checks explicitly. They do not block Task 9.

### Execution status

| Task | Status | Verification |
|---|---|---|
| 1 — Stop/ESTOP | Software complete | 43 focused core unit tests, 4 motor-service integration tests, 2 lifecycle integration tests; `cargo check -p maggcs-app --tests` and Rust formatting passed. Controlled transport regressions hold every write confirmation, fail motor 2 while attempting the remaining releases, and verify emergency latching. |
| 2 — Target parameters | Software complete | 730 core lib tests + 8 integration tests pass; clippy `-D warnings` clean. Session regressions: `_HASH_CHECK` sentinel excluded, out-of-range index dropped, REAL32 echo of an INT32 set is a type mismatch (never confirmation), encoding frozen mid-transaction. Codec already proves `0/1/2/−1` byte-concat round-trips. |
| 3 — Target binding + load | Software complete | 94 app lib tests pass (1 ignored); 730 core lib tests pass (1 ignored); motor-test service integration (4) and lifecycle binding (1) pass; clippy `-D warnings` and frontend `tsc --noEmit` clean. New regressions cover no target/telemetry/heartbeat/stale/armed/mismatched-heartbeat refusals, connection-identity replacement, non-idle start refusal, per-load caps/slew, and no `ACTUATOR_TEST` before the first heartbeat. Physical pulse verification remains part of Task 4/5 bench acceptance. |
| 4 — Node/port/channel mapping | Software complete; bench PWM check pending | 741 core lib tests pass (1 ignored), including 11 mapping tests; 97 app lib tests pass (1 ignored), including foreign-frame, selected-node/port and MAIN/AUX bank separation regressions; motor-test service integration (4) pass; clippy `-D warnings` and frontend `tsc --noEmit` clean. Permuted MAIN/AUX assignments select the intended physical channel and duplicate functions are reported invalid. Physical pulse verification still requires the connected 6X and firmware identification. |
| 5 — Target-compensated PWM matching | Software complete; bench PWM check pending | 751 core lib tests pass (1 ignored), including 49 pulse/model round-trip cases across five range/reversal/thrust-factor configurations (≤1 µs); 99 app lib tests pass (1 ignored) and 5 integration tests pass, including model fallback and target-model capture/send-before-start regressions; clippy `-D warnings`, `cargo fmt`, frontend `tsc --noEmit` and build clean. Physical pin tracking (±5 µs) still requires the connected 6X and firmware identification. |
| 6 — Target-shell SD capture control/status | Software complete; bench capture pending | 771 core lib tests pass (1 ignored), including 17 capture state-machine tests (check/existing recording, exact echo marker, unique path, storage, recovery stop, bounded finalization and timeout errors); 106 app lib tests pass (1 ignored) and 4 motor-service integration tests pass, including the mandatory owned-Recording gate; clippy `-D warnings`, `cargo fmt`, frontend `tsc --noEmit` and build clean. Bench repair verified in software: Check no longer fails on an existing recording, Stop is available after failure/existing state, and `SDLOG_MODE`/`SDLOG_BACKEND` are read-only hardware readouts. Physical capture acceptance requires the connected 6X and firmware identification. |
| 7 — Log-transfer timing, validation and ownership | Software complete | 771 core lib tests pass (1 ignored), including a byte-exact 960 s simulated transfer that keeps the retry budget reset, malformed-chunk rejection, and gap detection; 106 app lib tests pass (1 ignored), including the lag/lifecycle route classifier and replaced-runtime abort; clippy `-D warnings` and `cargo fmt` clean. |
| 8 — Per-test coordinator + recovery manifest | Software complete; 20-run bench pending | 779 core lib tests pass (1 ignored), including 8 coordinator tests (pre-roll gate, post-roll before stop, abort with motor release, stop timeout, capture failure, interruption); 111 app lib tests pass (1 ignored), including atomic manifest round-trip, safe test ids, still-recording reconciliation and finished-manifest preservation; 4 motor-service + 2 lifecycle integration tests pass; clippy `-D warnings`, `cargo fmt`, frontend `tsc --noEmit` and build clean. The manifest is written at every boundary under `<app_data>/motor-tests/<test_id>/manifest.json`; reconnect reconciles pending runs. The 20-consecutive-artifact acceptance still requires the connected 6X. |
| 9 — Responsive workspace + event contract alignment | Software complete; Task 9b (generated contracts) deferred | Task 9a/9c implemented: fixed header with target/armed/SD/configured-rate chips and durable failures, motor comparison table (expected pulse inverted from the sent command, ±5 µs tolerance), split PX4 SD capture / log retrieval, collapsible logging and signal legend, and one-scroll responsive layouts at 1024×640, 1366×768 and 1920×1080. Verification: `tsc --noEmit`, `vite build`, and `check-motor-controls.mjs` pass (quick/hold starts, header STOP, tracking math, persistent/dismissible errors, preset re-lock, independent capture, log save/delete, three-viewport screenshots). Task 9b is deferred by the owner; bench items remain Deferred until hardware/artifacts are available. |

Task 1 introduces typed transport-only `StopReceipt`/`MotorTestStopError`,
retains pending releases after failures, and makes both desktop stop commands
await actual writes. Stop events now carry a named `reason` field.

Task 2 routes target parameter transactions through the core `ParamService`
bound to the dedicated serial target (`actuator_param_read`/`actuator_param_set`),
adds `set_confirmed` (echo + read-back verification, mismatch is an error),
typed `ParamError` variants, transaction deadlines, capability-based integer
encoding, target-filtered routes, and removes the automatic
`COM_MOT_TEST_EN=1` write on connect (discovery is read-only).

Task 3 binds every session to a `MotorTestTargetRef` (connection + system +
component) and the declared `MotorTestLoad`. All starts run one readiness check
that requires a fresh, matching, disarmed PX4 heartbeat before any
`ACTUATOR_TEST` is sent. Reusing a session requires both target and load to
match; an idle mismatch is stopped with confirmed release before replacement,
and a sending session refuses replacement. The core session now holds its
first heartbeat without emitting a frame, so a stale pre-connect heartbeat can
never produce a command burst.

Task 4 binds the SITL feed to the dedicated source connection, node and PWM
bank. `PWM_MAIN_FUNC1..8` / `PWM_AUX_FUNC1..8` are read from the serial target
on connect and via `actuator_refresh_mapping`; PX4 functions 101–104 map to
Motor1–4. `SERVO_OUTPUT_RAW` is stored per MAIN/AUX bank, so a foreign node,
foreign port or AUX frame cannot overwrite MAIN feedback. The window shows the
mapped channel for each motor and feeds the µs reading from that channel;
unmapped, unreadable and duplicate assignments are explicit.

Task 5 reads each mapped channel's `PWM_*_MIN/MAX/REV/DISn` and the target's
`THR_MDL_FAC`, builds one `PwmModel` per motor, and sends the models to the
session before every start. Manual and preset values are fractions of the
target channel and are compensated before encoding; forwarded SITL pulses are
first converted to the same target fraction, so a reversed or narrow-range
output still reproduces the requested pulse. Missing or invalid parameters
fall back to the identity model; unmapped motors send identity. The right
panel shows the endpoints, reversal, thrust factor and disabled state.

Task 6 adds one target-shell controller per serial target. "Check logger" reads
`logger status` through `SERIAL_CONTROL` (shell device) and reports Ready,
Recording (this test's file) or Existing recording; it also refreshes
`SDLOG_MODE`/`SDLOG_BACKEND` for read-only display. Start sends `logger on`; the
Recording state requires a new `Log file:` path, then every motor start checks
that phase before creating or reusing a session. An existing recording never
authorises motor commands. Stop releases motors first, then re-observes status
before `logger off`, and requires stable "Not logging" observations inside the
finalization window. Stop remains available after a failed check, an existing
recording or a timed-out exchange. A non-file logger backend, disabled storage,
rejected commands, shell timeouts and path reuse are explicit failures; the
status/path is shown beside the logging controls. Capture is link-scoped and
reset on target replacement.

Risk boundary: only `logger status`, `logger on` and `logger off` are written,
only to the dedicated target's shell — never the primary link — and no
parameter is written. `SDLOG_MODE`/`SDLOG_BACKEND` changes are restart-required
and stay outside the per-test flow; the panel shows the observed values. The
PC-side received-message recorder was removed from this window because its
"Logger on/off" control never affected the PX4 SD file. Capture start/check is
refused while a motor session is active; a dropped link marks Recording failed
rather than leaving it trusted. Task 8 adds the per-test pre/post-roll,
manifest and recovery described below.

Task 7 fixes the log-download protocol. `LogProtocol::handle` now takes the
observation instant and every accepted `LOG_DATA` chunk (and `LOG_ENTRY`)
re-anchors the request timeout and clears the retry counter, so a healthy
transfer is no longer killed after the ~6 s retry budget. Chunks are validated
against the 90-byte frame and the declared log size before any buffer resize,
so a malformed `count`/`offset` is an explicit `MalformedChunk` error rather
than a panic or over-allocation; a chunk past the collected tail but inside the
log remains a `Gap`. Both download runtimes (primary link and the serial
target) now use a shared route classifier: broadcast `Lagged` and ordinary
connection lifecycle events are skipped, and only a closed bus ends the task;
replacing a runtime drops the old one, which aborts its task. New core tests
cover a 960 s simulated byte-exact transfer, malformed count, offset past the
declared size and a preserved gap; app tests cover the classifier and
replacement abort.

Task 8 adds a pure `RunCoordinator` per test. After the capture service reports
a new file, the run waits a 1 s pre-roll before the operator's existing START
is accepted. A transport-confirmed motor start moves it to `MotorRunning`; any
confirmed release starts a 1 s post-roll, after which the coordinator requests
`logger off`. Abort releases the motors first. A link/target loss marks the run
interrupted. The app writes `<app_data>/motor-tests/<test_id>/manifest.json`
atomically at every boundary with the test id, remote log path, FC identity,
mapping, read-only `SDLOG_MODE`/`SDLOG_BACKEND`, timestamps, stop reason and
error. On reconnect, pending manifests are reconciled against live `logger
status`: a still-recording matching file stays `interrupted`, otherwise the run
is closed as `reconciled`. A new run is refused while one is active.

| Order | Work | Acceptance criterion |
|---:|---|---|
| **1 — P0** | Repair Stop/ESTOP release delivery and receipts. | All configured releases complete transport writes before success; forced write failure never succeeds; no later active refreshes. |
| **2 — P1** | Fix parameter decoding, typed IPC and write confirmation. | Integer `0/1/2/−1` decode exactly; immediate echoes are captured; mismatches never succeed; transactions finish within an overall deadline. |
| **3 — P0/P1** | Bind services/readiness to target identity; enforce load on every start. | No active commands before fresh matching disarmed state; target changes invalidate pending work; restart under a different load applies its limits. |
| **4 — P1** | Add node/port/channel/function mapping. | Foreign frames cause zero source updates; permuted physical channels receive their intended commands and feedback. |
| **5 — P1** | Implement target-compensated PWM matching. | Offline round-trip error ≤1 µs; all mapped physical pins track within ±5 µs after ramp/slew settling. |
| **6 — P1** | Add target-shell SD capture control/status. | Capture start is confirmed before motor commands; incompatible modes, absent storage and uncertain responses produce explicit errors. |
| **7 — P1** | Repair log-transfer timing, validation and ownership. | A healthy transfer lasting ≥60 seconds completes byte-exactly; malformed chunks do not panic; replacement leaves no old runtime active. |
| **8 — P1** | Add the per-test coordinator and recovery manifest. | 20 consecutive disarmed tests create distinct artifacts; every logger stop follows motor release; interrupted runs reconcile correctly. |
| **9 — P2** | Implement the simplified responsive workspace and align existing event contracts. | Screenshots/manual verification at 1024×640, 1366×768 and 1920×1080; TypeScript strict check and build pass; existing Rust payloads match frontend contracts; stop controls always visible; stale/unknown data never looks confirmed. |

### Proposed backend contracts

Reuse the existing typed parameter model and introduce explicit bench operations. These are proposed signatures and error types, not implementation:

```rust
pub enum MotorIntent {
    Pulse(PwmUs),
    Disarmed,
    Release,
}

pub enum BenchError {
    TargetChanged,
    UnsupportedFirmware,
    InvalidMapping,
    UnreachablePulse,
    ParameterTimeout,
    ParameterMismatch { requested: ParamValue, observed: ParamValue },
    CommandRejected { result: u8 },
    MotorReleaseUnconfirmed,
    LoggerStateUnconfirmed,
    ArtifactInvalid,
    Transport(String),
}

pub async fn read_target_param(
    target: TargetRef,
    name: &str,
) -> Result<ParamObservation, BenchError>;

pub async fn set_target_param_confirmed(
    target: TargetRef,
    request: ParamWrite,
) -> Result<ParamObservation, BenchError>;

pub fn pwm_to_actuator_command(
    model: &VerifiedPwmModel,
    intent: MotorIntent,
) -> Result<ActuatorCommand, BenchError>;

pub async fn start_bench_run(
    config: VerifiedBenchConfig,
) -> Result<TestId, BenchError>;

pub async fn stop_bench_run(
    test: TestId,
    reason: StopReason,
) -> Result<StopReceipt, BenchError>;
```

`StopReceipt` should separately report transport delivery, command-result evidence and observed output state.

### Device-write risks to address

| Failure | Required handling |
|---|---|
| Wrong FC or stale target selection | Validate identity and generation for every operation. |
| Reconnect replays old motor commands | Expire controls and require a fresh start. |
| Logger control affects an existing capture | Establish capture ownership before issuing control commands. |
| Persistent settings differ from running configuration | Track configuration receipts and activation separately. |
| Shell contention or uncertain response | Serialize commands and reconcile state instead of reporting success. |

---

## 7. Verification and recommended first step

The review numerically checked the compensation model using **20 pulse cases across five range/model/reversal configurations**. The predicted outputs recovered the requested pulse widths within the proposed **1 µs** offline criterion. It also confirmed the bytewise integer examples behind the enable-indicator defect.

Hardware pulse measurements and SD-capture tests remain pending physical
acceptance. When the required hardware or artifacts are unavailable, record
these checks as deferred and proceed with the software/UI acceptance above.

**Task 9a/9c are software complete; Task 9b is deferred by the project owner.**
Tasks 1–8 are software complete. For later device verification, collect the
available PX4 release/hash, MAIN/AUX wiring and function assignments, and
establish whether the reported 1000 µs came from the UI or a pulse-width
measurement. Unavailable information stays explicitly unknown rather than
blocking the UI task.

### Firmware references

- [PX4 actuator-test processing](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/lib/mixer_module/actuator_test.cpp)
- [PX4 motor conversion](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/lib/mixer_module/functions/FunctionMotors.hpp)
- [PX4 logger implementation](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/logger/logger.cpp)
- [MAVLink parameter protocol](https://mavlink.io/en/services/parameter.html)
