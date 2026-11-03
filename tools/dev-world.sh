#!/usr/bin/env bash
# A DEV world: the server in --dev mode (a dev seller of every item at 100 ISK
# in every station, a billion ISK each) and the roster's bots, with your
# client able to log in. docs/guides/dev-mode.md.
#
#   tools/dev-world.sh                              # content/bots/dev/world50.tsv
#   tools/dev-world.sh pvp --home "Some LowSec"     # content/bots/dev/pvp.tsv
#   tools/dev-world.sh path/to/roster.tsv --home Jita --client
#
# ROSTER is a path, or a name in content/bots/dev/ (mining, pvp, pve,
# world50). --home SYSTEM seats every row without its own system there (a
# system NAME or id; PvP wants low or null sec). --client also brings up the
# jail and launches your client (as tools/serve.sh --client does); without it
# the server listens on 127.0.0.1:26000 only (EVE_DEV_PORT). --no-build, --debug, --secs N
# (stop the bots and the server after N seconds; default: until Ctrl-C).
# --brawl: a fleet-fight bench (content/bots/dev/brawl.tsv): hunters take any odds and
# never warp off (eve-bots --brawl), and no bubble, so no bot is shed under load.
# ROSTER none: no bots at the start; a GM spawns them with /bots spawn.
#
# The dev world is var/dev/world.dat, marked as dev (world.dat.dev): the
# server refuses it without --dev and refuses --dev on any other existing
# world, so the normal save var/world.dat is never touched. Wipe the dev
# world with: rm -r var/dev
#
# Needs out/content (tools/up.sh or tools/serve.sh makes it once). Bot
# passwords live in ~/.config/eve-offline/dev-bots.tsv (mode 600). Your own
# login is the one from tools/account.sh login; the listener signs up new
# names, so your first login creates your character (a billion ISK too).
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SRV=${EVE_SERVER_BIN:-eve-offline-server}   # the server binary; EVE_SERVER_BIN=eve-server for the plain core one
VAR=${EVE_DEV_VAR:-$REPO/var/dev}   # EVE_DEV_VAR: another world dir (a test run beside a live one)
PORT=${EVE_DEV_PORT:-26000}         # EVE_DEV_PORT: likewise
# The bots' loopback listener takes up to this many live connections. The
# server's default (LOCAL_CAP, 4096) covers the dedicated host; /bots spawns and bench
# runs go past it (the owner hit it at 125 bots, 2026-10-28).
BOT_CAP=${EVE_DEV_BOT_CAP:-2000}
WORLD=$VAR/world.dat
STOP_FILE=$WORLD.stop
LOGS=$REPO/logs
TOKENS=${EVE_DEV_BOT_TOKENS:-$HOME/.config/eve-offline/dev-bots.tsv}
HOST_IP=${EVE_JAIL_HOST_IP:-10.137.0.1}
JAIL_HELPER=/usr/local/sbin/eve-offline-jail
# shellcheck source=tools/stop-server.sh
source "$REPO/tools/stop-server.sh"

die()  { echo "dev-world.sh: $*" >&2; exit 1; }
deny() { echo "dev-world.sh: $*" >&2; exit 2; }
[[ $EUID -ne 0 ]] || deny "do not run this as root"

ROSTER=world50 HOME_SYSTEM='' CLIENT=0 BUILD=1 PROFILE=release SECS=0 BRAWL=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --home)     [[ $# -ge 2 ]] || deny "--home needs a system"; HOME_SYSTEM=$2; shift ;;
        --client)   CLIENT=1 ;;
        --no-build) BUILD=0 ;;
        --brawl)    BRAWL=1 ;;
        --debug)    PROFILE=debug ;;
        --secs)     [[ ${2:-} =~ ^[0-9]+$ ]] || deny "--secs needs a number"; SECS=$2; shift ;;
        -h|--help)  sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
        -*)         deny "unknown argument $1 (see --help)" ;;
        *)          ROSTER=$1 ;;
    esac
    shift
done
if [[ $ROSTER == none ]]; then
    ROSTER_FILE=''
elif [[ -f $ROSTER ]]; then
    ROSTER_FILE=$(cd "$(dirname "$ROSTER")" && pwd)/$(basename "$ROSTER")
