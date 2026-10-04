#!/usr/bin/env bash
# Reject hardcoded colors in TS/TSX sources (ADR-010: design tokens only).
# Design tokens live in design-system/index.css — excluded here.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Hex (3/4/6/8), rgb()/rgba(), hsl()/hsla() literals in src, excluding generated bindings.
matches=$(
  grep -rnE "#[0-9a-fA-F]{3,8}\b|rgba?\(|hsla?\(" src \
    --include='*.ts' --include='*.tsx' \
    -l --exclude-dir=generated-types 2>/dev/null || true
)

if [ -n "$matches" ]; then
  echo "Hardcoded colors found (use design tokens instead):" >&2
  echo "$matches" >&2
  exit 1
fi

echo "check:colors OK — no hardcoded colors in components"