# Motor test workspace

The Motor Test window hosts a serial-target motor bench, optional SITL input,
magnetic monitoring, and manually controlled SD capture/log retrieval.

## Layout

- **Header:** Q-Mag logo and tool title, target identity, FC armed/heartbeat
  state, active output protocol, one test-state label, durable operation
  failures, Stop, and Emergency Stop. Unavailable firmware/rate placeholders
  and capture state are not shown there.
- **Left:** SITL input details, the Motor control switch, unlock/start controls,
  ESC start/stop tones, and the magnetic alarm.
  SITL details open when Forward is selected and collapse for Manual/Preset;
  the operator can also toggle them explicitly.
- **Center:** a persistent **Next step** strip, safety limits and command
  source, 2×2 compact motor controls, motor tracking, and signal plots (servo
  commands, total field, ESC, and the total battery current from
  `BATTERY_STATUS`). The strip names the current gate and pulses only the
  matching enabled control. Desktop sliders are capped at 260 px and fall back
  to one column on narrow screens.
- **Right:** a three-row healthy PX4 target summary, collapsed connection/output
  details and display-only ESC RPM. Endpoint inputs appear while disconnected.
- **Bottom utility drawer:** capture state, Start/Stop and log count remain
  visible; logger configuration, retrieval, download and delete details open on
  demand. Activity and errors force it open. On narrow layouts it returns to
  normal full-width document flow.

Wide windows use a bounded three-column grid. Each column can scroll without
growing the document past the viewport. At widths up to 1100 px the columns
stack, with left-side start/alarm controls first and a sticky stop header.
The shared application wordmark becomes a compact Q at ≤1366 px, preceding
the Motor test title without displacing the stop actions. Updated header
screenshots and the layout regression passed on 2026-10-23.

The analysis workspace follows Signal Inspector: a searchable/collapsible
signal list, dedicated toolbar and two independent plots stacked top and bottom.
The list's **Top/Bottom** switch chooses which plot a checkbox edits, so each
plot keeps its own selection and the same signal may appear in both. Defaults
are Output in the top plot and Magnetic field in the bottom. Mixed units keep
separate axes. The list is resizable by dragging its separator, with Arrow keys
(16 px steps), Home/End and a double-click reset; width is clamped to 220–560 px
(300 px default) and persisted. Every signal has a color picker; overrides
persist by signal ID and survive protocol/theme changes without changing sample
history or motor commands.

The x-axis is elapsed seconds from the first accepted plotted value. It begins
at 0 s and, after the selected window fills, shows cumulative ranges such as
70–100 s. Clear, target disconnect or reopening resets the origin; protocol,
view and color changes do not.

The header distinguishes `LOCKED`, `UNLOCKED`, `STARTING`, `RUNNING`,
`RELEASING`, and `EMERGENCY`. Unlocking is a local confirmation state; the
target, heartbeat and armed-state checks determine whether START is available.
START requires an unlocked test and a 3 s hold. Demand spans the full normalized
0–1 range of each FC-read output model, with 2.0/s slew. For an 1100–1900 µs
channel, 50% requests 1500 µs and 100% requests 1900 µs; there is no application
30% cap clipping a preset's peak at 1340 µs. Ordinary STOP retains the local
unlock; an idle command/source/tone edit or emergency reset re-locks it. The
backend's raw command-session state is labelled separately as **Command transport**.

**Motor control** is a separate permission switch, OFF by default and never
restored from preferences. Enabling it only binds the current target and
transport generation; it does not start motors or play tones. While OFF, the
window is monitor-only: START, sliders, presets, forward input, tone playback
and motor parameter writes are refused, while telemetry, capture and the MPC
parameter editor stay available. Turning OFF cancels a pending start and
releases outputs owned by this session; if release is incomplete, the error
stays visible and STOP can be retried. Target change, reconnect, detach and
window close also revoke the permission. Primary buttons carry native `title`
descriptions of these actions.

### Main serial connection

