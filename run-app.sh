#!/usr/bin/env bash
# Build the frontend into frontend/dist and launch the MagGCS desktop app from
# those local assets (no dev server). This mirrors the field-sop launcher: the
# webview loads the built bundle via `frontendDist`, never a Vite dev URL.
#
#   ./run-app.sh            # build if needed, then launch
#   REBUILD_UI=1 ./run-app.sh   # force a frontend rebuild first
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"

# Prefer a cargo on PATH; fall back to the standard rustup location.
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

# Build the renderer if it has never been built (or on request).
if [ ! -d frontend/node_modules ]; then
  echo "==> installing frontend dependencies"
  (cd frontend && npm install)
fi
if [ ! -f frontend/dist/index.html ] || [ "${REBUILD_UI:-0}" = "1" ]; then
  echo "==> building the renderer"
  (cd frontend && npm run build)
fi

echo "==> building the desktop app"
cargo build -p maggcs-app

echo "==> launching MagGCS"
exec ./target/debug/maggcs-app
