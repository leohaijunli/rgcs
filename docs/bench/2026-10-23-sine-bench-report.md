# Bench report: Motor 1+3 sine run with total-field recording (2026-10-23)

**Vehicle:** Auterion PX4 FMU v6X on serial `/dev/ttyACM0` @ 115200 baud
(disarmed, propeller removed — operator-confirmed bench).
**Software:** MagGCS `maggcs-app` bench tests (`crates/app-tauri/src/actuator_service.rs`,
`serial_bench_tests` module), GCS MAVLink identity 250/250, target 1/1.

## Objective

1. Read the Motor 1–4 configuration from the PX4 over the dedicated serial
   link (MAIN/AUX assignment, per-motor PWM min/max) and verify that every
   motor-test command is encoded inside that FC-read range.
2. Drive a 30 s sine signal on Motor 1 **and** Motor 3 (20 Hz latest-value
   updates through the same command path the UI uses) and record the whole
   **total magnetic field** |B| curve for the process.

## Motor configuration read from the FC (acceptance 1 — PASS)

`COM_MOT_TEST_EN = 1`. Mapping read from `PWM_MAIN_FUNC1..8` / `PWM_AUX_FUNC1..8`:

| Motor function | Physical output | PWM range | Range source |
| -------------- | --------------- | --------- | ------------ |
| M1 | MAIN4 (`PWM_MAIN_FUNC4 = 101`) | 1100–1900 µs | `Fc` (read from PX4) |
| M2 | MAIN1 (`PWM_MAIN_FUNC1 = 102`) | 1100–1900 µs | `Fc` |
| M3 | MAIN3 (`PWM_MAIN_FUNC3 = 103`) | 1100–1900 µs | `Fc` |
| M4 | MAIN2 (`PWM_MAIN_FUNC2 = 104`) | 1100–1900 µs | `Fc` |

Mapping state: `Complete`, no duplicate/unreadable issues. All four channels
had a nonzero configured `PWM_*_DISn` disarmed pulse (the legacy `disabled`
flag is not a channel-enable indication). Active ACTUATOR_TEST commands are
normalized; the target model maps their physical output to the FC-read range.
The silent 1000–2000 fallback has been replaced by an explicit unread-range
marker and a start gate requiring FC-read ranges.

## Original sine run (delivery/recording passed; isolation issue observed)

* Sine: 0.5 Hz, driver `0.2 * (1 - cos(2πft))`, peaking at a 0.4 range
  fraction (1100–1420 µs for these channels), Motor 1 + Motor 3 in phase, Motors 2/4
  held at demand 0. Driver = 20 Hz `SetValues` (601 updates recorded), session
  tick 10 Hz, `limits_for_load(None)` applied.
* Result: command loop completed; `StopAndWait` receipt
  `StopReceipt { released_motors: 4, state: Idle }` — the full release pass was
  confirmed on transport before Idle; no ACK denial, no release failure.
* Data: `/tmp/opencode/bench_sine_mag.csv` (2660 samples),
  plot `/tmp/opencode/bench_sine_mag.svg`
  (script: `scripts/bench/plot_mag.py`, stdlib-only SVG).

### Total-field statistics

Overall swing: 1981 nT. The mean in the initial CSV 0–3 s window is approximately
49,525 nT. The original sampler also ran during configuration reads, and the
CSV did not retain motor-on/off timing markers. Fixed CSV intervals therefore
cannot be identified reliably as pre-roll, spin or post-roll; they are not
evidence that only M1/M3 rotated. New recordings retain timing markers.

## Finding: M2/M4 spin during the run

The operator observed Motors 2/4 spinning slowly during the run. Root cause:

* The original session tick sent an `ACTUATOR_TEST` frame to **every** motor each
  tick. For a zero-demand motor it sends `value = 0.0, timeout = 0.3 s`.
* PX4's semantics: `timeout > 0` with `value = 0` means **hold the function's
  output active at the configured minimum** (`PWM_MAIN_MIN`),
  *not* release. Timeout renews every 0.3 s, so M2 (MAIN1) and M4 (MAIN2)
  stayed pinned at 1100 µs for the whole run — the observed ESC idle spin,
  unlike the sine-spinning M1/M3.
* A release is `timeout = 0` (`ACTION_RELEASE_CONTROL`), exactly what the
  session's stop-path `release_frames()` already does.

The fix is implemented and unit-tested: untouched zero-demand motors are not
activated, and a positive-to-zero demand transition emits one release frame
(`value = 0, timeout = 0`). Full STOP still confirms release for all motors.
Mechanical isolation and servo recording in a repeated sine run remain a
pending hardware acceptance task.

## Current-consumption telemetry — evaluated, then abandoned by operator decision

The FC streams no `BATTERY_STATUS` (message absent even after a 20 Hz
`SET_MESSAGE_INTERVAL` request) and `SYS_STATUS` reports
`current_battery = -1 cA` / `voltage_battery = 65535 mV` (unknown). `ESC_STATUS`
(291) does not stream either. `BAT1_SOURCE = 0` (battery monitoring disabled /
no source configured) — a full parameter pull starves on the saturated
115200 link (single lazy reads work). Per operator decision the mechanical
run is recorded via the **total magnetic field** curve instead.

## Reproduce

```sh
MAGGCS_BENCH=1 MAGGCS_BENCH_UART=/dev/ttyACM0 \
  cargo test -p maggcs-app bench_sine_on_motors_1_and_3_records_mag_field -- --ignored --test-threads=1 --nocapture
python3 scripts/bench/plot_mag.py /tmp/opencode/bench_sine_mag.csv
```

## Next steps

1. Re-run the sine bench with timing markers and SERVO_OUTPUT_RAW recording;
   confirm M2/M4 remain mechanically still.
2. Optional: enable battery monitoring on the FC (`BAT1_SOURCE`) if a
   power/current sensor is wired, then repeat the run recording current.
3. The PWM range-source/start gate and converted command curves ship with the
   desktop fixes; use the recorded FC configuration when reviewing feedback.
