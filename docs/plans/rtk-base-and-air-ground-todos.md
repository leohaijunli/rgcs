# RTK base and air–ground link: detailed TODOs

Updated: 2026-10-25. This is the implementation and acceptance backlog from
the Survey-In discussion and the subsequent FC/GCS link review.

Current milestone: **RTK-01 through RTK-08 are complete at the software/browser
level. AUTO-01 is confirmed; AUTO-02/03 software and RTK/GPS HUD are implemented.**
Software verification and the operator checklist are recorded below. Hardware-dependent LINK/FIELD work
will be performed by the operator after software delivery with a detailed test
checklist. Automatic linking defaults to enabled, with manual connections kept.

Deployment clarification: **physical FC connections do not use UDP; UDP is
used for SITL.** Reboot/lifetime work is therefore evaluated against the actual
serial connection, with UDP retained as an optional reproducible test setup.

## Current contract

- Survey-In defaults: **2.00 m**, **180 s**, following QGroundControl.
- Duration is a minimum observation time. Both targets must be met; the
  receiver must report `NAV-SVIN.valid = 1` and `active = 0`.
- F9P configuration is an explicit operation, preceded by receiver identity
  checks. Changes use the **RAM layer** and include time mode, necessary
  navigation settings, and USB/UART1 protocol/message output.
- An application-owned base session can observe without a vehicle link.
  Vehicle disconnect retires its forwarder; application exit stops the base.
- Corrections follow `base → RTCM CRC validation → GPS_RTCM_DATA → FC → GNSS`.
  Successful local transport writes establish only the first transmission
  step, not reception or acceptance by the FC/rover.
- USB classification is a candidate hint. A generic USB-serial bridge does
  not identify the connected device; a receiver identity does not prove that
  survey-in is complete or that the rover has an RTK fix.

## Status and evidence

| ID | Priority | Status | Task / acceptance |
| --- | --- | --- | --- |
| RTK-01 | P0 | Complete | Pin QGC/F9P references, defaults, completion conditions and write boundaries. Independently verify `2 m → 20000 × 0.1 mm`, `180 s → 180`. |
| RTK-02 | P0 | Complete | Mixed UBX/RTCM parser, `MON-VER` F9P identification, old-survey stop barrier, matching ACK and exact RAM readback. Reject wrong model, invalid report, NAK, timeout and mismatching readback. |
| RTK-03 | P0 | Complete | Independent base session with opaque ID and cancellation. Fake receiver surveys without FC; stale Stop cannot stop a replacement; unanswered identification cancels within 1 s; vehicle detach preserves the base, app exit joins it. |
| RTK-04 | P0 | Complete (software) | Classify FC / RTK candidate / unknown; exclude occupied and known FC ports; recheck selected USB identity; prevent vehicle connection commands from opening the active base port. Physical hotplug acceptance remains FIELD-01. |
| RTK-05 | P0 | Complete (software) | Immediate forwarding, bounded oldest-first whole-frame queue, frame-age limit, shared fragment deadline, cancellation and transport generation guards. Verify Stop/Shutdown under blocked output, exact 1029-byte reconstruction and fresh-only reconnect. |
| RTK-06 | P0 | Complete (browser) | Drawer settings, independently sourced base/correction/write/rover status, stale-state handling, cancellation/retry and identity-aware selection. Browser regression and visually checked 1024×640, 1920×1080 and Field/light screenshots pass. |
| RTK-07 | P0 | Complete | Generated types, TypeScript/build/i18n, focused Rust/clippy/format and diff checks pass. READMEs, ADR-027 and manual steps are updated; precision-rounding completion boundaries are regression-tested. |
| RTK-08 | P0 | Complete | Persist this review, implementation status, dependencies and measurable acceptance criteria. |

Software checks already run during this implementation:

- `cargo test -p maggcs-core --lib rtk`: **66 passed**. This selection includes
  11 binding exports and related telemetry/safety checks.
