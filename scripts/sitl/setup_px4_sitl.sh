#!/usr/bin/env bash
#
# setup_px4_sitl.sh - PX4 development + SITL environment using the official
#                     PX4 Docker image (no Gazebo required).
#
# Firmware : v1.17.0
# Image    : px4io/px4-dev:v1.17.0 (toolchain only, no Gazebo)
# SITL     : `make px4_sitl sihsim_quadx` -> PX4 SITL using the built-in SIH
#            (Simulation In Hardware) dynamics. No Gazebo, no external sim.
# MAVLink  : shadows PX4's px4-rc.mavlink with the repo copy, which broadcasts
#            the GCS link on the LAN (UDP 14550).
# Origin   : resolves the SITL world position from MagGCS's Settings -> Vehicle
#            initial position, PX4_HOME_* env vars, or a preset area.
#
# Usage:
#   ./setup_px4_sitl.sh setup    # clone firmware + verify docker
#   ./setup_px4_sitl.sh build    # build the SITL binary
#   ./setup_px4_sitl.sh sim      # build & launch PX4 SITL (sihsim_quadx)
#   ./setup_px4_sitl.sh shell    # interactive shell inside the container
#   ./setup_px4_sitl.sh update   # pull latest + update submodules
#   ./setup_px4_sitl.sh clean    # remove build directory
#
# Environment overrides:
#   PX4_DIR           PX4 checkout (default $HOME/PX4-Autopilot)
#   PX4_VERSION       firmware tag (default v1.17.0)
#   PX4_DOCKER_IMAGE  Docker image (default px4io/px4-dev:$PX4_VERSION)
#   SITL_AIRFRAME     airframe (default sihsim_quadx)
#   MAGGCS_SITL_AREA  preset origin: sidney | renfrew | chilliwack
#   PX4_HOME_LAT/LON/ALT  explicit origin, wins over presets/settings
#   MAGGCS_SITL_HOME_FILE explicit MagGCS sitl-home.json path
#
set -euo pipefail

PX4_VERSION="${PX4_VERSION:-v1.17.0}"
PX4_DIR="${PX4_DIR:-$HOME/PX4-Autopilot}"
PX4_DOCKER_IMAGE="${PX4_DOCKER_IMAGE:-px4io/px4-dev:${PX4_VERSION}}"
SITL_AIRFRAME="${SITL_AIRFRAME:-sihsim_quadx}"
CCACHE_DIR="${CCACHE_DIR:-$HOME/.ccache}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

log()  { printf '\033[1;34m[px4]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[px4]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[px4] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

# SITL world origin: MagGCS settings file -> PX4_HOME_* -> preset -> Sidney.
MAGGCS_SITL_HOME_FILE_DEFAULT="$HOME/.config/io.maggcs.desktop/sitl-home.json"
MAGGCS_SITL_HOME_FILE="${MAGGCS_SITL_HOME_FILE:-${MAGGCS_SITL_HOME_FILE_DEFAULT}}"
DEFAULT_HOME_LAT="${DEFAULT_HOME_LAT:-48.6493}"
DEFAULT_HOME_LON="${DEFAULT_HOME_LON:--123.3982}"
DEFAULT_HOME_ALT="${DEFAULT_HOME_ALT:-5}"

preset_coords() {
    case "${1:-}" in
        renfrew)    printf '%s %s %s\n' 45.4726 -76.6858 120 ;;
        chilliwack) printf '%s %s %s\n' 49.1577 -121.9510 10 ;;
        sidney|"")  printf '%s %s %s\n' 48.6493 -123.3982 5 ;;
        *) return 1 ;;
    esac
}

resolve_sitl_home() {
    local file="$MAGGCS_SITL_HOME_FILE"
    if [ ! -f "$file" ]; then
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

    local area_lat="" area_lon="" area_alt=""
    if [ -n "${MAGGCS_SITL_AREA:-}" ]; then
        read -r area_lat area_lon area_alt < <(preset_coords "$MAGGCS_SITL_AREA" 2>/dev/null || printf '')
    fi

    PX4_HOME_LAT="${PX4_HOME_LAT:-${area_lat:-${saved_lat:-${DEFAULT_HOME_LAT}}}}"
    PX4_HOME_LON="${PX4_HOME_LON:-${area_lon:-${saved_lon:-${DEFAULT_HOME_LON}}}}"
    PX4_HOME_ALT="${PX4_HOME_ALT:-${area_alt:-${DEFAULT_HOME_ALT}}}"
}

need_docker() {
    command -v docker >/dev/null 2>&1 || die "docker is not installed"
    docker info >/dev/null 2>&1 || die "docker daemon not running or no permission"
}

clone_repo() {
    if [ -d "$PX4_DIR/.git" ]; then
        log "PX4 already present at $PX4_DIR"
    else
        log "Cloning PX4-Autopilot $PX4_VERSION into $PX4_DIR"
        mkdir -p "$(dirname "$PX4_DIR")"
        git clone --branch "$PX4_VERSION" --depth 1 --recurse-submodules \
            https://github.com/PX4/PX4-Autopilot.git "$PX4_DIR"
    fi
    log "Syncing submodules"
    git -C "$PX4_DIR" submodule update --init --recursive
}