The selected PX4 target is Motor Test's FC. A dedicated serial target supplies
its own connection status, heartbeat, armed/mode state and parameters; a separate
main-window FC connection is not required. Connecting it probes the selected port
and adopts the PX4's actual system/component IDs, so there is no routing-ID field
to type. It also snapshots that target's heartbeat at handoff, so FC status is
known before the next heartbeat arrives. When the main link is a hardware PX4 on
serial, **Use main serial link** selects that FC instead. All FC indicators then
describe the borrowed target. `COM_MOT_TEST_EN` is always read from the selected
target, and an unknown value never falls back to another FC's cache.

When the main window has a compatible, live PX4 serial connection, the target
card offers **Use main serial link**. It reuses that connection and parameter
cache, displays **Using main serial link**, and starts with Motor control OFF.
**Detach main link** releases this tool's owned outputs and keeps the main
connection open. Main disconnect/reconnect expires the attachment; a new
explicit attach and control enable are required. An obsolete offer is refused.

Rates apply to all subscribers on the shared connection. During attachment,
use Motor Test's Capture/Logs controls for shell/log operations; finish any
main-window log transfer before attaching. UDP or incompatible main connections
continue to use the dedicated target fields. See [ADR-026](../../../docs/adr/026-main-serial-target-reuse.md).

Focused browser check:
`MOTOR_TARGET_ONLY=1 node scripts/check-motor-controls.mjs http://localhost:5199/actuator.html screenshots/motor-test`.
Manual review: connect the main serial PX4, attach, verify monitor-only mode and
matching telemetry, detach and verify the main telemetry/Parameters still work;
reattach then reconnect the main link and verify control is OFF and target is
detached. Repeat tool close in the packaged desktop runtime.

### SITL feed over the main link

PX4 pins one GCS partner per MAVLink instance, so a second `udpout` to the
autopilot's listening port (for example `10.0.100.103:18570`) receives no
telemetry while the main window owns that partner. When the main link is
connected and the FC heartbeat is live, the SITL card offers **Use main link**.
It subscribes the forward feed to the same connection and displays **Using main
link** — no second socket and no stream-rate changes, and the main window keeps
receiving telemetry.

The offer carries the endpoint, PX4 system/component IDs and transport
generation; an obsolete offer is refused. A main-link reconnect or close
expires the feed: input values clear and Forward start refuses the stale feed
until the operator attaches again. Detaching the feed never shuts down the main
transport. The dedicated SITL fields remain for a separate source, such as a
second PX4 instance on `udpin:0.0.0.0:14540` or one GCS at a time on
`udpin:0.0.0.0:14550`. See
[ADR-029](../../../docs/adr/029-sitl-feed-main-link.md).

Focused checks: `cargo test -p maggcs-app --lib
actuator_service::main_sitl_feed_tests` and the main-link block in
`node scripts/check-motor-controls.mjs` (offer, borrowed attach, no second
connect command, disconnect). Manual review: connect the main window to PX4,
open Motor Test → Forward, select **Use main link**, verify **Using main link**
and live input PWM; reconnect the main link and confirm the SITL card returns
to offline and START refuses the stale feed.

### Selected FC and full-range verification — 2026-10-26

Passed numerical acceptance: 100% manual demand reaches the FC-read maximum,
and 50%/100% sine presets retain their requested shape and peak within 1 µs.
Readiness checks accept the selected target with no primary link and reject
foreign/stale heartbeats. Serial PTY checks cover a standalone target and a
borrowed main link, including detach preserving the main connection.

```bash
cargo test -p maggcs-app --lib motor_test_state::output_limits_tests
cargo test -p maggcs-app --lib actuator_service::start_readiness_tests
cargo test -p maggcs-app --lib main_link_service::tests
MOTOR_FC_RANGE_ONLY=1 node scripts/check-motor-controls.mjs \
  http://127.0.0.1:1420/actuator.html screenshots/motor-test
```

