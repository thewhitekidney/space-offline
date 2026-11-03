#!/usr/bin/env bash
# Bots on a hosted world (F-HOST H2): the society bots beside the friends,
# on the world tools/serve.sh keeps. No client, no jail, no debug binaries,
# no 10.137.0.1 (tools/bots.sh is the desk version of this).
#
#   tools/host-bots.sh accounts [--miners N] [--escorts N] ...   # once, serve.sh STOPPED
#   tools/host-bots.sh run      [--miners N] [--escorts N] ...   # serve.sh UP; Ctrl-C stops
#   tools/host-bots.sh roster   [--miners N] ...                 # print the names and exit
#
# Options: --miners N (default 5), --escorts N (2), --builders N (0),
# --ratters N (0), --difficulty LEVEL, --no-build, --no-chat.
# Env: EVE_BOT_TOKENS (default ~/.config/eve-offline/bots.tsv, mode 600),
# EVE_BIN_DIR (default server/target/release), EVE_VAR_DIR, EVE_LOGS_DIR,
# EVE_CONTENT, EVE_BOTS_NICE (default 10).
#
# **accounts** signs the bots up as players and grants gm and the bot mark
# offline (tools/bot-accounts.sh), which the world's lock refuses while a
# server holds it: stop serve.sh first, run this, start serve.sh. Release
# binaries, built in the container (nothing is installed on this host).
#
# **run** starts the release eve-bots at 127.0.0.1:26000, your own listener,
# which is where bots log in (the friends' listener is ceiling=player and
# sign-up; a bot's gm account is a player there). It runs the society minds
# and the closed economy as tools/soak.sh does (--society --closed
# --sessions always), their minds in var/bots/state, one log per start in
# logs/, under `nice` so a lagging bot never costs a friend a tick. The
# roster for fifty bots is the owner's choice: this is a count per kind, and
# the same counts must be given to accounts and run, or a new name has no
# password. It refuses while no server holds var/world.dat.lock.
#
# Exit codes: 0 stopped, 1 failed, 2 refused usage.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VAR=${EVE_VAR_DIR:-$REPO/var}
LOGS=${EVE_LOGS_DIR:-$REPO/logs}
BIN=${EVE_BIN_DIR:-$REPO/server/target/release}
SRV=${EVE_SERVER_BIN:-eve-offline-server}
TOKENS=${EVE_BOT_TOKENS:-$HOME/.config/eve-offline/bots.tsv}
CONTENT=${EVE_CONTENT:-$REPO/out/content}
WORLD=$VAR/world.dat
NICE=${EVE_BOTS_NICE:-10}

die()  { echo "host-bots.sh: $*" >&2; exit 1; }
deny() { echo "host-bots.sh: $*" >&2; exit 2; }
[[ $EUID -ne 0 ]] || deny "do not run this as root; run it as yourself"

[[ $# -ge 1 ]] || deny "usage: host-bots.sh accounts|run|roster [options] (see --help)"
CMD=$1; shift
case $CMD in
    -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    accounts|run|roster) ;;
    *) deny "unknown command $CMD (see --help)" ;;
esac

MINERS=5 ESCORTS=2 BUILDERS=0 RATTERS=0 BUILD=1 CHAT=on DIFFICULTY=''
num() { [[ $2 =~ ^[0-9]+$ ]] || deny "$1 needs a number"; }
while (( $# )); do
    case $1 in
        --miners)     num "$1" "${2:-}"; MINERS=$2; shift ;;
        --escorts)    num "$1" "${2:-}"; ESCORTS=$2; shift ;;
        --builders)   num "$1" "${2:-}"; BUILDERS=$2; shift ;;
        --ratters)    num "$1" "${2:-}"; RATTERS=$2; shift ;;
        --difficulty) [[ -n ${2:-} ]] || deny "--difficulty needs a level"; DIFFICULTY=$2; shift ;;
        --no-build)   BUILD=0 ;;
        --no-chat)    CHAT=off ;;
        *) deny "unknown argument $1 (see --help)" ;;
    esac
    shift
done

