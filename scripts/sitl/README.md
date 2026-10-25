# PX4 SITL integration test scripts.

Added in Phase 0 (task 5), acceptance per docs/ARCHITECTURE.md.

- `run_sitl_docker.sh` — headless PX4 SITL in Docker. `PX4_GCS_BROADCAST=1`
  turns it into a LAN SITL server reachable by an unchanged MagGCS
  (`udpin:0.0.0.0:14550`) — see docs/sitl-server.md.
- `px4-rc.mavlink` — v1.17 shadow of PX4's GCS MAVLink startup (+`-p` for LAN
  broadcast), mounted by `run_sitl_docker.sh` when `PX4_GCS_BROADCAST=1`.
- `sitl-home.sh` — resolves the SITL world origin.
- `run_heartbeat_test.sh` — heartbeat smoke test over the GCS link.
- `check-standard-streams.py` — MR-20 production stream-service E2E on an
  isolated native PX4 SIH instance, using a prebuilt `px4_sitl_default`.
  `standard-streams/px4-rc.mavlink` supplies only its custom loopback link.
  It writes firmware identity, phase snapshots and saved rates to `--artifacts`
  and cleans up the instance. See the
  [dated firmware report](../../docs/verification/mr20-standard-streams-20261025.md).

```bash
python3 scripts/sitl/check-standard-streams.py \
  --px4-build "$HOME/PX4-Autopilot-v1.17/build/px4_sitl_default" \
  --artifacts /tmp/opencode/mr20-streams
```

Use an unused nonzero `--instance` (default 42). The runner creates a fresh
`run-*` artifact subdirectory on each invocation so saved profiles from a
previous measurement cannot contaminate another run. Normal Cargo tests keep
the firmware check ignored; the runner sets the isolated endpoint and explicitly
selects it. The required 147/26/105 streams must meet ±20% receive-rate
tolerance, change within 3 s, stop on disable, become stale when firmware stops
them, and restore saved rates after reconnect. Additional standard streams and
a 1000 Hz probe record source availability and rate ceilings without inventing
support from catalog presence.
