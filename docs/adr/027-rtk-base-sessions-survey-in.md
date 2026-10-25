# ADR-027: Independent RTK base sessions and F9P Survey-In

- Status: Accepted (operator confirmation, 2026-10-25)
- Date: 2026-10-25
- Related: [ADR-008](008-rtcm-forwarding.md), [detailed TODOs](../plans/rtk-base-and-air-ground-todos.md)

## Context

The original drawer opened a preconfigured serial byte source inside the
primary MAVLink runtime. It could neither configure Survey-In nor observe a
base without an FC connection. Adding two numeric fields alone would leave
that ownership model and the receiver protocol incomplete.

QGC's receiver lifecycle is independent of vehicle forwarding. Its F9P path
identifies the device, resets old time mode, applies survey targets, observes
NAV-SVIN completion and activates RTCM output. Sources were pinned to QGC
commit `73e64bc81be50c8581ca1bd411a751f087e12b8f` and u-blox document
UBX-22008968 R01 (F9 HPG 1.32).

## Decision

1. An application-owned `RtkBaseServiceHandle` owns one serial worker and an
   opaque `BaseSessionId`. `BaseMode` explicitly selects preconfigured reading
   or F9P Survey-In. Start acknowledges session creation; state reports the
   asynchronous configuration outcome. Stop addresses that exact session.
2. Survey-In defaults to **2.00 m / 180 s**. The QGC operator bounds are
   0.01–5.00 m and 1–360 s. Wire accuracy uses 0.1 mm units. A receiver report
   must satisfy time, accuracy, `valid` and `!active`; duration is not a timer
   that forces completion.
3. The desktop checks current device metadata, known-FC classification and
   occupied ports. Supported F9P identity/protocol is required before CFG
   writes. Generic adapter metadata remains unknown-device evidence.
4. Configurations use RAM-only CFG-VALSET, matching ACK and exact RAM readback.
   The baud handoff instead uses identity/readback evidence because its ACK
   can be lost. Required NAK, timeout or mismatch ends configuration.
5. USB/UART1 output and necessary stationary/5 Hz navigation settings are
   established; configured RTCM output is disabled, old time mode is disabled
   and confirmed stopped, and the new survey starts.
6. Successful survey completion activates 1 Hz 1005/MSM7/1230 output. NAV-SVIN
   remains enabled at 1 Hz to invalidate readiness on a receiver mode change;
   this deliberately differs from QGC's post-activation disabling of NAV-SVIN.
7. A mixed framer consumes complete UBX and RTCM messages. Bytes inside valid
   UBX payloads cannot be interpreted as independent RTCM. Only complete,
   CRC-valid corrections enter a bounded, oldest-first queue.
8. Each vehicle forwarder accepts only the current base session's fresh
   frames after its transport-open time. Each whole frame shares one transport
   generation and one deadline. Stop/Shutdown cancels blocked sends; expired
   or cancelled active stream writes retire the transport to discard partial
   MAVLink framing.
9. Vehicle disconnect preserves base observations. Application exit closes
   the base. Native/UI status is scoped by base session/revision and ordered
   link revisions; retiring an old runtime cannot replace a newer snapshot.

## Device-write failure analysis

| Failure | Handling / remaining boundary |
| --- | --- |
| Selected FC, busy port or changed USB device | Reject before opening for base configuration; FC connection entry points also reject the active base port. |
| Generic bridge, wrong receiver or unsupported firmware | Identity polling precedes configuration; unsupported/missing identity fails. Adapter identity alone cannot establish what is attached behind it. |
| Wrong baud or lost handoff ACK | Bounded host-rate probing; new-rate identity and RAM baud readback establish handoff. |
| Late ACK, NAK or failed readback | One required transaction at a time, followed by exact readback; failure ends the session rather than crediting a later command with an ambiguous ACK. |
| Restarted survey or partial configuration | Earlier RAM writes may remain effective. Disconnect releases local ownership; it does not claim to undo receiver state. A new configured connection explicitly stops the previous survey. |
| Stop during identification/configuration | Bounded I/O and shared cancellation; an 800 ms join budget reports a timeout while retaining a still-running worker's reservation. |
| Link cancellation/timeout after some bytes were written | Discard the remaining RTCM fragments and retire a partially written MAVLink transport. A rover may have received a prefix, never an implied acknowledged frame. |
| New base position after re-survey | Session identity changes. Coordinate-displacement reporting and mission-time recovery policy are tracked in LINK-06. |

## Statistics and UI

The drawer exposes target settings, actual observation progress and receiver
identity. Base source data, valid RTCM, local transport writes and reported
rover solution remain separately visible. Raw UBX/NMEA traffic is not evidence
of valid corrections. Local write success is not an FC/GNSS acknowledgement.

`RtkStats.bytes_per_second` now counts successfully written complete RTCM
frames rather than merely queue acceptance. `dropped_chunks` retains its IPC
name, but its queue units are complete corrections, including expiry/send
losses. Base/transport identities scope the counters.

The TypeScript receiver-mode name is `RtkBaseMode`, avoiding the existing
MAVLink telemetry `BaseMode` binding. Settings are saved locally and applied
on Connect. A unique known RTK candidate can be preselected; automatic device
connection/reconnection remains the separately specified AUTO backlog.

## Acceptance

Protocol fixtures pin `20000 / 180`, message keys, RAM layer, NAV-SVIN offsets,
reset ordering, identity restrictions, ACK/readback failures and cancellation.
An offline base completes its survey. Loopback reconstructs maximum RTCM
frames and verifies fresh-only transport reconnection. Blocked output and
status delivery do not hold Stop/Shutdown beyond the 1 s software test bound.

Browser tests cover default/invalid input, FC exclusion, replacement device
selection, offline survey, expired state/cache replies, cancellation and retry.
Screenshots cover desktop, 1024×640 and Field/light mode. Real F9P/PX4/radio
acceptance is tracked with explicit evidence requirements in FIELD-01 and
LINK-01 through LINK-06.
