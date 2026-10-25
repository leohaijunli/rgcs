# ADR-028: Shared serial discovery, ownership and automatic intent

- Status: Accepted — operator confirmation and default-on amendment, 2026-10-25
- Date: 2026-10-25
- Related: [ADR-026](026-main-serial-target-reuse.md),
  [ADR-027](027-rtk-base-sessions-survey-in.md),
  [interface and acceptance draft](../plans/serial-auto-connect-design.md)

## Context

Serial hotplug currently refreshes frontend lists, while main FC, Motor Test,
auxiliary source and RTK base connection paths maintain separate ownership
checks. The main launch timer connects a saved endpoint; it does not discover
serial devices. RTK stores Survey-In preferences in the webview and starts
only on explicit Connect. Core MAVLink retries reopen the original path.

Automatic connection needs one authoritative inventory and a reservation that
outlives asynchronous opening, protocol probing and worker teardown. VID/PID
classification, an OS port name and a transport-open result supply different
evidence from a supported receiver response or autopilot heartbeat.

## Decision

1. Keep enumeration, stable matching, leases, candidate policy, retry budgets and
   shared state types in core. A process-wide desktop adapter coordinates intent
   transitions with connection/lifecycle and persistence, publishing revisioned
   snapshots to every window.
2. Match saved devices by unique VID/PID/USB serial. Track each observed
   attachment separately. Missing/duplicate serials and multiple candidates
   require selection; generic bridges need explicit saved selection and fresh
   protocol evidence. A bridge identity does not prove receiver uniqueness.
3. Reserve before opening on all manual/automatic serial paths, including
   reopen attempts. Move the lease into the I/O worker; release only after
   actual closure. Main-link borrowing uses the existing transport/lease.
4. Probe automatic FC candidates passively and transfer the verified live
   transport to normal services. Use a shared vehicle/autopilot predicate,
   including configured-ID paths. Initially attach PX4 multirotors; bind
   discovered IDs, reject ambiguity and local GCS ID collisions.
5. Run one retry supervisor per automatic role. Base reconnect creates a new
   base session; managed FC transport failure creates a newly probed connection.
   Scope FC state by `(handle_id, generation)` and preserve the independent
   base during vehicle-only disconnect/recovery.
6. Store versioned native serial policies in
   `app_config_dir/serial-connections.json`, separate from whole-webview
   `AppConfig` saves. Default both roles to enabled for eligible, unambiguous
   devices; retain manual Connect/Disconnect and port/baud selection, with
   manual selection taking priority. Persist user pause until
   explicit Connect/Resume; serialize updates with revision checks.
7. Enabling Survey-In automatic connection selects reapplication of the saved
   identified, RAM-only configuration after eligible reconnects. Read-only
   identification polls are still serial writes; passive mode sends none.

## Consequences and failure boundaries

The native coordinator selects the startup owner. The legacy frontend timer
can launch a nonserial endpoint when FC discovery is disabled; a legacy serial
path is an explicit selection/migration hint. Enabled FC policy and unreadable
serial settings suppress that timer. UI reload cannot start a second probe or
clear pause. Disk writes and enumeration stay outside device cancellation and
motor-release paths. Timed-out I/O retains ownership;
external busy/permission failures are reported without automatic escalation.

Polling cannot prove an indistinguishable replacement entirely between scans.
Receiver model evidence does not establish base-reference continuity. The
write/failure matrix and AC-01–AC-19 in the interface draft define software
acceptance; FIELD-01 and LINK-01–06 require installed-device evidence.

LINK-05 first measures actual serial handle/generation behavior on FC reboot.
A vehicle boot epoch is a separate proposal only if those traces demonstrate
that the existing transport boundary does not cover the installed topology.

## Confirmation and implementation

The operator confirmed the interfaces and subsequently requested automatic
linking by default with manual connections retained. Hardware testing is deferred
to the operator after software delivery. AUTO-02 and AUTO-03 proceed through the
design's small implementation slices and automated fixtures.

AUTO-02/03 software includes retained inventory, worker leases, native settings,
manual/automatic connection integration and passive FC handoff. The operator
also confirmed a HUD-style RTK/GPS instrument panel at the drawer top. Recorded
verification is in the [backlog](../plans/rtk-base-and-air-ground-todos.md);
physical execution uses the [hardware checklist](../verification/serial-rtk-hardware-checklist.md).
