# ADR-011: Link state model and GCS heartbeat

- Status: Accepted (implemented)
- Date: 2026-10-05
- Accepted: 2026-10-07 — four-level link display, 1 Hz GCS HEARTBEAT, and an
  independent heartbeat-timeout timer (issues.md #1, #3).

## Context

The operator must be able to tell "no packets", "packets but no flight
controller", and "flight controller online" apart at a glance — a single
connected/disconnected flag hides a dead FC behind unrelated traffic on the
same link. QGroundControl and mavlink-router are also presence-checked via
HEARTBEAT, so the GCS must announce itself the same way (Phase 0, task 0.3).

## Decision

- **GCS heartbeat**: MagGCS sends a `HEARTBEAT` with `MAV_TYPE_GCS` at 1 Hz
  (`system_id` 250, per ADR-003) as soon as a link is up.
- **Four-level link model** derived from the transport `LinkState` plus the
  target-FC `fc_alive` flag on `LinkStatus`:
  1. *Disconnected* — no socket bound.
  2. *Listening* — socket bound, no inbound traffic yet.
  3. *Data, no FC heartbeat* — inbound frames seen, but no target-FC
     `HEARTBEAT` within `heartbeat_timeout`.
  4. *FC online* — target-FC `HEARTBEAT` within `heartbeat_timeout`.
- **Independent timeout**: heartbeat loss is detected by a dedicated timer
  (`HeartbeatMonitor` polled by the worker) that other inbound traffic cannot
  reset. Only a `HEARTBEAT` from the target system/component refreshes it.

## Rationale

- HEARTBEAT is the only MAVLink message every peer is required to emit, so it
  is the portable liveness signal for both directions.
- Decoupling `fc_alive` from the transport state keeps "the radio is up" and
  "the vehicle is talking" independently observable.

## Consequences

- `LinkStatus { link_state, endpoint, fc_alive }` is the wire type shared with
  the UI (ADR-002).
- Message-source filtering lives in the connection/hub layer so foreign nodes
  on the same link cannot mask a lost FC (issues.md #6, #20).
- Richer diagnostics (rx rate, last-rx age, bound/peer address) are exposed per
  telemetry field on `TelemetrySnapshot.field_ages` rather than on
  `LinkStatus`.