The real-React IPC check verifies target-only parameter status, 100% demands at
the command API, a 1900 µs sine peak visible on the chart, main-link reuse, and
the guide's connect/enable actions with their pulsing controls.
Screenshots: [dedicated FC and full range](../../screenshots/motor-test/motor-dedicated-fc-full-range-1920x1080.png),
[borrowed main FC](../../screenshots/motor-test/motor-borrowed-main-fc-1366x768.png).

Manual verification:

1. Connect the serial PX4 on Motor Test with the main window disconnected;
   confirm there is no system-ID field, the detected ID appears in the target
   card, and the guide moves from connect to enable.
2. Enable motor control and unlock; confirm the guide pulses each matching
   control and then asks for the 3 s START hold.
3. Connect a hardware PX4 on the main serial link, select **Use main serial
   link**, and check the matching FC identity. Detach and confirm main telemetry
   and Parameters continue working.
4. During a motor test, request 50% and 100% or use a sine preset. For an
   1100–1900 µs model, Command PWM should reach 1500 and 1900 µs, respectively.
   STOP should release the output and insert a command-curve gap.

Magnetic monitoring displays **Disabled**, **No data**, **Monitoring**, or
**ALARM**. Threshold, calibration, and acknowledgement remain connected to the
existing watchdog handlers. SD capture actions stay visible when secondary log
details are collapsed. Capture start/stop is independent of the motor-test
session and stays available while motors run; motor START/STOP never touches
capture. The orchestrated run coordinator is a separate path, not used by the
manual controls.

MR-16 covers delayed capture replies: Check/Start/Stop can remain pending while
the fixed-header STOP or E-STOP completes. Capture completion does not restart
motors or clear the emergency latch. The backend loopback check observes every
motor release before unblocking the shell reply; the UI regression checks the
same boundary, including Escape while a motor slider has keyboard focus.

## Output protocol, ranges and converted commands

The window renders only the contiguous configured **M1..MN** prefix (1–4
motors). Empty slots are not controls, and a mapping gap or duplicate blocks
START. The protocol comes from PX4:`PWM_MAIN_TIMn` / `PWM_AUX_TIMn` select PWM,
OneShot, DShot or BDShot per bank (MAIN and AUX are independent). MAIN on a
Pixhawk 6X is the IO coprocessor, which does not run DShot; DShot motors must
be on AUX.

`ACTUATOR_TEST.param1` stays normalized as required by MAVLink. For
PWM/OneShot the tracking table shows **µs** (1100–1900 etc.); for DShot/BDShot
it shows **mixer throttle counts** (0–1999, with PX4 `DSHOT_MIN` as the first
non-zero command). **Command output** list labels change between `Command PWM`
and `Command throttle`. Every output sample carries the target connection,
configuration revision, motor, physical MAIN/AUX channel and unit. FC feedback
is plotted as mapped `fc.motorN`, not as an assumed servo-number identity.
Changing target, mapping, revision or unit clears only output history; late or
untagged samples are rejected. PWM/OneShot and DShot may share the command pane
on separate µs/count axes without relabelling prior history (MR-10). DShot 3D
(`DSHOT_3D_ENABLE != 0`) invalidates this model and blocks START.

No active command is represented by a typed `released` curve gap, not a
fabricated zero or minimum pulse. An active normalized-zero command for a
reversed motor remains distinguishable from release. ESC telemetry gets a stale
gap after 0.5 s; other telemetry after 2 s. Run completion inserts a boundary in
existing command, mapped-feedback and RPM series, so a later run resumes as a
new segment. Every applicable series uses `spanGaps=false` (MR-11). Full STOP
readback clears after confirmed release writes. `PWM_*_DISn` describes disarmed
output separately from active min/max; it is not a channel-disable flag.

## ESC RPM telemetry

`ESC_STATUS` provides per-ESC RPM (0 means stopped; an unseen ESC is shown as
`—`, not zero). Values older than 500 ms are marked **stale**. Bidirectional
DShot RPM requires `DSHOT_BIDIR_EN=1`; other ESC telemetry depends on the
hardware and the standard messages exposed by the connected PX4.

