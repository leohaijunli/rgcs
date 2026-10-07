# ADR-002: Frontend types generated from Rust (ts-rs)

- Status: Accepted (implemented)
- Date: 2026-10-03
- Accepted: 2026-10-07 — Frontend types are generated from Rust with ts-rs.

## Decision

TypeScript types consumed by the frontend are generated from Rust structs and
enums using `ts-rs`. No hand-maintained parallel type definitions.

`core::telemetry` and other boundary types derive `serde::Serialize` (for
serialization over IPC/REST/WebSocket) **and** `ts_rs::TS` (for type
generation). Only values that cross the boundary get the derives.

## Rationale

- The MAVLink message set is large; hand-duplicating shapes drifts quickly.
- The RPC/type boundary is the highest-churn part of the system.

## Consequences

- Boundary types must be plain-data: no `Duration`, no `Instant`; use
  primitive/time-as-ms fields so `ts-rs` can export them.
- `ts-rs` export output is committed (ADR checkpoints), not regenerated ad
  hoc. Until the frontend is scaffolded, exports land in
  `crates/core/bindings/`; they move to `frontend/generated-types/` in
  Phase 0 (task 4).
- Validation/parsing logic stays in `core`; the frontend is a thin renderer.