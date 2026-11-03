#!/usr/bin/env bash
# A virtual display for the EVE client, with no monitor and no human.
#
#   tools/client-headless.sh --build          # build the image (only networked step)
#   tools/client-headless.sh start [--gpu]    # start the display, detached
#   tools/client-headless.sh status
#   tools/client-headless.sh shot [file.png]  # capture a frame + scalar metrics
#   tools/client-headless.sh metrics <png>    # scalar metrics for a captured frame
#   tools/client-headless.sh run <cmd...>     # run a HOST command against the display
#   tools/client-headless.sh desktop on|off   # the wine virtual desktop (see below)
#   tools/client-headless.sh smoke            # the REAL client, headless, no network
#   tools/client-headless.sh selftest         # evidence bundle into out/ci-display/
#   tools/client-headless.sh stop
#
# WHAT RUNS WHERE, and why it is this way round
#
# The *display* runs in the container; the *client* does not. The client stays
# exactly where tools/run-client.sh puts it today: on the host, under Proton,
# inside the `eveoffline` network namespace. X11 is a unix socket on the
# filesystem, and filesystem sockets are not namespaced by a network namespace
# -- only abstract ones are -- so a client inside the jail reaches a display
# server outside it without any hole being opened in the jail. docs/guides/jail.md
# makes the same point about graphics generally; this lane measured it
# (docs/lanes/ci-display.md, E3).
#
# That ordering is deliberate. Putting the client in the container instead
# would mean handing a container the 48 GB resource tree, the Proton prefix
# and -- the part that matters -- a network path back to our server, which
# means rebuilding the jail's guarantees around Docker's networking. The jail
# is the one thing on this project that must not be re-derived casually. So
# the container supplies the single thing the host is missing (Xvfb) and
# nothing else, and gets `--network none` for its trouble.
#
#   host                                   container (--network none)
#   ┌────────────────────────────┐         ┌─────────────────────┐
#   │ netns eveoffline           │         │ Xvfb :99            │
#   │   exefile.exe (Proton)     │         │ no interfaces at all│
#   │   DISPLAY=:99 ─────────────┼─ unix ──┼→ /tmp/.X11-unix/X99 │
#   │   DX11→DXVK→Vulkan→lavapipe│  socket │                     │
#   └────────────────────────────┘         └─────────────────────┘
#
# SOFTWARE RENDERING, NOT THE GPU. Xvfb has no DRI3, and RADV refuses to
# present without it ("MESA: info: vulkan: No DRI3 support detected - required
# for presentation"). The GPU is still visible to a container -- RADV
# enumerates the 7900 XTX with the two --device flags `--gpu` adds -- it just
# cannot put a pixel on an Xvfb screen. So this script forces lavapipe, the
# software Vulkan driver the host already has. See docs/lanes/ci-display.md.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
IMAGE=${EVE_DISPLAY_IMAGE:-eve-offline/display:latest}
NAME=${EVE_DISPLAY_NAME:-eve-headless-display}
DISPLAY_NUM=${EVE_HEADLESS_DISPLAY:-99}
SCREEN=${EVE_HEADLESS_SCREEN:-1920x1080x24}
OUT="$REPO/out/ci-display"
# The host's software Vulkan driver. Present on this machine already
# (mesa-vulkan-drivers); nothing is installed to get it.
LAVAPIPE=${EVE_LAVAPIPE_ICD:-/usr/share/vulkan/icd.d/lvp_icd.json}
# The Proton prefix the `desktop` subcommand operates on. Same defaults as
# run-client.sh (tools/prefix-lib.sh): `offline`, the offline client's own
# prefix, is the first player (O19); 8501 is `--second`. The online prefix
# 8500 is refused: the desktop key is prefix state, and that prefix is not ours.
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"
: "${EVE_PREFIX_ID:=offline}"
: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
if [[ -z ${EVE_PREFIX:-} ]]; then
    EVE_PREFIX=$(eve_prefix_of "$EVE_PREFIX_ID" "$HOME" "$STEAM_ROOT") \
        || { echo "client-headless.sh: EVE_PREFIX_ID must be 'offline' or a number" >&2; exit 1; }
fi
eve_refuse_online client-headless.sh "EVE_PREFIX=$EVE_PREFIX"
: "${PROTON:=$STEAM_ROOT/compatibilitytools.d/GE-Proton10-34/proton}"
DESKTOP_SIZE=${EVE_HEADLESS_DESKTOP:-1600x900}

