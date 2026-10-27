# Test data: small DEM and .plan fixtures.

Committed test fixtures for unit/SITL tests.

- `architecture/af00-idle/` — clean, disabled-auto-role desktop profile for
  architecture startup/cleanup measurements; use a temporary XDG profile as
  described in its README.
- Flight-command ownership fixtures generate isolated loopback UDP traffic and
  Unix PTYs at test time. Browser fixtures inject typed Runtime/FC replies;
  captures are retained in `frontend/screenshots/commands/ownership/`. See the
  [AF-02b.4b acceptance record](../docs/verification/af-02b-flight-commands-20261026.md).
- Owned Mission protocol fixtures also generate loopback UDP/PTY exchanges at
  test time. Browser IPC fixtures and captures live under
  `frontend/screenshots/mission/ownership/`; no persisted FC mission recording is
  required. See [AF-02b.4 Mission acceptance](../docs/verification/af-02b-mission-20261026.md).

- `qgc-survey.plan` — a QGroundControl plan with a `SimpleItem` followed by a
  `Survey` `ComplexItem` (with generated `simpleItems`). Used to exercise the
  `.plan` importer, including complex-item handling (issues.md #12).

- `mag_calibration_demo.json` — a magnetometer calibration file for the Signal
  Inspector / motor-test windows (*Load mag calibration…*). Format: `a` is a
  3×3 matrix or a 3-vector (diagonal shorthand), `b` the per-axis hard-iron
  offset, `k` the optional per-axis current compensation (nT/A), and `domain`
  `"px4_raw"` or `"px4_calibrated"` (metadata only). Values are in nanotesla;
  the earth field is ~50 000 nT. Pinned by `crates/core/src/calib/mag.rs`.

- `mag_calibration_px4.json` — an **identity** magnetometer calibration for the
  motor-test total field. The PX4's raw `HIGHRES_IMU` total is already ≈ 50 000 nT
  (the true earth field — the FC applies its own `CAL_MAG*` correction before
  publishing), so re-applying the `CAL_MAG0_*` offsets double-corrects and
  inflates the total (the observed 80 000 nT). The identity calibration documents
  that no further correction is needed; `domain` `"px4_calibrated"` records the
  values are against PX4's corrected output. Load it via *Load mag calibration…*.

- No recorded RTCM base-station capture is committed yet. RTCM unit and
  integration tests generate frames with `core::rtk::rtcm::encode_frame`;
  a real F9P capture remains part of RTK hardware acceptance.

- No `.ulg` fixture is committed. ULog replay is tested against a minimal
  ULog writer in `crates/core/src/ulog/mod.rs` (`#[cfg(test)]`), which builds
  the header + `F`/`A`/`D` records in memory.