`ESC_STATUS.index` is the **first zero-based ESC index**, not a batch number:
PX4 sends 0 for ESC1–4 and 4 for ESC5–8. Aggregation, curve sampling and receive
timestamps use the same interpretation (MR-09). Either bank may arrive first;
one bank's new frame does not refresh the other bank's age. Unseen ESCs produce
no aggregate signal, while reported `0 RPM` remains a valid readout and trace.

The card and scope traces use **ESC indices**, not motor assignments. RPM is
display-only and does not gate START. RPM hold, motor→ESC mapping verification,
learned/manual RPM limits, feedforward and speed-performance verdicts have been
removed from the UI, IPC commands and core modules (MR-06). The remaining scope
panes are commands, total field, ESC telemetry and battery current.

The isolated IPC regression checks FC ranges, converted commands, release
gaps, display-only RPM, and the existing start/stop/capture/layout behavior:

```bash
node scripts/check-motor-controls.mjs \
  http://localhost:5199/actuator.html screenshots/motor-test
```

[PWM workspace screenshot](../../screenshots/motor-test/motor-controls-1366x768.png).
For native review, refresh the serial target's mapping and check each min/max;
during an explicitly authorized disarmed bench test, check Command PWM and
its µs curve, then STOP and confirm the gap.

## Display preferences

`main.tsx` applies the saved theme and Field mode before rendering and listens
for changes from another same-origin window. Scope plots rebuild their canvas
styles when the root appearance changes, retaining sample buffers and signal
selection.

Field mode uses at least 44 px interaction targets. Checkboxes retain a compact
glyph inside a larger labelled target. These rules are scoped to Motor Test.

## AM32 start/stop tones

The **ESC tones** card has independent **Start tone** and **Stop tone** choices,
saved under `maggcs.actuator.tones` and synchronized with the backend before
START becomes available. Defaults are Rising/1 for start and Falling/2 for stop.
Selections apply to configured DShot motors; PWM/OneShot benches keep the
preferences without sending DShot commands.

| Choice | AM32 pattern | Nominal duration |
| --- | --- | --- |
| Off | No sound command | — |
| Rising · 1 | Two ascending notes (`beep1`) | 300 ms |
| Falling · 2 | Two descending notes (`beep2`) | 300 ms |
| Sweep · 3 | Rising sweep (`beep3`) | 600 ms |
| Triple · 4 | Three descending short notes (`beep4`) | 225 ms |

After the 3 s hold, the header shows STARTING and the main button becomes
**CANCEL START**. Source, slider, target and tone settings are frozen during
this phase. Header STOP, CANCEL START and Escape cancel the pending start.
The backend checks `dshot status` against the configured physical output bank,
channel, mask and motor function, sends only the selected beacon commands, and
waits for the nominal playback window plus 250 ms before enabling output.

Normal STOP/E-STOP releases the motors first; the stop cue is scheduled after
confirmation with a 1 s coast allowance. Preset completion also uses the stop
cue. A busy or failed shell can prevent a cue but cannot delay motor release
or change capture. A failed start cue reports an error and does not start the
motors. Late/cancelled replies cannot restart a session. A short tone already
sent to the ESC may finish after cancellation; a subsequent start, even with
Start tone Off, waits for that playback fence.

These durations describe the upstream AM32 implementation and software waits.
The shell response is not acoustic feedback. Record the installed AM32/PX4
versions and observed sound/timing for hardware acceptance.

## MAVLink stream manager

Open **Streams…** in the analysis toolbar. The panel uses the dedicated Motor
Test PX4 connection; its endpoint, system/component, connection ID and transport
generation stay visible. Search the compiled standard catalog by exact message
name/ID, a field name or an exact verified PX4 uORB source, then select one
explicit result to inspect its wire
types/units. Search alone does not select or send a command. Unknown PX4 support
requires an explicit choice before Add stream.

