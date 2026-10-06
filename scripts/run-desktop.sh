#!/usr/bin/env bash
# Build the frontend + desktop app and launch MagGCS on the current display.
# Usage: scripts/run-desktop.sh   (from the repository root or from scripts/)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Prefer a cargo on PATH; fall back to the standard rustup location.
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

echo "==> building frontend"
npm --prefix frontend run build

echo "==> building desktop app (release)"
cargo build --release -p maggcs-app

echo "==> launching MagGCS on ${DISPLAY:-:0}"
exec ./target/release/maggcs-app
