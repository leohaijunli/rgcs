# Serial auto-link, RTK and air–ground hardware acceptance

Status: **Ready for operator execution; physical results are pending.**
Software and fixture results are recorded in the
[implementation backlog](../plans/rtk-base-and-air-ground-todos.md).
Run this checklist after the coding/automated checks, on the installed hardware.

## 1. Record the installation

Use one result folder per topology and test date. Record actual values; the
PX4/F9P versions used by software fixtures are not installation evidence.

| Item | Value to record |
| --- | --- |
| Q-Mag | Version/build, source revision and local modifications, packaged or development build |
| Host | OS/version, USB hub/cables, permissions, power source |
| FC | Board, PX4 version/build, MAVLink system/component IDs, native USB or UART/radio connection |
| Base | Receiver model, firmware, UBX protocol version, antenna, USB identity/serial, baud |
| Rover 1 / 2 | Model, firmware, antenna, FC port/bus, telemetry instance and correction input |
| Radios | Ground/air models and firmware, air rate, UART rates, routing/fan-out |
| QGC / router | Versions, endpoints, system/component IDs, whether packets are echoed or headers rewritten |
| Receiver settings | Survey-In or preconfigured mode, target accuracy/time, constellations and RTCM message rates |
| Reference | Antenna location/mark, known coordinates if available, coordinate system and height datum |

Draw both directions of the actual path:

```text
base GNSS -> host serial -> Q-Mag -> FC USB/UART/radio -> PX4 -> rover GNSS
                                      ^                         |
                                      +------ telemetry --------+
```

Add QGC, routers, secondary GNSS receivers and every additional vehicle to this
diagram. List every intended recipient of correction traffic.

Use the normal grounded/props-off bench setup for connection, reboot and
congestion tests. Survey-In Connect changes identified F9P RAM settings on
USB/UART1; Disconnect closes local I/O and does not undo those receiver settings.

## 2. Evidence and timing

Create a test folder with the following contents:

```text
<date>-<topology>/
  installation.md
  results.csv
  topology.svg (or a photographed diagram)
  diagnostics-before/
  diagnostics-after/
  screenshots/
  os-hotplug.log
  receiver-observations/
  mavlink-or-ulog/
  rtcm-capture/ (when available)
```

Use **Tools → Export diagnostics** before/after significant transitions.
The bundle includes `serial-connections.json` when present and
`serial-state.json`: capture time, software version, actual connection handle ID
and generation, selected target, inventory/leases, automatic policy and base
session/status. Keep the recorder files referenced by `session.json` separately.

Use an independent receiver/PX4 observation for receipt and RTK solution claims.
An app TX counter is evidence of a local write only. For continuous reboot
timelines, combine diagnostics snapshots with native logs, MAVLink/ULog captures
and OS events. On Linux a read-only hotplug monitor can be run with:

```bash
udevadm monitor --udev --property --subsystem-match=tty
```

Record at least these columns in `results.csv`:

```text
test_id,trial,topology,request_time,os_device_time,app_state_time,last_tx_time,
worker_closed_time,ready_time,handle_before,generation_before,handle_after,
generation_after,base_session_before,base_session_after,result,evidence,notes
```

Interpret the software budgets by stage:

- Inventory polls every **500 ms**; an individual scan has a **2 s** budget.
  Measure from OS enumeration visibility as well as physical insertion.
- FC observation uses **3 s per baud**, at most **25 s** per probe. Reservation,
  native lifecycle and handoff are separate stages.
- Base opening/identification has a **16 s** deadline; an OS call that fails to
  return retains ownership and reports timeout rather than permitting reuse.
- Stop cancellation is exercised within **1 s** in bounded-I/O fixtures; base
  join has an **800 ms** budget. Record actual driver closure separately.
- Settings I/O has a **2 s** queue/write budget. A persistent-pause reply can
  follow device cancellation; delayed disk I/O must not delay last device TX.
- Transient retry delays are **1, 2, 4, 8, 16, 30, 30… s**. Missing or internally
  occupied devices wait for inventory/lease changes.
