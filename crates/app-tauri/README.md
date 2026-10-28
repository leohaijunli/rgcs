# Q-Mag desktop entry (Tauri).

Scaffolded in Phase 0 (UI shell). See docs/ARCHITECTURE.md.

The desktop product and main-window title are **Q-Mag**, configured in
`tauri.conf.json`. The stable application identifier `io.maggcs.desktop`
provides continuity for installed settings. The Rust package/binary is
`maggcs-app`.

## Owned flight commands and native client cleanup

`flight_command_operations::flight_command_execute` accepts the Rust-derived
`FlightCommandRequest` for Takeoff, Land, RTL, Set mode, Arm/Disarm, Go to,
Change altitude/speed and Set home. It authorizes the actual native window/session
and captures the existing primary command worker/transport. Supplied heights are
datum-tagged AMSL.

`ChangeAltitude { target }` explicitly distinguishes DTM-relative
`FlightAltitudeTarget::Agl { agl_m }` from `Amsl { altitude: Height }`. AGL is
validated in the 1–120 m envelope and converted through `core::height`; an AMSL
target is sent directly in `MAV_FRAME_GLOBAL` without requiring a DEM. Both
require an armed vehicle and an allowed flight mode. Absolute targets must be
finite `AMSL_EGM96` and representable in MAVLink's numeric field.

Takeoff and its optional arm share one owner-bound operation. Preflight is
rechecked after the takeoff ACK; forced-arm audit persistence is target-bound and
precedes dispatch. The original deadline also bounds asynchronous preparation.
`command_cancel` and `command_operation_status` require the exact original native
caller/session/operation for normal and emergency commands.

The coordinator reserves emergency capacity and native admission supersedes
normal commands, including their inter-step gaps. Confirmed local retry retirement
releases flight protocol claims while retaining any uncertain FC outcome and ACK
quarantine. The Parameters reboot adapter retains its whole-target reconciliation
policy. Native window destruction expires its captured client ID and retires only
that client's work; primary I/O and other clients' caches/operations remain alive.

```bash
cargo test -p maggcs-app --lib flight_command_operations
```

Seventeen Unix PTY fixtures exercise actual production command/transport services,
zero-write admission failures, preflight rechecks, partial takeoff, emergency
supersession, owner-only cancellation, native-close/reopen isolation and uncertain
ACK outcomes. See the [reviewed contract](../../docs/architecture/af-02b4b-flight-command-ownership.md)
and [delivery evidence](../../docs/verification/af-02b-flight-commands-20261026.md).

## Signal Inspector full-log history

`inspector_ulog_history(traces, max_points)` returns columnar, peak-preserving
history over the entire active `.ulg`. An independent ULog reader and core DSP
session keep history loading separate from playback position and filter state.
A semaphore permits one blocking scan at a time; source/request epochs cancel
obsolete scans. Per-trace and aggregate point budgets bound the snapshot.

```bash
cargo test -p maggcs-app --lib inspector
cargo test -p maggcs-core --lib ulog::
```

The dense 180 s fixture exceeds the live ring's capacity and still retains both
ends and early/late peaks within 256 output points. Short-log/filter and
cancellation/error fixtures also pass. See
[ADR-016](../../docs/adr/016-inspector-window-and-channel.md) and the
[frontend module guide](../../frontend/src/inspector/README.md).

## Owned Mission exchanges

`mission_operations` replaces the main-window enqueue/global-event adapter with
typed upload, download, clear and set-current replies. Every request authorizes
the actual native window, captures the existing primary `MissionService` and
transport generation, claims `MissionTransfer`, and retains its operation result.
`mission_cancel`/`mission_operation_status` require the exact original
session/operation.

Progress envelopes identify operation/request/target; passive `MISSION_CURRENT`
snapshots identify target and receive time. Terminal/downloaded data is returned
through the owned invocation, not an unscoped event. Native forwarding revalidates
target generation and drops obsolete/finished operation events.

Cancelled dispatched Mission work stops local retries only. It retains uncertain
FC state and directs the UI to read back instead of rolling back upload/clear/current.
Confirmed local retirement releases the operation claim while core reply quarantine
prevents ambiguous same-generation reuse. A new verified transport generation is
required after an unresolved wire exchange.

