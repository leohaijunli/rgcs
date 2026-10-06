#!/usr/bin/env bash
#
# Run PX4 SITL in Docker (headless), MAVLink GCS telemetry out on UDP 14550.
# Builds on first run; subsequent runs restart the built container fast.
#
# Usage:
#   scripts/sitl/run_sitl_docker.sh [PX4_DIR]
#   PX4_INTERACTIVE=1 scripts/sitl/run_sitl_docker.sh [PX4_DIR]
#
# Env:
#   PX4_DIR     PX4-Autopilot checkout. Defaults to a v1.17 checkout when one
#               is present ($HOME/PX4-Autopilot-v1.17), else $HOME/PX4-Autopilot.
#   PX4_IMAGE   docker image (default px4io/px4-dev:v1.17.0, matching the
#               PX4 v1.17.0 source; built with
#               --no-sim-tools, so it has no jmavsim/Gazebo)
#   PX4_SIM     simulator target (default none = no physics sim, still runs the
#               MAVLink GCS link; use jmavsim/gz only with a *-simulation image)
#   PX4_SIM_MODEL
#               PX4 airframe model. Defaults to sihsim_quadx for PX4_SIM=none:
#               a bare 'none' run leaves SYS_AUTOSTART=0, so no airframe
#               defaults are applied, the baro/mag sim modules never start and
#               the calibration IDs stay 0 -> 'Preflight Fail: Found 0 compass
#               / barometer 0 missing / Accel 0 uncalibrated'. sihsim_quadx
#               (SYS_AUTOSTART=10040) gives simulated IMU/baro/mag/GPS plus
#               calibration, so the SITL reaches 'Ready for takeoff'.
#   PX4_CONTAINER container name (default px4-sitl)
#   PX4_INTERACTIVE
#               1 = run in the foreground and drop into the PX4 'pxh>' shell
#                   (build prints to the terminal and is saved to build.log)
#               0 = default: detached/headless SITL with the pxh shell off
#   PX4_HOME_LAT/PX4_HOME_LON/PX4_HOME_ALT
#               SITL world origin (default: Sidney BC, Canada, ~5 m MSL)
#
# The GCS (MagGCS on Windows, or scripts/sitl/run_heartbeat_test.sh) connects
# to udpin:0.0.0.0:14550. WSL2: use mirrored networking or the WSL IP
# (see docs/sitl-wsl-windows.md).
#
# Why the pxh shell needs two modes:
#   * Detached containers have stdin on /dev/null. Without '-d' (pxh_off=true)
#     the PX4 shell reads EOF and reprints 'pxh>' in a tight loop, growing
#     build.log by gigabytes. 'make px4_sitl none' enables that shell, so this
#     script builds the px4_sitl_default target and runs bin/px4 itself.
#   * To actually type commands you need a TTY, hence PX4_INTERACTIVE=1, which
#     runs 'docker run -it ... bin/px4' without '-d'. Ctrl-D is NOT an exit in
#     pxh; quit with Ctrl-C or by typing the 'shutdown' command.

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
PX4_INTERACTIVE="${PX4_INTERACTIVE:-0}"
export PX4_INTERACTIVE
# The 'none' simulator relies on SIH for sensors; default to an SIH airframe so
# the vehicle has a barometer/magnetometer and passes preflight (see header).
PX4_SIM_MODEL="${PX4_SIM_MODEL:-}"
if [ "$PX4_SIM" = "none" ] && [ -z "$PX4_SIM_MODEL" ]; then
  PX4_SIM_MODEL="sihsim_quadx"
fi
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
    if [ "$PX4_INTERACTIVE" = "1" ]; then
      # The interactive run needs a real terminal, and the heredoc form below
      # would replace stdin with the here-document, so use `newgrp -c` instead.
      if newgrp docker -c true >/dev/null 2>&1; then
        MAGGCS_DOCKER_REEXEC=1 exec newgrp docker -c "$(printf '%q ' "$0" "$@")"
      fi
      echo "interactive mode needs the docker group in this shell." >&2
      echo "Run 'newgrp docker' first, then re-run this script." >&2
      exit 2
    fi
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
PX4_LOG="$PX4_DIR/build.log"

echo "==> PX4 source: $PX4_DIR ($(git -C "$PX4_DIR" describe --tags 2>/dev/null || echo unknown))"
echo "==> image: $PX4_IMAGE"
echo "==> airframe: ${PX4_SIM_MODEL:-<sim default>} (PX4_SIM=$PX4_SIM)"
docker image inspect "$PX4_IMAGE" >/dev/null 2>&1 || docker pull "$PX4_IMAGE"