The six verified mappings are BATTERY_STATUS, SCALED_IMU, HIGHRES_IMU,
ESC_STATUS, GPS_RAW_INT and SERVO_OUTPUT_RAW. Selection shows the uORB inputs,
interpretation notes and a pinned PX4 v1.17 source link. `vehicle_imu` is shared
by SCALED_IMU/HIGHRES_IMU and requires explicit message selection. Unmapped
topics explain that an interval request cannot expose arbitrary uORB data and
send nothing. This UI offers only compiled standard messages.

Add a finite positive Hz value, edit requested Hz to apply a new interval
immediately, or Disable to request `-1`. Invalid drafts send nothing. Concurrent
edits retain the latest valid value, including reverting to the prior value
before a previous write completes. Requested and Observed Hz are separate:
observed rate requires at least two fresh target frames; Waiting, Receiving and
Stale reflect received traffic. A Disabled request can still show incoming
frames if the firmware has not stopped them. Reapply is an explicit action for
an error/stale stream; PX4 can clamp or ignore rate requests.

The backend saves requests per endpoint/system/component in
`motor-streams.json` and restores them once per reconnected transport. Panel
open/close/reload issues no restoration writes. The target's default scope
requests share the same manager, so they cannot override operator choices.
Motor STOP/E-STOP remain available while stream requests or persistence wait.
The bounded panel fits above the capture bar, with internal list scrolling,
including narrow and Field-mode layouts. Closing it restores the existing plot
and history.

The [MR-20 firmware report](../../../docs/verification/mr20-standard-streams-20261025.md)
records PX4 v1.17.0 end-to-end checks: required stream add/rate/disable/stale and
reconnect recovery passed. The UI regression passes independently with six
mapping links, shared-source refusal and the ordinary stream controls.
ESC_STATUS reception on installed hardware remains open; a documented stream
is not evidence that the current board has a publishing source.

## Verification — motor remediation, MR-18 and MR-21, 2026-10-24

Passed:

- `npm run build` — TypeScript checking and production Vite build.
- `npm run check:i18n`, `npm run check:colors`, `npm run check:contrast`.
- `node scripts/check-motor-controls.mjs http://localhost:5199/actuator.html screenshots/motor-test`
- MR-06: `cargo test -p maggcs-core --lib motor_test` — 104 passed, including confirmed
  STOP/E-STOP writes for every motor and failed-release handling.
- MR-09: `cargo test -p maggcs-core --lib signals::derived` — 8 passed;
  `cargo test -p maggcs-core --lib motor_test::scope` — 11 passed.
- MR-10: `cargo test -p maggcs-core --lib motor_test` — 109 passed;
  MR-11: `cargo test -p maggcs-core --lib motor_test::scope` — 15 passed.
- `npm run check:motor-scope` — 11 numerical identity/unit/history/gap/time/color cases.
- MR-16: `cargo test -p maggcs-app --lib capture_service::tests` — 7 passed;
  `cargo test -p maggcs-app --lib run_service::tests` — 7 passed. The real capture
  adapter holds its operation lock while a simulated PX4 withholds replies;
  both stop paths release all four outputs before the reply resumes. Coordinated
  starting-state and exact 1 s pre/post-roll boundaries are also covered.
- MR-21: the core tone model/driver checks pass 3 tests; the complete
  `motor_test` selection passes 112. The capture-adapter suite now passes 17
  tests, including tone ordering, cancellation, disconnect, failed/busy shell,
  timeout, preset completion and the playback fence between runs.
- MR-18b: core MAVLink tests — 32 passed; connection integration — 7 passed;
  app stream selection — 7 passed, 1 ignored hardware diagnostic.
- `cargo test -p maggcs-app --lib` — 158 passed, 7 ignored hardware/data-fixture tests.
- `cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings`.
- `cargo fmt --all -- --check`.
- `git diff --check`.

