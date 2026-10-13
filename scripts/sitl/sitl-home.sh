#!/usr/bin/env bash
#
# Resolve the SITL world origin from MagGCS's Settings -> Vehicle initial
# position (the desktop shell mirrors it into `sitl-home.json` in the app
# config dir), falling back to the PX4_HOME_* env vars, then to a named
# preset area, then to the built-in default (Sidney BC, Canada, ~5 m MSL).
#
# Preset areas mirror `frontend/src/cesium/presets.ts` — keep the two in
# sync (Chilliwack is the recommended SITL drape-verification area).
#
# Usage (either form):
#   bash scripts/sitl/sitl-home.sh              # prints "48.6493 -123.3982 5"
#   bash scripts/sitl/sitl-home.sh chilliwack   # prints the Chilliwack coords
#   source scripts/sitl/sitl-home.sh            # sets PX4_HOME_LAT/LON/ALT
#   MAGGCS_SITL_AREA=renfrew bash scripts/sitl/run_sitl_docker.sh
#
# Env:
#   MAGGCS_SITL_HOME_FILE  explicit path to the settings file (default:
#                          ~/.config/io.maggcs.desktop/sitl-home.json)
#   PX4_HOME_LAT/LON/ALT   explicit override; always wins over the file
#   MAGGCS_SITL_AREA       preset area id (sidney|renfrew|chilliwack);
#                          used when neither the file nor PX4_HOME_* is set
#   DEFAULT_HOME_LAT/LON/ALT
#                          last-resort fallback when nothing else is set

set -euo pipefail

MAGGCS_SITL_HOME_FILE_DEFAULT="$HOME/.config/io.maggcs.desktop/sitl-home.json"
MAGGCS_SITL_HOME_FILE="${MAGGCS_SITL_HOME_FILE:-${MAGGCS_SITL_HOME_FILE_DEFAULT}}"

# Preset areas (mirror of frontend/src/cesium/presets.ts).
preset_coords() {
  case "${1:-}" in
    renfrew)    printf '%s %s %s\n' 45.4726 -76.6858 120 ;;
    chilliwack) printf '%s %s %s\n' 49.1577 -121.9510 10 ;;
    sidney|"")  printf '%s %s %s\n' 48.6493 -123.3982 5 ;;
    *) return 1 ;;
  esac
}

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

  # Preset area: an explicit AREA beats the persisted app setting (the
  # operator asked for a specific site), but an explicit PX4_HOME_* env
  # override still wins over everything.
  local area_lat="" area_lon="" area_alt=""
  local area="${MAGGCS_SITL_AREA:-${1:-}}"
  if [ -n "$area" ]; then
    read -r area_lat area_lon area_alt < <(preset_coords "$area" 2>/dev/null || printf '')
  fi

  PX4_HOME_LAT="${PX4_HOME_LAT:-${area_lat:-${saved_lat:-${DEFAULT_HOME_LAT}}}}"
  PX4_HOME_LON="${PX4_HOME_LON:-${area_lon:-${saved_lon:-${DEFAULT_HOME_LON}}}}"
  PX4_HOME_ALT="${PX4_HOME_ALT:-${area_alt:-${DEFAULT_HOME_ALT}}}"
}

resolve_sitl_home "${1:-}"

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  printf '%s %s %s\n' "$PX4_HOME_LAT" "$PX4_HOME_LON" "$PX4_HOME_ALT"
fi