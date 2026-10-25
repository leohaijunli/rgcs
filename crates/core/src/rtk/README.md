# RTK receiver sessions and correction forwarding

`base` owns a receiver independently of vehicle links; `ubx` implements F9P
identification, RAM configuration and mixed UBX/RTCM framing; `rtcm` validates
RTCM3; `forward` packetizes `GPS_RTCM_DATA`; `service` subscribes to fresh
corrections and owns one vehicle forwarder. See
[ADR-008](../../../../docs/adr/008-rtcm-forwarding.md) and
[ADR-027](../../../../docs/adr/027-rtk-base-sessions-survey-in.md).

## F9P Survey-In

```rust
pub struct SurveyInConfig {
    pub accuracy_limit_m: f64,
    pub min_duration_s: u32,
}

pub enum BaseMode {
    Preconfigured,
    SurveyIn(SurveyInConfig),
}

impl RtkBaseServiceHandle {
    pub async fn start_serial(
        &self,
        serial: SerialSourceConfig,
        mode: BaseMode,
    ) -> Result<BaseSessionId, BaseError>;

    pub async fn stop(&self, session_id: BaseSessionId) -> Result<(), BaseError>;
}
```

- Defaults match QGC: **2.00 m**, **180 s**. Operator bounds are 0.01–5.00 m
  and 1–360 integer seconds; these are application bounds, not F9P wire limits.
- `CFG-TMODE-SVIN_ACC_LIMIT` (`0x40030011`) is an unsigned 0.1 mm quantity:
  2 m encodes as 20000. `CFG-TMODE-SVIN_MIN_DUR` (`0x40030010`) is seconds.
  Accuracy is rounded to the nearest wire unit; completion compares against
  that same applied threshold, including inputs with finer decimal precision.
- Start returns an opaque session ID when the worker is created. Read
  `snapshot()` or `subscribe()` for identification/configuration/survey status.
  IDs serialize as strings, preserving all 64 bits over IPC.
- `MON-VER` must identify a ZED-F9P with HPG firmware and protocol 27+ before
  any configuration write. Other u-blox models, including F9R, are rejected.
- Auto baud (`baud_rate = 0`, Survey-In only) probes the QGC baud list using
  identity polls, then moves UART1 to 115200. The baud handoff is verified by
  another identity poll and RAM readback.
- Every required `CFG-VALSET` selects RAM only (`layers = 0x01`), waits for a
  matching ACK and verifies the requested keys with RAM `CFG-VALGET`.
- Configuration enables USB/UART1 UBX and RTCM output, selects the stationary
  navigation model at 5 Hz, disables configured RTCM messages, disables time
  mode, confirms `NAV-SVIN.active = valid = 0`, then starts the new survey.
- Duration is a **minimum**, not a forced stop. Completion requires the
  receiver's `valid = 1`, `active = 0`, sufficient observed time and the
  requested mean accuracy. Missing survey status has its own 5 s timeout;
  total survey duration has no forced deadline. After Ready, the same 5 s
  NAV-SVIN freshness requirement prevents old validity from remaining current
  while unrelated bytes or RTCM continue arriving.
- After completion, confirmed RTCM output is 1 Hz: **1005, 1077, 1087, 1097,
  1127, 1230**. Alternate MSM4 output is disabled. A 1 Hz NAV-SVIN report is
  retained to detect loss of surveyed mode; QGC disables it after activation.
- `Preconfigured` reads the existing output without sending UBX commands.
  Its Ready state means local reading is established, not proof of a base fix.

### Session and write boundaries

The desktop re-enumerates the selected device, checks USB identity and port
ownership, and excludes known FC ports before starting. USB metadata is a
candidate hint; generic bridges still need protocol identification. The core
also validates input before opening an operator-pluggable port.

Stop cancels the reader and any queued/in-flight correction, with an 800 ms
join budget. A timed-out worker retains its reservation until it actually
exits; a stale session ID cannot stop a replacement. Disconnect ends the local
session: the receiver may continue its autonomous survey/output using the last
applied RAM settings. Starting another configured session explicitly resets
that survey. Failures report their step and do not claim that earlier writes
were rolled back.

A vehicle disconnect/reconnect preserves the base. A new forwarder subscribes
to new corrections; one surviving handle also checks its transport generation
and open time. Application shutdown stops the independent base.

Rust's receiver `BaseMode` exports as **`RtkBaseMode`** in TypeScript to avoid
colliding with MAVLink telemetry's existing `BaseMode`.

## Full-size RTCM frames on PX4

`rtcm::MAX_FRAME_LEN` is **1029 bytes**: a 3-byte header, up to 1023 payload
bytes and a 3-byte CRC. This is separate from the **720-byte transport group**
limit imposed by MAVLink's two-bit fragment ID.

```rust
pub fn packetize_px4(
    frame: &[u8],
    sequence_id: u8,
) -> Result<RtcmPacketBatch, RtcmForwardError>;

pub struct RtcmPacketBatch {
    pub packets: Vec<RtcmFragment>,
    pub next_sequence_id: u8,
}
```

