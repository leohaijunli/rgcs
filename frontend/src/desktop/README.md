# Desktop Runtime and flight-control bridge

`flightRuntime.ts` attaches the `flight-controls` module to one exact available
primary Runtime target. Session lookup/attachment is serialized, and each command
gets a fresh request ID. Native IPC checks the actual window/session; the frontend
comparison helpers retain all vehicle/session/link/generation/MAVLink identities.

`stores/command.ts` captures that context before asynchronous checklist or terrain
preparation. It validates native authority before dispatch and before applying a
terminal reply. Target-scope reset invalidates old callbacks immediately, detaches
the captured native attachment and clears preflight/map authority.

Each request has a separate local token. Only the newest current-target request
settles the visible pending/status/preflight state. A displaced same-target result
can still be logged and reconcile its own map marker. A reply from an obsolete
native target/session is discarded, even if the endpoint and UI heartbeat have
not changed yet. Discovering a replacement during a click requires a fresh
operator command rather than redirecting that click.

Flight intents go through `flight_command_execute` and Rust-derived generated
types. `bridge.ts` subscribes to telemetry/link scope changes; the typed invocation
reply supplies each flight verdict. Arm/Disarm labels retain the requested action
when heartbeat read-back changes the armed state. Preflight confirmation returns
`true` only for the current request's accepted verdict; denial or uncertainty keeps
the panel and its reason available. Long status reasons are bounded on the TopBar
and available in full through the tooltip.

## Mission Runtime

`missionRuntime.ts` independently attaches the `mission` module to one exact
primary target. `stores/mission.ts` captures the clicked wire items and editor
identity before attachment, uses a fresh request ID for every upload/read-back/
clear/set-current, and validates native authority before applying its reply.

An upload ACK records only the sent snapshot. A second owned download must match
that snapshot before the UI reports FC sync. Edits made during either exchange
remain in the editor and cannot be overwritten or falsely verified. Explicit
downloads similarly refuse to replace a plan edited while waiting. Reconnect
revokes the old baseline/session and reads the retained local plan back against a
new session; it never replays an upload, clear or set-current intent.

Mission progress/status events must match the current request/operation/owner/
target. Passive current observations are target/timestamp scoped, coalesced to one
native validation at a time, and cannot complete the wrong set-current request.
Cancel sends the exact operation/session; the UI distinguishes retiring local
retries from unknown FC state and preserves the plan for reconciliation.

## Verification

From the repository root:

```bash
# Browser prerequisites on a fresh development checkout (Node 20+).
npm --prefix frontend install --no-save --package-lock=false playwright@1.63.0
npm --prefix frontend exec -- playwright install chromium
npm --prefix frontend run check:flight-acceptance
npm --prefix frontend run check:flight-delivery
npm --prefix frontend run check:mission-sync
npm --prefix frontend run check:mission-acceptance
npm --prefix frontend run check:mission-delivery
```

The browser runner owns and stops its loopback Vite process. Screenshots and exact
manual steps are in the [flight acceptance record](../../../docs/verification/af-02b-flight-commands-20261026.md).
`check:generated-types` compares all core/native exports with the complete frontend
mirror, including `emergency_command`, and runs in CI.
Mission screenshots/manual steps are in the
[owned Mission acceptance](../../../docs/verification/af-02b-mission-20261026.md).

## Manual serial selection

Settings → Connection shows an empty selection and disables Connect when the
saved serial port is absent from the live inventory. Choosing an available port
updates the same draft used for both the connection request and config save.
An attachment replacement still requires explicit reselection.

`npm run check:serial-auto -- http://localhost:5199/` (with Vite running at that
address) covers a saved `/dev/ttyACM0` replaced by a radio on `/dev/ttyUSB0`,
including the empty selection, submitted attachment, persisted endpoint and
reopened dialog. Browser-fixture screenshots: [missing saved port](../../screenshots/serial-auto/fc-missing-saved-port-1920x1080.png)
and [radio selected](../../screenshots/serial-auto/fc-radio-selected-1920x1080.png).

Manual verification:

1. Save a direct-USB serial endpoint, then replace that device with a radio on a
   different port. Open Settings → Connection.
2. Confirm **Select serial device…** is shown and Connect is disabled.
3. Select the radio and its baud, then click Connect. Reopen Settings and restart
   the app; confirm the radio endpoint is retained in both cases.