say() { printf '%s\n' "$*" >&2; }
die() { printf '%s\n' "$*" >&2; exit 1; }

build() {
    docker build --network default \
        -t "$IMAGE" \
        -f "$REPO/containers/display.Dockerfile" \
        "$REPO/containers"
}

have_image() { docker image inspect "$IMAGE" >/dev/null 2>&1; }
running()    { [[ -n "$(docker ps -q -f "name=^${NAME}$" 2>/dev/null)" ]]; }

# Run a command inside the display container, sharing its display.
in_display() {
    docker exec -e DISPLAY=":$DISPLAY_NUM" -e HOME=/tmp/home "$NAME" "$@"
}

# Is something actually listening on the display's socket?
#
# The socket file is a bind mount from the host, so it OUTLIVES the container:
# when Xvfb is killed it never gets the chance to unlink it, and the next
# `start` would otherwise refuse to run for the rest of the machine's uptime.
# A connect() tells live from stale without trusting a PID file.
socket_live() {
    local sock=/tmp/.X11-unix/X$DISPLAY_NUM
    [[ -S "$sock" ]] || return 1
    python3 - "$sock" <<'PY'
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(2)
try:
    s.connect(sys.argv[1])
except OSError:
    sys.exit(1)
sys.exit(0)
PY
}

# Remove a socket nothing is listening on; refuse to touch a live one, which
# would be somebody else's display.
reap_socket() {
    local sock=/tmp/.X11-unix/X$DISPLAY_NUM
    [[ -e "$sock" ]] || return 0
    if socket_live; then
        die "client-headless.sh: a LIVE X server is already on :$DISPLAY_NUM -- pick another EVE_HEADLESS_DISPLAY"
    fi
    rm -f "$sock" && say "client-headless.sh: removed stale socket $sock"
}

start() {
    local gpu=0
    [[ "${1:-}" == "--gpu" ]] && gpu=1
    have_image || { say "client-headless.sh: image missing, building once..."; build; }
    if running; then
        say "client-headless.sh: display already running as '$NAME' on :$DISPLAY_NUM"
        return 0
    fi
    docker rm -f "$NAME" >/dev/null 2>&1 || true

    # The socket crosses into the host at the standard path, which is what
    # makes DISPLAY=:N work for a client that knows nothing about containers.
    # /tmp/.X11-unix is 1777 on this host, so the container user can create
    # its socket there; the display number is 99 by default precisely so it
    # cannot collide with the owner's own :0 / :1.
    [[ -d /tmp/.X11-unix ]] || die "client-headless.sh: /tmp/.X11-unix does not exist on this host"
    reap_socket

    local devargs=()
    if [[ $gpu -eq 1 ]]; then
        # Kept because it is cheap and it is what a future DRI3-capable
        # display server would need. It does NOT make Xvfb accelerated.
        for d in /dev/dri/card1 /dev/dri/renderD128; do
            [[ -e "$d" ]] && devargs+=(--device "$d")
        done
        for g in render video; do
            gid=$(getent group "$g" | cut -d: -f3 || true)
            [[ -n "${gid:-}" ]] && devargs+=(--group-add "$gid")
        done
    fi

    mkdir -p "$OUT"
    docker run -d --rm --init \
        --name "$NAME" \
        --network none \
        --cap-drop ALL \
        --security-opt no-new-privileges \
        --user "$(id -u):$(id -g)" \
        ${devargs[@]+"${devargs[@]}"} \
        --tmpfs "/tmp/home:exec,size=1g,uid=$(id -u),gid=$(id -g),mode=0700" \
        -e HOME=/tmp/home \
        -v /tmp/.X11-unix:/tmp/.X11-unix \
        -v "$OUT:/out" \
        "$IMAGE" \
        Xvfb ":$DISPLAY_NUM" -screen 0 "$SCREEN" -nolisten tcp -noreset >/dev/null

    for _ in $(seq 1 100); do
        [[ -e "/tmp/.X11-unix/X$DISPLAY_NUM" ]] && break
        sleep 0.1
    done
    [[ -e "/tmp/.X11-unix/X$DISPLAY_NUM" ]] \
        || { docker logs "$NAME" >&2 || true; die "client-headless.sh: Xvfb never created its socket"; }

    say "display up: DISPLAY=:$DISPLAY_NUM  screen $SCREEN  (container '$NAME', --network none)"
    say "launch the client against it with:"
    say "    sudo -v && tools/client-headless.sh run tools/run-client.sh"
}

