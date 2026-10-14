# Test data: small DEM, .plan, RTCM recordings.

Committed test fixtures for unit/SITL tests.

- `qgc-survey.plan` — a QGroundControl plan with a `SimpleItem` followed by a
  `Survey` `ComplexItem` (with generated `simpleItems`). Used to exercise the
  `.plan` importer, including complex-item handling (issues.md #12).

- `mag_calibration_demo.json` — a magnetometer calibration file for the Signal
  Inspector / motor-test windows (*Load mag calibration…*). Format: `a` is a
  3×3 matrix or a 3-vector (diagonal shorthand), `b` the per-axis hard-iron
  offset, `k` the optional per-axis current compensation (nT/A), and `domain`
  `"px4_raw"` or `"px4_calibrated"` (metadata only). Values are in nanotesla;
  the earth field is ~50 000 nT. Pinned by `crates/core/src/calib/mag.rs`.

- No `.ulg` fixture is committed. ULog replay is tested against a minimal
  ULog writer in `crates/core/src/ulog/mod.rs` (`#[cfg(test)]`), which builds
  the header + `F`/`A`/`D` records in memory.