# Command executed inside the container. For the "none" simulator we build the
# px4_sitl_default target and launch bin/px4 ourselves so we can choose between
# the daemon form (-d, no pxh shell) and the interactive form (TTY + pxh).
# Other simulators keep the stock 'make px4_sitl <sim>' entry point, which
# launches the simulator after building.
if [ "$PX4_SIM" = "none" ]; then
  if [ "$PX4_INTERACTIVE" = "1" ]; then
    RUN_CMD='set -o pipefail; git config --global --add safe.directory "*" 2>/dev/null || true; cd /px4 && make px4_sitl 2>&1 | tee /px4/build.log && cd /px4/build/px4_sitl_default/rootfs && exec ../bin/px4'
  else
    RUN_CMD='git config --global --add safe.directory "*" 2>/dev/null || true; cd /px4 && make px4_sitl > /px4/build.log 2>&1 || { echo "==> build failed; last 40 log lines:"; tail -n 40 /px4/build.log; exit 1; }; cd /px4/build/px4_sitl_default/rootfs && exec ../bin/px4 -d >> /px4/build.log 2>&1'
  fi
else
  RUN_CMD="git config --global --add safe.directory '*' 2>/dev/null || true; cd /px4 && HEADLESS=1 make px4_sitl $PX4_SIM > /px4/build.log 2>&1"
fi

# Stamp the container with a hash of the command so a stale container (older
# image or older command) is recreated instead of silently reused.
CMD_SHA="$(printf '%s' "$RUN_CMD|model=$PX4_SIM_MODEL" | sha256sum | cut -c1-16)"
LABEL_KEY="maggcs.sitl.cmd"
DOCKER_ARGS=(
  --network host
  --user "$HOST_UID:$HOST_GID"
  -e HOME=/tmp
  -e CCACHE_DIR=/tmp/ccache
  -v "$PX4_DIR":/px4
  --label "$LABEL_KEY=$CMD_SHA"
)
if [ -n "$PX4_SIM_MODEL" ]; then
  DOCKER_ARGS+=( -e "PX4_SIM_MODEL=$PX4_SIM_MODEL" )
fi

container_exists() {
  docker ps -a --format '{{.Names}}' | grep -qx "$PX4_CONTAINER"
}

if [ "$PX4_INTERACTIVE" = "1" ]; then
  # The interactive shell needs this terminal, so an existing container cannot
  # be reused in the background.
  if container_exists; then
    echo "==> removing container $PX4_CONTAINER (interactive run needs the foreground)"
    docker rm -f "$PX4_CONTAINER" >/dev/null
  fi
  echo "==> interactive SITL: building if needed, then the 'pxh>' prompt"
  echo "==> quit pxh with Ctrl-C, or type 'shutdown' (Ctrl-D is not an exit)"
  echo "==> build log: $PX4_LOG"
  exec docker run -it --rm --name "$PX4_CONTAINER" "${DOCKER_ARGS[@]}" "$PX4_IMAGE" bash -c "$RUN_CMD"
fi

if container_exists; then
  have_sha="$(docker inspect -f "{{index .Config.Labels \"$LABEL_KEY\"}}" "$PX4_CONTAINER" 2>/dev/null || true)"
  have_img="$(docker inspect -f '{{.Config.Image}}' "$PX4_CONTAINER" 2>/dev/null || true)"
  if [ "$have_sha" = "$CMD_SHA" ] && [ "$have_img" = "$PX4_IMAGE" ]; then
    echo "==> container $PX4_CONTAINER exists; restarting (build artifacts kept)"
    docker start "$PX4_CONTAINER" >/dev/null
    echo "==> SITL log: $PX4_LOG (tail -f it)"
    echo "==> headless: the pxh shell is off; use PX4_INTERACTIVE=1 for an interactive pxh"
    exit 0
  fi
  echo "==> container $PX4_CONTAINER is stale (image or command changed); recreating"
  docker rm -f "$PX4_CONTAINER" >/dev/null
fi

docker run -d --name "$PX4_CONTAINER" "${DOCKER_ARGS[@]}" "$PX4_IMAGE" bash -c "$RUN_CMD" >/dev/null

echo "==> container $PX4_CONTAINER started (building on first run)"
echo "==> log: $PX4_LOG — tail -f it"
echo "==> headless SITL: pxh shell is off, so the log stays small"
echo "==> interactive pxh shell: PX4_INTERACTIVE=1 $0"
echo "==> connect MagGCS to udpin:0.0.0.0:14550"
