#!/usr/bin/env bash
#
# Run PX4 SITL in Docker (headless), MAVLink GCS telemetry out on UDP 14550.
# Builds on first run; subsequent runs restart the built container fast.
#
# Usage:
#   scripts/sitl/run_sitl_docker.sh [PX4_DIR]
#
# Env:
#   PX4_DIR     PX4-Autopilot checkout. Defaults to a v1.17 checkout when one
#               is present ($HOME/PX4-Autopilot-v1.17), else $HOME/PX4-Autopilot.
#   PX4_IMAGE   docker image (default px4io/px4-dev:v1.17.0, matching the
#               PX4 v1.17.0 source; built with
#               --no-sim-tools, so it has no jmavsim/Gazebo)
#   PX4_SIM     simulator target (default none = no physics sim, still runs the
#               MAVLink GCS link; use jmavsim/gz only with a *-simulation image)
#   PX4_CONTAINER container name (default px4-sitl)
#   PX4_HOME_LAT/PX4_HOME_LON/PX4_HOME_ALT
#               SITL world origin (default: Sidney BC, Canada, ~5 m MSL)
#
# The GCS (MagGCS on Windows, or scripts/sitl/run_heartbeat_test.sh) connects
# to udpin:0.0.0.0:14550. WSL2: use mirrored networking or the WSL IP
# (see docs/sitl-wsl-windows.md).

set -euo pipefail

# Resolve the PX4 source: explicit arg/env wins, otherwise prefer a v1.17
# checkout so the default run targets v1.17.
PX4_DIR="${1:-${PX4_DIR:-}}"
if [ -z "$PX4_DIR" ]; then
  if [ -d "$HOME/PX4-Autopilot-v1.17" ]; then
    PX4_DIR="$HOME/PX4-Autopilot-v1.17"
  else
    PX4_DIR="$HOME/PX4-Autopilot"
  fi
fi
PX4_IMAGE="${PX4_IMAGE:-px4io/px4-dev:v1.17.0}"
PX4_SIM="${PX4_SIM:-none}"
PX4_CONTAINER="${PX4_CONTAINER:-px4-sitl}"
# SITL world origin — default Sidney BC, Canada (survey site), ~5 m MSL.
export PX4_HOME_LAT="${PX4_HOME_LAT:-48.6493}"
export PX4_HOME_LON="${PX4_HOME_LON:--123.3982}"
export PX4_HOME_ALT="${PX4_HOME_ALT:-5}"

if [ ! -d "$PX4_DIR" ]; then
  echo "PX4 source not found at $PX4_DIR (clone PX4-Autopilot first)" >&2
  exit 2
fi

if ! docker version >/dev/null 2>&1; then
  # The current shell may predate `usermod -aG docker`, so it does not carry the
  # docker group yet. Re-run once under `newgrp docker` instead of demanding a
  # full re-login (guarded by MAGGCS_DOCKER_REEXEC so it cannot loop).
  if [ "${MAGGCS_DOCKER_REEXEC:-0}" != "1" ] \
     && command -v newgrp >/dev/null 2>&1 \
     && getent group docker 2>/dev/null \
        | awk -F: -v u="$(id -un)" '{ n = split($4, a, ","); for (i = 1; i <= n; i++) if (a[i] == u) found = 1 } END { exit !found }'; then
    echo "==> docker not in this shell's group set; re-running under 'newgrp docker'"
    exec newgrp docker <<MAGGCS_REEXEC
cd "$(pwd)"
MAGGCS_DOCKER_REEXEC=1 exec "$0" "$@"
MAGGCS_REEXEC
  fi
  echo "docker unavailable. Start the daemon and make sure this user is in the docker group." >&2
  echo "If you were just added to the group, log out and back in, or run: newgrp docker" >&2
  exit 2
fi

# Ensure the container runs with the host uid:gid so build outputs stay ours.
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

echo "==> PX4 source: $PX4_DIR ($(git -C "$PX4_DIR" describe --tags 2>/dev/null || echo unknown))"
echo "==> image: $PX4_IMAGE"
docker image inspect "$PX4_IMAGE" >/dev/null 2>&1 || docker pull "$PX4_IMAGE"

if docker ps -a --format '{{.Names}}' | grep -qx "$PX4_CONTAINER"; then
  echo "==> container $PX4_CONTAINER exists; restarting (build artifacts kept)"
  docker start "$PX4_CONTAINER"
  echo "==> SITL log: $PX4_DIR/build.log (tail -f it)"
  exit 0
fi

# The container command keeps the SITL running after the build (make px4_sitl
# runs it). Logs go to <PX4_DIR>/build.log, visible from the host.
docker run -d --name "$PX4_CONTAINER" \
  --network host \
  --user "$HOST_UID:$HOST_GID" \
  -e HOME=/tmp \
  -e CCACHE_DIR=/tmp/ccache \
  -v "$PX4_DIR":/px4 \
  "$PX4_IMAGE" \
  bash -c "git config --global --add safe.directory '*' 2>/dev/null || true; cd /px4 && HEADLESS=1 make px4_sitl $PX4_SIM > /px4/build.log 2>&1"

echo "==> container $PX4_CONTAINER started (building on first run)"
echo "==> build log: $PX4_DIR/build.log — tail -f it until you see the 'pxh>' prompt"
echo "==> connect MagGCS to udpin:0.0.0.0:14550"