The UI regression uses an isolated Tauri IPC stub. It checks:

- A single left-sidebar start control and magnetic alarm.
- Source disclosure and locked/unlocked presentation.
- Short-click rejection, one start per three-second hold, and no unintended
  STOP on pointer release.
- Header STOP, tracking values, persistent errors, and idle-edit re-locking.
- Independent SD capture, including starting and stopping capture while the
  motor test runs, and the existing log save/delete workflows.
- Six pending-capture cases: Check/Start/Stop × STOP/E-STOP. The test keeps each
  IPC reply unresolved until the motor session stops, then confirms its late
  reply cannot restart the session or clear Emergency.
- All AM32 tone choices plus Off, independent start/stop persistence and reload,
  frozen source/sliders during STARTING, and cancellation through header STOP,
  sidebar CANCEL START or Escape with no late output.
- Stream catalog search/units and explicit selection, immediate valid rate
  edits, invalid/unknown-support refusal, latest pending edit, disable versus
  observed data, stale recovery, backend reconnect/reload restoration and
  motor STOP/E-STOP during pending stream writes. The panel's full bounds stay
  between the critical header and capture bar at standard, narrow and Field
  densities; Field interaction targets retain the 44 px minimum.
- Alarm enable/no-data/alarm/acknowledgement/disabled states.
- Display-only live/stale/missing ESC RPM, no RPM control actions or mapping
  verification, and four remaining scope panes.
- All eight ESC readouts, a live second bank alongside stale/missing first-bank
  data, and a reported-zero second-bank scope trace.
- One legend entry per signal ID; PWM/DShot/OneShot plot reconstruction;
  rejection of late/untagged history; retained checkbox selection; mapped
  MAIN/AUX feedback and simultaneous µs/count axes.
- Explicit release and run-boundary nulls in the mounted uPlot data with
  `spanGaps=false`, plus stale-RPM recovery as a separate segment.
- Fixed active-protocol/armed/session/stop state; no firmware/rate/SD
  placeholders; tracking cannot fold during a run, stale target or output
  blocker, and pinned motor details survive temporary warnings.
- Two-row/two-column compact motor controls, a concise right target summary and
  a center/right logging drawer with always-reachable Capture Start/Stop.
- Two independently selected analysis plots, a searchable signal list, the
  Top/Bottom selection switch, one signal shown in both plots, mixed units on
  separate axes, persistent per-signal colors, color changes with no command
  side effects, cumulative 0–30/70–100 s ranges, and Clear reset.
- Signal-list resizing by pointer, Arrow keys, Home/End and double-click reset;
  width persists across reloads and both plots retain usable height at desktop
  and narrow sizes.
- Bounded layouts at **1024 × 640**, **1366 × 768**, and **1920 × 1080**,
  reachable last plot/capture controls, and a visible Emergency Stop while
  scrolling.
- Live theme/Field-mode propagation, restored preferences after reload,
  and 44 px targets in Field mode. Preference changes issue no motor commands.

The three viewport checks establish bounded layout and reachable controls.
Chart identity/unit correctness, gap semantics, critical-state visibility and
the compact Signal Inspector-style layout are complete under MR-10–MR-14.

### Dual-plot verification — 2026-10-25

`npm run typecheck`, `npm run check:motor-scope` and the complete isolated IPC
regression (`node scripts/check-motor-controls.mjs …`) pass. The regression
covers independent pane selections, one signal in both plots, mixed units,
list resizing by pointer/keyboard with persistence, Clear, and old-preference
migration.

### Owned actuation checkpoint — 2026-10-27

The production UI now attaches a native `motor-test` Runtime session and carries
its exact permission, target, operation, request/config/value revision through
ON/OFF, manual/preset/forward START, updates, STOP/E-STOP and RESET. A pending
start learns its operation from `motor_operation`, so STOP does not wait for the
first-bank invocation reply. Tone settings require that permission/config; mapping
refresh and parameter access carry the selected Motor target context. Preflight
opens the Motor tool instead of issuing a global STOP.