- Complete correction frames expire at **2 s**; local fragment writes share a
  deadline of at most **500 ms**. Source/RTCM silence warnings use **5 s**.
- Configured Survey-In validity expires after **5 s without NAV-SVIN**, including
  after Ready. Fresh unrelated bytes do not refresh that validity.

## 3. Connection and ownership tests

### HW-01 — Default automatic connection

1. Start with a fresh serial profile, or explicitly enable both automatic roles.
2. Attach one supported PX4 multirotor and one supported F9P, with unique USB IDs.
3. Launch Q-Mag, open Settings → Connection and the top-bar RTK/GPS drawer.
4. Observe discovery, target IDs, receiver identification and Survey-In startup.

**Expected:** automatic linking is on by default; one owner per port. FC and
base can connect in either order. Base observation starts without an FC.
Capture the selected port/USB identity, actual FC IDs and both session scopes.

### HW-02 — Manual connection remains available

1. Use the explicit manual type/port/baud controls in Settings → Connection.
2. In the RTK drawer, select receiver mode, port, baud, accuracy and duration,
   then use **Connect base**.
3. Repeat while automatic discovery is waiting or probing the same candidate.
4. Try an invalid endpoint/baud, then correct it and retry.

**Expected:** manual choice takes priority; the old probe is cancelled and its
port retired. A failed manual replacement preserves a working existing link.
With no existing link, failure does not silently select a different auto device.
Connecting to the already-connected same attachment does not open it twice.

### HW-03 — Disconnect, persistent pause and Resume

1. Disconnect the base through its manual button; disconnect the primary FC.
2. Leave devices attached for at least **65 s**.
3. Unplug/replug while paused; reload the UI and restart Q-Mag.
4. Use **Resume auto** or explicit **Connect** for one role at a time.

**Expected:** no automatic reopen while paused, including after restart. Resume
creates one current attempt. Pausing the FC preserves the independent base.
Record last TX, port closure and saved-pause completion separately.

### HW-04 — Stable identity and OS port renumbering

1. Save/select receiver A explicitly and establish a working connection.
2. Unplug A; attach another device B at its old OS path if the OS permits.
3. Reattach A with a changed OS name, including through the deployment USB hub.
4. Repeat **five times** for each installed device/transport.

**Expected:** the saved identity selects A under its new name; no fallback to B.
The new attachment/base session has a new ID. Changed UI selections require
reselection. Every FC transport replacement has a distinct handle/generation
tuple. Keep inventory and before/after diagnostics.

### HW-05 — Ambiguous and unidentified devices

1. Attach two eligible FCs or bases; exercise missing/duplicate USB serials if
   the installed adapters exhibit them.
2. Attach a generic USB–UART bridge before assigning it a role.
3. Explicitly select/save that bridge, then connect a supported receiver.

**Expected:** ambiguity requires selection; an arbitrary bridge is not UBX-probed
automatically. Manual selection works without inventing a stable identity.
Weak/duplicate identities do not become silent automatic reconnect authority.
The bridge's identity is not proof of the receiver behind it.

### HW-06 — Mutual exclusion, aliases and Motor Test borrowing

1. While the base owns a port, attempt primary/target/auxiliary FC connection
   to that same port. Repeat in reverse order.
2. Repeat using a canonical path alias where the OS supports it.
3. With a main serial FC link, use Motor Test's **Use main serial link**.
4. Detach/close Motor Test and verify that primary telemetry still works.

**Expected:** one actual owner; losing requests identify the occupied port.
Borrowing creates no second serial open or parameter worker. A reconnect revokes
borrowed motor-control permission. Use passive OS tracing/a physical analyzer for
open/write evidence; do not add another competing raw serial reader as a sniffer.

### HW-07 — External ownership and permissions

1. Let QGC or the receiver tool own a raw serial port; request it in Q-Mag.
2. Release it and retry. If practical, test a host account without port access.

**Expected:** busy/permission errors are visible; no automatic privilege prompt
or configuration on an unverified device. Confirm OS exclusive-open behavior for
the installed driver; an in-process lease cannot guarantee another process's policy.

