# Test data: small DEM, .plan, RTCM recordings.

Committed test fixtures for unit/SITL tests.

- `qgc-survey.plan` — a QGroundControl plan with a `SimpleItem` followed by a
  `Survey` `ComplexItem` (with generated `simpleItems`). Used to exercise the
  `.plan` importer, including complex-item handling (issues.md #12).

- No `.ulg` fixture is committed. ULog replay is tested against a minimal
  ULog writer in `crates/core/src/ulog/mod.rs` (`#[cfg(test)]`), which builds
  the header + `F`/`A`/`D` records in memory.