The isolated IPC regression additionally asserts session-owned START and exact
operation STOP. Its 12 scenario groups and 24 captures under
`screenshots/motor-test-owned/` pass at 1024×640, 1366×768, 1920×1080 and Field/
light variants. See the [native/core acceptance record](../../../docs/verification/af-02b-motor-actuation-20261027.md).

### Screenshots

All captures use the simulated IPC fixture identified in the header.

| View | Capture |
| --- | --- |
| 1024 × 640 | [Stacked controls](../../screenshots/motor-test/motor-controls-1024x640.png) |
| 1366 × 768 | [Desktop workspace](../../screenshots/motor-test/motor-controls-1366x768.png) |
| Dual independent plots | [Top Output, bottom Magnetic field](../../screenshots/motor-test/motor-controls-independent-plots-1366x768.png) |
| 1920 × 1080 | [Wide workspace](../../screenshots/motor-test/motor-controls-1920x1080.png) |
| DShot, display-only ESC RPM | [RPM readout](../../screenshots/motor-test/motor-controls-dshot-rpm-1366x768.png) |
| One motor, eight ESC indices | [Independent bank states](../../screenshots/motor-test/motor-controls-esc-banks-1366x768.png) |
| Mixed PWM/DShot mapped outputs | [Independent axes and MAIN/AUX identity](../../screenshots/motor-test/motor-controls-mixed-output-units-1366x768.png) |
| OneShot output | [Pulse-unit plot after configuration change](../../screenshots/motor-test/motor-controls-oneshot-1366x768.png) |
| Release/run boundary | [Separated output segments](../../screenshots/motor-test/motor-controls-scope-gaps-1366x768.png) |
| Logging utility drawer | [Expanded capture/log details](../../screenshots/motor-test/motor-controls-logging-drawer-1366x768.png) |
| STOP, capture reply still pending | [Independent stopped state](../../screenshots/motor-test/motor-controls-capture-pending-stop-1366x768.png) |
| E-STOP, capture reply still pending | [Emergency latch retained](../../screenshots/motor-test/motor-controls-capture-pending-estop-1366x768.png) |
| AM32 tone selections | [Independent start/stop choices](../../screenshots/motor-test/motor-controls-am32-tones-1366x768.png) |
| AM32 start tone in progress | [Frozen inputs and available cancel/stop actions](../../screenshots/motor-test/motor-controls-am32-starting-1366x768.png) |
| Streams, standard | [Requested/observed rates and wire fields](../../screenshots/motor-test/motor-stream-manager-1366x768.png) |
| Streams, minimum | [1024×640 panel and fixed stop actions](../../screenshots/motor-test/motor-stream-manager-1024x640.png) |
| Streams, Field density | [Touch-sized stream controls](../../screenshots/motor-test/motor-stream-manager-field-1366x768.png) |
| Streams, live-rate fixture | [Measured arrival rate](../../screenshots/motor-test/motor-stream-manager-live-1366x768.png) |
| Field mode, light | [Touch layout](../../screenshots/motor-test/motor-controls-field-light.png) |

### Manual review

1. Open Tools → Motor test. Check that SITL source, Test controls, ESC tones
   and Mag alarm appear in the left column in that order.
2. Select Forward, then Manual. Verify that input details expand/collapse and
   that Show/Hide remains keyboard operable.
3. Use the IPC fixture to exercise unlock/start/stop and alarm acknowledgement.
   A short click must not start; a 3 s hold starts once, and releasing the pointer
   must not trigger STOP. Check the header's test-state label and local errors.
4. Expand Logger configuration and Log retrieval. Confirm that capture actions
   are available without expanding either section.
5. Resize through the three screenshot sizes. Scroll each wide pane and the
   stacked workspace; verify the last plot and log controls remain reachable
   while Stop/Emergency Stop stay visible.