### HW-08 — Cancel during opening and identification

1. Start a probe against a silent receiver or wrong fixed baud.
2. Cancel/Disconnect during identification and during configuration.
3. Immediately request the same port from another role.

**Expected:** cancellation is prompt; no further command is issued after the
worker observes it. A still-running driver retains its lease and reports timeout.
No second opener wins until actual closure. Earlier RAM writes may remain applied.

## 4. Receiver, correction and dashboard tests

### HW-09 — Identity and configuration boundaries

1. Connect the installed F9P and record MON-VER model/firmware/protocol evidence.
2. Where available, repeat with unsupported GNSS or an unexpected device behind
   the explicitly selected bridge.
3. Compare independent RAM readback with the requested Survey-In values.

**Expected:** supported F9P identity precedes configuration; unsupported identity
receives no CFG writes. Default targets are **2.00 m / 180 s**, encoded as
**20000 × 0.1 mm / 180 s**. Writes use RAM only. Required NAK, timeout or wrong
readback reports failure rather than starting an endless configuration loop.

### HW-10 — Survey-In and receiver validity

1. Place the antenna on a stationary, documented mark with appropriate sky view.
2. Start Survey-In and compare duration, estimated mean accuracy, active and
   valid flags with an independent receiver tool.
3. Observe a case where time is met before accuracy, if conditions allow it.
4. Confirm completion and repeat with a different allowed accuracy/time target.

**Expected:** both targets and receiver `valid = 1`, `active = 0` are required.
The duration target is a minimum, not a forced end. Mean accuracy is receiver
estimated survey accuracy, not a proof of absolute coordinate accuracy.

### HW-11 — Preconfigured mode

1. Configure the receiver externally, close that tool, then select preconfigured
   reading with a fixed baud in Q-Mag.
2. Observe raw data, valid RTCM and local TX independently.

**Expected:** zero UBX/configuration bytes from this mode. Reader Ready does not
claim Survey-In completion, rover reception or an RTK fix.

### HW-12 — RTK/GPS HUD and stale-data handling

1. Open the RTK/GPS drawer's top instrument panel at desktop, compact and Field
   sizes. Check FIXED/FLOAT/3D, satellites, HDOP/VDOP and RTCM age against telemetry.
2. Observe the two Survey-In progress bars reaching their targets independently.
3. Interrupt rover telemetry and then correction input separately.
4. Reconnect/change the base or FC and inspect the TX history.

**Expected:** missing values show NO DATA; old FIXED is visibly STALE. DOP is
dimensionless, not metres. Full progress bars alone do not declare survey valid.
TX is explicitly local; the bounded history resets at session/transport boundaries.
Manual controls and Disconnect remain reachable while scrolling the drawer.

### HW-13 — Source silence, receiver restart and ready-state expiry

1. After Ready, stop valid RTCM while allowing other receiver output to continue.
2. Stop NAV-SVIN reports, if the independent receiver setup can do so safely.
3. Reboot the receiver behind its USB–UART adapter, where applicable.

**Expected:** unrelated bytes do not hide stale RTCM or survey validity. Missing
configured NAV-SVIN for over 5 s invalidates Ready. Recovery re-identifies the
receiver and creates a new base session before applying the selected mode.
Record line-control/reset effects and baud changes on the installed adapter.

### HW-14 — End-to-end reception and positioning (LINK-01)

1. Establish a baseline with this correction source paused and competing sources off.
2. Resume valid corrections; record local source/CRC/TX stages separately.
3. Independently observe FC correction reception, PX4-to-GNSS injection, GNSS
   acceptance and the rover solution's FLOAT/FIXED transitions.
4. Interrupt corrections, restore them, and measure freshness and recovery.

**Expected:** each claimed stage has its own timestamped evidence. Successful
`GPS_RTCM_DATA` writes alone never establish receiver acceptance. Record the
actual receiver's fix-hold/loss behavior; no universal time-to-FIXED is assumed.

### HW-15 — RTCM fixture and compatibility (LINK-03 / FIELD-01)

1. Save a small real capture using an independent read-only receiver tool or
   supported passive capture route, with firmware/message-rate metadata.
