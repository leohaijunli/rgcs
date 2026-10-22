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
| `SDLOG_MODE` | Armed until disarm — `0` | Confirmed | Read / Select / Apply |
| `SDLOG_BACKEND` | SD + MAVLink — `3` | Restart pending | Read / Edit / Apply |

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
   - **PC telemetry recording**
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
8. **Generate IPC types from Rust.**
   Current hand-maintained status/event shapes have drifted. Stop events also lack the named `reason` field expected by the frontend.

While running, freeze load, routing and mapping changes. Changes made while idle should reset the start confirmation.

---

## 6. Staged implementation plan

Each row should be a separately approved, verified change.

### Execution status

| Task | Status | Verification |
|---|---|---|
| 1 — Stop/ESTOP | Software complete | 43 focused core unit tests, 4 motor-service integration tests, 2 lifecycle integration tests; `cargo check -p maggcs-app --tests` and Rust formatting passed. Controlled transport regressions hold every write confirmation, fail motor 2 while attempting the remaining releases, and verify emergency latching. |
| 2 — Target parameters | Software complete | 730 core lib tests + 8 integration tests pass; clippy `-D warnings` clean. Session regressions: `_HASH_CHECK` sentinel excluded, out-of-range index dropped, REAL32 echo of an INT32 set is a type mismatch (never confirmation), encoding frozen mid-transaction. Codec already proves `0/1/2/−1` byte-concat round-trips. |
| 3–9 | Pending | Implementation will proceed in order. Physical pulse measurements and SD capture acceptance require the connected 6X and firmware identification. |

Task 1 introduces typed transport-only `StopReceipt`/`MotorTestStopError`,
retains pending releases after failures, and makes both desktop stop commands
await actual writes. Stop events now carry a named `reason` field.

Task 2 routes target parameter transactions through the core `ParamService`
bound to the dedicated serial target (`actuator_param_read`/`actuator_param_set`),
adds `set_confirmed` (echo + read-back verification, mismatch is an error),
typed `ParamError` variants, transaction deadlines, capability-based integer
encoding, target-filtered routes, and removes the automatic
`COM_MOT_TEST_EN=1` write on connect (discovery is read-only).

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
| **9 — P2** | Implement the responsive workspace and generated events. | Screenshots/manual verification at 1024×640, 1366×768 and 1920×1080; stop controls always visible; stale/unknown data never looks confirmed. |

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

Hardware pulse measurements and SD-capture tests remain the acceptance work for implementation.

**Start with the Stop/ESTOP correction, then the parameter decoding and read-back fix.** For device verification, collect the installed PX4 release/hash, MAIN/AUX wiring and function assignments, and establish whether the reported 1000 µs came from the UI or a pulse-width measurement.

### Firmware references

- [PX4 actuator-test processing](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/lib/mixer_module/actuator_test.cpp)
- [PX4 motor conversion](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/lib/mixer_module/functions/FunctionMotors.hpp)
- [PX4 logger implementation](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/logger/logger.cpp)
- [MAVLink parameter protocol](https://mavlink.io/en/services/parameter.html)
