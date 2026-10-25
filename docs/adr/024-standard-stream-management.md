# ADR-024: Standard MAVLink Stream Management

- Status: Accepted (2026-10-24); MR-18 software-verified.
- Date: 2026-10-24
- Relates to: ADR-003, ADR-023, motor-test remediation MR-18–MR-20.

## Context

The installed PX4 firmware cannot be changed. MAVLink
`MAV_CMD_SET_MESSAGE_INTERVAL` changes an existing stream; it cannot expose an
arbitrary uORB topic. A decoded standard message also does not prove a given
firmware build implements its stream.

Motor Test previously requested the same target streams during connect,
telemetry attachment and every 3 s of scope flushing. Those writers used
different defaults and would undo operator rate/disable choices. A connection
handle survives transport reconnect, so a handle identity alone cannot reject
queued requests for an obsolete transport.

## Decision

1. Generate the standard catalog from the locked direct `mavlink` dependency's
   `common`/included XML definitions. Commit the Rust table with input hashes,
   and check it against regeneration in CI and compiled message IDs/names/CRC
   in tests. Standard field types and wire units are shown verbatim.
2. Search results are suggestions. Sending requires an explicit exact numeric
   ID from a selected compiled message. Documented PX4 source support is separate
   from live receipt; unknown support requires an explicit choice. MR-19 adds
   six source-reviewed uORB mappings with pinned PX4 v1.17 links and notes.
   Exact standard names/IDs take precedence over source aliases; shared
   `vehicle_imu` is ambiguous and unverified topics have `NoStandardMapping`.
   Custom dialect/firmware paths are not offered.
3. One dedicated-target stream service owns effective defaults and user
   requests. Saved overrides replace defaults by ID. View attachment and scope
   flushing never reassert default rates. The primary-link Inspector is a
   different connection in the dedicated workflow. When explicitly borrowing
   the main serial connection (ADR-026), rates affect every subscriber on that
   shared link; the Inspector retains its existing per-signal control.
4. Save requested rates in a versioned, atomically replaced
   `app_config_dir/motor-streams.json`, keyed by endpoint/system/component.
   A failed transport write retains the desired preference and reports its
   error. Reapply is explicit; reconnect restores each effective ID once after
   receiving the selected node's new-generation traffic. Corrupt existing files
   remain intact and are reported.
5. Connection IDs are monotonic and not reused. Transport opens have a separate
   monotonic generation and start time. Rate writes carry a deadline/generation
   guard which the connection worker checks before writing; expired, cancelled
   and old-generation entries cannot migrate to a replacement transport. Lost
   lifecycle events resynchronize through the handle.
6. Requested state and observations are independent. Observed Hz requires two
   fresh frames; disabled streams may still report received data. Bounded arrival
   history resets at rate, transport and stale/gap boundaries. Staleness scales
   with slow requested periods rather than assuming every stream is high-rate.
7. Rate writes and preference I/O do not acquire the motor-operation lock.
   STOP/E-STOP release receipts remain independent. The stream panel occupies
   bounded analysis space and keeps motor stop actions outside it.

## Consequences and verification

The operator sees exact message metadata and requested/observed/stale state,
with no inference that a transport receipt is a firmware ACK or honored rate.
The GCS can configure existing standard streams while leaving actual source
availability and rate clamping to explicit firmware observations.

Recorded software acceptance: 32 core MAVLink tests, 7 connection integration
tests, 7 app stream tests (1 hardware diagnostic ignored), the 158-test app suite
(7 hardware/data tests ignored), clippy/format checks and the real React IPC
regression. Regressions cover default/override ownership, command counts,
generation rejection, lost-event recovery, persistence isolation, requested
versus observed behavior, latest pending edits, and motor stop while stream work
waits. Full panel bounds and Field targets are measured in browser checks.

MR-20's [PX4 v1.17.0 SIH report](../verification/mr20-standard-streams-20261025.md)
records required add/rate/disable/stale and reconnect acceptance. Rate changes
were observed in 572 ms. A 1000 Hz HIGHRES_IMU request delivered 218.13 Hz on
the 250 Hz SIH source loop; this is observed under-delivery, not a claimed
universal clamp. ESC_STATUS had no producer in the fixture. Installed-hardware
and native desktop observations remain separate acceptance.
