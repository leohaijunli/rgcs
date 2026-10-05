#!/usr/bin/env bash
#
# Run PX4 SITL in Docker (jmavsim headless), telemetry on UDP 14550.
# Builds on first run; subsequent runs restart the built container fast.
#
# Usage:
#   scripts/sitl/run_sitl_docker.sh [PX4_DIR]
#
# Env:
#   PX4_IMAGE   docker image (default px4io/px4-dev-simulation-jammy:latest)
#   PX4_SIM     simulator target (default jmavsim)
#   PX4_CONTAINER container name (default px4-sitl)
#
# The GCS (MagGCS on Windows, or scripts/sitl/run_heartbeat_test.sh) connects
# to udpin:0.0.0.0:14550. WSL2: use mirrored networking or the WSL IP
# (see docs/sitl-wsl-windows.md).

set -euo pipefail

PX4_DIR="${1:-${PX4_DIR:-$HOME/PX4-Autopilot}}"
PX4_IMAGE="${PX4_IMAGE:-px4io/px4-dev-simulation-jammy:latest}"
PX4_SIM="${PX4_SIM:-jmavsim}"
PX4_CONTAINER="${PX4_CONTAINER:-px4-sitl}"

if [ ! -d "$PX4_DIR" ]; then
  echo "PX4 source not found at $PX4_DIR (clone PX4-Autopilot first)" >&2
  exit 2
fi

if ! docker version >/dev/null 2>&1; then
  echo "docker unavailable. Start the daemon and make sure this user is in the docker group." >&2
  exit 2
fi

# Ensure the container runs with the host uid:gid so build outputs stay ours.
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

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