stop() {
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    # The container is gone, so whatever is left at that path is stale.
    rm -f "/tmp/.X11-unix/X$DISPLAY_NUM"
    say "display stopped"
}

status() {
    if running; then
        say "display '$NAME' is running on :$DISPLAY_NUM"
        in_display xdpyinfo | sed -n '1,12p'
        say "-- X clients connected --"
        in_display xlsclients -l || true
    else
        say "display '$NAME' is NOT running"
        return 1
    fi
}

# Capture a frame. The capture happens INSIDE the container, so it needs no
# X tooling on the host and works whatever the client is.
shot() {
    running || die "client-headless.sh: display is not running (start it first)"
    local dest=${1:-$OUT/frame-$(date +%Y%m%d-%H%M%S).png}
    mkdir -p "$(dirname "$dest")"
    local base; base=$(basename "$dest")
    in_display import -window root "/out/$base"
    # /out is the same directory as $OUT; move only if the caller asked for
    # somewhere else.
    [[ "$OUT/$base" == "$dest" ]] || mv "$OUT/$base" "$dest"
    say "captured $dest"
    metrics "$dest"
}

# The frame oracle: scalar metrics, not an image diff. Mean saturation alone
# would have caught the greyscale scene (docs/archive/plan-client-ci.md §1).
metrics() {
    local png=${1:?usage: metrics <file.png>}
    running || die "client-headless.sh: display is not running"
    local base; base=$(basename "$png")
    [[ -f "$OUT/$base" ]] || cp "$png" "$OUT/$base"
    in_display sh -c "convert /out/$base -colorspace HSL -channel G -separate +channel -format 'saturation_mean=%[fx:mean]\n' info: ; convert /out/$base -format 'luma_mean=%[fx:mean]\nluma_std=%[fx:standard_deviation]\ncolors=%k\ngeometry=%wx%h\n' info:"
}

# Run a HOST command against the headless display.
#
# WAYLAND_DISPLAY is cleared deliberately: run-client.sh forwards it, and a
# Proton client that finds a Wayland socket will happily open a window on the
# owner's own session -- which is the one thing this lane must never do.
run_host() {
    [[ $# -gt 0 ]] || die "usage: client-headless.sh run <command...>"
    running || die "client-headless.sh: display is not running (start it first)"
    [[ -f "$LAVAPIPE" ]] || say "client-headless.sh: warning: no lavapipe ICD at $LAVAPIPE"
    DISPLAY=":$DISPLAY_NUM" \
    WAYLAND_DISPLAY= \
    VK_DRIVER_FILES="$LAVAPIPE" \
    VK_ICD_FILENAMES="$LAVAPIPE" \
    LIBGL_ALWAYS_SOFTWARE=1 \
    exec "$@"
}

# The one thing that is NOT optional for a headless EVE client: a Wine
# virtual desktop.
#
# Launched straight onto an Xvfb root window, the client gets as far as
# creating its D3D11 device on lavapipe and then exits, silently, every time,
# immediately after DXVK's
#
#     err:   readMonitorEdidFromKey: Failed to get EDID reg key size
#     err:   DXGI: Failed to parse display metadata + colorimetry info, using blank.
#
# -- no client log is ever written, because it dies before Python boots. Give
# Wine a virtual desktop and the same command line starts, boots Stackless
# 2.7 and draws its login screen. (Measured, not reasoned: see
# docs/lanes/ci-display.md §5.)
#
# This is a REGISTRY setting on the prefix, deliberately, because it is the
# only place to put it that does not mean editing tools/run-client.sh: the
# client's own command line stays exactly what it is today, and the jail,
# the manifest and the `tq-offline` copy are all untouched.
#
# It is prefix state, so it is reversible and it must be reverted -- with the
# key set, a client launched for a HUMAN gets a window-in-a-window too.
desktop() {
    local mode=${1:-}
    [[ -x "$PROTON" ]] || die "client-headless.sh: proton not found at $PROTON"
    [[ -d "$EVE_PREFIX" ]] || die "client-headless.sh: no Proton prefix at $EVE_PREFIX"
    local -a penv=(
        env HOME="$HOME" DISPLAY=":$DISPLAY_NUM" WAYLAND_DISPLAY=
        PROTON_USE_XALIA=0
        STEAM_COMPAT_DATA_PATH="$EVE_PREFIX"
        STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_ROOT"
        "$PROTON" run reg
    )
    case "$mode" in
        on)
            "${penv[@]}" add 'HKCU\Software\Wine\Explorer' /v Desktop /t REG_SZ /d Default /f >/dev/null 2>&1
            "${penv[@]}" add 'HKCU\Software\Wine\Explorer\Desktops' /v Default /t REG_SZ /d "$DESKTOP_SIZE" /f >/dev/null 2>&1
            say "prefix $EVE_PREFIX_ID: wine virtual desktop ON ($DESKTOP_SIZE)"
            ;;
        off)
            "${penv[@]}" delete 'HKCU\Software\Wine\Explorer' /v Desktop /f >/dev/null 2>&1
            say "prefix $EVE_PREFIX_ID: wine virtual desktop OFF"
            ;;
        status|"")
            grep -A3 -i 'Wine\\\\Explorer\]' "$EVE_PREFIX/pfx/user.reg" 2>/dev/null \
                | sed -n 's/^"Desktop"=/    Desktop=/p' \
                | grep . || say "prefix $EVE_PREFIX_ID: no virtual desktop set"
            ;;
        *) die "usage: client-headless.sh desktop on|off|status" ;;
    esac
}