- `cargo test -p maggcs-core --test rtk_service_integration`: **4 passed**.
- `cargo test -p maggcs-core --lib guarded`: **2 passed**.
- `cargo test -p maggcs-app --lib rtk`: **5 passed**.
- `cargo test -p maggcs-core --lib devices`: **12 passed**, including binding exports.
- `cargo test -p maggcs-core --test connection_integration`: **7 passed**.
- RTK TypeScript binding exports to the frontend: **11 passed**.
- `npm run typecheck`, `npm run build`, `npm run check:i18n` and `npm run check:rtk -- http://localhost:5199/ screenshots/rtk`: passed.
- Focused core clippy (library/RTK integration) and core/app library clippy with `-D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Final precision-quantization follow-up: 7 base tests and 8 UBX tests passed;
  completion uses the same rounded 0.1 mm limit that was written to the receiver.
- Screenshots and manual steps: [frontend RTK section](../../frontend/README.md#rtk-drawer).

These are software/fixture results. Hardware evidence is tracked below.

## AUTO / HUD software delivery

- Shared discovery/leases are wired through primary, dedicated/auxiliary FC
  and base entry points; manual controls take priority over pending automatic work.
- Native profiles, role-isolated settings updates, persistent pause, stable-ID
  reconnect, passive FC identification and same-transport handoff are implemented.
- The RTK drawer includes the HUD-style solution, satellite/DOP, correction-age,
  local-TX trend and dual Survey-In progress instruments.
- Configured NAV-SVIN freshness also expires after Ready; own reflected RTCM
  headers are excluded from foreign-injector warnings.
- Diagnostics now export `serial-state.json` and the serial profile for reboot
  and ownership evidence.

Recorded checks include the 47-test discovery/lease checkpoint, 8 FC probe
tests, 11 connection/RTK integration tests, the final core library run
(944 passed / 1 ignored), app library run (183 passed / 8 ignored), and
70 RTK-filtered core tests including ready-state expiry. TypeScript, i18n,
production build, RTK browser and serial-auto/HUD browser checks pass. The new
screenshots are linked in the [frontend README](../../frontend/README.md#automatic-links-and-rtkgps-hud).
Core/app library and test clippy passes with `-D warnings`; workspace format
checking passes. Documentation validation resolves 106 local links and four
anchors across ten delivery documents. Generated TypeScript is retained
byte-for-byte as emitted by ts-rs for the binding-sync check.

The [hardware checklist](../verification/serial-rtk-hardware-checklist.md) has
20 numbered cases, installation/result templates, measured-stage boundaries,
expected outcomes and evidence paths. It covers AUTO, LINK-01–06 and FIELD-01.
Physical tests remain for operator execution after delivery.

## Air–ground link gaps

### LINK-01 — End-to-end correction reception (P0, pending)

**Finding:** `GPS_RTCM_DATA` has no per-frame acknowledgement. A GCS transport
write, including a UDP send, does not prove the FC received a packet or the
GNSS consumed its corrections. A displayed RTK fix can also be stale.

- [ ] Define the evidence and freshness fields available from the installed
  PX4 version and rover receiver.
- [ ] Keep local source bytes, CRC-valid RTCM, successful local writes, FC
  reception, rover correction acceptance and positioning solution distinct.
- [ ] Capture an independent FC/receiver observation alongside GCS counters.
- [ ] Exercise valid corrections, corrupt frames, source silence and dropped
  packets; verify that stale status cannot remain presented as current FIXED.

**Acceptance:** a versioned trace identifies which stages are measured and
which are unknown, shows independently observed correction acceptance and
FLOAT/FIXED transitions, and records freshness/timeout behavior after loss.
Fixture tests must never turn successful writes alone into an RTK-fix claim.

**Dependency:** receiver models/firmware, installed PX4 version and topology.

### LINK-02 — Radio budget, congestion and control latency (P0, pending)

**Finding:** full-size RTCM can require six MAVLink packets. A bounded GCS
queue does not establish that the radio and PX4/GNSS queues sustain bursts.
RTCM and control traffic share transport resources.

- [ ] Measure sustained and burst correction bytes/s with the configured
  constellations and message set, including MAVLink overhead.
- [ ] Record the actual telemetry radio rates, effective throughput and loss.
- [ ] Measure command latency distributions (p50/p95/p99) with corrections
  off, nominal, saturated, and interrupted.
- [ ] Test PX4's injection queue and the actual GNSS output interface under
  burst load, rather than relying only on UDP loopback.
- [ ] Propose/confirm a bandwidth budget and control-latency limit, then
  select message rates/MSM format or scheduling policy from the measurements.
- [ ] Verify cancellation drops the unfinished frame and that expired data
  is not replayed after recovery.

**Acceptance:** publish a reproducible load matrix with measured rates, loss,
queue/drop counts and latency quantiles; the agreed latency and freshness
limits pass. Budget thresholds require measurement and confirmation.

### LINK-03 — PX4-to-GNSS targets and message compatibility (P0, pending)

**Finding:** a functioning MAVLink path does not establish the receiver-side
route or protocol compatibility, especially with two GNSS receivers.

- [ ] Read the installed PX4 configuration/metadata to identify injection
  targets and serial/CAN receiver routes.
- [ ] Record supported RTCM messages for each installed rover model/firmware.
- [ ] Verify the current F9P output set: 1005, 1077, 1087, 1097, 1127 and 1230.
- [ ] Test single-GNSS and dual-GNSS configurations; identify which receiver
  accepts corrections and which telemetry instance reports its solution.
- [ ] Surface unsupported or unknown routing/compatibility without equating
  it with successful GNSS reception.

**Acceptance:** a per-receiver routing/message matrix and independent receive
evidence match the configured target in each tested topology.

### LINK-04 — Multiple vehicles, links and correction producers (P0, pending)

**Finding:** `GPS_RTCM_DATA` has no target-system field. Shared routers may
deliver it to multiple vehicles. Current foreign-injector detection is a
warning, not a complete arbitration policy; reflected own traffic may look
like another injector.

- [ ] Define target-link ownership and desired behavior on a shared router.
- [ ] Define one active correction source per vehicle and explicit source
  selection/failover behavior; confirm interfaces before implementation.
- [ ] Deduplicate output when multiple vehicle/link records share a transport.
- [ ] Identify and exclude own reflected packets from foreign-source alerts.
- [ ] Test two vehicles, two GCS instances, duplicate links, router echo,
  foreign-source disappearance and recovery.

**Acceptance:** fixtures show one selected correction stream on each intended
route, no duplicate output from shared links, no foreign alarm for own echo,
and the documented alert/failover behavior for another real source.

### LINK-05 — Verify physical serial reboot boundaries (P1, pending)

**Purpose:** avoid reusing pre-reboot vehicle state or completing an operation
with stale protocol state when the FC has restarted. The need for an additional
vehicle boot epoch depends on whether that restart retires the GCS transport.

- Native FC USB normally disconnects/re-enumerates on reboot. If the actual
  hardware reliably does so, the existing connection-generation guard may
  already cover this case.
- A USB-to-UART adapter or ground telemetry radio can remain connected to the
  computer while the FC restarts. The serial handle then survives, so a
  transport-generation change alone cannot identify the new FC boot.
- Per the deployment clarification, UDP is a SITL test path, not a physical
  FC requirement for this project.

- [ ] Record the actual physical connection: native FC USB, USB-to-UART, or
  a ground telemetry radio.
- [ ] Reboot/power-cycle the installed FC and record port enumeration, handle
  identity, connection generation and heartbeat/boot-time observations.
- [ ] Compare `(handle_id, generation)` and transport-open timestamps: a new
  connection handle may start again at generation 1. Proposed evidence target:
  five software reboots and five power cycles per installed serial topology,
  recording physical failures separately from user-triggered GCS reconnect.
- [ ] Confirm existing state invalidation/recovery when the serial port does
  reconnect; retain this as a regression if it fully covers the deployment.
- [ ] Only for a demonstrated surviving-serial-handle case, propose additional
  vehicle identity/boot-epoch types and reset events for confirmation.
- [ ] For that case, distinguish boot-time wrap, reordered packets, heartbeat
  loss and a genuinely restarted/replaced FC; define invalidation of telemetry
  caches, commands and mission/parameter exchanges.
- [ ] Preserve the independent base session while refreshing vehicle-side
  state. A vehicle restart does not establish a new physical base position.

**Acceptance:** physical reboot traces show whether generation protection
already covers the installed connection. If the serial handle survives, an
in-place restart fixture proves that old-boot state cannot appear fresh or
complete new operations, and wrap/reordering do not falsely declare a reboot.
A PTY or SITL/UDP fixture may reproduce this case; physical evidence remains
the deciding input.

### LINK-06 — Base reference changes during operation (P0, pending)

**Finding:** a new survey can choose a different absolute reference position.
Successful RTK relative positioning does not remove that reference offset.
Restarting a base can interrupt corrections and shift the vehicle's reported
absolute position during a mission.

- [ ] Define a reference identity/change event and the UI presentation.
- [ ] If exposing base LLH, use the core datum-tagged `Height` convention.
- [ ] Record old/new surveyed coordinates, displacement and interruption time.
- [ ] Confirm restart/reconfiguration behavior during an active mission.
- [ ] Test stationary reference, moved antenna, new survey, receiver restart,
  temporary FLOAT/FIXED loss and recovery.

**Acceptance:** known reference changes produce one correctly scoped change
event with measurable displacement; a new session cannot silently inherit
the old readiness. Real-flight/bench traces document the associated position
and navigation effects before calling the behavior field-validated.

## Automatic serial discovery and connection

### AUTO-01 — Shared discovery/ownership contract (P1, confirmed)

QGC uses its device database and hotplug inventory to select receiver
candidates, reserves the port, probes identity and baud rate, and then applies
the selected receiver configuration. Saved-device connections and discovered
known RTK devices have explicit retry/pause policies.

MagGCS has a device database and hotplug watcher. Its existing FC connection
can discover MAVLink IDs after opening a selected endpoint; this is distinct
from automatically finding an FC across available serial ports.

| Subtask | Status | Deliverable / measurable acceptance |
| --- | --- | --- |
| AUTO-01a | Complete — source audit | Trace hotplug, primary/dedicated/auxiliary FC, independent base, shutdown and both persistence paths to named source functions. Identify one process-wide discovery/lease integration point. |
| AUTO-01b | Complete — accepted contract | Present inventory/attachment/stable identity, lease, probe/handoff, session/intent, settings/pause and error declarations; define serial query versus RAM configuration boundaries and failure handling. |
| AUTO-01c | Complete — confirmed | Publish the [interface design](serial-auto-connect-design.md), [ADR-028](../adr/028-shared-serial-discovery.md), implementation slices and AC-01–AC-19 with explicit timing/open/write/state criteria. |

**Original audit findings (before implementation):** device lists came from independent enumerations;
the watcher has no retained snapshot/join API. Current automatic startup uses
a webview endpoint timer. RTK selection is not persisted. FC reopen uses the
old path, ownership checks are asymmetric across vehicle entry points, and
signalling MAVLink shutdown does not prove the worker closed. The existing
autopilot predicate only excludes GENERIC; INVALID/GCS and configured-ID
fallback paths need explicit coverage in AUTO-03.

- [x] Submit the declarations, ownership/handoff rules, stable-ID ambiguity
  behavior, bounded probing, retry/pause and device-write failure analysis.
- [x] Confirm saved-device priority, persistent pause, automatic Survey-In
  reapplication and initial PX4 multirotor scope. Operator amendment: automatic
  linking enabled by default; retain manual Connect/Disconnect and port/baud selection.
- [x] Confirm native `serial-connections.json` ownership and arbitration with
  the legacy primary-endpoint startup timer.

**Documentation acceptance:** local links/source paths resolve, all seven
contract areas above have declarations, AC-01–AC-19 map to implementation
slices, and proposed behavior is distinguished from recorded results.
Verified on 2026-10-25: five submission documents, 51 local links and two
heading anchors passed; all 19 unique acceptance cases map to implementation
slices, and whitespace/diff checks passed. Software implementation/checks are
now recorded in the delivery section; the hardware checklist tracks physical cases.

**Implementation acceptance after approval:** deterministic inventory/probe
fixtures cover known FC, F9P, unsupported GNSS, generic bridge, ambiguous/busy
ports, changed identities, wrong baud, cancellation, native restart and stale
events. Assert zero CFG writes before F9P identification, zero automatic FC
probe writes and no second open while another worker retains ownership.

### AUTO-02 — RTK base automatic connection/reconnection (P1, complete in software)

AUTO-01 is confirmed. Slices and exact bounds are in the
[interface design](serial-auto-connect-design.md#8-small-implementation-slices-after-confirmation).

- [x] AUTO-02a: implement shared inventory/leases and migrate every manual
  serial selector/IPC reference and open/reopen to actual-worker ownership
  before enabling RTK auto start.
  - [x] AUTO-02a-1: retained inventory, single-flight scans, attachment IDs and
    cancellable/joined shutdown. Device tests: 38 passed, including 15 new
    behavior cases and 11 new binding exports.
  - [x] AUTO-02a-2: worker-owned leases and canonical/stable-key exclusion.
  - [x] AUTO-02a-3: shared application service and manual selector/IPC/open migration.
- [x] AUTO-02b: implement native settings/intent, receiver reconnect and pause.
- [x] AUTO-02c: expose RTK policy/status controls, HUD, bindings and browser
  verification/screenshots; packaged physical review is in HW-12.
- [x] Prefer the saved receiver; discover only eligible known candidates
  when no receiver is explicitly selected.
- [x] Identify F9P/protocol and baud, then use the confirmed RAM Survey-In path.
- [x] Retry with bounded backoff; recover on matching-device hotplug.
- [x] Start a new session after receiver reconnect and reject prior events.
- [x] User Disconnect pauses automatic reconnect until explicitly resumed.
- [x] Handle generic adapters through explicit saved selection and protocol
  identification; do not assume the adapter identifies the attached GNSS.
- [x] Preserve unreadable/newer settings files, reject stale native writes,
  and verify pause serialization/rehydration plus webview reload. Physical app
  restart and device recovery are HW-03/HW-04.

**Acceptance:** startup, unplug/replug with port renumbering, mismatch and
manual-pause fixtures demonstrate exactly one active receiver and the agreed
retry timing, with no configuration of an unverified replacement. Cover
AC-01–12 and AC-17–19; retain independent base/forwarding regressions.

### AUTO-03 — FC serial discovery (P1, complete in software)

AUTO-01 is confirmed. Receiver identification and FC identification use
different protocol evidence.

- [x] AUTO-03a: implement the common autopilot predicate, passive bounded probe
  and same-open-transport handoff.
- [x] AUTO-03b: integrate the primary-link supervisor, startup arbitration,
  fresh-identity retries and manual override.
- [x] AUTO-03c: add FC status/selection/Resume UI and browser screenshots;
  packaged manual review is covered by the hardware checklist.
- [x] Select free FC candidates, then examine MAVLink at candidate baud rates.
- [x] Require a valid autopilot heartbeat; exclude GCS and non-autopilot nodes.
- [x] Apply the predicate to configured-ID/adoption/liveness paths as well as
  discovery. Test INVALID/GCS heartbeats at the expected system/component IDs.
- [x] Learn the FC system/component IDs and hand the selected endpoint to the
  normal connection manager.
- [x] Transfer the successful probe's live transport; do not reopen via the
  ordinary `connect` command or run two retry schedulers for the same role.
- [x] Preserve manual override, ambiguity handling and retry/pause behavior.
- [x] Share port ownership with the base service; never probe through its
  active serial stream.

**Acceptance:** fixtures distinguish PX4 heartbeats, GCS heartbeats, GNSS-only
output, noise and an occupied port. Discovery selects the documented FC/IDs,
never opens an owned base port, and remains bounded/cancellable.
Cover AC-13–16 plus shared ownership, lifecycle and UI cases AC-05–07,
AC-11–12 and AC-17–19. Negative fixtures assert zero normal-session writes.

## FIELD-01 — Physical integration evidence (P0, pending hardware setup)

- [ ] Record base and rover models/firmware, PX4 version, radio models/rates,
  baud rates and a connection/router diagram.
- [ ] Identify native FC USB versus USB-to-UART versus telemetry-radio serial;
  record QGC's connection and correction-source routes. Populate the installed
  hardware record before starting LINK-01–04/LINK-06; software versions used
  by SITL fixtures do not substitute for it.
- [ ] Save a small real RTCM fixture with independent CRC/type counts and
  message-rate metadata in `testdata`.
- [ ] Compare Survey-In time, estimated accuracy and completion with an
  independent receiver tool, using the same configured targets.
- [ ] Verify FC/GNSS reception, FLOAT → FIXED, correction interruption and
  recovery; record timestamps and solution freshness.
- [ ] Measure base unplug/replug, port renumbering, Stop response and failed
  configuration recovery.
- [ ] Observe concurrent QGC injection and restoration to a single source.
- [ ] Review the packaged Tauri UI at desktop/compact/Field sizes.

**Acceptance:** attach the versioned captures, measurements and manual results
to this plan. Software fixtures and screenshots establish their own layers;
each hardware claim needs its corresponding recorded observation.

## References

- [RTK drawer review](rtk-drawer-review.md)
- [Operator hardware checklist — HW-01–HW-20](../verification/serial-rtk-hardware-checklist.md)
- [Shared serial discovery interface and AC-01–AC-19](serial-auto-connect-design.md)
- [Shared-discovery ADR](../adr/028-shared-serial-discovery.md)
- [RTCM forwarding ADR](../adr/008-rtcm-forwarding.md)
- [Core RTK README](../../crates/core/src/rtk/README.md)
- [QGC settings, pinned 73e64bc81b](https://github.com/mavlink/qgroundcontrol/blob/73e64bc81be50c8581ca1bd411a751f087e12b8f/src/Settings/RTK.SettingsGroup.json)
- [QGC UBX configuration](https://github.com/mavlink/qgroundcontrol/blob/73e64bc81be50c8581ca1bd411a751f087e12b8f/src/GPS/Core/Protocols/UBX/UBXConfigurator.cc)
- [QGC receiver connection policy](https://github.com/mavlink/qgroundcontrol/blob/73e64bc81be50c8581ca1bd411a751f087e12b8f/src/GPS/Receiver/GPSReceiverConnectionPolicy.cc)
- [u-blox F9 HPG 1.32 protocol, UBX-22008968 R01](https://content.u-blox.com/sites/default/files/documents/u-blox-F9-HPG-1.32_InterfaceDescription_UBX-22008968.pdf)
