#!/usr/bin/env bash
#
# PX4 SITL heartbeat integration test (Phase 0, task 5).
#
# Acceptance: no heartbeat losses during the run (default 30 minutes),
# verified via the `sitl_monitor` example. Also usable as a shorter smoke
# test in CI (e.g. SITL_DURATION=60).
#
# Usage:
#   scripts/sitl/run_heartbeat_test.sh [duration_s] [heartbeat_timeout_s] [target_sys_id]
#
# Environment:
#   PX4_DIR          path to a PX4-Autopilot checkout (start SITL automatically).
#                    If unset, the test expects an external SITL/QGC already
#                    broadcasting on the MAVLink endpoint.
#   SITL_ADDRESS     MAVLink endpoint for the monitor (default udpin:0.0.0.0:14550)
#   SITL_SIMULATOR   jmavsim (default) | none (external SITL only)
#   SITL_DURATION    seconds (default 1800)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

DURATION="${SITL_DURATION:-${1:-1800}}"
HB_TIMEOUT="${2:-3}"
TARGET_SYS="${3:-1}"
ADDRESS="${SITL_ADDRESS:-udpin:0.0.0.0:14550}"
SIMULATOR="${SITL_SIMULATOR:-jmavsim}"

echo "==> building sitl_monitor"
cargo build --release --example sitl_monitor

SITL_PID=""
cleanup() {
  if [ -n "$SITL_PID" ]; then
    echo "==> stopping SITL (pid $SITL_PID)"
    kill "$SITL_PID" 2>/dev/null || true
    wait "$SITL_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

if [ -n "${PX4_DIR:-}" ]; then
  echo "==> starting PX4 SITL from $PX4_DIR (simulator=$SIMULATOR)"
  pushd "$PX4_DIR" >/dev/null
  case "$SIMULATOR" in
    jmavsim)
      HEADLESS=1 make px4_sitl jmavsim > /tmp/maggcs-px4-sitl.log 2>&1 &
      ;;
    none)
      ;;
    *)
      echo "unsupported SITL_SIMULATOR=$SIMULATOR" >&2
      exit 2
      ;;
  esac
  SITL_PID=$!
  popd >/dev/null

  echo "==> waiting for SITL MAVLink traffic on :14550 (up to 90s)"
  for i in $(seq 1 90); do
    if timeout 1 bash -c "echo > /dev/udp/127.0.0.1/14550" 2>/dev/null; then
      sleep 3
      break
    fi
    sleep 1
  done
fi

echo "==> running monitor: duration=${DURATION}s timeout=${HB_TIMEOUT}s sys=${TARGET_SYS} endpoint=${ADDRESS}"
set +e
./target/release/examples/sitl_monitor "$ADDRESS" "$DURATION" "$HB_TIMEOUT" "$TARGET_SYS"
RC=$?
set -e

case "$RC" in
  0) echo "==> PASS: 0 heartbeat losses over ${DURATION}s" ;;
  1) echo "==> FAIL: heartbeat losses detected" >&2 ;;
  2) echo "==> FAIL: no heartbeats received" >&2 ;;
  *) echo "==> FAIL: monitor crashed with exit $RC" >&2 ;;
esac
exit "$RC"