```bash
cargo test -p maggcs-app --lib mission_operations
cargo test -p maggcs-core --test mission_owned_integration
```

Nine PTY/native and nine UDP/core owned cases cover all four verdicts, zero-write
admission failures, owner-only cancellation, native close/reopen, borrowed Motor
exclusion, reboot conflict, target event scoping, source/type validation, bounded
observers and ambiguous failures. See the [contract](../../docs/architecture/af-02b4-mission-ownership-proposal.md)
and [acceptance evidence](../../docs/verification/af-02b-mission-20261026.md).

## RTK base and vehicle forwarding

`RtkState::base` is application-owned. `rtk_start_serial` takes an enumerated
device snapshot, baud rate and explicit receiver mode, rechecks identity and
port use, and returns a `BaseSessionId` after launching the worker. The
`rtk_base_status` command/event reports identification, configuration,
Survey-In, readiness and failure. `rtk_stop(session_id)` bypasses vehicle
lifecycle operations and cancels that exact local receiver session.

The primary link owns `RtkRuntime`'s forwarder/observers, subscribing to the
shared base. Disconnecting the vehicle retires forwarding but preserves the
base; app shutdown also stops and joins the base. `rtk_link_status` carries
ordered revisions, generation/session-scoped stats and foreign-injector state.
Obsolete tasks cannot publish into a replacement runtime's cache.

`rtk_serial_ports` supplies device role hints and unavailable reasons. Known
FC ports and active vehicle ports are excluded from base configuration. Main,
Motor Test target and auxiliary connection entry points check base ownership
under the common lifecycle lock. F9P protocol identity is checked by core
before configuration; USB-serial bridge identity is only a port hint.

```bash
cargo test -p maggcs-app --lib rtk
```

App RTK/lifecycle tests cover classification, changed USB identity,
retired publishers, independent receiver lifetime, primary-link teardown and
foreign-injector filtering of reflected own corrections.
See [ADR-027](../../docs/adr/027-rtk-base-sessions-survey-in.md) and the
[detailed backlog](../../docs/plans/rtk-base-and-air-ground-todos.md).

## Automatic and manual serial connections

`serial_service` owns the process-wide inventory, per-role automatic intents and
`app_config_dir/serial-connections.json`. Automatic FC/base linking defaults to
enabled for eligible unambiguous devices; existing explicit opt-outs are retained.
Manual port/baud/mode controls take precedence, and Disconnect persists a pause
until Connect/Resume. UI reload cannot restart a paused role.

All native serial entry points use worker-owned leases. FC automatic probing is
passive and moves the verified transport into the primary connection; ordinary
manual connection remains available. Startup arbitration prevents the old network
timer from racing native FC discovery. Stale selections and intent/settings
revisions are rejected, and writes stay serialized through the cache commit.

The first RTK startup imports valid legacy webview receiver settings before
automatic configuration. Receiver reconnect starts a new base session; survey
validity expires on missing NAV-SVIN even if other bytes continue arriving.

Diagnostics exports include the serial sidecar and `serial-state.json`, with
capture time, actual connection handle/generation, inventory, policy and base
status for physical reboot/connection evidence.

```bash
cargo test -p maggcs-app --lib serial_service
cargo test -p maggcs-core --lib mavlink::serial
```

See the [implemented contract](../../docs/plans/serial-auto-connect-design.md),
[ADR-028](../../docs/adr/028-shared-serial-discovery.md) and the
[operator hardware checklist](../../docs/verification/serial-rtk-hardware-checklist.md).

## Motor Test capture boundaries

`capture_service` owns the selected target's `SERIAL_CONTROL` shell and SD
capture worker. Manual capture Check/Start/Stop can run while a motor test is
sending. Motor STOP/E-STOP bypass the capture operation lock and return after
confirmed motor-release writes, independently of a pending shell reply.

`run_service` adds the coordinated path: require an idle motor session and no
active run, confirm a new recording, wait 1 s pre-roll, accept the operator's
motor start, then wait 1 s post-roll after confirmed release before stopping
capture. Its stop request remains bound to the original target connection.

