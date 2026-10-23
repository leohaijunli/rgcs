# Motor test workspace

The Motor Test window hosts a serial-target motor bench, optional SITL input,
magnetic monitoring, and manually controlled SD capture/log retrieval.

## Layout

- **Header:** Q-Mag logo and tool title, target identity, FC state, configured rate, one test-state label,
  durable operation failures, Stop, and Emergency Stop.
- **Left:** SITL input details, unlock/start controls, and the magnetic alarm.
  SITL details open when Forward is selected and collapse for Manual/Preset;
  the operator can also toggle them explicitly.
- **Center:** prop load and command source, the selected source's controls,
  motor tracking, and signal plots (servo commands, total field, ESC, and the
  total battery current from `BATTERY_STATUS`).
- **Right:** the serial target, capture/run status and actions, expandable
  read-only logger configuration, and expandable log retrieval.

Wide windows use a bounded three-column grid. Each column can scroll without
growing the document past the viewport. At widths up to 1100 px the columns
stack, with left-side start/alarm controls first and a sticky stop header.
The shared application wordmark becomes a compact Q at ≤1366 px, preceding
the Motor test title without displacing the stop actions. Updated header
screenshots and the layout regression passed on 2026-10-23.

The scope signal list can be toggled at every width. Plot panes have a minimum
height that leaves room for axes and legends; additional panes scroll rather
than compressing the curves into thin strips.

The header distinguishes `LOCKED`, `UNLOCKED`, `STARTING`, `RUNNING`,
`RELEASING`, and `EMERGENCY`. Unlocking is a local confirmation state; the
existing target, heartbeat, armed-state, and full-props conditions determine
whether START is available. The backend's raw command-session state is labelled
separately as **Command transport**.

Magnetic monitoring displays **Disabled**, **No data**, **Monitoring**, or
**ALARM**. Threshold, calibration, and acknowledgement remain connected to the
existing watchdog handlers. SD capture actions stay visible when secondary log
details are collapsed.

## FC PWM range and converted commands

The tracking table always displays each mapped motor's **PWM range (PX4)**
in microseconds. Unread ranges show **Not read** and block START. Refresh
mapping reads the channel's min/max again.

`ACTUATOR_TEST.param1` stays normalized as required by MAVLink. The sender
converts its queued, post-safety-limit command back to physical PWM using
the same min/max, reversal and thrust model that encoded it. **Command PWM**
is displayed and plotted in **µs**, separately from measured FC servo feedback.
For example, 25% of 1100–1900 is 1300 µs; 25% of 1200–1800 is 1350 µs.

No active command is represented by a curve gap, not a fabricated zero or
minimum pulse. An active normalized-zero command for a reversed motor remains
distinguishable from release. Full STOP readback clears after confirmed release
writes. `PWM_*_DISn` describes disarmed output separately from active min/max;
it is not a channel-disable flag.

Verified on 2026-10-23 with 80 core motor-test tests and the isolated IPC check
at 1024 × 640, 1366 × 768 and 1920 × 1080. The check covers the visible FC range,
microsecond command trace, release gap and existing controls/layout behavior:

```bash
PLAYWRIGHT_MODULE=/path/to/playwright node scripts/check-motor-controls.mjs \
  http://127.0.0.1:1420/actuator.html screenshots/motor-pwm
```

[PWM workspace screenshot](../../screenshots/motor-pwm/motor-controls-1366x768.png).
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

## Verification — 2026-10-22

Passed:

- `npm run build` — TypeScript checking and production Vite build.
- `node scripts/check-motor-controls.mjs http://localhost:5199/actuator.html screenshots/motor-test`
- `cargo check -p maggcs-app --lib` with the Rust toolchain on `PATH`.
- `git diff --check`.

The UI regression uses an isolated Tauri IPC stub. It checks:

- A single left-sidebar start control and magnetic alarm.
- Source disclosure and locked/unlocked presentation.
- One short-click start, the full-props three-second hold, and no unintended
  STOP on release.
- Header STOP, tracking values, persistent errors, and idle-edit re-locking.
- Independent SD capture and the existing log save/delete workflows.
- Alarm enable/no-data/alarm/acknowledgement/disabled states.
- Bounded layouts at **1024 × 640**, **1366 × 768**, and **1920 × 1080**,
  reachable last plot/capture controls, and a visible Emergency Stop while
  scrolling.
- Live theme/Field-mode propagation, restored preferences after reload,
  and 44 px targets in Field mode. Preference changes issue no motor commands.

The full-workspace `cargo fmt --all -- --check` reports existing formatting
differences in the log/capture portions of `actuator_service.rs`,
`capture_service.rs`, and `core/src/logs/protocol.rs`. The backend edit in this
layout increment is the Motor Test window title; those unrelated formatting
differences were not rewritten.

### Screenshots

All captures use the simulated IPC fixture identified in the header.

| View | Capture |
| --- | --- |
| 1024 × 640 | [Stacked controls](../../screenshots/motor-test/motor-controls-1024x640.png) |
| 1366 × 768 | [Desktop workspace](../../screenshots/motor-test/motor-controls-1366x768.png) |
| 1920 × 1080 | [Wide workspace](../../screenshots/motor-test/motor-controls-1920x1080.png) |
| Field mode, light | [Touch layout](../../screenshots/motor-test/motor-controls-field-light.png) |

### Manual review

1. Open Tools → Motor test. Check that input details, Test controls, and
   Mag alarm appear in the left column in that order.
2. Select Forward, then Manual. Verify that input details expand/collapse and
   that Show/Hide remains keyboard operable.
3. Use the IPC fixture to exercise unlock/start/stop and alarm acknowledgement;
   check the header's single test-state label and local error messages.
4. Expand Logger configuration and Log retrieval. Confirm that capture actions
   are available without expanding either section.
5. Resize through the three screenshot sizes. Scroll each wide pane and the
   stacked workspace; verify the last plot and log controls remain reachable
   while Stop/Emergency Stop stay visible.
6. Change theme and Field mode in the main window, then reopen Motor Test.
   Check current and restored preferences, canvas colors, and labelled targets.

Native WebKit/window-focus checks and physical bench acceptance remain separate
from this simulated layout regression.
