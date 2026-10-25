# RTK GPS drawer review — 2026-10-25

The active implementation and air–ground/automatic-connection backlog is in
[RTK base and air–ground TODOs](rtk-base-and-air-ground-todos.md). The assessment
below records the earlier preconfigured-source review and its evidence.

## Assessment

The rover → base → injection hierarchy is usable for a preconfigured RTCM
base. It is not yet evidence of field-ready base configuration or correction
delivery. The drawer and core statistics required the following corrections:

| Finding | Resolution / acceptance |
| --- | --- |
| RTCM type decoded from the preamble | Use `RtcmFrame::message_type`; integration asserts 1005 and 1074 independently |
| Immediate stall before the first source byte | Separate startup grace from last actual read; null age stays null until data arrives |
| Raw source bytes counted as forwarded B/s | Count only valid forwarded frame bytes; reject noise from throughput |
| Assumed exactly one second per sample | Divide by actual monotonic elapsed time; 900 bytes / 3 seconds = 300 B/s |
| Slow stats consumer could block Stop | Coalesce pending snapshots; full-channel Stop/Shutdown are bounded |
| Lost FC heartbeat disabled local source Stop | Stop depends on busy state only; errors are visible and retryable |
| Late cached stats could replace live data | Revision guards reject stale cache replies and replies after reset |
| Truncated health values / stale rover fix | Wrap values, show source-data age and annotate old GPS reports |
| Valid RTCM frames over 720 bytes ended the service | Confirmed full-frame packetizer supports up to 1029 bytes across ordered groups; counts, sequence rollover and Stop/Start are regression-tested |

The Rust/IPC `RtkStats` shape is retained. Throughput is queue acceptance,
not transport-write confirmation, rover reception or RTK fix proof. The stall
alarm measures nonempty source reads; a stream of non-RTCM bytes can keep that
source alive while valid correction throughput remains zero.

## Automated coverage

- RTCM parsing/CRC/resynchronization and MAVLink fragmentation tests.
- Exact arithmetic, startup/first-data/reset, source queue overflow tests.
- Fake source → production service → UDP peer byte reconstruction and counts,
  including maximum-size RTCM, rejected CRCs, sequence rollover and restart.
- Backpressured stats receiver with bounded source teardown.
- Real React drawer/store with IPC fixtures: cached and live stats, source
  age/stall/foreign injection, start gate, failed-stop retry after heartbeat
  loss, stale cache rejection and disconnect reset.
- Layout screenshots at 1024×640, 1920×1080 and Field/light mode in
  `frontend/screenshots/rtk/`.

Commands: core module README and frontend `npm run check:rtk`.

Verified on 2026-10-25: 39 RTK-filtered library tests and 2 service integration
tests passed; the browser regression, TypeScript check, i18n check and focused
core clippy check passed. The 39 include binding exports and two related
telemetry/safety tests, not 39 independent hardware scenarios.

Large-frame follow-up (2026-10-25): **42 RTK-filtered library tests and 3 service
integration tests passed**. The regression independently pins the full 1600-byte
stream and counts 34 RTCM frames, including one 1029-byte frame and following
small frames. It failed before the fix after the 372-byte prefix. Stopping and
restarting with a 721-byte frame also passes. See the
[packetization contract](../../crates/core/src/rtk/README.md#full-size-rtcm-frames-on-px4)
and ADR-008 for the PX4-specific grouping and failure analysis.
Focused core clippy (`--lib --test rtk_service_integration -- -D warnings`),
workspace format checking and `git diff --check` also passed.

## Remaining acceptance and design gaps

| Gap | Required evidence or follow-up |
| --- | --- |
| Real RTCM fixture | Record receiver/firmware, enabled message set and baud; commit a small capture with expected CRC/type counts |
| End-to-end GNSS delivery | Demonstrate PX4 forwards to the installed rover, corrections are accepted, and FLOAT → FIXED transitions match independent receiver/QGC observations |
| Base unplug/replug | Measure error/stall timing, source release, port renumbering and explicit restart on actual hardware |
| Multiple injectors | Observe actual QGC/router topology, foreign warning onset/expiry and single-source recovery |
| Base configuration | Message-set selection, survey-in and UBX setup are absent; the current path requires an externally configured base |
| Large-frame field throughput | Software now supports 1029-byte RTCM frames; measure loss and recovery against PX4's eight-entry injection queue and the real radio/GNSS link under burst load |
| Port identity | The first pluggable device is selected by default; hotplug can leave a stale selection. Device identity/exclusion of an already used FC port needs a separate contract |
| Dialog keyboard behavior | Focus trapping/restoration and Escape handling need a dedicated accessibility pass |
| Automatic reconnect | Runtime cache clearing on explicit disconnect is covered; late untagged events and automatic transport-generation recovery need additional lifecycle design |
| CI / native UI | Browser regressions are runnable scripts, not currently part of the tag-only CI build job; packaged Tauri behavior still needs manual observation |

SITL standard-message stream verification (MR-20) cannot substitute for F9P
correction reception. MR-19 source mappings and MR-22b Motor Test serial reuse
are tracked in the motor-test remediation plan.
