# MR-20: Standard-stream firmware E2E — 2026-10-25

**Result: PASS for the required BATTERY_STATUS, SCALED_IMU and HIGHRES_IMU
workflow.** ESC_STATUS had no producer in this SIH fixture and remains a
separate installed-hardware observation.

## Fixture and reproducibility

- PX4 `v1.17.0`, source and executable git hash
  `d6f12ad1c4f70ad3230afd7d86e971421e02fef4`.
- Executable `ver all`: Release 1.17.0 (`17891583`), `PX4_SITL`, default build,
  GNU GCC 13.3.0; build datetime Oct 6 2026 08:30:32.
- MAVLink `AUTOPILOT_VERSION` independently returned `flight_sw_version =
  17891583`, version `1.17.0`, board version 1.
- SIH quadrotor, `SYS_AUTOSTART=10040`, 250 Hz simulation loop, 1× speed,
  isolated process instance 42, MAVLink target 43/1, GCS 250/190.
- One custom loopback UDP link, FC port 54237 → GCS port 35298, configured
  link budget 4,000,000 B/s. Each runner invocation allocates new ports.
- Native prebuilt PX4 runs in its own temporary rootfs. The runner terminates
  only its own process group. Default test selection skips this opt-in test.

```bash
python3 scripts/sitl/check-standard-streams.py \
  --px4-build "$HOME/PX4-Autopilot-v1.17/build/px4_sitl_default" \
  --artifacts /tmp/opencode/mr20-streams-20261025
```

The test exercises production catalog selection, the app's stream owner and
persistence, MAVLink transport, actual PX4 stream implementations, the compiled
decoder and the requested/observed/stale snapshots. Real React rendering and
interaction are verified by the separate `check-motor-controls.mjs` IPC
regression. This is not a packaged native-window or physical serial test.

## Measured rates

Rate tolerance for the three required streams is ±20%; the ordinary requested
rates below were within approximately 1.6%. Observations use monotonic GCS
receive time, rather than the simulator clock or requested interval.

| Message | Initial requested / observed Hz | Changed requested / observed Hz | After reconnect requested / observed Hz |
| --- | --- | --- | --- |
| BATTERY_STATUS (147) | 2 / 1.971053 | 4 / 3.948545 | 4 / 3.937647 |
| SCALED_IMU (26) | 10 / 9.855409 | 20 / 19.709761 | 20 / 19.690004 |
| HIGHRES_IMU (105) | 20 / 19.724301 | 40 / 39.420116 | 40 / 39.393754 |
| GPS_RAW_INT (24) | 2 / 1.971083 | 2 / 1.971095 | Disabled / no frames |
| SERVO_OUTPUT_RAW (36) | 10 / 9.854962 | 10 / 9.855233 | 10 / 9.852140 |
| ESC_STATUS (291) | 5 / no frames | 5 / no frames | 5 / no frames |

All three required rate changes matched the expected receive rate within
**572 ms**, including persistence and transport writes (acceptance ≤3 s).

### Disable, stale and recovery

- Disabling 26/105/147 with interval `-1` produced no decoded frames during
  the following 3 s observation; all three showed Disabled plus stale/no recent
  frames, with null observed Hz.
- Stopping HIGHRES_IMU directly in the isolated PX4 CLI preserved the GCS's
  desired 40 Hz. At the snapshot, last frame age was **2957 ms**; state was
  `stale`, observed Hz null. Thus an accepted request cannot fabricate receipt.
- Reapply followed by transport reconnect changed generation 1 → 2 and restored
  the saved 4/20/40 Hz requests. GPS_RAW_INT stayed disabled.
- The persisted profile contained exactly one endpoint/node profile and six
  unique message requests. Exact-once command counts are additionally covered
  by the production service's controlled loopback regression.
- Names, decimal IDs and lowercase canonical names resolved to the same exact
  required messages. Unmapped `mag_sensor_custom_topic` returned
  `NoStandardMapping`; shared `vehicle_imu` returned `AmbiguousSelection`.
  Neither lookup entered rate application or changed saved requests.

### Rate ceiling observation

HIGHRES_IMU at a requested **1000 Hz** delivered **218.131755 Hz**. This fixture
has a 250 Hz SIH source loop; the result demonstrates source/scheduler-limited
delivery. It does not establish a universal 218 Hz firmware clamp. Low-rate
requests recovered after returning to 40 Hz.

### Remaining evidence

- Actual ESC_STATUS delivery/rate and installed ESC telemetry method.
- Physical serial throughput, USB unplug/replug, native window close and
  electrical motor release timing.
- F9P RTCM reception and FLOAT/FIXED transitions; these are RTK hardware
  acceptance, not supplied by the standard GPS message test.
- Arbitrary unmapped uORB topics require firmware/companion changes and cannot
  be enabled by the current standard-message interval UI.

## Artifacts and checks

The dated local artifact directory contains `context.json`,
`standard-streams.json` (all phase snapshots), `stream-preferences.json` and
`px4.log`. This document records the measured release evidence in the repo;
the runner regenerates machine-readable artifacts on each execution.

The opt-in Rust test passed in **20.24 s**. The complete browser IPC regression
passed with source-mapping/ambiguity screenshots, rate editing, unknown-support
gating, disable/observed/stale states and reconnect behavior. A known unrelated
PX4 EVENT enum decode mismatch was skipped by the existing connection worker;
none of the required standard-message streams failed decoding.

A repeat run with the final fresh-directory runner also passed in **20.04 s**,
recording **521 ms** rate-change convergence and **220.963416 Hz** for the
1000 Hz probe. Its artifacts are under
`/tmp/opencode/mr20-streams-20261025/run-96twk5i5/`. Future runs create their
own `run-*` directories; the tables above retain the initial measured run.
