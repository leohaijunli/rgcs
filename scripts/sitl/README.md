# PX4 SITL integration test scripts.

Added in Phase 0 (task 5), acceptance per docs/ARCHITECTURE.md.

- `run_sitl_docker.sh` — headless PX4 SITL in Docker. `PX4_GCS_BROADCAST=1`
  turns it into a LAN SITL server reachable by an unchanged MagGCS
  (`udpin:0.0.0.0:14550`) — see docs/sitl-server.md.
- `px4-rc.mavlink` — v1.17 shadow of PX4's GCS MAVLink startup (+`-p` for LAN
  broadcast), mounted by `run_sitl_docker.sh` when `PX4_GCS_BROADCAST=1`.
- `sitl-home.sh` — resolves the SITL world origin.
- `run_heartbeat_test.sh` — heartbeat smoke test over the GCS link.
