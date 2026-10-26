# Motor Test Review: Correctness and Layout Remediation

- **Date:** 2026-10-24
- **Status:** Review complete; remediation in progress. MR-01–MR-04b,
  MR-06, MR-09–MR-19, MR-21 and MR-22a/b are software-complete;
  MR-20 required-stream acceptance passed on PX4 v1.17.0 SIH.
  MR-21 AM32 start/stop tones are implemented and software-verified. Physical
  sound/timing acceptance on the installed ESC firmware remains pending.
- **Baseline:** `35630e50fe4ae4bea843d5264502d9e4183e5c14` (`HEAD`,
  `feat(core): detect PX4 output protocol and model DShot ranges`). The review
  includes tracked edits, new modules/bindings, documentation, checks and screenshots.
- **Related:** [ADR-023](../adr/023-motor-test-protocols-rpm-hold.md),
  [ADR-024](../adr/024-standard-stream-management.md),
  [ADR-025](../adr/025-motor-control-permission.md),
  [Motor Test README](../../frontend/src/actuator/README.md),
  [Parameters README](../../frontend/src/params/README.md).
- **Execution rule:** Implement one small task at a time. Confirm the affected
  types, signatures and error contracts before implementation. Record actual
  acceptance evidence before checking off a task.

## 1. Confirmed Product Boundary

```text
MagGCS -- MAVLink ACTUATOR_TEST --> PX4 -- PWM / OneShot / DShot --> ESC
       <-- output and RPM telemetry --
```

The ESC is connected to PX4. MagGCS needs three operator-facing output families:
**PWM, OneShot and DShot**. Bidirectional operation and bitrate are DShot
attributes, rather than additional top-level UI families. Unknown/unread or
conflicting timer configuration within one bank is a readiness state, not a
fourth supported protocol. Different known families on MAIN/AUX are supported.

- Read the output configuration from PX4; keep the motor command normalized.
- Use the mapped `SERVO_OUTPUT_RAW` output for tracking and PX4 `ESC_STATUS`
  for RPM. Receiving RPM and selecting an output protocol are distinct facts.
- Keep the configured contiguous M1..MN set, with a maximum of four motors.
- RPM hold, RPM calibration and speed-tracking metrics are out of scope
  (2026-10-24). ESC RPM is display-only telemetry.
- Manual SD capture start/stop remains independent of motor start/stop.

### Operator correction — 2026-10-26

The unrequested fixed 0.3 demand cap is removed. Manual, preset and Forward
inputs use the full FC-read output range: an 1100–1900 µs model reaches 1500 µs
at 50% and 1900 µs at 100%. This supersedes the fixed-cap part of MR-04a and the
capped sine expectation recorded under MR-17.

