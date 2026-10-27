# AF-00 clean idle desktop profile

Copy `config.json` and `serial-connections.json` into a fresh temporary
`XDG_CONFIG_HOME/io.maggcs.desktop/`. Use separate temporary XDG data/cache roots.
Do not copy this fixture over an operator's profile.

This version-1 profile disables both native serial automatic roles, pauses them,
disables the legacy frontend timer and mutes alert audio. It starts without a
project, DEM, calibration sidecar, trace preferences or recorded session.
Device inventory still runs as the native shared discovery service.

Acceptance: startup status has no connection/attempt/base session, optional tool,
motor control, capture/run, recorder or served terrain. Closing disconnected tools
returns their status to this baseline. See the
[measurement report](../../../docs/verification/af-00-baseline-20261025.md).