Schema-v1 motor-test manifests remain readable when their PWM snapshots predate
`range_source`, `protocol` and `dshot_min`. Those missing fields deserialize as
`Fallback`, `Unknown` and `0.0`; recorded endpoints and flags are preserved.
Malformed values and missing original required fields still fail to load.

MR-16 regression commands:

```bash
cargo test -p maggcs-app --lib capture_service::tests
cargo test -p maggcs-app --lib run_service::tests
```

The capture tests use a loopback MAVLink peer with real parameter, motor and
capture services. They hold shell replies while checking all four release
frames arrive within a 1 s software-test deadline, before capture is resumed.
The coordination test uses explicit core timestamps for exact pre/post-roll
boundaries and checks release-before-`logger off` on the wire. Hardware stop
timing is a separate acceptance measurement.

See the [remediation plan](../../docs/plans/motor-test-review-remediation-plan.md)
and [Motor Test workspace](../../frontend/src/actuator/README.md) for current
scope, screenshots and manual review steps.

## Motor-control permission

`motor_control_service` gates every command that could move a motor or trigger
an ESC tone. Permission is OFF by default, is not persisted, and binds the
current `MotorTestTargetRef` plus transport generation. Enabling is passive: it
writes no motor, tone, parameter or stream command. A target change, reconnect,
detach, teardown or window close revokes permission; the window also forces OFF
on open.

While OFF, START, manual set-values, preset/forward starts, tone playback and
target parameter writes are refused before acquiring the operation lock.
Telemetry, SD capture and ordinary parameter reads stay available. OFF revokes
first, cancels a pending start and releases only outputs owned by this session;
an incomplete release remains an error and STOP can be retried. Release paths
themselves are never gated, so STOP/E-STOP still work from any state. The UI
shows the switch and monitor-only state, disables dependent controls, and uses
native `title` tooltips on primary actions.

```bash
cargo test -p maggcs-app --lib capture_service::tests
```

Loopback coverage includes default OFF with zero motor/shell writes, passive
enable, pending-start cancellation, running-OFF release without a stop tone,
failed-release retention, stale enable races, reconnect revocation and no
permission survival across reload. Hardware behavior remains part of the
props-off acceptance session.

## Main serial target reuse

The selected Motor Test target is its FC: either the dedicated serial PX4 or
the explicitly borrowed main serial PX4. A dedicated connection passively probes
the selected port for a PX4, adopts its actual system/component IDs, and keeps
MagGCS's own IDs distinct; the operator does not supply a routing ID.
`actuator_status` connection, endpoint, heartbeat, armed/mode and parameter
fields describe that selected FC. The retained target heartbeat is seeded at
transport handoff, so status is available before the next heartbeat arrives. A
dedicated target works with no primary connection; a borrowed target uses the
primary transport and parameter worker. Only a fresh matching PX4 heartbeat
counts as alive, and unrelated primary-link state never fills missing target
data. The UI guide names the next gate and pulses its matching control.

`main_link_service::actuator_use_main_target` borrows the primary serial
connection and its existing parameter worker. The UI echoes the offered
connection/node/generation/endpoint and shows **Using main serial link**.
Dedicated-open requests for that same port are rejected. Tool detach/close
stops tool-local tasks and preserves the main transport and parameter service.
Main failure/reconnect/replacement expires the attachment and motor permission.

The main console/log tools share a protocol ownership boundary with Motor Test:
finish a main log transfer before attaching, then use Motor Test's Capture/Logs
controls until detached. Primary telemetry and parameter inspection remain
available. Actuator and shell writes cannot migrate to a replacement transport.
See [ADR-026](../../docs/adr/026-main-serial-target-reuse.md).

```bash
cargo test -p maggcs-app --lib main_link_service
cargo test -p maggcs-core --test motor_test_service_integration
```

Four Unix PTY tests cover shared identity/cache, owned-only release and retained
main I/O, reconnect/old-offer rejection, unavailable target and log-operation
exclusion. Five core motor-service UDP tests include obsolete-generation writes.

## AM32 start/stop tones