# The names, the way soak.sh makes them (MinerN, EscortN, BuilderN, RatterN).
NAMES=() GROUPS_=()
for pair in Miner:$MINERS Escort:$ESCORTS Builder:$BUILDERS Ratter:$RATTERS; do
    p=${pair%%:*} n=${pair#*:}
    (( n > 0 )) || continue
    for ((i = 1; i <= n; i++)); do NAMES+=("$p$i"); done
    GROUPS_+=(--prefix "$p" --count "$n")
done
(( ${#NAMES[@]} > 0 )) || deny "no bots asked for"

if [[ $CMD == roster ]]; then printf '%s\n' "${NAMES[@]}"; exit 0; fi

# True while an eve-server holds the world's lock (serve.sh's server does for
# its whole life; account.sh's check_unlocked uses the same test).
server_up() { [[ -e $WORLD.lock ]] && ! flock -n "$WORLD.lock" true 2>/dev/null; }

build() {
    (( BUILD )) || return 0
    echo "== building release binaries (in the container)"
    local pkgs=(-p eve-bot --bin eve-bots)
    # `run` needs only the bots; `accounts` also needs the server (its grant tools).
    [[ $CMD == accounts ]] && pkgs=(-p "$SRV" -p eve-bot --bins)
    "$REPO/tools/dev.sh" cargo build --locked --release "${pkgs[@]}"
}
[[ $CONTENT == /* ]] || CONTENT=$REPO/$CONTENT
[[ -s $CONTENT/MANIFEST.tsv ]] || die "no content bundle $CONTENT (tools/serve.sh --bundle-frozen needs it too)"

# One host-bots.sh at a time, and wipe/restore refuse while one runs: the lock
# is held (not just checked) for the whole command. Its children inherit it,
# so bots that outlive a killed run still hold it.
mkdir -p "$VAR"
exec 7>>"$VAR/bots.lock"
flock -n 7 || die "another host-bots.sh holds $VAR/bots.lock (bots running, or accounts in progress)"

case $CMD in
    accounts)
        server_up && die "a server holds $WORLD.lock: stop serve.sh first (a grant under it would be lost)"
        build
        [[ -x $BIN/$SRV && -x $BIN/eve-bots ]] || die "no $SRV and eve-bots in $BIN; run without --no-build"
        mkdir -p "$LOGS" "$VAR"
        bash "$REPO/tools/bot-accounts.sh" --bin "$BIN" --world "$WORLD" --tokens "$TOKENS" \
            --logs "$LOGS" "${NAMES[@]}" -- --content "$CONTENT" --authored "$REPO/content"
        echo "== ${#NAMES[@]} bot accounts ready; start serve.sh, then: tools/host-bots.sh run"
        ;;
    run)
        server_up || die "no server holds $WORLD.lock: start tools/serve.sh first"
        build
        [[ -x $BIN/eve-bots ]] || die "no eve-bots in $BIN; run without --no-build"
        [[ -f $TOKENS ]] || die "no tokens file $TOKENS: run 'tools/host-bots.sh accounts' with the same counts"
        for n in "${NAMES[@]}"; do
            awk -F'\t' -v n="$n" '!/^#/ && $1 == n { f = 1 } END { exit !f }' "$TOKENS" \
                || die "$n has no password in $TOKENS: run 'tools/host-bots.sh accounts' with the same counts"
        done
        mkdir -p "$LOGS" "$VAR/bots/state"
        LOG=$LOGS/$(date -u +%Y%m%dT%H%M%SZ)-eve-bots.log
        args=(--connect 127.0.0.1:26000 "${GROUPS_[@]}"
              --society --closed --state "$VAR/bots/state" --sessions always
              --content "$CONTENT" --tokens "$TOKENS" --chat "$CHAT")
        [[ -n $DIFFICULTY ]] && args+=(--difficulty "$DIFFICULTY")
        echo "== ${#NAMES[@]} bots -> 127.0.0.1:26000 (nice $NICE); log ${LOG#"$REPO"/}; Ctrl-C stops them"
        nice -n "$NICE" "$BIN/eve-bots" "${args[@]}" >"$LOG" 2>&1 &
        PID=$!
        stop() { kill "$PID" 2>/dev/null || true; }
        trap stop INT TERM
        trap 'stop; wait "$PID" 2>/dev/null || true' EXIT
        status=0; wait "$PID" || status=$?
        while kill -0 "$PID" 2>/dev/null; do status=0; wait "$PID" || status=$?; done
        trap - EXIT
        echo "== bots stopped ($status); their minds are in ${VAR#"$REPO"/}/bots/state"
        # A signal-stop is a stop, not a failure.
        (( status == 0 || status == 143 || status == 130 )) || exit 1
        ;;
esac