elif [[ -f $REPO/content/bots/dev/$ROSTER.tsv ]]; then
    ROSTER_FILE=$REPO/content/bots/dev/$ROSTER.tsv
else
    deny "no roster '$ROSTER' (a file, or one of: $(cd "$REPO/content/bots/dev" && ls *.tsv | sed 's/\.tsv$//' | tr '\n' ' '))"
fi
[[ -f $REPO/out/content/MANIFEST.tsv ]] || die "no out/content; run tools/serve.sh or tools/up.sh once to extract it"

# The bots' names: <group>1..<count> for each roster row (eve_bot::dev).
NAMES=()
if [[ -n $ROSTER_FILE ]]; then
    mapfile -t NAMES < <(awk -F'\t' '/^#/ || NF < 6 { next } { for (i = 1; i <= $4; i++) print $1 i }' "$ROSTER_FILE")
    ((${#NAMES[@]} > 0)) || die "$ROSTER_FILE has no rows"
fi

if ((BUILD)); then
    echo "== building $SRV and eve-bots ($PROFILE, in the container)"
    flags=(--locked -p "$SRV" --bin "$SRV" -p eve-bot --bin eve-bots)
    [[ $PROFILE == release ]] && flags+=(--release)
    "$REPO/tools/dev.sh" cargo build "${flags[@]}"
fi
BIN=$REPO/server/target/$PROFILE
[[ -x $BIN/$SRV && -x $BIN/eve-bots ]] || die "no binaries in $BIN; run without --no-build"

mkdir -p "$VAR" "$LOGS"
exec 9>"$VAR/dev-world.lock"
flock -n 9 || die "another dev-world.sh holds $VAR/dev-world.lock"
DATA=(--content "$REPO/out/content" --authored "$REPO/content")

# 1. The bots' accounts, on the dev world (a one-shot server, --dev so the
#    marker rule is met; then gm and the bot mark, offline).
if ((${#NAMES[@]} > 0)); then
    echo "== bot accounts (${#NAMES[@]} bots)"
    bash "$REPO/tools/bot-accounts.sh" --bin "$BIN" --world "$WORLD" --tokens "$TOKENS" \
        --logs "$LOGS" --wait 900 "${NAMES[@]}" -- --dev "${DATA[@]}" >"$LOGS/dev-world-accounts.log" 2>&1 \
        || { tail -n 20 "$LOGS/dev-world-accounts.log" >&2; die "bot accounts failed (logs/dev-world-accounts.log)"; }
fi

# 2. The jail, for a local client.
JAIL=0
jail_down() { ((JAIL)) && sudo -n "$JAIL_HELPER" down >/dev/null 2>&1 || true; }
if ((CLIENT)); then
    sudo -n "$JAIL_HELPER" status >/dev/null 2>&1 || die "--client needs the jail helper ($JAIL_HELPER) runnable without a password; see docs/guides/jail.md"
    sudo -n "$JAIL_HELPER" down >/dev/null 2>&1 || true
    JAIL=1
    sudo -n "$JAIL_HELPER" up && sudo -n "$JAIL_HELPER" verify || { jail_down; die "the jail did not come up"; }
    sudo -n "$JAIL_HELPER" dns >"$LOGS/eve-dns.log" 2>&1 || { jail_down; die "eve-dns did not start"; }
fi

# 3. The server and the bots.
EVE_GIT_SHA=$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo unknown); export EVE_GIT_SHA
# The human client's traffic is recorded (one .evecap per connection in
# $VAR/capture, read with `evecap FILE`), the bots' loopback is not.
ARGS=(--dev --world "$WORLD" "${DATA[@]}" --world-accounts --capture "$VAR/capture")
if [[ $PORT == 26000 ]]; then
    ARGS+=(--chat --bind "127.0.0.1:$PORT,signup=on,capture=off,cap=$BOT_CAP")
else  # a test run beside a live one: its own chat port too
    ARGS+=(--bind "127.0.0.1:$PORT,signup=on,capture=off,cap=$BOT_CAP,chat=127.0.0.1:$((PORT + 1))")
fi
((CLIENT)) && ARGS+=(--bind "$HOST_IP:$PORT,signup=on")
# The client's public-gateway requests (cosmetics, mercenary dens, ...) get an
# instant answer instead of waiting out their own 3-10 s deadlines: the slow
# fitting window and right-click menu (docs/lanes/ui-lag.md). 8443 is fixed
# (EVE_PUBLIC_GATEWAY_ADDRESS), so only the main world takes it.
[[ $PORT == 26000 ]] && ARGS+=(--gateway)
# TT-A: per-turn scheduler sampling for the [turn-slow] lines (a pair of reads
# of /proc/thread-self/schedstat per payload turn).
ARGS+=(--turn-sched)
# Admin web (--admin): EVE_ADMIN_ADDR=127.0.0.1:8090 turns it on; off by default.
# The token is $VAR/admin.token (the control directory is the world's).
[[ -n ${EVE_ADMIN_ADDR:-} ]] && ARGS+=(--admin "$EVE_ADMIN_ADDR")
SPID='' BPID='' CPID=''
cleanup() {
    trap - EXIT INT TERM
    [[ -n $BPID ]] && kill "$BPID" 2>/dev/null || true
    [[ -n $CPID ]] && kill "$CPID" 2>/dev/null || true
    [[ -n $SPID ]] && { stop_server "$SPID" "$STOP_FILE" || true; wait "$SPID" 2>/dev/null || true; }
    jail_down
    echo "== stopped; the dev world is kept in ${WORLD#"$REPO"/} (rm -r var/dev to wipe it)"
}
trap cleanup EXIT INT TERM
"$BIN/$SRV" "${ARGS[@]}" >"$LOGS/dev-world-server.log" 2>&1 &
SPID=$!
for _ in $(seq 180); do
    grep -q "listening" "$LOGS/dev-world-server.log" 2>/dev/null && break
    kill -0 "$SPID" 2>/dev/null || { tail -n 20 "$LOGS/dev-world-server.log" >&2; die "the server died on startup"; }
    sleep 1
done
BARGS=(--connect 127.0.0.1:$PORT --tokens "$TOKENS" --content "$REPO/out/content"
       --society-pvp --chat on)
[[ -n $ROSTER_FILE ]] && BARGS+=(--roster "$ROSTER_FILE")
[[ -n $HOME_SYSTEM && -n $ROSTER_FILE ]] && BARGS+=(--roster-home "$HOME_SYSTEM")
if ((BRAWL)); then
    BARGS+=(--brawl)   # BRAWL: a fight to the end; nothing shed, so the bench keeps its load
else
    BARGS+=(--bubble)  # BUBBLE: bots gather where a human is; shed under tick load (docs/lanes/bubble.md)
fi
BARGS+=(--control "$VAR")  # BOT-CTL: GM /bots from the server's control link (bot-control.* in the world dir)
((SECS > 0)) && BARGS+=(--run "$SECS")
# Lower priority: a 200-bot gang decoding the same fan-out took the CPUs the
# server's lock holders needed (docs/lanes/fight-stall.md); exec keeps the pid.
nice -n 10 "$BIN/eve-bots" "${BARGS[@]}" >"$LOGS/dev-world-bots.log" 2>&1 &
BPID=$!
if ((CLIENT)); then
    "$REPO/tools/run-client.sh" & CPID=$!
fi

if ((CLIENT)); then
    LOGIN="your client was launched; it logs in with the account from tools/account.sh login"
else
    LOGIN="not launched (no --client); the server listens on 127.0.0.1:$PORT, signup on"
fi
cat <<MSG

== DEV WORLD UP: ${#NAMES[@]} bots (${ROSTER_FILE:+${ROSTER_FILE#"$REPO"/}}${ROSTER_FILE:-no roster})${HOME_SYSTEM:+ around $HOME_SYSTEM}
   client login: $LOGIN
                 (a new name signs up; every character here has 1,000,000,000 ISK)
   market:       every station sells every item at 100 ISK (10,000,000 a lot, never runs out)
   in the client console: /devskills all5, /devhome <system>, /devcorp <ticker>
   logs:         logs/dev-world-server.log, logs/dev-world-bots.log
   capture:      var/dev/capture/*.evecap (your client only; tools: eve-server/src/bin/evecap.rs)
   Ctrl-C stops everything; the world is saved first.
MSG
if ((SECS > 0)); then wait "$BPID" || true; else wait "$SPID" || true; fi