`core::motor_test::tone` defines Off and the four distinct AM32 beacon patterns.
`motor_tone_service` binds playback to a target, output-configuration revision
and run generation. The existing target capture worker serializes tone jobs
with shell operations. Each job reads `dshot status` and verifies the driver's
bank, output mask, physical channel and PX4 motor function before sending
allowlisted `dshot beepN -m <channel>` commands. This avoids the implicit ESC
settings save in PX4 v1.17's standard `CONFIGURE_ACTUATOR/BEEP` implementation.

All motor sources pass the start-tone fence. Cancellation returns promptly;
the worker drains any already dispatched tone and its nominal playback time
plus 250 ms before admitting another start. A confirmed release schedules one
stop cue after a 1 s coast allowance, including preset completion. This work
does not delay the release receipt, and tone errors are reported separately.
Non-DShot outputs receive no beep commands. The frontend persists the two
choices and synchronizes them through `actuator_set_tones` at window startup.

```bash
cargo test -p maggcs-core --lib motor_test::tone
cargo test -p maggcs-app --lib capture_service::tests
```

The loopback tests exercise the real source-start adapter, cancellation,
physical-output mapping, release ordering, shell failure/timeout, busy capture,
disconnect, preset completion and the quiet-output fence between runs. Actual
sound and rotor timing on the installed AM32/PX4 versions remain a hardware
acceptance item.

## Dedicated-target stream manager

`stream_service` owns Motor Test's dedicated PX4 stream requests. It exposes
`actuator_stream_catalog`, `actuator_stream_status` and
`actuator_stream_set_rate`. The last command carries the expected
`MotorTestTargetRef` and validated `StreamRequest`; stale targets or transport
generations are refused before a write. Commands use standard
`MAV_CMD_SET_MESSAGE_INTERVAL`; successful transport completion is not a
firmware ACK or evidence of the requested rate being honored.

Effective defaults are SERVO_OUTPUT_RAW 20 Hz, HIGHRES_IMU 50 Hz, BATTERY_STATUS
5 Hz and ESC_STATUS 10 Hz. Operator requests replace defaults by ID, including
Disabled. These are the sole rate writers for the dedicated target; opening a
telemetry view and scope flushing no longer reassert conflicting defaults.

Preferences live in `app_config_dir/motor-streams.json`, keyed by canonical
endpoint plus system/component. Atomic writes happen on the blocking I/O pool.
Unreadable existing files report an error and are retained. A saved desired
request survives a transport failure for explicit Reapply or reconnect; it does
not silently become a successful transmission. Each new transport generation
waits for selected-node traffic, resets arrival history and restores one request
per effective ID. Link-state events missed through broadcast lag are recovered
from the connection handle. The managed runtime stops with target teardown.

Stream control and persistence do not hold `AppState::ops`; motor STOP/E-STOP
and release receipts are independent. The loopback tests hold the profile lock
while verifying full release completes before the stream request resumes.

```bash
cargo test -p maggcs-app --lib stream
```

The stream selection records 7 passing tests and 1 ignored hardware diagnostic:
real loopback rates/add/disable, exact command counts, stale/foreign traffic,
reconnect restoration, queued generation rejection, stop priority, profile
isolation/round-trip, corrupt-file retention and lost-event recovery. Current
complete app verification has 168 passing tests and 8 default-ignored tests;
the new opt-in SITL test was also explicitly run and passed.

### MR-19/MR-20 verification — 2026-10-25

The catalog includes six verified PX4 uORB mappings, notes and pinned source
links. A unique exact source alias resolves to its standard ID; `vehicle_imu`
is ambiguous and unverified topics return `NoStandardMapping`.

`scripts/sitl/check-standard-streams.py` runs the opt-in
`stream_sitl_tests::standard_streams_sitl_end_to_end` against an isolated actual
PX4 v1.17.0 process. Required 147/26/105 rate changes, disable, firmware-induced
stale and persisted reconnect recovery passed. GPS_RAW_INT/SERVO_OUTPUT_RAW
also arrived; ESC_STATUS had no producer in the SIH fixture. The 1000 Hz IMU
probe delivered about 218 Hz, preserving the requested/observed distinction.
See the [measured report](../../docs/verification/mr20-standard-streams-20261025.md).