The selected PX4 target is Motor Test's FC. A dedicated serial target supplies
its own heartbeat and parameters with no primary connection; **Use main serial
link** selects the hardware PX4 already on the main serial transport. Numerical
session tests, dedicated/borrowed serial PTY checks and the focused real-React
IPC regression pass. See the [verification and screenshots](../../frontend/src/actuator/README.md#selected-fc-and-full-range-verification--2026-10-26).

### Current support within this boundary

| PX4 output family | Output model and display | Current control support | Remaining acceptance |
| --- | --- | --- | --- |
| PWM | FC min/max, reversal and thrust model; µs | Manual, preset and pulse-preserving Forward | Required-read and refresh checks pass; physical range/release evidence is pending |
| OneShot | Pulse model; µs | Manual, preset and pulse-preserving Forward | Software unit/configuration checks pass; hardware evidence is pending |
| DShot | `DSHOT_MIN × 1999 … 1999`; counts | Manual/preset/normalized Forward; display-only ESC RPM; AM32 start/stop tones | Software conversion/plot/tone checks pass; hardware range/release/acoustic evidence is pending |

ESC RPM does not gate START; there is no RPM hold, calibration or performance
verdict. Keep the existing explicit rejection of 3D mode.

## 2. Review Findings at the Baseline

**Baseline conclusion:** variable motor counts, protocol-aware presentation,
capture independence and parameter readability still had software defects at
review time. The findings and numerical reproductions below preserve that
baseline. Current remediation status and evidence are recorded in §4 and §8.

Evidence labels distinguish executable reproductions from control-flow review.
An inherited dependency problem is explicitly identified below rather than
being presented as newly introduced code.

| ID | Priority | Finding and impact | Evidence / location |
| --- | --- | --- | --- |
| R1 | P1 | DShot Forward interprets source PWM microseconds as target DShot counts. An idle source can request substantial output. | Core reproduction; [target model wiring](../../crates/app-tauri/src/actuator_service.rs#L2179), [Forward conversion](../../crates/core/src/motor_test/source.rs#L275) |
| R2 | P1 | ESC verification uses an ordinary manual session. Sliders remain enabled and the backend accepts their values, breaking the one-motor/10% pulse boundary. | Browser IPC and core reproductions; [verification start](../../crates/app-tauri/src/actuator_service.rs#L2321), [manual update gate](../../crates/app-tauri/src/actuator_service.rs#L559) |
| R4 | P1 | A partially read function map can become `Complete`. Missing timer reads are also ignored by the detector used in the new start path. The inferred active set/protocol is not necessarily fully known. | Core reproduction; [map classification](../../crates/core/src/motor_test/mapping.rs#L235), [timer folding](../../crates/core/src/motor_test/protocol.rs#L156) |
| R5 | P1 | Mapping, models, protocols and DShot flags are published under separate locks. Refresh has no operation/idle guard and returns success after read failures were converted to missing values. The atomic snapshot/retain-on-failure contract is unmet. | Control-flow review; [refresh](../../crates/app-tauri/src/actuator_service.rs#L2210), [start snapshot](../../crates/app-tauri/src/actuator_service.rs#L244) |
| R9 | P2 | Command definitions are recreated on lookup, defeating reference-based deduplication. Plot rebuild keys omit units/labels, so a protocol change updates the list but leaves the mounted plot in its old unit. | Browser reproduction; [definitions](../../frontend/src/actuator/scopeStore.ts#L125), [deduplication](../../frontend/src/actuator/SignalScope.tsx#L245), [plot key](../../frontend/src/actuator/SignalScope.tsx#L491) |
| R10 | P2 | The store accepts null gaps only for `cmd.pwmN`; other series gaps are discarded and `spanGaps` joins invalid regions. | Store reproduction and plot review; [ingestion](../../frontend/src/actuator/scopeStore.ts#L339), [series](../../frontend/src/actuator/SignalScope.tsx#L530) |
| R11 | P2 | The entire tracking table can be folded during a run. Protocol is absent from the fixed header. Critical state must stay visible. | Browser reproduction and screenshots; [tracking fold](../../frontend/src/actuator/MotorTrackingTable.tsx#L106), [header](../../frontend/src/actuator/BenchHeader.tsx) |
| R12 | P2 | The new RPM freshness code inherits an incorrect ESC batch-index assumption: PX4 sends indices 0, 4, …, not 0, 1, …. ESC5–8 are consequently unavailable. | Core reproduction and PX4 source; [freshness](../../crates/app-tauri/src/actuator_service.rs#L3002), [existing aggregator](../../crates/core/src/signals/derived.rs#L41) |
| R13 | P3 | The parameter connection check can read the editor before the effect synchronizes its draft. One run failed with `5` instead of `12.5`; a retry passed. This is observed test fragility, not a demonstrated persistent parameter-value defect. | [Editor assertion](../../frontend/scripts/check-params-connection.mjs#L177), [draft effect](../../frontend/src/params/ParamsApp.tsx#L464) |

For R4, distinguish a board's absent parameters from failed reads of parameters
that should exist. Requiring nonexistent channels on every board would be a
different defect. A supported-board capability description should resolve that
distinction before a configuration is accepted.

For R12, the authoritative PX4 v1.17 stream sets
`msg.index = batch_number * batch_size`:
[ESC_STATUS.hpp](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/mavlink/streams/ESC_STATUS.hpp).
The multiplication in the aggregator predates this diff. MR-09 corrects it
end to end: the aggregator, scope samples and receive timestamps share the
same first-ESC range calculation.

### Numerically reproduced cases

| Case | Observed baseline result | Required result |
| --- | --- | --- |
| SITL raw 1000 µs, normalized source 0; DShot min 0.055 | Forward demand `0.471149`; PWM 1000–2000 gives `0` | Idle source produces no active motor demand |
| Verify M1 while synchronized sliders move to maximum | UI issues `SetValues([1,1,1,1])`; the manual session accepts overrides | Verification exclusively owns its selected motor and bounded output |
| M1 read as 101, MAIN2 unread, remaining functions disabled | `Complete`, active count 1, with an unreadable issue | Failed required reads prevent a complete snapshot |
| Command samples `(1,1500), (2,null), (3,1600)` | Only times 1 and 3 are retained | Retain the explicit gap at time 2 |
| PWM → DShot while a command plot remains mounted | Two command legend entries; list changes to counts but plot stays PWM | One series per ID; matching list, legend and axis units |

## 3. Contracts to Confirm Before Implementation

These are contract sketches, not implemented APIs. Confirm the relevant subset
for each task; refine it against the surrounding module before writing code.

```rust
// Unknown remains a configuration/readiness outcome.
enum OutputFamily { Pwm, OneShot, DShot }
fn output_family(protocol: OutputProtocol) -> Option<OutputFamily>;

enum ForwardError { InvalidSample, UnknownProtocol }
fn forward_fraction(
    raw_pwm_us: u16,
    source_fraction: f32,
    target: &PwmModel,
) -> Result<f32, ForwardError>;

enum CommandOwner { Manual, Preset, Forward }
struct SessionToken { generation: u64 }
enum OwnershipError { Busy, WrongOwner, ObsoleteSession }
fn authorize_manual_update(owner: CommandOwner) -> Result<(), OwnershipError>;

struct OutputConfigSnapshot {
    revision: u64,
    target: MotorTestTargetRef,
    map: OutputMap,
    models: Vec<PwmModel>,
    protocols: OutputProtocols,
    dshot_3d_enable: Option<i32>,
    dshot_bidir_en: Option<i32>,
}
enum OutputConfigError {
    UnreadableParameter { name: String },
    AmbiguousMapping,
    UnknownProtocol,
    TargetChanged,
    SessionBusy,
}
async fn read_output_config(
    params: &ParamService,
) -> Result<OutputConfigSnapshot, OutputConfigError>;
```

The final configuration-reader signature must carry enough target/capability
context to populate and validate its snapshot.

Further per-task contracts:

- **Plots:** identity is signal ID plus configuration/unit revision. Define an
  explicit gap value accepted for every applicable series.
- **Frontend:** confirm props for compact setup, motor rows and a shared fold
  section. Distinguish user pinning from temporary warning-driven expansion.

### MR-09 confirmed contract (2026-10-24, implemented)

```rust
pub fn esc_frame_range(
    first_esc_index: u8,
    esc_count: usize,
) -> std::ops::Range<usize>;

fn store_esc_status(
    telemetry: &mut MotorTelemetry,
    esc: &ESC_STATUS_DATA,
    received_at: std::time::Instant,
);
```

Existing `EscAggregator::feed/sample/signals` and `scope::esc_samples`
signatures are retained. Internal samples are `Vec<Option<EscSample>>`;
receive timestamps are per ESC (`MAX_ESC = 8`). An out-of-range frame is
ignored without a new error type; a partial final frame is clipped to the
supported slots. Unseen ESCs emit no aggregate signal and remain `None` in
status, while a reported zero remains valid data.

### MR-10 confirmed contract (2026-10-24, implemented)

```rust
pub struct ScopeOutputMeta {
    pub connection_id: usize,
    pub revision: u64,
    pub motor: u8,
    pub channel: ServoChannel,
    pub unit: OutputUnits,
}

pub enum OutputScopeError {
    IncompleteConfiguration,
    InvalidMapping { motor: u8 },
    MissingModel { motor: u8 },
    UnknownProtocol { motor: u8 },
}

pub fn output_metadata(
    connection_id: usize,
    revision: u64,
    map: &OutputMap,
    models: &[PwmModel],
) -> Result<Vec<ScopeOutputMeta>, OutputScopeError>;
```

`ScopeSample.output` carries this metadata for predicted commands and mapped
FC feedback. `command_samples()` emits `cmd.pwmN`; `mapped_feedback_samples()`
emits `fc.motorN` only for the configured MAIN/AUX channel. The frontend's
`configureOutputs()` accepts one complete, monotonic snapshot, clears only
output history when target/revision/mapping/unit changes, and rejects late or
untagged samples. Stable signal IDs retain operator selection. Plot identity
includes ID, label, unit and configuration context; mixed PWM/DShot outputs use
separate axes in the same command pane rather than relabelling one another.

### MR-11 confirmed contract (2026-10-24, implemented)

```rust
#[serde(rename_all = "snake_case")]
pub enum ScopeGapReason {
    Released,
    Stale,
    RunBoundary,
}

pub struct ScopeSample {
    pub id: String,
    pub t: f64,
    pub v: Option<f64>,
    pub gap: Option<ScopeGapReason>,
    pub output: Option<ScopeOutputMeta>,
}
```

The backend sends command release as an explicit typed gap rather than relying
on non-finite JSON conversion. The frontend assigns each signal a stale
deadline, inserts one stale gap per overdue interval, and translates a run-end
marker into boundaries for existing command, mapped-feedback and RPM series.
Gaps never create unseen signals. New values clear the displayed gap state but
retain the null in history; all applicable uPlot series use `spanGaps: false`.

### MR-12 confirmed contract (2026-10-24, implemented)

```ts
type HeaderOutputState =
  | { kind: 'ready'; protocol: string }
  | { kind: 'blocked'; protocol: string; reason: string }

function activeProtocolSummary(status: ActuatorStatus): string
function outputConfigurationIssue(status: ActuatorStatus): string | null
```

The fixed header derives armed/heartbeat and active-protocol state from the
same status snapshot. Unavailable firmware/rate placeholders and capture state
do not occupy safety-header space. Tracking is forced open and its Fold action
disabled while running, while target feedback is stale, or while the output
configuration is blocking. User-expanded motor details are independent and
remain pinned after a temporary warning clears.

### MR-14 confirmed contract (2026-10-24, updated 2026-10-25)

```ts
type AnalysisView = 'cmd' | 'mag' | 'esc' | 'bat'
type PaneId = 'top' | 'bottom'
type TraceColors = Record<string, `#${string}`>

interface PaneSelection {
  checked: string[]
  seen: string[]
}

interface PersistedScope {
  panes: Record<PaneId, PaneSelection>
  activePane: PaneId
  windowS: number
  listWidth: number
  colors: TraceColors
}

class ScopeStore {
  readonly originT: number | null
  elapsedAt(epochSeconds: number): number
  clear(): void
}
```

The analysis workspace renders two independently selected stacked plots. The
compact signal list is searchable, exposes a color picker for every signal, and
has a Top/Bottom switch that chooses which plot its checkboxes edit. Both plots
draw from the same rolling time window and cursor; the same signal may appear in
either or both, and mixed units keep separate axes. Defaults are Output on top
and Magnetic field below. The list separator supports pointer dragging, Arrow
keys, Home/End and double-click reset, with a persisted 220–560 px width
(300 px default). Valid six-digit colors persist by stable signal ID and alter
presentation only. The first accepted value defines x=0 s; markers and gaps do
not. A rolling window displays elapsed intervals such as 70–100 s. Clear, target
disconnect and reload reset the clock; configuration, selection and color
changes do not. Older `checked`/`seen`/`activeView` preferences migrate into the
pane structure on load.

### MR-16 confirmed contract (2026-10-24, verified)

```rust
async fn start_capture_locked(
    state: &AppState,
    test_id: String,
) -> Result<SdCaptureStatus, String>;

async fn stop_capture_locked(state: &AppState) -> Result<SdCaptureStatus, String>;

fn ensure_coordinated_run_start(state: &AppState) -> Result<(), String>;
```

Capture failures use `SdCaptureError`; coordinator failures use `RunError`.
The application command boundary formats those errors as strings. The shared
capture helpers do not require an idle motor session. `actuator_run_start`
applies the idle-session/active-run guard before calling the start helper;
its failed-setup cleanup may call the stop helper before a motor is started.
The run ticker calls the target-bound `request_stop_for_target` only for a
coordinator `StopCapture` action. Run abort confirms motor release first.
Manual STOP and E-STOP bypass the operation lock held by capture requests.

### MR-21 confirmed contract (2026-10-24, implemented)

```rust
enum MotorTone { Off, Rising, Falling, Sweep, Triple }
struct MotorToneSettings { start: MotorTone, stop: MotorTone }
enum MotorToneTrigger { Starting, Stopped }

struct MotorToneRequest {
    target: MotorTestTargetRef,
    config_revision: u64,
    session_generation: u64,
    trigger: MotorToneTrigger,
    tone: MotorTone,
}

enum MotorToneError {
    UnsupportedOutput { motor: u8 },
    InvalidMapping { motor: u8 },
    TargetChanged,
    SessionChanged,
    Busy,
    Cancelled,
    Timeout,
    Shell { message: String },
}

async fn actuator_set_tones(
    state: State<'_, AppState>,
    settings: MotorToneSettings,
) -> Result<(), String>;

async fn play_motor_tone(
    state: &AppState,
    request: MotorToneRequest,
) -> Result<(), MotorToneError>;
```

Start and stop have independent saved selections, including Off. The
defaults for the operator's AM32/DShot bench are Rising (`beep1`) and Falling
(`beep2`). A request captures target, mapping revision and session generation;
pending work is revalidated at dispatch and before enabling motor output.
Tone status/errors remain distinct from the motor release receipt. Successful
shell completion means the command was accepted, not that an audible tone or
its completion was measured.

The frontend persists both selections under `maggcs.actuator.tones` and
synchronizes them through `actuator_set_tones` before enabling START. The
backend captures the stop choice with the run, and all three command sources
share a cancellable start path. Each run has a generation-bound event observer.
The target shell worker owns queued jobs and the playback fence: a cancelled
START returns promptly, while any already dispatched tone finishes its exchange
and nominal duration plus 250 ms margin before another START (including Off)
may proceed. Stop tones wait 1 s after release before checking the driver.
These are conservative software delays, not measured rotor-stop or sound times.

### MR-18a confirmed contract (2026-10-24, implemented)

MR-18 starts with the implemented catalog/search slice; the live rate-manager
contract follows in MR-18b. The catalog is derived from the compiled `common` dialect.

```rust
struct StreamField {
    name: String,
    wire_type: String,
    units: Option<String>,
}

enum Px4StreamSupport { DocumentedV1_17, Unknown }

struct StandardStream {
    id: u32,
    name: String,
    fields: Vec<StreamField>,
    px4_support: Px4StreamSupport,
}

enum StreamLookupError {
    UnknownMessage { selection: String },
    AmbiguousSelection { selection: String, candidates: Vec<u32> },
}

fn search_standard_streams(query: &str) -> Vec<StandardStream>;
fn resolve_standard_stream(selection: &str) -> Result<StandardStream, StreamLookupError>;
```

Search ranks exact name/ID matches before partial name/field matches. Resolution
requires an exact message name or numeric ID; a partial query never silently
selects a message. Firmware support metadata records documented PX4 v1.17
streams, not a claim that a connected FC is currently sending frames. Verified
Verified uORB aliases are implemented under MR-19 below.

Acceptance: `BATTERY_STATUS`/147, `SCALED_IMU`/26 and `HIGHRES_IMU`/105 resolve
to their exact compiled definitions and rank first by name or ID. Field types
(including array sizes) and units match the dialect. Unknown and ambiguous
selections produce the typed errors above. Catalog entries never invent an ID.

### MR-18b confirmed contract (2026-10-24, implemented)

The first live manager binds to the Motor Test window's dedicated PX4 target.
The status snapshot exposes that target identity so an IPC request can reject
a replaced connection before writing a rate command.

```rust
enum StreamRate { Hertz(f64), Disabled }

struct StreamRequest {
    message_id: u32,
    rate: StreamRate,
    allow_unknown_support: bool,
}

enum StreamReceiveState { Waiting, Receiving, Stale }

struct StreamStatus {
    message_id: u32,
    requested_rate: StreamRate,
    observed_hz: Option<f64>,
    last_seen_age_ms: Option<u64>,
    receive_state: StreamReceiveState,
    error: Option<String>,
}

enum StreamRequestError {
    Lookup(StreamLookupError),
    InvalidRate,
    UnknownSupportNotConfirmed,
    NotConnected,
    TargetChanged,
    Transport { message: String },
}

fn stream_interval_us(rate: StreamRate) -> Result<i32, StreamRequestError>;

async fn actuator_stream_set_rate(
    state: State<'_, AppState>,
    target: MotorTestTargetRef,
    request: StreamRequest,
) -> Result<StreamStatus, String>;
```

Finite positive rates must produce a representable positive microsecond
interval; Disabled maps to -1. Unknown catalog support requires an explicit
operator choice. Requested/disabled state is separate from received traffic;
an observed rate appears only after frames arrive. The UI must not infer that
PX4 honored a rate solely from sending the request. Requests persist per
endpoint/system/component and reapply once per transport reconnection, with
exact-ID deduplication and target-scoped observations. UI integration adds
catalog search, exact selection, immediate rate editing and disable controls.

Before implementation, account for wrong-target requests, unrepresentable
rates, a stale reply from a replaced link, firmware clamping/ignoring requests,
duplicate reconnect handlers and competition with existing default stream
requests. Motor STOP/E-STOP must retain its release priority.

Acceptance: each apply/change/disable produces one correctly addressed interval
command; invalid or unconfirmed selections send none. The receiver ignores
foreign nodes and old connection generations, reports live/stale data correctly,
and reconnect restores saved requests without duplicates. Loopback/IPC tests
precede the separate SITL/firmware observations in MR-20.

Actual serialization uses `{ "kind": "hertz", "hz": 20.0 }` or
`{ "kind": "disabled" }`. The live manager owns all dedicated-target default
rate requests (36/20 Hz, 105/50 Hz, 147/5 Hz, 291/10 Hz); saved overrides replace
defaults by exact ID. The former attachment and 3 s scope reassertion writers
were removed so they cannot undo an operator's rate or disable request.

Requested rates persist in `app_config_dir/motor-streams.json` per canonical
endpoint/system/component. A request is saved before its bounded transport
write; a failed write retains the desired request with its error for explicit
Reapply or reconnect. The file is replaced atomically, and unreadable existing
preferences are reported rather than overwritten. Transport confirmation does
not imply a firmware ACK, frame receipt or honored rate.

The manager waits for this transport generation's target traffic, then restores
one effective request per ID. `ConnectionHandle` uses non-reused IDs plus a
monotonic transport generation; guarded writes reject expired, cancelled and
old-generation queue entries at the worker. Lost lifecycle events resynchronize
from the handle. Receive history resets on reconnect, rate changes, broadcast
lag or a stale gap. Observed Hz needs at least two fresh arrivals; a Disabled
request is independent from whether frames still arrive. Stale thresholds are
`max(2 s, 3 × requested period)` with bounded arrival history.

The **Streams…** action opens a bounded analysis workspace. Search and selection
show fields and wire units, immediate valid Hz edits, Disable and explicit
unknown-support confirmation. Opening/closing/reloading the panel sends no
restore requests. Controls use the backend's current target identity, and
queued edits preserve the latest valid draft. Fixed motor STOP/E-STOP remain
outside the panel and bypass stream/persistence work. The panel positions itself
between the header and actual capture-drawer bounds, including Field mode.

### MR-19 confirmed contract (2026-10-25, implemented)

Scope confirmed: the six mappings below, shared `vehicle_imu` as ambiguous, and
`NoStandardMapping` for any unverified topic. Keep the confirmed standard-message path. Source mappings are documentation
metadata for verified PX4 v1.17 stream implementations, not a generic uORB
transport or a promise of every topic field being sent.

```rust
struct Px4StreamMapping {
    uorb_sources: Vec<String>,
    source_url: String,
    note: String,
}

// Add to StandardStream:
// px4_mapping: Option<Px4StreamMapping>

// Add to StreamLookupError:
// NoStandardMapping { topic: String }

fn px4_stream_mapping(message_id: u32) -> Option<Px4StreamMapping>;
fn resolve_uorb_stream(topic: &str) -> Result<StandardStream, StreamLookupError>;
```

`search_standard_streams` includes exact verified uORB-source aliases after
exact standard name/ID matches. A unique verified alias may resolve; a topic
shared by several messages produces `AmbiguousSelection` with candidate IDs.
Topics without a verified standard mapping produce `NoStandardMapping`, and
the UI states the installed firmware cannot expose arbitrary uORB topics through
an interval request. Selection still sends only an exact compiled message ID.

Source-reviewed initial mappings:

| Standard message | PX4 v1.17 uORB inputs | Interpretation |
| --- | --- | --- |
| BATTERY_STATUS (147) | `battery_status` | Updated battery instances; selected fields and engineering-to-wire scaling |
| SCALED_IMU (26) | `vehicle_imu`, `vehicle_imu_status`, `sensor_mag` | Instance 0; integrated accel/gyro converted to scaled axes, raw sensor-mag values |
| HIGHRES_IMU (105) | `vehicle_imu`, `sensor_selection`, `vehicle_magnetometer`, `vehicle_air_data`, `differential_pressure`, `estimator_sensor_bias`, `estimator_selector_status` | Selected IMU; estimated bias removal and combined magnetic/pressure fields |
| ESC_STATUS (291) | `esc_status` | RPM/voltage/current in first-index ESC batches |
| GPS_RAW_INT (24) | `sensor_gps` | Instance 0; selected GPS fields and no-GPS fallback |
| SERVO_OUTPUT_RAW (36) | `actuator_outputs` | Instances 0/1 become stream names SERVO_OUTPUT_RAW_0/_1 and the message's port field |

Evidence is the corresponding
`https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/mavlink/streams/<MESSAGE>.hpp`.
The canonical search names `battery_status`, `scaled_imu` and `highres_imu`
remain exact matches for their standard messages; `scaled_imu` and `highres_imu`
are not themselves PX4 uORB topic names.

Acceptance: initial mappings expose verified source links and notes; names,
numeric IDs and canonical aliases each resolve correctly. Shared `vehicle_imu`
is ambiguous rather than silently selecting a stream. Unknown topic queries
produce an actionable refusal and no interval request. Existing add/rate/
disable and firmware-support gating regressions remain valid. Actual requested-
rate receipt on firmware is recorded separately by MR-20.

## 4. Ordered Remediation Tasks

### A — Output correctness and ownership

- [x] **MR-01 — Correct DShot Forward conversion (R1).**
  - Files: `source.rs`, output-model tests and motor-service integration tests.
  - Work: retain pulse-preserving PWM/OneShot behavior; use the source's
    normalized demand for a DShot target before target-model compensation.
  - Acceptance: source idle produces zero demand/release; 0%, 50% and 100%
    source cases map monotonically into the target range. Exercise 1/2/4 motors,
    reversal and nonzero thrust factor. No inactive motor receives a command.

- [x] **MR-02 — Give verification exclusive output ownership (R2).**
  - Historical fix: the verification feature and its ownership guard are
    subsequently removed under MR-06.
  - Files: `actuator_service.rs`, `motor_test_state.rs`, command boundary and UI.
  - Work: authorize updates by owner/session, disable irrelevant controls, and
    prevent delayed slider requests from overriding a verification or hold.
  - Acceptance: direct IPC manual updates during verification are refused;
    only the selected motor receives the 10% pulse; STOP/E-STOP remain accepted;
    success, timeout, cancellation and send failure all exercise release writes.

- [x] **MR-03 — Reject incomplete required configuration (R4).**
  - Files: `mapping.rs`, `protocol.rs`, target configuration reader.
  - Work: distinguish supported-board absent channels/groups from failed reads;
    preserve explicit unknown/partial diagnostics instead of inferring disabled.
  - Acceptance: a timeout on a required FUNC/TIM parameter blocks START;
    known-disabled outputs and supported reduced-channel boards remain usable;
    a hidden possible duplicate is not accepted as a complete mapping.

- [x] **MR-04 — Publish one versioned configuration snapshot (R5).**
  - Depends: MR-03.
  - Files: target state, initial read, refresh, status and prepare-start paths.
  - Work: validate and publish map/models/protocol/flags together; serialize
    refresh with relevant session operations and reject obsolete-target results.
  - Acceptance: concurrent refresh/status/start tests cannot observe mixed
    revisions; failed refresh preserves the last complete snapshot and reports
    an error; refresh cannot change a running session's configuration.

- [x] **MR-04a — Remove the reduced-prop bench mode (operator directive).**
  - Files: `motor_test_state.rs`, `actuator_service.rs`, command boundary,
    `ActuatorApp.tsx`, motor-control regression.
  - Work: delete the prop-load choice and typed phrase; every source uses the
    full-prop envelope (0.3 maximum command, 2.0/s slew, 3 s hold). Keep STOP
    and E-STOP available in every ordinary session.
  - Acceptance: no load argument remains in commands or state; manual, preset,
    and Forward all use the same limits; short clicks never
    start; the UI regression passes at all three viewport sizes.

- [x] **MR-04b — Require two confirmations for every delete (operator directive).**
  - Files: project delete, parameter-group delete, actuator/log delete, i18n.
  - Work: route destructive UI actions through two separate confirmation
    prompts; declining either cancels without invoking the backend.
  - Acceptance: project disk delete, parameter-group delete, SD-log delete and
    delete-all each require two confirmations; the motor UI check observes both
    dialogs and no delete command is sent after a decline.

### B — RPM telemetry integrity and product cleanup

- [x] **MR-06 — Remove the RPM-hold product surface.**
  - Scope decision (2026-10-24): RPM hold, calibration and speed evaluation
    are not required in any form.
  - Files: `ActuatorApp.tsx`, `benchTypes.ts`, scope catalog/styles,
    `actuator_service.rs`, target/session state, command registration,
    hold/calibration/mapping modules, generated bindings, tests and docs.
  - Work: remove hold start/stop, target entry, minimum learning, manual
    overrides, feedforward, metrics and verdicts. Remove motor→ESC mapping
    verification UI/commands and the associated state/modules; keep raw ESC RPM
    as display-only telemetry indexed by ESC.
  - Acceptance: no RPM-hold, RPM-calibration, mapping-verification or
    performance-verdict control or command is reachable; typechecking,
    ordinary-session STOP/E-STOP release tests and the three-viewport UI
    regression pass. The RPM card remains read-only and distinguishes live,
    stale and missing samples.

- [x] **MR-09 — Correct PX4 ESC batch indexing (R12).**
  - Files: `signals/derived.rs`, telemetry freshness and scope ESC samples.
  - Work: interpret `ESC_STATUS.index` as the first ESC index and make freshness
    bookkeeping agree with aggregation. Keep never-seen data distinct from zero.
  - Acceptance: index 0 maps to ESC1–4; index 4 maps to ESC5–8; each bank can
    independently be fresh, stale or missing; existing first-bank behavior passes.

### C — Plot correctness and compact layout

- [x] **MR-10 — Stabilize chart identity and units (R9).**
  - Files: `scopeStore.ts`, `SignalScope.tsx`, FC-output series producers.
  - Work: deduplicate by ID, rebuild on unit/configuration changes, and prevent
    old-unit history from being relabeled as new-unit data. Label mapped FC
    feedback correctly as well as the predicted command.
  - Acceptance: exactly one legend entry per selected ID; PWM/OneShot use µs;
    DShot uses counts; switching protocol updates the actual plot axes/legend;
    MAIN/AUX feedback is not combined into a misleading motor trace.

- [x] **MR-11 — Preserve stale/released gaps end to end (R10).**
  - Files: command/feedback sample producers, store ingestion, plot series and
    IPC check.
  - Work: retain explicit gaps for command and measured output/feedback
    series; emit run-end boundaries and disable interpolation across invalid
    regions.
  - Acceptance: the three-sample gap reproduction retains its null; stale and
    stopped intervals are visibly broken; separate runs are not joined by a
    continuous measured-RPM or active-command line.

- [x] **MR-12 — Keep critical state visible (R11).**
  - Files: `BenchHeader.tsx`, tracking table and fold behavior.
  - Work: put active output protocol and relevant stale/limit state in the fixed
    header; make critical motor rows non-collapsible while running or warning.
  - Acceptance: protocol/armed/session/STOP/E-STOP remain visible during scrolling;
    clicking Fold during a run cannot hide current output/RPM state; user-pinned
    details remain open when temporary warnings clear.

- [x] **MR-13 — Compact setup and connection presentation.**
  - Depends: MR-12. Layout is specified in §5.
  - Files: `ActuatorApp.tsx`, extracted presentation components, `actuator.css`.
  - Work: put safety first in the left column, use a compact setup strip, cap
    desktop motor sliders at 360 px, and collapse healthy target configuration
    to a concise summary. Keep only target summary and display-only ESC RPM in
    the right column. Move capture/logging into a bottom utility drawer spanning
    the center/right workspace; its Capture Start/Stop trigger stays visible
    while logger configuration, retrieval, download and delete details default
    closed.
  - Acceptance: 1366×768 has no document overflow; target setup does not dominate
    the right column after connection; 1/2/4-motor controls and ranges remain
    readable; desktop motor sliders do not exceed 360 px; the right column has
    no logging card or expanded parameter prose; the collapsed logging bar and
    Capture Start/Stop are reachable; Field-mode action targets are at least
    44 px; no hold, calibration or speed-analysis panel consumes space.

- [x] **MR-14 — Consolidate motor data and analysis space.**
  - Depends: MR-10, MR-11, MR-13.
  - Files: motor rows/tracking, scope layout and UI regression.
  - Work: reduce repeated per-motor information and follow the Signal Inspector
    hierarchy: compact searchable/collapsible signal list, dedicated analysis
    toolbar, and two independently selected stacked plots instead of
    automatically stacking every pane. A Top/Bottom switch chooses which plot a
    checkbox edits; defaults are Output on top and Magnetic field below. Allow a
    persistent color override per signal ID, with the existing motor color as
    its default; changing color rebuilds presentation only and never clears
    history or sends a command. Raw ESC RPM is optional display telemetry; there
    is no hold, calibration or speed verdict.
  - Acceptance: at 1366×768 standard density, four active motor rows and two
    usable plots fit together; the 1024×640 layout keeps fixed critical controls;
    inactive motor slots do not clutter the motor view; capture and plot controls
    remain reachable in Field mode; each plot keeps a usable height and mixed
    units use separate axes; each selected signal has a keyboard-reachable color
    control whose value survives reload and protocol changes; the signal list is
    resizable by pointer and keyboard and its width survives reload.

### D — Regression and delivery reconciliation

- [x] **MR-15 — Make parameter-value checks deterministic (R13).**
  - Files: `check-params-connection.mjs`.
  - Work: wait for both the row value and synchronized editor draft, using an
    eventual assertion rather than a fixed delay or immediate read. Observe
    selected parameter, rendered row and editor in one browser task, and
    exercise recorded 120/150/250 ms parameter-reply delays.
  - Acceptance: delayed-read and reconnect fixtures pass without transient
    `5`/`12.5` failures; current/default wrapping and scroll-reset checks still pass.

- [x] **MR-16 — Cover manual-capture and coordinated-run boundaries.**
  - Files: capture/run adapter tests and the motor UI check.
  - Work: add backend evidence for manual capture while a motor test runs; audit
    callers of shared capture helpers so the coordinated pre-roll/post-roll
    contract remains explicit after removal of the helpers' idle-session check.
  - Acceptance: manual motor/capture START/STOP operations are independent in
    backend tests; a coordinated run follows its documented starting state and
    ordering; STOP/E-STOP is not delayed by a capture operation.

- [x] **MR-17 — Reconcile delivery status and hardware acceptance.**
  - Depends: the accepted remediation tasks above.
  - Files: ADR-023, module READMEs, FEATURES and check scripts.
  - Work: use actual test evidence for completion claims; record deliberate
    deferrals, current product scope and the agreed three-family presentation.
    Reconcile the README's fresh-unlock wording with the current ordinary STOP
    behavior (the local unlock is retained, while E-STOP requires a reset).
  - Acceptance: documentation matches executable behavior and screenshots;
    hardware-only bench cases remain pending until a documented run covers
    them. Final release acceptance requires that evidence.

### E — Standard MAVLink stream manager (operator request, 2026-10-24)

MR-18 provides the compiled standard-message catalog and live **Streams…**
manager on the dedicated Motor Test PX4 link. The Inspector's primary-link
per-signal `set_message_interval` remains a separate connection's control.
MR-19 supplies verified uORB source/alias metadata; MR-20 records the actual
PX4 v1.17.0 SIH end-to-end observations.

**Scope decision (2026-10-24): the target firmware cannot be modified; only
standard MAVLink messages are in scope.** PX4 chooses which uORB topics become
MAVLink messages. If a topic has no standard MAVLink message/stream in the
installed firmware, the GCS cannot fetch it: `MAV_CMD_SET_MESSAGE_INTERVAL` only
changes the rate of an existing stream. No custom dialect import, custom
message-ID decoder or companion bridge is part of this plan. The UI must state
that limitation instead of offering a request that cannot work.

- [x] **MR-18 — Add a searchable standard-stream picker and rate manager (software).**
  - [x] **MR-18a:** compiled standard-message catalog, metadata, search and exact
    resolution; the contract in §3 was confirmed before implementation.
  - [x] **MR-18b:** live picker/rate manager, requested/observed/stale states,
    persisted requests and reconnect restoration; the adapter contract was
    confirmed after MR-18a.
  - Files: core MAVLink stream catalog, app command/service, telemetry streams
    panel, persisted connection settings, IPC regression.
  - Work: generate a catalog from the compiled `common` dialect with exact
    numeric ID, name, fields and units. Search ranks exact name and ID first,
    then field names and explicit uORB-topic aliases; never fuzzy-select an
    ambiguous ID. Mark messages known to be streamed by PX4 v1.17 and allow
    requesting others only with an explicit "support unknown" warning. Add one
    exact message through `SET_MESSAGE_INTERVAL`, with an editable rate in Hz
    (converted to microseconds), apply changes immediately, support disabling
    a stream (`-1`), show requested/observed/stale state, and reapply requests
    after reconnect. Reject non-finite/out-of-range rates and make clear that
    PX4 may clamp or ignore a request.
  - Acceptance: a query returns the exact message as the first row and displays
    its ID/fields/units; adding sends that ID and interval once; changing the
    rate sends one new interval immediately; disabling sends `-1` and stops the
    observed stream; invalid rates send nothing; an unknown or ambiguous
    selection sends nothing; observed Hz appears only after frames arrive;
    reconnect restores the enabled requests and rates without duplicates.

- [x] **MR-19 — Make standard-message/uORB coverage explicit (software).**
  - Contract confirmed in §3: `BATTERY_STATUS`, `SCALED_IMU`, `HIGHRES_IMU`,
    `ESC_STATUS`, `GPS_RAW_INT` and `SERVO_OUTPUT_RAW`; shared `vehicle_imu`
    topics refuse as ambiguous and unmapped topics return `NoStandardMapping`.
  - Depends: MR-18. Catalog metadata, alias resolution and picker are implemented.
  - Files: stream catalog metadata, picker messaging, FEATURES/README.
  - Work: associate standard messages with their PX4 uORB sources where the
    mapping is documented. Initial required examples:
    `BATTERY_STATUS` (147), `SCALED_IMU` (26) and `HIGHRES_IMU` (105), searched
    by exact name/ID and by their common aliases (`battery_status`,
    `scaled_imu`, `highres_imu`). Add `ESC_STATUS`, `GPS_RAW_INT` and the other
    standard streams used by MagGCS after the same verification. If the
    operator searches a uORB topic with no standard MAVLink mapping, show a
    clear refusal. Firmware-side custom streams and companion bridges are
    explicitly out of scope; the UI must not offer either as a working path.
    Do not send an interval request for a topic the GCS cannot decode or the
    firmware cannot stream.
  - Acceptance: `BATTERY_STATUS`, `SCALED_IMU` and `HIGHRES_IMU` each resolve
    to exactly one standard message by name, ID and alias, and receive frames
    at the requested rate; an unmapped topic produces an actionable refusal and
     no command; the UI never presents a custom/dialect-import path.
  - Software evidence: 7 catalog tests, locked catalog regeneration check
    (229 messages / 1883 fields), frontend typecheck and the complete React IPC
    regression. The picker displays all six source links, refuses implicit
    selection of `vehicle_imu`, and sends no request for an unmapped topic.
    Screenshots: `motor-stream-uorb-{mapping,ambiguous}-1366x768.png`.
    Requested/observed firmware rates are recorded by MR-20 below.

- [x] **MR-20 — Verify standard streams end to end and reconcile docs (SITL).**
  - Depends: MR-18, MR-19.
  - Files: SITL scripts, IPC regression, ADR/module README and FEATURES.
  - Work: exercise search, add, rate change, disable, observed-rate, stale
    warning and reconnect against SITL. Record the exact PX4 version, which
    standard streams exist, and which rates PX4 clamps. Document that an
    unmapped uORB topic needs a firmware change or companion bridge.
  - Acceptance: the exact standard message is found by name and ID, arrives at
    the requested rate, changes rate immediately, stops after disable, survives
    reconnect with its saved rate, is visibly marked stale when the firmware
     stops it, and an unmapped uORB query never sends a request.
  - Evidence (2026-10-25): isolated actual PX4 v1.17.0 SIH, source/executable
    `d6f12ad1c4f70ad3230afd7d86e971421e02fef4`; required BATTERY_STATUS,
    SCALED_IMU and HIGHRES_IMU passed add/rate/disable, firmware-stopped stale
    and saved-rate reconnect. Changes matched within 572 ms. GPS_RAW_INT and
    SERVO_OUTPUT_RAW also arrived; ESC_STATUS had no producer. A 1000 Hz IMU
    probe delivered 218.13 Hz, recorded as source/scheduler-limited rather than
    a universal firmware clamp. The browser IPC regression passed separately.
    [Full measurements and command](../verification/mr20-standard-streams-20261025.md).

### F — AM32 motor start/stop tones (operator request, 2026-10-24)

- [x] **MR-21 — Add independently selectable start and stop motor tones (software).**
  - Depends: MR-16 and the confirmed contract in §3.
  - Files: motor-test tone model/state, target-shell adapter, start/stop
    integration, Motor Test controls, IPC checks and module documentation.
  - Work: expose Off plus AM32's four distinct built-in patterns, separately
    for start and stop. Start plays after the long-press is accepted and before
    active output; stop plays only after all motor release writes are confirmed.
    STOP/E-STOP cancels pending start work and releases immediately. A stop-tone
    failure must not turn a successful motor stop into a failed release.
  - Transport: use the existing target-scoped MAVLink `SERIAL_CONTROL` shell
    with bounded `dshot beepN -m <physical-output>` commands. Resolve each
    configured motor through the validated DShot mapping; the CLI index is a
    physical output index, not automatically the logical M1..MN number.
    The available PX4 shell/driver and installed AM32 version must be checked.
    The implementation reads `dshot status` before playback and validates its
    parameter prefix, output mask, zero-based channel/function assignments and
    disarmed output values. Invalid or duplicate mapping sends no beep.
  - Acceptance: independent selections persist; Off sends no tone command;
    PWM/OneShot/unknown outputs do not receive DShot tone commands. Only mapped
    active DShot outputs are addressed. A cancelled or obsolete start never
    emits active output, and a new session cannot overlap a previous tone's
    playback window. A busy/failed capture shell cannot delay motor release or
    alter logging. Tests cover target swaps, timeout, shell failure, normal STOP
    and E-STOP, including a late tone reply after cancellation.
    Record actual sound/timing on the installed hardware before claiming
    acoustic acceptance.
  - [ ] **Hardware acceptance:** record PX4/AM32 versions, physical mapping,
    chosen start/stop sounds and observed timing; confirm all four patterns,
    normal STOP and E-STOP on the actual ESCs.

Source-verified AM32 upstream patterns (installed firmware may differ):

| Choice | DShot command | Current upstream sound | Nominal duration |
| --- | --- | --- | --- |
| Rising | `beep1` | Two ascending notes | 300 ms |
| Falling | `beep2` | Two descending notes | 300 ms |
| Sweep | `beep3` | Rising sweep | 600 ms |
| Triple | `beep4` | Three descending short notes | 225 ms |

Upstream `beep5` currently repeats `beep1`. Saved startup melodies are a
different ESC configuration feature, not arbitrary audio accepted by these
beacon commands. Sources: AM32
[`dshot.c`](https://github.com/am32-firmware/AM32/blob/master/Src/dshot.c),
[`main.c`](https://github.com/am32-firmware/AM32/blob/master/Src/main.c),
[`sounds.c`](https://github.com/am32-firmware/AM32/blob/master/Src/sounds.c),
and PX4 v1.17
[`DShot.cpp`](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/drivers/dshot/DShot.cpp).

### G — Default-OFF motor-control permission (operator request, 2026-10-25)

- [x] **MR-22a — Require explicit motor control and describe primary actions.**
  - Depends: MR-16, MR-21.
  - Confirmed contract: OFF by default and never restored from preferences.
    Enabling is passive: it binds the current target and transport generation
    without starting motors, playing tones, changing parameters or writing
    stream rates. While OFF, START, manual values, preset/forward starts, tone
    playback and target parameter writes send nothing; telemetry and
    capture stay available. STOP, E-STOP and confirmed release remain available.
    Turning OFF cancels a pending start, revokes permission before acquiring the
    operation lock, releases only outputs owned by this session, and keeps STOP
    available if release is incomplete. Target change, reconnect, detach,
    teardown and window close revoke permission. Primary buttons use native
    `title` tooltips; tooltips never change button behavior.
  - Files: `motor_control_service`, actuator/run/capture/lifecycle service,
    core emergency release ownership, Motor Test UI and IPC regression.
  - Acceptance/evidence: `cargo test -p maggcs-app --lib` (164 passed,
    7 ignored); `cargo test -p maggcs-core --lib` (841 passed, 1 ignored);
    clippy `--all-targets -- -D warnings`, frontend typecheck/build and
    `scripts/generate-stream-catalog.py --check` (229 messages, 1883 fields).
    The real-React IPC regression asserts the OFF default, passive enable,
    disabled controls and tooltips, running-OFF release, pending-start
    cancellation, release-failure retention and no ON persistence across
    reload. Added
    [Monitor-only default](../../frontend/screenshots/motor-test/motor-controls-control-off-1366x768.png).

- [x] **MR-22b — Reuse the main-window serial link for Motor Test (software).**
  - Confirmed contract (2026-10-25, implemented): offer this target only when the
    main application connection is serial; attach Motor Test to that existing
    link instead of opening a second port; never allow two Motor Test owners.
    Main-link disconnect, reconnect or teardown revokes MR-22a permission and
    clears the attachment; STOP still releases only this tool's owned outputs.
    UDP, unavailable and incompatible main links keep the dedicated-target path.
  - Depends: MR-22a.
  - Implementation: reuse the exact handle and primary ParamService; separate
    borrowed task ownership from transport ownership. Echo the offered
    target/generation/endpoint through IPC. Duplicate dedicated opens of the
    main port are refused. Shared shell/log work belongs to Motor Test while
    attached, with primary log-transfer exclusion; see ADR-026.
  - Evidence: four Unix PTY integration tests; five motor-service UDP tests
    including obsolete-generation write rejection; focused React IPC check
    and `motor-main-serial-{offer,attached}-1366x768.png`. Main-link detach keeps
    primary parameter reads alive and releases only the configured owned M1.
    Physical USB unplug and packaged native-window close remain bench checks.

## 5. Delivered UI Layout

The original 260/flexible/320-pixel review layout has been replaced by
**236/flexible/250 px** columns above 1100 px. At 1100 px and below, the regions
stack into one scrolling workspace with a fixed critical header. The center
uses compact 2×2 manual controls and two independent analysis plots. RPM hold,
calibration and performance panels are removed. The delivered arrangement
below supersedes the initial left-side Safety card and intermediate two-column
concept; its actual viewport checks and screenshots are recorded in §8.

```text
Header: Target | FC state | Active protocol | Session | STOP | EMERGENCY STOP
        Current failure / stop reason, when present
┌ Left 236 ─────────┬ Flexible center ───────────────────┬ Right 250 ────────┐
│ SITL source ▸    │ Safety limits + command source      │ Target summary ▸ │
│ Unlock / START   │                                    │                  │
│ Gate reason      │ Short M1..MN controls + tracking     │ ESC RPM          │
│ ESC tones        │                                    │                  │
│ Mag alarm        │ Inspector-style analysis workspace  │ Output details ▸ │
│                  │ Signals | top / bottom plots        │                  │
├──────────────────┴────────────────────────────────────┴──────────────────┤
│ Capture status + Start/Stop | Logs ▸ bottom utility drawer               │
└──────────────────────────────────────────────────────────────────────────┘
```

1. **Header:** prioritize target, armed/heartbeat state, active protocol and stop
   controls. Remove permanent placeholder chips such as unavailable firmware or
   measured rate from the main visual hierarchy; retain details where useful.
2. **Left:** SITL source disclosure precedes Test controls, ESC tones and Mag
   alarm. Source configuration opens for Forward. Unlock, START/CANCEL START,
   tone selections and alarm state remain reachable within the scrolling pane.
3. **Center:** use one compact setup strip. Combine motor/output/protocol/range
   as a readable identity block; place command, reported output/delta and status
   alongside a short control (260 px maximum on desktop). Keep protocol/range
   visible without requiring details. Advanced reversal/model fields belong in
   details.
4. **Right:** show only a concise target/configuration summary and display-only
   ESC RPM. Expand target details on disconnect or invalid configuration. There
   is no RPM mapping, hold or persistent logging card.
5. **Logging:** use a bottom utility drawer across the center/right workspace.
   Its collapsed bar always exposes capture state and Start/Stop; opening Logs
   reveals configuration, retrieval, download, delete and errors. At ≤1100 px
   it returns to normal full-width document flow.
6. **Analysis:** follow Signal Inspector: compact signal list, separate toolbar
   and two independently selected stacked plots; a signal may appear in both.
   Hide the list by default below 700 px height, with an explicit toggle. The
   list width is adjustable and persisted. Each signal has a persistent color
   picker. Avoid automatically stacking many small charts.
7. **Responsive:** use the 236/flexible/250 layout above 1100 px and stacked
   scrolling at ≤1100 px. Critical header state stays fixed. Standard checks
   cover 1024×640, 1366×768 and 1920×1080 plus Field mode.

## 6. Delivery Reconciliation and Remaining Acceptance

| Area | Required reconciliation |
| --- | --- |
| Required reads | Verified by MR-03: failed required reads do not yield a complete active set |
| Refresh | Verified by MR-04: atomic publish and retain-on-failure behavior |
| Presentation | MR-10 verifies chart units; MR-12 verifies fixed state/folding; MR-13–MR-14 verify compact controls, logging drawer and primary plot |
| Release | Ordinary STOP/E-STOP transport-write tests pass under MR-06; physical bench evidence is pending |
| ESC telemetry | Verified by MR-09: first-index interpretation, per-ESC receive time and unseen-vs-zero data |
| Charts | Axis/unit identity is verified by MR-10; typed stale/release/run-boundary gaps by MR-11 |
| Screenshots | Protocol/unit/bank/gap/layout captures are recorded, plus MR-16 pending-capture stop states, MR-21 cancellable startup and MR-22a monitor-only default |
| Status wording | Three families, display-only RPM and ordinary STOP retaining unlock are documented; no hold/calibration/speed verdict is claimed |
| Tones | MR-21 software checks pass; acoustic/timing measurements on the installed AM32 version are pending |
| Control permission | MR-22a software checks pass: OFF default, passive enable, revoke-before-release, release-failure retry and reload non-persistence |
| Streams | MR-18/MR-19 software-verified; MR-20 required-stream workflow passed on PX4 v1.17.0 SIH. Installed ESC_STATUS/native desktop evidence remains open |
| Main-link reuse | MR-22b software-verified with serial PTY and UI checks; physical USB/native-window observations remain open |

Generated binding-export tests are useful contract checks but are not evidence
of runtime safety-path coverage.

### Physical acceptance evidence still required

| Area | Record before release acceptance |
| --- | --- |
| PWM / OneShot / DShot | FC/ESC versions, physical mapping, configured min/max or DSHOT_MIN, 1/2/4-motor cases, reversal/thrust model and observed output |
| STOP / E-STOP / link loss | Request time, confirmed release frames, observed cessation of output/rotation, and any failed-release retry |
| ESC telemetry | Installed telemetry method, genuine zero/missing/stale samples and independent banks where the hardware exposes them |
| AM32 tones | Selected start/stop patterns, physical motor/channel identity, audible result and actual cue/output timing for all patterns and Off |
| Native desktop | Window close/focus and stop-control behavior in the packaged Tauri runtime |

The passing loopback and browser checks do not supply these measurements.
Record a dated bench report with the fixture/device identities and measured
results; final hardware release acceptance remains open until that report exists.

## 7. Failure Analysis and Verification

### Before implementing actuator-writing changes

- **Wrong units:** source PWM values interpreted as DShot counts can spin an
  idle motor. Verify normalized idle and target-model conversions first.
- **Unexpected output writer:** removing RPM controls alone leaves their IPC
  commands and background control loop callable. Remove the entry points,
  task/state and modules together; verify ordinary STOP/E-STOP release receipts.
- **Incomplete configuration:** distinguish failed required reads from known
  absent outputs. Publish validated snapshots atomically and retain diagnostics.
- **Lost feedback or release failure:** preserve the true stop reason, stop
  active refresh, confirm releases and keep failed release state retryable.
- **AM32 tone command side effects:** PX4 v1.17 maps the standard
  `MAV_CMD_CONFIGURE_ACTUATOR` beep action to `beacon1` and then queues
  `save_settings`. AM32 implements that save as a flash write. The implemented
  tone path uses only the allowlisted `dshot beep1`–`beep4` shell verbs, which
  do not queue that save, direction changes or 3D-mode changes.
- **Delayed or misaddressed tones:** use the physical-output mapping and a
  target/configuration/session-bound request. Cancel queued start work on stop
  or target change. AM32 accepts beacons only in its stopped/ready state, and
  some tone routines block ESC processing; allow a bounded playback window
  before motor output. An already dispatched short tone may finish after
  cancellation, while the pending motor start must remain cancelled.

### Evidence recorded by this review

- Current core library build and public-API numerical reproductions completed.
- Frontend typecheck and parameter layout check passed.
- Motor-control IPC regression passed at 1024×640, 1366×768 and 1920×1080.
- Parameter connection regression failed once at the draft-value assertion and
  passed on retry; R13 records this limitation.
- Additional isolated browser probes reproduced duplicate/stale-unit legends,
  discarded speed gaps, slider updates during verification and folding while running.
- `git diff --check HEAD` passed.
- Review probes used an in-memory plant and isolated IPC, not hardware. The
  numerical cases in §2 are the durable specification for repository regressions.

### Verification after remediation

Run each task's targeted tests first, then the applicable integration checks:

```bash
export PATH="/home/dev/.cargo/bin:$PATH"
cargo fmt --all -- --check
cargo test -p maggcs-core motor_test
cargo test -p maggcs-app actuator
cargo test -p maggcs-core --test motor_test_service_integration
cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings
git diff --check
```

From `frontend`, with Vite serving `http://localhost:5199`:

```bash
npm run typecheck
npm run check:colors
npm run check:contrast
npm run check:i18n
npm run build
node scripts/check-motor-controls.mjs http://localhost:5199/actuator.html screenshots/motor-test
npm run check:params -- http://localhost:5199/params.html
npm run check:params-connection -- http://localhost:5199/params.html
```

Add focused checks for the cases above before treating a general pass as
acceptance. Physical verification remains a separately arranged props-off bench
session, with target/configuration identity and observed results recorded.

## 8. Remediation Log

- **MR-22a (2026-10-25):** Added the non-persisted, default-OFF Motor control
  permission gate. Enabling binds the current target and transport generation
  without writing motors, tones, parameters or stream intervals. START, manual
  set-values, preset/forward starts, tone playback and target parameter writes
  are refused while OFF; telemetry, capture, STOP/E-STOP and release remain
  available. OFF revokes permission synchronously, cancels a pending start and
  releases only owned outputs; an incomplete release stays retryable. Target
  change, reconnect, detach, teardown and window close revoke permission, and
  core idle emergency no longer releases outputs owned by another controller.
  Primary controls gained native `title` descriptions without changing their
  behavior. Verified with `cargo test -p maggcs-app --lib` (164 passed,
  7 ignored), `cargo test -p maggcs-core --lib` (841 passed, 1 ignored), clippy
  `--all-targets -- -D warnings`, formatting, frontend typecheck/build, catalog
  `--check` and the real-React IPC regression. Added
  [Monitor-only default](../../frontend/screenshots/motor-test/motor-controls-control-off-1366x768.png).
  MR-22b main-link reuse was subsequently delivered under ADR-026 and §4G.
- **Scope change (2026-10-24):** RPM hold is not required in any form. The
  former MR-06 (hold lifecycle), MR-07 (calibration/overrides) and MR-08
  (performance evaluation) are cancelled; no speed-hold start/stop, target
  entry, minimum learning, feedforward or verdict will be delivered. Raw ESC
  RPM remains display-only telemetry. The new MR-06 removes the dead product
  surface. The ordinary motor-test STOP/E-STOP release path is unchanged. The
  superseded parent plan and the root `plan.md` were deleted.
- **MR-01 (2026-10-24):** DShot forwarding now uses the normalized source
  demand; PWM/OneShot keep target-model pulse conversion. Verified with
  `cargo test -p maggcs-core --lib motor_test` (151 passed), including idle
  1000 µs → 0, monotonic 1/2/4-motor demand, reversal and thrust factor, and
  no command to an inactive motor.
- **MR-02 (2026-10-24):** Verification claims exclusive output ownership and
  returns a guard that clears on every exit; manual slider IPC is refused and
  serialized against verification, and the UI disables sliders/sync while
  verification or a hold owns the outputs. Verified with
  `cargo test -p maggcs-app motor_test_state` (7 passed) and
  `npm run typecheck`.
- **MR-03 (2026-10-24):** Mapping reads now distinguish `Value`/`Absent`/
  `Unreadable`; a failed required FUNC/TIM read keeps the map or protocol set
  incomplete, while a complete FC parameter inventory proves reduced-channel
  boards' missing channels absent. START requires a `Complete` map and known
  protocols, and the UI reports the specific blocking category. Verified with
  `cargo test -p maggcs-core --lib` (869 passed), `cargo test -p maggcs-app
  --lib` (130 passed), and `cargo clippy -p maggcs-core -p maggcs-app
  --all-targets -- -D warnings` (clean), plus `cargo fmt --all -- --check`.
- **MR-04 (2026-10-24):** Map, models, protocols and DShot flags are now one
  `OutputConfigSnapshot { revision, target, ... }` published under one lock;
  connect, refresh, status, START and the run manifest read one clone. Refresh
  is serialized with start/stop only for validation plus publish, rejects a
  replaced target, refuses while a session is not idle, keeps the previous
  snapshot on a read failure and marks it stale so START stays blocked.
  Verified with `cargo test -p maggcs-core --lib` (869 passed),
  `cargo test -p maggcs-app --lib` (133 passed), `cargo clippy ... -D warnings`
  and `cargo fmt --all -- --check`.
- **MR-04a (2026-10-24):** The prop-load choice and `START PROPS` phrase are
  removed. `MotorTestLoad`, `RunningMotorTest.load` and `PreparedStart.load`
  are gone; `motor_test_limits()` fixes 0.3 maximum and 2.0/s slew for every
  source. Verification no longer requires a props-off mode; STOP and E-STOP
  remain independent of ownership. The UI gate is unlock → 3 s hold,
  and the motor-control regression holds start for 3 s, rejects a short click
  and passes at 1024×640, 1366×768 and 1920×1080. Verified with
  `cargo test -p maggcs-core --lib` (869 passed), `cargo test -p maggcs-app
  --lib` (132 passed, 7 ignored), clippy `-D warnings`, `cargo fmt --check`,
  `npm run typecheck`, `npm run check:i18n`, `npm run check:colors`,
  `npm run check:contrast` and `npm run build`.
- **MR-04b (2026-10-24):** Destructive actions now share
  `confirmDeleteTwice()`; project disk deletion adds two prompts after the
  typed-name panel, and parameter-group deletion replaces its inline confirm
  with two prompts. SD-log and delete-all paths already prompted twice and were
  audited. Declining either prompt sends no command. Verified with
  `npm run typecheck`, `npm run check:i18n` and the motor-control IPC check,
  which asserts two dialogs per SD-log delete and four across delete-all.
- **MR-06 (2026-10-24):** Removed seven RPM mapping/verification/hold/minimum
  IPC commands, their target/session state and control task, four core modules
  (`rpm`, `speed`, RPM `calibration`, speed `metrics`) and 20 generated bindings.
  Removed their UI, types, styles, scope definitions and fixture data. ESC RPM
  remains a read-only per-ESC readout with live/stale/missing states; the scope
  retains command, magnetic, ESC and battery panes. Verified with
  `cargo test -p maggcs-core --lib motor_test` (104 passed),
  `cargo test -p maggcs-app --lib` (127 passed, 7 ignored),
  `cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`, front-end build/i18n/color/contrast checks and
  the motor-control IPC regression at 1024×640, 1366×768 and 1920×1080.
  Ordinary STOP/E-STOP tests confirm every release transport write and failed
  release handling; the UI rejects short clicks and starts once per 3 s hold.
  Updated all workspace screenshots and added
  [DShot RPM readout](../../frontend/screenshots/motor-test/motor-controls-dshot-rpm-1366x768.png).
  Physical bench acceptance remains pending.
- **MR-09 (2026-10-24):** Confirmed the interface contract before implementation.
  `esc_frame_range()` is shared by the ESC aggregator, scope sampling and
  receive-time bookkeeping. Wire index 0 maps to ESC1–4 and index 4 to ESC5–8;
  no second multiplication occurs. Samples are optional until seen, aggregate
  events omit unseen ESCs, and timestamps update only the covered slots. Tests
  cover either bank arriving first, independent refresh/staleness, true zero,
  overlapping frames, a clipped final frame and out-of-range indices.
  Verified with `cargo test -p maggcs-core --lib signals::derived` (8 passed),
  `cargo test -p maggcs-core --lib motor_test::scope` (11 passed),
  `cargo test -p maggcs-app --lib` (131 passed, 7 ignored), clippy
  `--all-targets -- -D warnings`, `cargo fmt --all -- --check`, frontend
  typecheck and the three-viewport motor-control IPC regression. The UI fixture
  includes all eight ESC indices, an independently live second bank and a
  genuine zero-RPM trace. Added
  [eight-ESC readout](../../frontend/screenshots/motor-test/motor-controls-esc-banks-1366x768.png).
  Evidence is software/IPC-based; physical bench acceptance remains pending.
- **MR-10 (2026-10-24):** Confirmed the output-metadata contract before
  implementation. Predicted commands and mapped FC feedback now carry target
  connection, configuration revision, motor, physical MAIN/AUX channel and
  `OutputUnits`. The backend refuses incomplete/ambiguous/unknown metadata,
  captures it with each running session and will not reuse an idle session
  after its configuration changes. FC traces are `fc.motorN`, selected from the
  configured physical channel instead of treating raw servo positions as motor
  identity. The frontend keeps stable definitions by signal ID, includes unit
  and configuration context in the mounted-plot signature, clears only output
  history on a context change, and rejects late, untagged or mismatched samples.
  PWM/OneShot use µs; DShot/BDShot use mixer counts; mixed families share the
  command pane on independent axes. Verified with the core motor-test suite
  (109 passed), app suite (133 passed, 7 ignored), clippy `-D warnings`, fmt,
  typecheck, six numerical scope-store cases, production build, i18n/color/
  contrast checks and the three-viewport real-React IPC regression. The browser
  regression verifies one legend entry per ID, actual canvas replacement for
  PWM→DShot and PWM→OneShot changes, selection retention, old-sample rejection,
  and distinct MAIN1/AUX1 feedback. Added
  [mixed-unit mapped outputs](../../frontend/screenshots/motor-test/motor-controls-mixed-output-units-1366x768.png)
  and [OneShot output](../../frontend/screenshots/motor-test/motor-controls-oneshot-1366x768.png)
  screenshots after checking rendered trace pixels. Physical bench evidence is
  still pending.
- **MR-11 (2026-10-24):** Replaced implicit NaN serialization with typed
  `ScopeGapReason::{Released, Stale, RunBoundary}` and an optional sample value.
  Command release carries its output metadata end to end. The scope store accepts
  only valid typed gaps for existing signals, inserts one stale boundary at each
  signal's deadline (0.5 s for ESC telemetry, 2 s for other telemetry), and adds
  run-end boundaries to existing command, mapped-feedback and RPM series. Fresh
  values resume after, rather than overwrite, the null. Every plotted series now
  has `spanGaps: false`. Verified with core scope tests (15 passed), app tests
  (133 passed, 7 ignored), nine numerical store cases, typecheck and the real
  React/uPlot IPC regression. The mounted uPlot instance contains null between
  finite segments and reports `spanGaps=false` for both release and run-boundary
  fixtures; stale RPM resumes as a new segment. Added
  [gap rendering](../../frontend/screenshots/motor-test/motor-controls-scope-gaps-1366x768.png).
  Physical bench evidence remains pending.
- **MR-12 (2026-10-24):** The fixed header now shows target, exact
  armed/heartbeat state, the protocol set used by active motors, one session
  state and STOP/E-STOP. Removed firmware, measured-rate and SD placeholders;
  capture moves to the planned bottom utility drawer. Mapping/range/protocol/3D
  blockers remain visible as `BLOCKED` with their reason. Tracking enters a
  disabled `required` fold state while running, stale or blocked, and user-
  expanded motor details survive temporary requirements. Verified by typecheck
  and the real-React motor-control regression across PWM, DShot, mixed outputs,
  unknown protocol, heartbeat loss, run/stop and all three viewport sizes.
- **MR-13 (2026-10-24):** Manual motor controls are a 2×2 desktop grid with
  260 px maximum sliders and a single-column narrow fallback. The healthy right
  column now contains a three-row PX4 summary plus display-only ESC RPM;
  endpoint inputs appear only while disconnected, and connection/output detail
  is collapsed unless invalid. Temporary force-open no longer changes the
  operator's saved disclosure state. Capture/logging moved to a fixed bottom
  utility drawer spanning center/right: capture state, Start/Stop and file count
  stay visible; configuration/retrieval/delete details open inside and activity
  or errors force the drawer open. At ≤1100 px it returns to normal full-width
  flow. Verified with typecheck, build and the motor-control regression at
  1024×640, 1366×768, 1920×1080 and Field mode. The check measures two motor
  rows/two columns, ≤260 px sliders, three healthy target-summary rows and the
  drawer span. Added
  [logging drawer](../../frontend/screenshots/motor-test/motor-controls-logging-drawer-1366x768.png).
- **MR-14 (2026-10-24):** Replaced stacked plots with one Signal
  Inspector-style selected view (Output, Magnetic field, ESC telemetry or
  Battery), a compact searchable signal list and a dedicated toolbar. Healthy
  idle tracking defaults folded, so the 1366×768 selected plot exceeds the
  280 px acceptance floor. Each signal now has a six-digit color picker; colors,
  active view and checkbox choices persist by stable signal ID, survive protocol
  and theme changes, and never clear samples or issue motor commands. The scope
  store records the first accepted value as its time origin. Axes start at 0 s,
  then show cumulative rolling intervals; Clear/disconnect/reload reset the
  origin while protocol/view/color changes retain it. Verified with typecheck,
  11 numerical scope-store cases, production build, color/contrast checks and
  the real-React regression at all target sizes. Browser checks cover search,
  tab switching, custom-color canvas replacement and persistence, no command
  side effects, 0–30 and 70–100 s ranges, Clear reset, single-plot layout and
  the 280 px minimum. Updated the standard
  [1366×768](../../frontend/screenshots/motor-test/motor-controls-1366x768.png)
  and [1920×1080](../../frontend/screenshots/motor-test/motor-controls-1920x1080.png)
  screenshots.
- **MR-14 follow-up (2026-10-25):** Replaced the single active view with two
  independently selected stacked plots. A Top/Bottom switch in the signal list
  chooses which plot a checkbox edits; defaults are Output on top and Magnetic
  field below, and one signal may appear in both plots. Each pane keeps its own
  `checked`/`seen` sets, and legacy `checked`/`seen`/`activeView` preferences
  migrate on load. Mixed units retain separate µs/count axes per plot. The
  signal list gained a persisted, keyboard-operable width (pointer drag,
  Arrow keys, Home/End, double-click reset; 220–560 px, 300 px default).
  Plot sizing is decoupled to avoid uPlot/ResizeObserver feedback loops
  (240×80 px minimums). Verified with `npm run typecheck`,
  `npm run check:motor-scope` and the complete isolated IPC regression covering
  independent selections, duplicate signals, mixed units, resize persistence,
  Clear, damaged storage and old-preference migration. Added the
  [dual independent plots](../../frontend/screenshots/motor-test/motor-controls-independent-plots-1366x768.png)
  screenshot and updated the standard viewport captures.
- **MR-15 (2026-10-24):** Added the confirmed
  `waitForSelectedValueAndDraft(page, { row, draft, label, editor, name })`
  regression helper. One browser evaluation checks the selected parameter,
  displayed FC value and synchronized editor draft together. Exact-name search
  readiness excludes the reboot decoration. The IPC fixture records actual
  delays: 250 ms for the numeric `MPC_XY_VEL_MAX` read, 150 ms for retained
  selection after reconnect and 120 ms for a window opened before connection.
  Enum zero, external changes, long enum meanings, firmware-only parameters and
  reconnects all use the helper. Old-reply assertions also check the recovered
  draft. Verified with the complete 1912-parameter connection regression,
  including two concurrent successful repeat runs, and the parameter-layout
  regression (current/default text alignment within 1 CSS px, wrapping and
  scroll reset). The delayed read is captured in
  [row and editor synchronization](../../frontend/screenshots/params/connection/params-delayed-float.png).
- **MR-16 (2026-10-24):** Confirmed the capture/idle-run contracts before
  implementation. Added a loopback PX4 fixture with real `ParamService`,
  `MotorTestService`, capture command adapters and the target-shell capture
  worker. Tests verify manual capture start/stop during motor output, manual
  motor start/stop during recording, target-bound capture stop, the coordinated
  start guard and exact pre-roll/post-roll boundaries. The coordination test
  drives the core clock explicitly and observes actual motor-release and
  `logger off` frames through the shared link; adapter callers were audited
  against the contract in §3.
  For each of capture Check/Start/Stop, the fake PX4 holds shell replies while
  the real adapter owns `AppState::ops`. Both STOP and E-STOP complete and all
  four release frames reach the peer within the test's 1 s deadline before
  replies are resumed. Late capture completion cannot restart motors or clear
  Emergency. This is a software regression deadline, not physical stop timing.
  Corrected a stale core capture comment that had still assumed every caller
  required a prior motor release.
  Verified with capture adapter tests (7 passed), run adapter tests (7 passed),
  the complete app suite (141 passed, 7 ignored), clippy
  `--all-targets -- -D warnings`, `cargo fmt --all -- --check`, frontend
  typecheck and the real-React motor-control regression. Its six held-reply
  cases cover both stop actions across Check/Start/Stop, including Escape with
  slider focus; late replies preserve the stopped state. The existing
  1024×640, 1366×768, 1920×1080 and Field-mode checks pass. Added
  [STOP during pending capture](../../frontend/screenshots/motor-test/motor-controls-capture-pending-stop-1366x768.png)
  and [E-STOP during pending capture](../../frontend/screenshots/motor-test/motor-controls-capture-pending-estop-1366x768.png).
- **MR-21 scope (2026-10-24):** Operator identified AM32 ESCs and requested
  selectable sounds for both motor start and stop. The upstream sound/command
  mapping and PX4's standard-beep save side effect were reviewed. The interface
  was confirmed before implementation.
- **MR-21 implementation (2026-10-24):** Added the core `motor_test::tone`
  model and driver-status validation, plus the app's target/configuration/run-
  bound tone jobs. Commands are transient, allowlisted `dshot beep1`–`beep4`
  operations on verified physical outputs. PWM/OneShot motors are excluded;
  pulse-only benches retain their saved preferences without issuing DShot
  commands. Manual/Preset/Forward share the same start-tone fence. STOP/E-STOP,
  target/source changes and window teardown cancel a pending start before
  waiting on lifecycle locks. Actual output enqueue and cancellation use one
  short mutex. Confirmed release schedules at most one stop tone, including
  preset completion; tone failure remains separate from a successful release.
  Dropped or cancelled shell jobs release their busy state, and late replies
  cannot restart motors. A new run, even with Start tone Off, waits for an
  already dispatched tone's playback fence.
  The left-side ESC tones card independently saves start and stop selections;
  defaults are Rising/1 and Falling/2. During preparation, source/sliders,
  target settings and tone choices are frozen; header STOP, sidebar CANCEL
  START and Escape remain available.
  Verified with `cargo test -p maggcs-core --lib motor_test` (112 passed),
  `cargo test -p maggcs-app --lib` (151 passed, 7 ignored), clippy
  `--all-targets -- -D warnings`, formatting, production frontend build,
  i18n/color/contrast checks and the real-React motor-control regression.
  The 17 capture-adapter tests include the real start-source path, scrambled AUX
  mapping, release-before-stop-tone ordering, both cancellation paths, preset
  completion, busy capture, shell error/timeout, changed configuration,
  disconnect and teardown, and an Off start behind a pending previous tone.
  Browser checks cover all four patterns plus Off, independent selection and
  reload persistence, frozen controls and all three start-cancel gestures.
  Added [tone selections](../../frontend/screenshots/motor-test/motor-controls-am32-tones-1366x768.png)
  and [cancellable startup](../../frontend/screenshots/motor-test/motor-controls-am32-starting-1366x768.png).
  Software acceptance is complete; acoustic/physical timing remains pending.
- **MR-17 (2026-10-24):** Reconciled current documentation with code, screenshots
  and recorded tests. Baseline findings are labelled historical; required-read,
  refresh and compact-layout fixes are no longer listed as unresolved. Documented
  the three-family boundary and supported mixed MAIN/AUX families separately
  from conflicting timer protocols within one bank. Corrected ordinary STOP
  retaining unlock, the actual left-column order, stacked ≤1100 px layout,
  260 px sliders, 30 s default scope window (10/30/60/120 choices), pulse/DShot
  tracking tolerances, left-side alarm controls and wire release `param2=0`.
  The ignored sine bench check now expects the capped demand rather than the
  uncapped requested peak: `min(2 × 0.2, 0.3) = 0.3`, or 1340 µs for an
  1100–1900 µs model. Verified that numerical case, compiled all app targets with
  clippy `-D warnings`, passed formatting/diff checks, and searched for the
  superseded active claims. A consolidated hardware-evidence table records the
  outstanding protocol/range, stop, ESC telemetry, AM32 sound and native-window
  observations. These remain pending; no physical release acceptance is claimed.
- **MR-18a (2026-10-24):** Implemented `mavlink::streams` with the confirmed
  message/field/support/error types and search/exact-resolution APIs. A
  reproducible Python generator selects the direct `maggcs-core` MAVLink
  dependency through locked Cargo metadata, walks common/standard/minimal XML
  and emits a committed Rust table with version and input hashes. Current
  output: 229 messages and 1883 fields from `mavlink 0.17.1`. PX4 support metadata
  cites the v1.17 stream registry and distinguishes documented source support
  from unknown support and actual live traffic.
  Verified with 5 `mavlink::streams` tests, including every compiled ID/name and
  CRC_EXTRA, exact BATTERY_STATUS/147, SCALED_IMU/26 and HIGHRES_IMU/105 ranking,
  arrays/extensions/units, field search and rejection of unknown or ambiguous
  selections. A unique partial name also requires explicit resolution. Both
  normal and offline generator `--check` passed; core all-target clippy,
  formatting and diff checks passed. Added the generator check to release CI
  and documented regeneration/provenance in the MAVLink module README.
  Live picker/rate management, observed rates and reconnection remain MR-18b;
  Verified uORB aliases and end-to-end SIH firmware evidence were subsequently
  delivered in MR-19/MR-20.
- **MR-18b (2026-10-24):** Confirmed the live rate/status/error contract before
  implementation. Added core interval validation and bounded arrival statistics,
  plus the dedicated-target app stream service and atomic, endpoint/node-keyed
  preferences. One service owns the effective defaults and operator overrides;
  attachment duplicates and the scope's periodic default-rate rewrites were
  removed. Guarded connection writes use stable non-reused handle IDs, transport
  generations and deadlines. Reconnect resets history and restores each effective
  ID once; lost Connected events resynchronize from the handle. Unknown support,
  invalid intervals, wrong targets and expired generations send no rate command.
  Disabled state is independent from measured arrivals; Hz requires fresh frames.
  The UI adds explicit catalog selection, field/unit metadata, immediate Hz edits,
  disable, observed/stale feedback, Reapply and support confirmation. Returning a
  draft to its previous acknowledged rate while a write is pending still applies
  the latest value. Target changes discard obsolete UI completions; panel reload
  does not duplicate backend restoration. The panel's 360 px minimum and measured
  capture-bar clearance keep selected-rate/field controls visible in standard,
  narrow and Field layouts without hiding the fixed stop controls.
  Verified with core MAVLink tests (32 passed), connection integration (7 passed),
  app stream selection (7 passed, 1 hardware test ignored), complete app suite
  (158 passed, 7 ignored), clippy `--all-targets -- -D warnings`, formatting,
  production frontend build and i18n/color/contrast checks. The real-React IPC
  regression covers exact selection/units, invalid/ambiguous/unconfirmed input,
  immediate edits and pending-value reversal, Disabled-but-receiving state,
  staleness, backend-owned reconnect/reload persistence and motor STOP/E-STOP
  while stream writes wait. Full panel bounds are asserted above the capture bar
  at 1024×640, 1366×768 and Field density.
  Added [standard workspace](../../frontend/screenshots/motor-test/motor-stream-manager-1366x768.png),
  [minimum workspace](../../frontend/screenshots/motor-test/motor-stream-manager-1024x640.png),
  [Field workspace](../../frontend/screenshots/motor-test/motor-stream-manager-field-1366x768.png)
  and [live-rate fixture](../../frontend/screenshots/motor-test/motor-stream-manager-live-1366x768.png).
  These are software/IPC observations; actual PX4 supported streams and rate
  behavior were subsequently verified under MR-20; installed ESC telemetry
  and native desktop observations remain open.

## 9. Final checks for MR-19/MR-20/MR-22b (2026-10-25)

- `cargo test -p maggcs-core -p maggcs-app --lib --quiet`: core **846 passed,
  1 ignored**; app **168 passed, 8 ignored**. The app count includes four Unix
  PTY main-link tests; the default-ignored SITL test was explicitly run twice
  and passed separately.
- RTK service UDP integration: **2 passed**. Motor-service UDP integration:
  **5 passed**, including old-generation command/release rejection.
- `cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`, locked stream-catalog regeneration check and
  `git diff --check` passed.
- Frontend build/typecheck, i18n and color checks passed. The RTK browser
  regression and the complete Motor Test IPC regression passed, with updated
  screenshots and manual review steps in the module READMEs.
- MR-20 repeatability check: a fresh artifact directory passed the required
  workflow again, with 521 ms rate-change convergence. The dated report retains
  both observations without turning SIH success into hardware acceptance.