docker_run() {
    [ -d "$PX4_DIR/.git" ] || die "firmware not found, run: $0 setup"
    need_docker
    mkdir -p "$CCACHE_DIR"

    local tty=()
    [ -t 0 ] && [ -t 1 ] && tty=(-it)

    # Shadow PX4's px4-rc.mavlink with the repo copy so the GCS link
    # broadcasts on the LAN. The override is a v1.17 file.
    local rc_mount=()
    local rc_path=""
    local home_env=()
    local px4_version
    px4_version="$(git -C "$PX4_DIR" describe --tags 2>/dev/null || true)"
    if [[ "$px4_version" == v1.17* ]]; then
        rc_mount=(-v "$SCRIPT_DIR:/maggcs-sitl:ro")
        # shellcheck disable=SC2016 # $PATH must expand in the container
        rc_path='export PATH="/maggcs-sitl:$PATH"; '
    else
        warn "px4-rc.mavlink override needs PX4 v1.17 (found '${px4_version:-unknown}'); using the stock localhost link"
    fi
    if [ -n "${PX4_HOME_LAT:-}" ]; then
        home_env=(
            -e "PX4_HOME_LAT=$PX4_HOME_LAT"
            -e "PX4_HOME_LON=$PX4_HOME_LON"
            -e "PX4_HOME_ALT=$PX4_HOME_ALT"
        )
    fi

    log "Running in $PX4_DOCKER_IMAGE"
    docker run --rm "${tty[@]}" \
        --network host \
        --user "$(id -u):$(id -g)" \
        -e HOME="$HOME" \
        -e CCACHE_DIR="$CCACHE_DIR" \
        -v "$PX4_DIR:$PX4_DIR" \
        -v "$CCACHE_DIR:$CCACHE_DIR" \
        "${rc_mount[@]}" \
        "${home_env[@]}" \
        -w "$PX4_DIR" \
        "$PX4_DOCKER_IMAGE" \
        bash -lc "${rc_path}$1"
}

cmd_setup() {
    need_docker
    clone_repo
    log "Setup complete."
    log "Next:  $0 build   (compile SITL)"
    log "       $0 sim     (build + launch PX4 SITL, no simulator)"
}

cmd_build() {
    docker_run "make px4_sitl_default"
}

cmd_sim() {
    resolve_sitl_home
    log "Launching PX4 SITL airframe '$SITL_AIRFRAME' (no Gazebo)."
    log "SITL origin: $PX4_HOME_LAT, $PX4_HOME_LON, ${PX4_HOME_ALT} m"
    log "MAVLink GCS output is on UDP 14550 (host network); SITL has one MAIN output bank."
    docker_run "make px4_sitl $SITL_AIRFRAME"
}

cmd_shell() {
    log "Opening interactive shell inside the PX4 dev container."
    docker_run "bash"
}

cmd_update() {
    need_docker
    [ -d "$PX4_DIR/.git" ] || die "firmware not found, run: $0 setup"
    log "Updating firmware + submodules"
    git -C "$PX4_DIR" fetch --tags
    git -C "$PX4_DIR" pull --ff-only || warn "pull skipped (detached tag?)"
    git -C "$PX4_DIR" submodule update --init --recursive
}

cmd_clean() {
    log "Removing $PX4_DIR/build"
    rm -rf "$PX4_DIR/build" "$PX4_DIR/build-*"
}

usage() {
    cat <<EOF
PX4 SITL environment (Docker, $PX4_VERSION)

  $0 setup    clone firmware and verify docker
  $0 build    compile SITL binary (px4_sitl_default)
  $0 sim      build & launch PX4 SITL ($SITL_AIRFRAME, SIH, no Gazebo)
  $0 shell    interactive shell in the dev container
  $0 update   pull latest firmware + submodules
  $0 clean    delete build output

Environment overrides:
  PX4_VERSION       firmware tag         (default: v1.17.0)
  PX4_DIR           checkout location    (default: \$HOME/PX4-Autopilot)
  PX4_DOCKER_IMAGE  dev image            (default: px4io/px4-dev:\$PX4_VERSION)
  SITL_AIRFRAME     airframe             (default: sihsim_quadx)
                                         use none_iris if you supply your own
                                         external simulator on TCP 4560)
  CCACHE_DIR        ccache location       (default: \$HOME/.ccache)
EOF
}

case "${1:-}" in
    setup)  cmd_setup ;;
    build)  cmd_build ;;
    sim|run) cmd_sim ;;
    shell)  cmd_shell ;;
    update) cmd_update ;;
    clean)  cmd_clean ;;
    ""|-h|--help|help) usage ;;
    *) die "unknown command '$1' (try: $0 --help)" ;;
esac
