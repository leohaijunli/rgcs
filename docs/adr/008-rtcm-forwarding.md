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

## Addendum (2026-10-11, verified against PX4 v1.17 source)

How PX4 actually handles the fragments (`MavlinkReceiver::
handle_message_gps_rtcm_data` → `gps_inject_data` uORB → `GPS::
injectDataLoop` → `injectData`):

- **PX4 does not reassemble.** The receiver republishes each
  `GPS_RTCM_DATA` as `gps_inject_data` verbatim, and the GPS driver writes
  each message's `data[0..len]` straight to the GNSS serial port in arrival
  order. The driver's own comment: "the message could be fragmented. But as
  we don't write anywhere else to the device during operation, we don't need
  to assemble the message first."
- **The `flags` bits (fragment id / sequence) are ignored** by the v1.17
  receiver path. Even PX4's own RTCM relay (`publishRTCMCorrections`) only
  sets the fragmented LSB and never the sequence/fragment-id bits.
- The RTCM reassembly therefore happens in the GNSS module's parser
  (e.g. the u-blox driver feeds `RTCMParsing::addByte`), i.e. on the byte
  stream. Consequences for `core::rtk::forward`:
  - Zero-length terminator fragments are pointless — not sent (confirmed).
  - Fragment order is what matters; `fragment()` emits fragments in order,
    and the service must not interleave fragments of different RTCM frames.
  - A lost fragment corrupts one RTCM frame only; the GNSS parser resyncs.
    The v1.17 driver warns about dropped `gps_inject_data` generations and
    switches links after 5 s without a valid RTCM message — matching our
    injection-stall alarm threshold.
- The "360/540-byte boundary" concern from the plan is moot: multi-fragment
  frames are just sequential byte-stream writes; no per-message framing is
  reconstructed FC-side.