# The real client, headless, with NO network whatsoever -- reproducibly, and
# without sudo.
#
# This is how this lane's evidence was produced and how a sceptic re-runs it.
# It is a DISPLAY smoke test, not the harness: `unshare --net` gives the
# client a network namespace with no interfaces at all, which is strictly more
# isolated than the jail (the jail at least has a veth to our own server), so
# the client gets as far as its login screen and reports
#
#     Unable to connect to tranquility.servers.eveonline.com on port 26000
#
# which is the correct and desired outcome here: it proves the pixels AND the
# isolation in one frame. To get a client that can actually log in, use
# `run` with tools/run-client.sh and the real jail.
#
# It defaults to the SECOND prefix so it cannot collide with a primary client
# a human or another lane is using, and it refuses the live `tq` install for
# the same reasons run-client.sh does.
smoke() {
    local prefix_id=${EVE_SMOKE_PREFIX_ID:-8501}
    local client_dir=${EVE_SMOKE_CLIENT_DIR:-tq-offline-2}
    local prefix="$STEAM_ROOT/steamapps/compatdata/$prefix_id"
    local client="$prefix/pfx/drive_c/CCP/EVE/$client_dir"
    local secs=${EVE_SMOKE_SECONDS:-75}

    [[ "$client_dir" == "tq" ]] && die "client-headless.sh: refusing to smoke-test the live install"
    eve_refuse_online client-headless.sh "the smoke prefix=$prefix" "its ResFiles=$client/../ResFiles"
    [[ -f "$client/bin64/exefile.exe" ]] || die "client-headless.sh: no client at $client"
    [[ -x "$PROTON" ]] || die "client-headless.sh: proton not found at $PROTON"
    unshare --map-current-user --net -- true 2>/dev/null \
        || die "client-headless.sh: unprivileged network namespaces are unavailable; use the real jail instead"
    running || start
    EVE_PREFIX_ID=$prefix_id EVE_PREFIX=$prefix desktop on

    mkdir -p "$OUT"
    say "smoke: launching $client_dir with no network at all, onto :$DISPLAY_NUM"
    # No `exec`: we want to come back and capture.
    ( cd "$client" && unshare --map-current-user --net -- \
        env HOME="$HOME" DISPLAY=":$DISPLAY_NUM" WAYLAND_DISPLAY= \
            XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" \
            PROTON_USE_XALIA=0 \
            VK_DRIVER_FILES="$LAVAPIPE" VK_ICD_FILENAMES="$LAVAPIPE" \
            STEAM_COMPAT_DATA_PATH="$prefix" \
            STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_ROOT" \
            "$PROTON" run "$client/bin64/exefile.exe" \
            /ssoToken=offline '/fileLogDirectory=C:\eve-logs' \
        > "$OUT/smoke-proton.log" 2>&1 ) &

    local seen=0
    for _ in $(seq 1 "$secs"); do
        sleep 1
        if in_display xwininfo -root -tree 2>/dev/null | grep -q '"EVE"'; then seen=1; break; fi
    done
    sleep 10   # let it finish drawing the login screen
    if [[ $seen -eq 1 ]]; then
        say "smoke: the client has an X window"
        shot "$OUT/smoke-eve.png"
    else
        say "smoke: NO EVE window after ${secs}s -- see $OUT/smoke-proton.log"
    fi
    local clog
    clog=$(ls -t "$prefix/pfx/drive_c/eve-logs"/client-*.log 2>/dev/null | head -1)
    if [[ -n "$clog" ]]; then
        cp "$clog" "$OUT/smoke-client.log"
        say "smoke: client log -> $OUT/smoke-client.log"
        grep -m1 'EVE Client version .* started' "$OUT/smoke-client.log" || true
    fi
    say "smoke: stopping the client"
    pkill -f "$client_dir[/\\]bin64" >/dev/null 2>&1 || true
    pkill -f "${client_dir}.bin64.exefile" >/dev/null 2>&1 || true
    sleep 3
    EVE_PREFIX_ID=$prefix_id EVE_PREFIX=$prefix desktop off
    [[ $seen -eq 1 ]]
}

