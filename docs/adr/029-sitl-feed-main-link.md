# ADR-029: Motor Test SITL feed borrowing the primary link

- Status: Accepted, 2026-10-26.
- Related: ADR-026, ADR-024, motor-test remediation plan.

## Context

The operator connected the main window to a PX4 SITL and then configured the
Motor Test SITL source as `udpout:<host>:18570`, PX4's MAVLink listening port.
The main window worked, Motor Test stayed at "no data yet". PX4 v1.17 pins one
UDP `client_source_address` per MAVLink instance: packets from a second address
are parsed (commands work) but telemetry is only sent to the pinned partner.
The main link already owns that partner, so the second socket never receives
`SERVO_OUTPUT_RAW`. Opening `udpin:*:18570` either collides with PX4's own bind
or, on another host, never receives telemetry.

## Decision

Offer **Use main link** when the primary link is connected, its FC heartbeat is
live, and its target identity is known. IPC carries
`MainSitlSource { endpoint, system_id, component_id, transport_generation }`
plus the operator-selected output bank; the offer is echoed back and an
obsolete generation is refused. The dedicated SITL fields remain unchanged for
a separate PX4 instance or a routed setup.

The feed clones the primary `ConnectionHandle`, subscribes a tap to its frames,
and stores `SitlSourceOrigin::MainLink`. `AuxiliaryLinkRuntime::borrowed` owns
only the tap task and never shuts the primary transport down. The tap does not
write `MAV_CMD_SET_MESSAGE_INTERVAL`: stream rates affect every subscriber, and
SITL already streams `SERVO_OUTPUT_RAW` at its configured rate.

A borrowed feed is current only while the primary handle still has the
generation it borrowed. Main reconnect, replacement or close expires it:
status reports the SITL source offline, input values are cleared, and Forward
start returns `SITL source link changed; reconnect it` until an explicit
re-attach. The dedicated origin stays current for the lifetime of its runtime,
which owns and retries its own transport.

## Failure analysis

| Failure | Handling |
| --- | --- |
| Second UDP partner receives no telemetry | Borrow the primary handle instead of opening a second stream |
| Tool detach/close stops the main window | Borrowed runtime has no transport shutdown ownership; the primary stays connected |
| Stale UI attaches a replaced vehicle | Exact endpoint/system/component/generation match under `AppState::ops` |
| Main-link reconnect keeps the old tap alive on the shared event bus | Generation check hides the feed and refuses Forward start; the tap only feeds an unreachable slot |
| Stream-rate requests disturb the main window | The feed never sends message-interval commands |
| Selecting an output bank is lost | The bank is passed separately to `actuator_use_main_sitl` and stored in `SitlSourceConfig` |
| Dedicated SITL regressions | The dedicated connect/disconnect path is unchanged |

## Verification

`cargo test -p maggcs-app --lib actuator_service::main_sitl_feed_tests` covers
the offer, a loopback primary plus fake PX4, `SERVO_OUTPUT_RAW` reaching the
forward slot, borrowed detach preserving the primary transport, stale
generation expiry after reconnect, cleared input values, and the Forward-start
refusal. `cargo clippy -p maggcs-app --all-targets` and `npm run build` pass.

The React IPC check adds the offer, exact echoed generation, "Using main link",
no `actuator_connect_sitl` call and disconnect. Browser screenshots require
Playwright (`PLAYWRIGHT_MODULE=/path/to/playwright node frontend/scripts/check-motor-controls.mjs ...`);
it is not installed in this environment, so the packaged-window visual check
remains a bench step.

Live SITL validation remains necessary: confirm that the main window and Motor
Test receive the same telemetry while the feed is borrowed, and that a main
reconnect requires re-attaching before Forward can start.
