# ADR-008: RTCM source abstraction and forwarding policy

- Status: Draft
- Date: 2026-10-03

## Decision

- `core::rtk` defines an `RtcmSource` trait. Only one source may inject RTCM
  into the MAVLink link at a time.
- Phase 2 ships the serial base-station source (u-blox ZED-F9P); NTRIP/TCP/UDP
  sources are later optional implementations.
- RTCM3 framing is parsed in `core::rtk::rtcm` (sync, CRC-24 verified), then
  forwarded as MAVLink `GPS_RTCM_DATA` via `core::rtk::forward`.

## Forwarding rules

- `GPS_RTCM_DATA` payload limit is 180 bytes per packet; longer RTCM frames
  are fragmented (max 4 fragments, sequenced). Boundary cases (exact
  multiples of 180) follow the MAVLink spec and get unit tests.
- Bandwidth is measured (B/s) and surfaced in the UI; message-set selection
  (e.g. MSM4 over MSM7) is supported to fit slow telemetry radios.
- Base-station data stalls longer than a configurable threshold (default 5 s)
  raise an alarm.
- Multiple injectors (e.g. QGC also forwarding RTCM) are detected and warned;
  a single "RTCM injection" enable switch guards the link.

## Consequences

- `core::rtk` owns parsing, fragmentation, bandwidth stats, and the
  base-station config protocol (u-blox UBX first).
- A recording of real RTCM frames is committed under `testdata/` for tests.