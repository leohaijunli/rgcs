#!/usr/bin/env bash
#
# Desktop smoke test (ADR-012): launch the real MagGCS binary and check that the
# window actually paints.
#
# The app is started on a hidden Hyprland "headless" output (a window rule pins
# it there), so the visible workspace is not disturbed. That output is captured
# with grim and measured: a window whose page failed to execute (CSP, stale
# frontend embed) shows only the flat background colour and fails the
# bright-pixel threshold.
#
# Usage: scripts/desktop-smoke.sh [path/to/maggcs-app]
# Env:
#   MAGGCS_SMOKE_WAIT        seconds to wait before capturing (default 15)
#   MAGGCS_SMOKE_MIN_BRIGHT  minimum bright-pixel ratio in percent (default 15)
#
# Requires: Hyprland (hyprctl), grim, python3, node + frontend/node_modules.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${1:-$root/target/release/maggcs-app}"
wait_s="${MAGGCS_SMOKE_WAIT:-15}"
min_bright="${MAGGCS_SMOKE_MIN_BRIGHT:-15}"

for tool in hyprctl grim python3 node; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "skip: $tool not found in PATH" >&2
    exit 2
  fi
done
if [ ! -x "$bin" ]; then
  echo "error: no executable at $bin" >&2
  echo "       build it first: cargo build --release -p maggcs-app" >&2
  exit 2
fi

tmp="$(mktemp -d /tmp/maggcs-smoke.XXXXXX)"
monitor=""
app_pid=""
before="$(hyprctl -j monitors | python3 -c '
import json, sys
print("\n".join(m["name"] for m in json.load(sys.stdin)))
')"
prev_ws="$(hyprctl -j monitors | python3 -c '
import json, sys
focused = [m for m in json.load(sys.stdin) if m.get("focused")]
print(focused[0]["activeWorkspace"]["id"] if focused else "")
')"

cleanup() {
  if [ -n "$app_pid" ]; then
    kill "$app_pid" 2>/dev/null || true
  fi
  # The rule lives in Hyprland's Lua state; drop it so later launches are normal.
  hyprctl repl 'if mgr_smoke_rule then mgr_smoke_rule:set_enabled(false) end; return "off"' >/dev/null 2>&1 || true
  for name in $(hyprctl -j monitors | python3 -c '
import json, sys
print(" ".join(m["name"] for m in json.load(sys.stdin) if "HEADLESS" in m["name"]))
' 2>/dev/null || true); do
    case " $before " in
      *" $name "*) ;;
      *) hyprctl output remove "$name" >/dev/null 2>&1 || true ;;
    esac
  done
  # Removing an output can move its (empty) workspace onto the real monitor;
  # restore the operator's view last.
  sleep 0.5
  if [ -n "$prev_ws" ]; then
    hyprctl dispatch "hl.dsp.focus({ workspace = \"$prev_ws\" })" >/dev/null 2>&1 || true
    sleep 0.2
    hyprctl dispatch "hl.dsp.focus({ workspace = \"$prev_ws\" })" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

hyprctl output create headless >/dev/null
sleep 1
monitor="$(hyprctl -j monitors | MON_BEFORE="$before" python3 -c '
import json, os, sys
before = set(os.environ["MON_BEFORE"].split())
names = [m["name"] for m in json.load(sys.stdin)]
print(next((n for n in names if n not in before), ""))
')"
if [ -z "$monitor" ]; then
  echo "error: Hyprland did not create a headless output" >&2
  exit 1
fi
echo "headless output: $monitor"

rule_result="$(hyprctl repl "mgr_smoke_rule = hl.window_rule({ match = { class = \"maggcs-app\" }, monitor = \"$monitor\" }); return \"rule added\"" 2>&1 || true)"
case "$rule_result" in
  *error*|*Error*)
    echo "skip: Hyprland refused the window rule: $rule_result" >&2
    exit 2
    ;;
esac
sleep 0.3

setsid "$bin" >"$tmp/app.log" 2>&1 &
app_pid=$!
sleep "$wait_s"

idx="$(hyprctl -j monitors | MON="$monitor" python3 -c '
import json, os, sys
name = os.environ["MON"]
print(next((i for i, m in enumerate(json.load(sys.stdin)) if m["name"] == name), -1))
')"
windows="$(hyprctl -j clients | IDX="$idx" python3 -c '
import json, os, sys
idx = int(os.environ["IDX"])
wins = [c for c in json.load(sys.stdin) if c["monitor"] == idx]
for c in wins:
    print("  window:", c["class"], repr(c["title"][:40]), c["at"], c["size"])
print("count", len(wins))
' | tee "$tmp/windows.txt" | tail -n 1 | awk '{print $2}')"
if [ "${windows:-0}" = "0" ]; then
  echo "FAIL: the app never opened a window on $monitor" >&2
  echo "      app log: $tmp/app.log" >&2
  exit 1
fi

shot="$tmp/window.png"
grim -o "$monitor" "$shot"
measured="$(NODE_PATH="$root/frontend/node_modules" node "$root/scripts/measure-render.cjs" "$shot")"
bright="$(MEASURED="$measured" python3 -c '
import json, os, sys
print(json.loads(os.environ["MEASURED"])["brightPct"])
')"

echo "capture: $shot"
echo "render : $measured (threshold ${min_bright}% bright pixels)"
if BRIGHT="$bright" MIN="$min_bright" python3 -c '
import os, sys
sys.exit(0 if float(os.environ["BRIGHT"]) >= float(os.environ["MIN"]) else 1)
'; then
  echo "PASS: the window renders content"
else
  echo "FAIL: the window looks blank - check the CSP (ADR-012) and that frontend/dist is current" >&2
  echo "      app log: $tmp/app.log" >&2
  exit 1
fi
