#!/usr/bin/env bash
# Build the frontend + desktop app and launch MagGCS on the current display.
# Usage: scripts/sitl/../desktop/run-desktop.sh   (or: scripts/run-desktop.sh)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

echo "==> building frontend"
npm --prefix frontend run build

echo "==> building desktop app (release)"
cargo build --release -p maggcs-app

echo "==> launching MagGCS on ${DISPLAY:-:0}"
exec ./target/release/maggcs-app