2. Independently count CRC-valid/corrupt frames and message types.
3. Check configured **1005, 1077, 1087, 1097, 1127, 1230** output, noting absent
   constellations or firmware differences.
4. Record each rover's supported message set and actual accepted messages.

**Expected:** fixture bytes and independent counts agree. An incompatible rover
is reported as a compatibility gap, not successful reception. Close the capture
tool before handing the raw port back to Q-Mag; note any reset from opening it.

## 5. Air–ground and reference tests

### HW-16 — Load and latency matrix (LINK-02)

Run corrections **off**, **nominal**, **saturated/burst**, and **interrupted** on
each real radio/UART topology. Measure source payload B/s, MAVLink overhead,
effective throughput, loss, drop counters and command round-trip p50/p95/p99.
Use benign supported requests on the grounded bench; keep flight commands out
of the congestion load generator.

**Expected:** publish measured budgets and agree the operational latency/loss
limits from those results. Expired corrections are dropped; reconnect does not
replay old frames. Local bounded queues do not establish downstream radio/PX4/
GNSS capacity. Record the actual receiver route's sustainable rate.

### HW-17 — Single/dual GNSS and route selection (LINK-03)

For every installed receiver, record the exact PX4 parameter names/values and
bus/serial route from that firmware's metadata. Repeat HW-14 with single and
dual GNSS. Match correction receipt and solution telemetry to the intended
receiver instance.

**Expected:** a per-receiver route/message matrix with independent receipt
evidence. Unknown route/instance is recorded as unknown, not inferred from local TX.

### HW-18 — QGC, shared routers and multiple vehicles (LINK-04)

1. Test Q-Mag alone, QGC alone, both connected with one injector, then competing
   correction injection and foreign-source disappearance/recovery.
2. Exercise router echo and duplicate routes; include two vehicles if deployed.
3. Compare captured outgoing streams and per-recipient corrections.

**Expected:** own unchanged system/component reflections do not trigger a foreign
injector alert. A real foreign source does; disappearance expires the warning.
`GPS_RTCM_DATA` has no target-system field, so explicitly verify fan-out to every
recipient. Current foreign-source handling is advisory, not automatic failover
or multi-GCS arbitration. Record routing/source-selection decisions before using
a shared topology operationally.

### HW-19 — FC reboot and connection lifetime (LINK-05)

Run **five software reboots and five power cycles per deployed serial topology**:
native FC USB, USB–UART and radio as applicable. Capture OS events, heartbeats,
available boot-time observations and before/after diagnostics. Distinguish
physical restart from an operator-triggered GCS reconnect.

**Expected:** compare `(handle_id, generation)`, not generation alone. A new
handle can start at generation 1. If every reboot retires the transport, confirm
old protocol/cache/permission state is invalidated and the independent base
session survives. If any genuine FC restart retains the same tuple, attach that
trace for a vehicle boot-epoch design; test wrap/reordering before adopting one.

### HW-20 — Base reference changes (LINK-06)

Keep the antenna stationary for one re-survey, then move it by a documented
distance for another. Record independent old/new ECEF or LLH coordinates with
datum, displacement, correction interruption and rover position changes.

**Expected:** every restarted receiver session clears old readiness. Compare
reference displacement independently; the dashboard's survey accuracy and rover
FIXED label do not prove reference continuity. Define mission-time re-survey
policy from these observations before operational flight acceptance.

## 6. Completion record

For each test record **PASS / FAIL / NOT APPLICABLE / BLOCKED**, the topology,
measurements and evidence paths. A blocked receiver/counter capability needs a
named follow-up, not an inferred pass. Return:

1. Filled installation/topology and result tables.
2. Before/after diagnostics, screenshots and independent receiver/FC captures.
3. Timing/load distributions, message/route compatibility matrix and real RTCM fixture.
4. LINK-05 tuple/reboot traces and LINK-06 reference-displacement observations.
5. Firmware/driver-specific failures and the agreed operational limits.

This evidence completes the physical portions of AUTO, LINK and FIELD acceptance.