6. Change theme and Field mode in the main window, then reopen Motor Test.
   Check current and restored preferences, canvas colors, and labelled targets.
7. Check the ESC RPM card and scope: values are indexed by ESC, missing values
   show `—`, stale values are labelled, and the RPM card has no input, select,
   verification or hold button.
8. In the eight-ESC fixture, verify ESC5–8 readouts and scope signals. ESC7's
   reported `0 RPM` is valid data; the first bank's missing/stale values retain
   their own states when the second bank is live.
9. Exercise PWM → DShot → mixed MAIN/AUX → PWM → OneShot. Verify one legend
   entry per selected ID, mapped channel labels, separate µs/count axes, retained
   checkbox selection and no old-unit values after each change.
10. Exercise release, stale telemetry and two runs. Verify the command, mapped
    feedback and RPM lines visibly break and resume rather than connecting the
    invalid interval.
11. Verify M1/M2 and M3/M4 form two compact rows at desktop width. Open the
    bottom logging drawer; Capture Start/Stop stays in its bar, the drawer spans
    center/right, and the healthy right target card remains concise.
12. Search the signal list, switch between Top and Bottom and give each plot a
    different signal; then select one signal in both plots. Choose a custom
    trace color for each and confirm selections/colors survive reload and
    protocol changes without issuing a motor command. Drag the separator and
    use Arrow keys/Home/End/double-click; the width persists after reload. Clear
    the plots and verify elapsed time restarts at 0 s; a filled rolling window
    shows cumulative seconds.
13. In the isolated IPC fixture, set
    `window.__bench.holdCapture = 'actuator_capture_start'`, start the motor
    session, then click Start capture. While its reply is pending, use header
    STOP and check that the motor session stops. Call
    `window.__bench.completeCapture()` and confirm it stays stopped. Repeat with
    E-STOP, then with `actuator_capture_stop` and `actuator_capture_verify`;
    late capture completion must retain Emergency until RESET. Check Escape
    while focus is in a motor slider as well as the header button.
14. Choose different Start tone and Stop tone values, including Off, and reopen
    the window. Verify both selections survive independently and changing them
    sends no motor or sound command. In a DShot IPC fixture, set
    `window.__bench.holdMotorStart = true`, unlock and hold START. Check STARTING,
    frozen input/source/tone controls, and available STOP/CANCEL START/Escape.
    Cancel, then call `window.__bench.completeMotorStart()`; the delayed reply
    must not start motors or clear Emergency.
15. For physical acoustic acceptance, record the installed PX4 and AM32 versions
    and motor/output mapping. Check each of the four selected tones and Off,
    start-tone-before-output and release-before-stop-tone ordering, then record
    the observed timing. These hardware observations are still pending.
16. Open Streams… and search BATTERY_STATUS/147, SCALED_IMU/26 and
    HIGHRES_IMU/105. Choose a result, inspect fields/units, then Add or edit its
    rate. Search/invalid rate drafts must send no command. Disable and compare
    the requested state to actual arrivals; check Stale when telemetry pauses.
    Reconnect and reopen: saved requests survive without UI-generated duplicate
    restores. Hold a stream reply in the IPC fixture and use STOP or Escape;
    the motor session must stop before the reply resumes. Resize the panel and
    change Field mode; its selected-rate controls, Close and header stop actions
    remain visible while request lists scroll internally.

Native WebKit/window-focus checks and physical bench acceptance remain separate
from this simulated layout regression.

### Delivery reconciliation — MR-17

Current software completion claims refer to the checks above. The remediation
plan's [remaining physical acceptance](../../../docs/plans/motor-test-review-remediation-plan.md#physical-acceptance-evidence-still-required)
lists the device/version, mapping, range, release, ESC telemetry and acoustic
evidence needed for a hardware release. The baseline review findings and older
per-task test counts remain historical records; the latest software results are
263 app tests (8 opt-in ignored) and 146 filtered core Motor tests. Dedicated
capture/log ownership and the stated hardware checks remain pending.
