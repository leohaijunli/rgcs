# ADR-008: RTCM source abstraction and forwarding policy

- Status: Draft
- Date: 2026-10-03

The current receiver ownership, F9P configuration and transport-confirmed
statistics contract is specified in [ADR-027](027-rtk-base-sessions-survey-in.md).
The dated addenda below preserve the packetization and earlier review history.

## Decision

- `core::rtk` defines an `RtcmSource` trait. Only one source may inject RTCM
  into the MAVLink link at a time.
- Phase 2 ships the serial base-station source (u-blox ZED-F9P); NTRIP/TCP/UDP
  sources are later optional implementations.
- RTCM3 framing is parsed in `core::rtk::rtcm` (sync, CRC-24 verified), then
  forwarded as MAVLink `GPS_RTCM_DATA` via `core::rtk::forward`.

## Forwarding rules

- `GPS_RTCM_DATA` payload limit is 180 bytes per packet; a transport group
  has at most four numbered fragments. The PX4-specific full-frame policy
  below carries larger RTCM frames across groups.
- Bandwidth is measured (B/s) and surfaced in the UI; message-set selection
  (e.g. MSM4 over MSM7) remains future base-configuration work.
- Base-station data stalls longer than the current 5 s threshold raise an alarm.
- Multiple injectors (e.g. QGC also forwarding RTCM) are detected and warned;
  a single "RTCM injection" enable switch guards the link.

## Consequences

- `core::rtk` owns parsing, fragmentation and bandwidth stats. The base-station
  config protocol (u-blox UBX first) is planned.
- Tests currently use generated RTCM frames. A real F9P recording still needs
  to be captured and committed under `testdata/`.

## Addendum (2026-10-11, verified against PX4 v1.17 source)

How PX4 actually handles the fragments (`MavlinkReceiver::
handle_message_gps_rtcm_data` → `gps_inject_data` uORB → `GPS::
handleInjectDataTopic` → `injectData`):

- **PX4 does not reassemble.** The receiver republishes each
  `GPS_RTCM_DATA` as `gps_inject_data` verbatim, and the GPS driver writes
  each message's `data[0..len]` straight to the GNSS serial port in arrival
  order. The driver's own comment: "the message could be fragmented. But as
  we don't write anywhere else to the device during operation, we don't need
  to assemble the message first."
- **The `flags` bits (fragment id / sequence) are preserved by the MAVLink
  receiver but ignored by the GPS driver's injection path.** Even PX4's own
  RTCM relay (`publishRTCMCorrections`) only
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

## Addendum (2026-10-25)

- Stats derive the RTCM type from the parsed `RtcmFrame::message_type`, not
  from the frame preamble. The UI therefore shows real message numbers
  (1074/1084/…) rather than values decoded from `0xD3`.
- The stall clock starts when the source opens, so the first 1 Hz tick cannot
  report "stalled" before the 5 s threshold has elapsed.
- A disconnected link clears the frontend's cached injection and foreign
  snapshots with the link-owned runtime. Opening the drawer seeds the cached
  stats immediately instead of waiting for the next 1 Hz event.
- Verified with `cargo test -p maggcs-core --lib rtk` (36 passed),
  `cargo test -p maggcs-core --test rtk_service_integration` (1 passed),
  clippy/format checks, `npm run typecheck` and `npm run build`.
- Still open: real F9P capture/fix transition, message-set selection, UBX
  base configuration and serial-unplug recovery.

### Statistics review follow-up (2026-10-25)

The startup grace period now has its own timestamp: `seconds_since_data`
remains null until a nonempty source read. B/s counts only valid RTCM frame
bytes queued to the link and divides by the actual elapsed interval. It is
not a transport receipt or proof of rover correction acceptance. Source-read
age is measured at the reader, not when the service drains the queue.

Pending statistics coalesce under backpressure so they cannot block source
Stop/Shutdown. The drawer permits Stop after heartbeat loss and displays
failures. The [review](../plans/rtk-drawer-review.md) records coverage and
remaining hardware/design gaps.

### Full-size RTCM frames (2026-10-25, confirmed and implemented)

The previous service passed an entire RTCM frame to a single-group fragmenter.
Valid frames over 720 bytes produced `TooLong`, which was treated as a dead
link and terminated injection. RTCM's actual maximum is **1029 bytes** including
header and CRC.

`packetize_px4(frame, sequence_id) -> Result<RtcmPacketBatch, RtcmForwardError>`
now emits groups of at most 720 bytes. `RtcmPacketBatch` carries ordered packets
and the next five-bit group sequence. `InvalidFrameLength { len }` rejects
lengths outside 6..=1029; the service performs CRC validation before packetizing.
Each group consumes one sequence, including a single-packet group. Group IDs
never wrap within the same sequence: a maximum frame at sequence 31 uses four
packets at sequence 31 and two at sequence 0, leaving sequence 1 for the next
group. All packets of one RTCM frame are queued before the next frame begins.
Statistics count the original RTCM frame and its bytes once.

This policy is specific to PX4's source-verified byte-stream behavior, not a
claim of arbitrary-autopilot compatibility. Evidence:

- [PX4 v1.17 MAVLink receiver](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/mavlink/mavlink_receiver.cpp#L2603-L2620)
  publishes each packet's payload as `gps_inject_data`.
- [PX4 v1.17 GPS injection](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/drivers/gps/gps.cpp#L539-L636)
  writes that payload directly to the GNSS.
- The GPS driver's eight-entry queue can still lose generations under load;
  ordering/throughput on real radio and GNSS serial links remains field evidence.

Failure analysis: group-sequence reuse could confuse a reassembling receiver;
interleaving, missing bytes or forwarding padding would corrupt the RTCM CRC.
Explicit next-sequence state, one-frame-at-a-time queueing and exact wire-byte
checks cover those software failure modes. No additional base configuration
or serial-port selection is introduced by packetization.

Verification: 42 RTK-related library tests and 3 service integration tests
passed. Boundaries include 720/721/900/901/1029 bytes and sequence 31 → 0. The
new UDP test failed before the fix at **372 received / 1600 expected bytes**;
it now receives all 34 valid frames, skips a corrupt large frame, and confirms
following-frame traffic plus bounded Stop/Start/Shutdown on the same service.
