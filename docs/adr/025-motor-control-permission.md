# ADR-025: Default-OFF Motor-Control Permission

- Status: Accepted (2026-10-25); MR-22a software-verified.
- Date: 2026-10-25
- Relates to: ADR-023, motor-test remediation MR-16–MR-22.

## Context

Motor Test writes normalized actuator demands and, on DShot outputs, ESC beacon
commands through the target. Telemetry, SD capture and parameter inspection are
read-only and should remain usable while the bench is merely connected. A single
"unlock" control already guarded START, but it was a local UI state and did not
gate manual set-values, preset/forward starts, tone playback or motor-related
parameter writes. Reconnecting or retargeting could also leave a stale intent to
send.

## Decision

1. Permission is explicit, default OFF, and never restored from preferences.
   It binds the current target reference and transport generation; changes to
   either revoke it. Opening a window also forces OFF.
2. Enabling is passive. It may bind permission but must not start motors, play
   tones, change parameters or write stream rates. An enable superseded by a
   newer OFF request fails instead of restoring permission.
3. While OFF, START, manual set-values, preset/forward starts, tone playback and
   target parameter writes are refused before acquiring the operation lock.
   Telemetry, capture, log retrieval and ordinary parameter reads remain
   available.
4. OFF revokes permission synchronously, cancels pending start work and releases
   only outputs owned by this Motor Test session. Release paths are never gated.
   If release is incomplete, the error is retained and STOP can be retried.
5. Idle STOP/E-STOP must not release outputs owned by another controller. The
   core emergency release therefore acts only when this session owns outputs.
6. Primary controls expose native `title` descriptions. Tooltips explain the
   action and its safety boundary without changing click/hold behavior.

## Consequences and verification

The operator can attach, inspect telemetry, run SD capture and configure tones
without granting motor authority. Live motor commands require a per-connection
deliberate action. The remaining cost is one extra step before START and a
prominent monitor-only state in the UI.

Recorded software acceptance: 164 app tests (7 ignored) and 841 core tests
(1 ignored), clippy `--all-targets -- -D warnings`, formatting, frontend
typecheck/build, catalog regeneration check and the real-React IPC regression.
Coverage includes default OFF with zero motor/shell writes, passive enable,
running-OFF release, pending-start cancellation, stale enable races,
release-failure retention, reconnect revocation and no ON persistence across
reload. Hardware release behavior remains part of the props-off acceptance
session.

MR-22b main-window serial-link reuse is confirmed and implemented under
[ADR-026](026-main-serial-target-reuse.md).
