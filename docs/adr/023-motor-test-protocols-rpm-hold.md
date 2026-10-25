# ADR-023: Protocol-aware Motor Test and ESC RPM telemetry

- Status: Accepted (2026-10-24). RPM-hold, calibration and performance-metric
  decisions are superseded (2026-10-24); those features are not delivered.
- Date: 2026-10-24
- Relates to: `docs/plans/motor-test-review-remediation-plan.md`
- AGENTS.md height/datum rules do not apply to this bench tool, but its
  safety/risk-analysis and test rules do.

## Decision

1. **The output protocol is detected, never guessed.** PX4 bank parameters
   `PWM_MAIN_TIMn` / `PWM_AUX_TIMn` decide the PWM, OneShot or DShot family per
   bank; bitrate and bidirectional operation are DShot attributes. A motor
   inherits its bank's protocol. MAIN and AUX may use different known families.
   Unread configuration or incompatible timer protocols within one bank disable
   START instead of falling back to PWM. `DSHOT_3D_ENABLE != 0` disables START: 3D mode changes
   the throttle range, so the normal DShot conversion is invalid. On a Pixhawk
   6X the IO coprocessor drives MAIN and does not support DShot; DShot motors
   must be on AUX/FMU.
2. **One normalized command path, protocol-aware presentation.**
   `ACTUATOR_TEST.param1` remains the normalized actuator value. The session
   releases test control with `param2=0`; its scope emits a typed release gap.
   PWM/OneShot are shown in `µs`; DShot/BDShot in mixer throttle counts
   (`DSHOT_MIN × 1999 … 1999`). Output samples carry target/configuration,
   motor, physical channel and unit identity. Mixed families use separate axes
   in the command pane; old-unit history is never relabelled. Release, stale
   telemetry and run completion are typed gaps with interpolation disabled. Only the
   contiguous configured M1..MN prefix (1–4 motors here) is rendered and sent;
   empty slots are not motors.
   The core session width follows `SetMotorCount`, and the frontend sends
   exactly the active value count.
3. **ESC RPM is display-only telemetry.** `ESC_STATUS.rpm == 0` means stopped, a
   missing bank is shown as missing, and samples older than 500 ms are stale.
   `ESC_STATUS.index` is the first ESC index, so index 4 refers to ESC5–8; banks
   are aggregated without being merged or treated as zero. One shared range
   calculation drives aggregation, scope samples and per-ESC receive timestamps.
   An unseen ESC has no sample; only a reported zero is displayed as zero.
   A motor→ESC binding
   is never assumed. There is no automatic mapping verification, no learned
   minimum and no RPM hold.
4. **Motor Test remains operator-controlled.** START is gated by connection,
   valid target, complete required configuration, known/supported protocol and
   the agreed long-press. Each source drives the configured active motor set
   with fixed bench limits (0.3 maximum normalized demand, 2.0/s slew, 3 s
   long-press). STOP and E-STOP always release every output the session owns.
5. **No speed-tracking verdict.** Command-versus-reported-output may be plotted,
   and raw ESC RPM may be shown when available. The GCS does not report
   settling, overshoot, steady-state error or a pass/fail performance result.
6. **The UI keeps critical state visible.** Protocol, range, active motors,
   armed/heartbeat state, session, STOP/E-STOP and warnings never depend on
   opening a details panel. Tracking cannot fold while running, stale or
   blocked; user-pinned motor details survive temporary warnings. Manual motor
   controls use a compact 2×2 desktop grid, target details collapse when healthy,
   and capture/logging uses a bottom utility drawer with persistent Start/Stop.
   Analysis uses one large Signal Inspector-style plot, searchable signals,
   persistent per-signal colors and elapsed seconds from the first sample. The
   workspace is a bounded 1024×640…1920×1080 layout with internal scrolling;
   warnings auto-expand their section.
7. **Transient AM32/DShot start and stop tones.** Start and stop independently
   select Off or one of the four distinct AM32 beacon patterns. Defaults are
   Rising/1 and Falling/2. Use the target-scoped `SERIAL_CONTROL` shell with
   allowlisted `dshot beepN` commands: PX4 v1.17's standard actuator beep action
   also queues `save_settings`, an unnecessary persistent ESC write for this
   feature. Validate the current driver's bank, mask, physical channel and motor
   function from `dshot status`. Start audio precedes motor output; stop audio
   follows confirmed release. Cancellation invalidates pending start work, and
   the serialized worker retains the playback fence for already dispatched
   audio. Tone failure is distinct from motor-release failure.

## Safety and failure analysis (AGENTS.md rule 5)

- **Wrong output configuration** (PWM/OneShot/DShot mismatch, conflicting timer protocols within a bank,
  3D mode): read back both bank TIM parameters, show protocol/range before
  START, and refuse on unknown/conflicting/3D configuration instead of converting.
- **Wrong device** (serial port belongs to another FC or device): unchanged
  serial targeting plus unlock/START gating; the operator selects the target
  explicitly and the window shows it.