The service passes already CRC-validated frames to `packetize_px4`. It emits
ordered groups of at most four 180-byte packets and advances the five-bit
sequence once per group, wrapping 31 → 0. A 1029-byte frame uses lengths
`180, 180, 180, 180, 180, 129`; its tail gets the next group sequence. The
following RTCM frame starts only after every packet has been queued. One RTCM
frame still increments the total/type counters once and contributes its byte
length once, regardless of the number of packets or groups.

`fragment` retains the single-group helper contract and rejects inputs over
720 bytes with `TooLong`. `packetize_px4` reports `InvalidFrameLength { len }`
outside 6..=1029 bytes. CRC/content validation remains the framer's job.

This follows the source-verified PX4 v1.17 byte-stream path: its GPS driver
forwards packet payloads in order rather than validating an RTCM frame per
MAVLink group. Compatibility with another autopilot's reassembly policy needs
its own verification. Exact 360/540-byte groups retain the ADR-008 policy of
omitting zero-length terminators.

## Statistics contract

- `bytes_per_second`: complete, valid RTCM frame bytes whose local transport
  writes succeeded, divided by the actual monotonic sampling interval. It
  excludes noise, corrupt/incomplete frames and MAVLink overhead. A failed
  partial send does not count as a complete frame; successful writes still do
  not prove FC/rover reception.
- `frames_per_type`: counts for the last sampling interval, using the parsed
  12-bit RTCM payload message number; `total_frames` is cumulative per source.
- `dropped_chunks`: cumulative queue losses, expired frames and failed sends.
  Queue units are now whole CRC-valid corrections, dropping oldest first.
- `seconds_since_data`: age of the actual last nonempty source read; `None`
  until the first read. Source age and valid correction throughput are separate.
- `stalled`: more than five seconds since the last nonempty source read, or
  since source startup if no data has arrived. Opening a source never invents
  an arrival. Empty reads do not postpone the alarm.
- A slow stats consumer coalesces pending snapshots without blocking
  correction forwarding, Stop or Shutdown. Stop resets session statistics.
- `base_session_id` and `connection_generation` scope each active snapshot;
  `error` reports the most recent failed frame write until a successful send.
- `BaseStatus` separately reports raw-source and valid-RTCM ages. A stream of
  UBX/NMEA bytes can keep the source alive while valid correction age grows.

Forwarding is immediate; statistics alone tick at 1 Hz. Corrections older than
2 s, from another base session, or received before the current transport open
are discarded. Every fragment of a frame shares one generation and one
deadline (at most 500 ms, also capped by correction age). Cancellation or a
deadline during an active MAVLink stream write retires that transport, since
appending to a partially written frame would be unsafe for framing.

## Verification

```bash
cargo test -p maggcs-core --lib rtk
cargo test -p maggcs-core --test rtk_service_integration
cargo test -p maggcs-core --lib guarded
```

Numerical tests pin wire keys/units/offsets, elapsed-time arithmetic, startup
grace, source age, counters and oldest-first overflow. Scripted receivers cover
split UBX replies, nested RTCM, wrong model, missing ACK, NAK, bad readback,
old-survey reset, dual completion conditions, offline surveying and cancelled
identification. UDP integration reconstructs exact bytes/types and excludes
8192 noise bytes from throughput. Blocked output and stats consumers must
still permit Stop/Shutdown within one second.

Large-frame regression: 6/180/181/360/540/720/721/900/901/1029-byte frame
boundaries, wire sequence rollover and invalid sizes. The UDP integration
rejects a corrupt maximum-size frame, then reconstructs **1600 bytes / 34 valid
RTCM frames** including a 1029-byte frame and its ordinary successors. It checks
exact flags, type counts and byte rate, then stops and restarts the same service
with a 721-byte frame plus a small frame. Before the fix it received only the
372-byte prefix and the service ended; after the fix the full sequence and
bounded Stop/Start/Shutdown pass.

Verified on 2026-10-25: **66 RTK-related library tests, 4 service integration
tests, 2 guarded-transport tests and 5 app RTK/lifecycle tests passed**. The
library selection includes binding exports and related telemetry/safety checks.

Real F9P recordings, PX4-to-GNSS reception, FLOAT/FIXED transitions, serial
unplug/replug and concurrent-QGC field observations remain hardware acceptance
items. See the [detailed backlog](../../../../docs/plans/rtk-base-and-air-ground-todos.md)
for automatic discovery, shared-router arbitration, FC reboot epochs and base
reference-change handling.
The [AUTO contract](../../../../docs/plans/serial-auto-connect-design.md)
implements shared leases, stable-device reconnect and persistent user pause,
including query/configuration write boundaries and AC-01–AC-19. The desktop
keeps manual controls and defaults automatic linking on for eligible devices.
The [hardware checklist](../../../../docs/verification/serial-rtk-hardware-checklist.md)
contains the operator's deferred physical acceptance steps.
Physical acceptance also needs burst/throughput measurements against PX4's
eight-entry injection queue; byte-perfect loopback does not establish rover
receipt under a saturated radio or GNSS serial link.
