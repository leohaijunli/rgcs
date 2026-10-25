# Q-Mag desktop entry (Tauri).

Scaffolded in Phase 0 (UI shell). See docs/ARCHITECTURE.md.

The desktop product and main-window title are **Q-Mag**, configured in
`tauri.conf.json`. The stable application identifier `io.maggcs.desktop`
provides continuity for installed settings. The Rust package/binary is
`maggcs-app`.

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