- **Wrong motor spins**: commands are limited to the configured M1..MN prefix
  selected by the ordinary Manual/Preset/Forward source. Identification pulses
  and the RPM control loop are removed along with their IPC entry points.
- **RPM feedback lies or disappears**: RPM is not used for control. The UI marks
  stale or missing banks and never treats them as zero or as a motor assignment.
- **Runaway or lost control**: every stop path sends release frames; E-STOP is
  always available; manual SetValues is serialized with session operations, and
  STOP/E-STOP waits for confirmed transport writes for every release frame.
- **Wrong units** (µs vs DShot counts): a single protocol model drives both
  command and display; versioned sample metadata rejects stale/mismatched data,
  and the regression checks actual plot replacement and independent axes.
- **Residual risk**: this ADR does not authorize loaded/hover testing. RPM is
  not thrust; prop load, thrust stands and flight testing remain out of scope.

## Consequences

- Motor Test depends on PX4 parameter reads and, for RPM display, `ESC_STATUS`.
  Bidirectional DShot requires `DSHOT_BIDIR_EN=1`; other ESC telemetry depends
  on the hardware and standard messages exposed by the connected PX4.
- Operators get raw per-bank RPM but no motor-labeled RPM, learned minimum or
  hold target, matching the operator's reduced product scope.
- No mapping is persisted because no mapping is created.
- RPM-hold, calibration and performance-metric controls, commands and modules
  are removed by MR-06; the ordinary motor-test stop/release path is unchanged.

## Verification

- MR-06: `cargo test -p maggcs-core --lib motor_test` — 104 passed, including
  protocol detection, normalized command conversion, active-slot handling,
  confirmed STOP/E-STOP release writes and failed-release handling.
- MR-09: `cargo test -p maggcs-core --lib signals::derived` — 8 passed;
  `cargo test -p maggcs-core --lib motor_test::scope` — 11 passed. These verify
  first-ESC indexing, independent banks, unseen-vs-zero data and range bounds.
- MR-10: core motor-test 109 passed and six frontend numerical scope cases
  verify metadata, mapped MAIN/AUX feedback, monotonic context and history
  isolation. Browser checks verify unique legends, actual plot replacement and
  PWM/DShot/OneShot axes.
- MR-11: core scope 15 passed and the frontend numerical suite now has nine
  cases. The real React/uPlot regression confirms finite data on both sides of
  typed nulls with `spanGaps=false`, plus stale-RPM recovery.
- MR-12: the real-React regression checks fixed protocol/armed/session/stop
  state and required-open tracking across run, heartbeat loss and configuration
  blockers.
- MR-13: responsive UI checks measure compact 2×2 controls, concise target
  summary and a center/right logging drawer with persistent capture actions.
- MR-14: 11 numerical scope cases and browser checks cover two independently
  selected plots, Top/Bottom editing, duplicate signals across plots, search,
  color persistence/no command side effects, cumulative time origin and reset,
  and persisted keyboard/pointer signal-list resizing.
- MR-16: capture and run adapter suites each pass 7 tests. Loopback transport
  checks prove manual capture/motor independence and release completion while
  capture replies are held; explicit core timestamps check the coordinated
  pre/post-roll boundaries. The browser checks both stop paths across all three
  pending capture operations and preserves the emergency latch on late replies.
- MR-21: core motor-test selection — 112 passed; capture-adapter suite — 17
  passed, covering mapped beacon commands, start/stop ordering, cancellation,
  preset completion, busy capture, shell errors/timeouts, configuration change,
  disconnect and an Off start behind an already dispatched previous tone.
  Browser checks verify independent saved selections, all four patterns and
  Off, frozen inputs and header/sidebar/Escape cancellation.
- MR-18b: core MAVLink — 32 passed; connection integration — 7 passed; app
  stream selection — 7 passed, 1 ignored hardware diagnostic. Browser checks
  cover explicit catalog selection, units, requested/observed/stale state,
  latest-edit/disable behavior, reconnect ownership and stop priority. Stream
  preferences and defaults have one dedicated-target owner.
- App: `cargo test -p maggcs-app --lib` — 158 passed; 7 hardware/data-fixture
  tests ignored. Covers configuration snapshots, ordinary sessions and teardown.
- `cargo clippy -p maggcs-core -p maggcs-app --all-targets -- -D warnings` and
  `cargo fmt --all -- --check` pass.
- Frontend: production build/typechecking and i18n/color/contrast checks pass.
  The motor-controls regression at 1024×640, 1366×768 and 1920×1080 checks
  unlock, long-press, short-click rejection, display-only RPM and bounded layout.
- MR-17 reconciles delivery wording, protocol families, ordinary STOP/unlock
  behavior and the hardware evidence still required by the remediation plan.
- MR-18 catalog/live stream management is software-verified. uORB coverage
  metadata is implemented by MR-19 and SIH firmware end-to-end evidence is
  recorded by MR-20. MR-21 tone software is implemented and verified;
  actual AM32 sound and physical timing remain a hardware acceptance item.
- Physical props-off bench timing remains pending. Loaded props, thrust
  response and flight behavior are out of scope for this ADR.