# Everything this lane claims, re-run from scratch, into out/ci-display/.
selftest() {
    mkdir -p "$OUT"
    local log="$OUT/selftest.log"
    : > "$log"
    exec > >(tee -a "$log") 2>&1

    echo "=== client-headless selftest  $(date -Is) ==="
    echo "--- 0. host has no Xvfb of its own (this is why the image exists) ---"
    command -v Xvfb || echo "Xvfb: not installed on the host  [expected]"

    stop
    start "${1:-}"

    echo "--- 1. the display exists, and what it can do ---"
    in_display xdpyinfo | sed -n '/^name of display/p;/dimensions/p;/^number of extensions/,/^default screen/p' \
        | tee "$OUT/xdpyinfo.txt"
    echo "--- DRI3? (the GPU question, answered by its absence) ---"
    grep -q DRI3 "$OUT/xdpyinfo.txt" && echo "DRI3: PRESENT" || echo "DRI3: ABSENT -- GPU presentation impossible here"

    echo "--- 2. a WINDOWS GUI program, headless, in the container ---"
    in_display env WINEPREFIX=/tmp/home/wp WINEDLLOVERRIDES="mscoree,mshtml=" WINEDEBUG=-all \
        wineboot -i > "$OUT/wineboot.log" 2>&1 \
        && echo "    wineboot: ok" || echo "    wineboot: FAILED (see $OUT/wineboot.log)"
    in_display sh -c 'cd /tmp/home && WINEPREFIX=/tmp/home/wp WINEDEBUG=-all setsid wine notepad > /out/notepad-wine.log 2>&1 < /dev/null &'
    for _ in $(seq 1 30); do
        in_display xwininfo -root -tree 2>/dev/null | grep -q Notepad && break
        sleep 1
    done
    in_display xwininfo -root -tree | tee "$OUT/xwininfo.txt" | sed -n '1,12p'
    shot "$OUT/notepad.png"

    echo "--- 3. Vulkan presentation to this display (software) ---"
    in_display sh -c 'timeout 30 vkcube --c 30 2>&1 | tail -5' | tee "$OUT/vkcube.txt" || true

    echo "--- 4. a HOST process in its OWN network namespace reaches the display ---"
    echo "    (the jail crossing: unshare -rn has no interfaces but keeps the filesystem)"
    if unshare -r -n sh -c "ip -o link show 2>/dev/null | wc -l; DISPLAY=:$DISPLAY_NUM xdpyinfo | sed -n '1,3p'" \
         2>&1 | tee "$OUT/netns-crossing.txt"; then
        echo "    netns crossing: OK"
    else
        echo "    netns crossing: FAILED (see $OUT/netns-crossing.txt)"
    fi

    echo
    echo "=== evidence in $OUT ==="
    ls -l "$OUT"
}

cmd=${1:-}
shift || true
case "$cmd" in
    --build|build) build ;;
    start)         start "${1:-}" ;;
    stop)          stop ;;
    status)        status ;;
    shot)          shot "${1:-}" ;;
    metrics)       metrics "${1:-}" ;;
    run)           run_host "$@" ;;
    desktop)       desktop "${1:-}" ;;
    smoke)         smoke ;;
    selftest)      selftest "${1:-}" ;;
    *) sed -n '2,20p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
esac
