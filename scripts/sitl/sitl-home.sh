#!/usr/bin/env bash
#
# Resolve the SITL world origin from MagGCS's Settings -> Vehicle initial
# position (the desktop shell mirrors it into `sitl-home.json` in the app
# config dir), falling back to the PX4_HOME_* env varsaine then to the built-in
# default (Sidney BC, Canada, ~5 m MSL).
#
# Usage (either form):
#   bash scripts/sitl/sitl-home.sh        # prints "48.6493 -123.3982 5"
#   source scripts/sitl/sitl-home.sh      # sets PX4_HOME_LAT/LON/ALT in caller
#
# Env:
#   MAGGCS_SITL_HOME_FILE  explicit path to the settings file (default:
#                          ~/.config/io.maggcs.desktop/sitl-home.json)
#   PX4_HOME_LAT/LON/ALT   explicit override; always wins over the file
#   DEFAULT_HOME_LAT/LON/ALT
#                          last-resort fallback when nothing else is set

set -euo pipefail

MAGGCS_SITL_HOME_FILE_DEFAULT="$HOME/.config/io.maggcs.desktop/sitl-home.json"
MAGGCS_SITL_HOME_FILE="${MAGGCS_SITL_HOME_FILE:-${MAGGCS_SITL_HOME_FILE_DEFAULT}}"

DEFAULT_HOME_LAT="${DEFAULT_HOME_LAT:-48.6493}"
DEFAULT_HOME_LON="${DEFAULT_HOME_LON:--123.3982}"
DEFAULT_HOME_ALT="${DEFAULT_HOME_ALT:-5}"

resolve_sitl_home() {
  local file="${MAGGCS_SITL_HOME_FILE}"
  if [ ! -f "$file" ]; then
    # Windows desktop app under WSL2: the setting lives in AppData.
    local wsl_file
    wsl_file="$(ls /mnt/c/Users/*/AppData/Roaming/io.maggcs.desktop/sitl-home.json 2>/dev/null | head -n 1 || true)"
    if [ -n "$wsl_file" ] && [ -f "$wsl_file" ]; then
      file="$wsl_file"
    fi
  fi

  local saved_lat="" saved_lon=""
  if [ -f "$file" ]; then
    saved_lat="$(sed -n 's/.*"lat":\([-0-9.eE]*\).*/\1/p' "$file" | head -n 1)"
    saved_lon="$(sed -n 's/.*"lon":\([-0-9.eE]*\).*/\1/p' "$file" | head -n 1)"
  fi
  if ! [[ "$saved_lat" =~ ^-?[0-9]+(\.[0-9]+)?$ ]]; then saved_lat=""; fi
  if ! [[ "$saved_lon" =~ ^-?[0-9]+(\.[0-9]+)?$ ]]; then saved_lon=""; fi

  PX4_HOME_LAT="${PX4_HOME_LAT:-${saved_lat:-${DEFAULT_HOME_LAT}}}"
  PX4_HOME_LON="${PX4_HOME_LON:-${saved_lon:-${DEFAULT_HOME_LON}}}"
  PX4_HOME_ALT="${PX4_HOME_ALT:-${DEFAULT_HOME_ALT}}"
}

resolve_sitl_home

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  printf '%s %s %s\n' "$PX4_HOME_LAT" "$PX4_HOME_LON" "$PX4_HOME_ALT"
fi