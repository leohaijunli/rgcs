# Test data: small DEM, ULog, .plan, RTCM recordings.

Committed test fixtures for unit/SITL tests.

- `qgc-survey.plan` — a QGroundControl plan with a `SimpleItem` followed by a
  `Survey` `ComplexItem` (with generated `simpleItems`). Used to exercise the
  `.plan` importer, including complex-item handling (issues.md #12).
