# ADR-003: Coexistence with QGroundControl over MAVLink

- Status: Draft
- Date: 2026-10-03

## Decision

- MAVLink 2 only.
- MagGCS uses its own system/component IDs, distinct from both the flight
  controller and QGC, so QGC and MagGCS can observe the same link at once.
- Traffic splitting: shared UDP endpoint (e.g. `udpin:0.0.0.0:14550`) or a
  dedicated mavlink-router instance.
- ID values are validated against PX4 documentation before first use.

## Rationale

- Operators run QGC and MagGCS side by side in the field; they must not
  interfere with each other.
- UDP broadcast/multicast to all GCSs avoids configuring mavlink-router.

## Consequences

- Components must never claim the FC's system id or a QGC-reserved id.
- Sending commands requires the FC's target system/component id and correct
  MAV_CMD/MISSION encoding; mission protocol must tolerate competing writers
  (see ADR-008 for the RTCM injection analogue).