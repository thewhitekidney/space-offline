#!/usr/bin/env bash
# Bots beside you: the server, N player-like bots, and your client.
#
#   sudo -v && tools/bots.sh 3                  # you + Miner1..3, client launches
#   sudo -v && tools/bots.sh 2 miners 1 escort  # you + Miner1..2 + Escort1
#   tools/bots.sh 1 escort -- --chat            # an escort, and the server's chat on
#   tools/bots.sh --no-client 5                 # headless: server + 5 bots, ctrl-c stops
#   tools/bots.sh 3 -- --world out/world.txt    # everything after `--` goes to eve-server
#   tools/bots.sh --catch-all --me Alice 3   # the old dev mode (no accounts)
#   tools/bots.sh --pace fast 3                 # no human pacing (for watching a test run)
#   tools/bots.sh 2 miners 1 builder 1 sites    # the market builder and a site runner too
#   tools/bots.sh --agent ID 1 courier          # a courier hauling for agent ID
#   tools/bots.sh --hostile 1 hostile 2 escorts # an opponent: opt in with --hostile
#   tail -f logs/eve-bots.log
#
# Roles (a count, then the role; a bare count is miners):
#
#   miners   board a Merlin, mine, haul home; follow a fleet boss to his rocks
#   escorts  board a Rifter with two autocannons and EMP rounds, fly on the
#            fleet boss's wing and kill the NPCs shooting the fleet (or near
#            the boss), loot the wreck, come back; alone, they rat in the belt
#   sites    (Runner1..) a Rifter that runs the combat anomalies of its system
#   builder  (Builder1..) lists goods and buys minerals on the market (FX6)
#   courier  (Courier1..) takes courier missions from --agent ID and hauls
#            them; with no --agent it logs MISSING and idles
#   hostile  (Hostile1..) attacks players by content/bots/hostile.tsv, never
#            where it would be a crime; refused unless you pass --hostile
#
# Role words take a count before them, singular or plural (1 builder, 2
# builders). Each role's bots are added to the roster after you, in the
# order miners, escorts, sites, builders, couriers, hostiles, so adding a
# later role never renumbers an earlier one.
#
# NPC profiles (CP46): when the content bundle out/content/MANIFEST.tsv is
# there (tools/up.sh makes it), the bots get --content out/content, so
# escorts pick targets and retreat by the client's own NPC data and the
# builder knows which minerals to buy. Without it they play on without.
# The hostile rules are read from ./content/bots/hostile.tsv, so run this
# from the repo root to use an edited copy (else the compiled-in one).
#
# Invite a bot to your fleet and it follows you. With the server's chat on
# (`-- --chat`; the jail helper must have been re-installed once since chat
# landed) they say a line in fleet chat, or in Local outside a fleet;
# without it they stay silent and play on.
#
# With accounts the select screen shows only your own character. With
# --catch-all it shows everyone: pick YOURS (the first one, --me, default
# Capsuleer) -- two sessions flying one ship is legal and confusing.
#
# What it does (docs/lanes/bots.md §6):
#
#   1. builds in the container (nothing is installed on this machine);
#   2. checks that out/belt-rocks-30000001.tsv exists -- rocks are what the
#      bots mine -- and prints the command that makes it if not;
#   3. starts server/target/debug/eve-bots in the background, logging to
#      logs/eve-bots.log. The bots start before the server; each one retries
#      with a back-off until the server is listening, so there is no
#      ordering race and no sleep;
#   4. runs tools/up.sh with the roster you + every bot and --belt-rocks,
#      exactly as you would by hand;
#   5. stops the bots when up.sh exits -- unless --keep, which leaves the
#      bots up with everything else.
#
# The bots are a host process, like eve-server: std-only, zero dependencies.
# Each opens ONE TCP socket, to the server's IP literal, and resolves no
# names (docs/guides/jail.md) -- and, with chat on, one more to the chat port of
# the IP literal the server tells them. They give themselves their ship,
# modules and rounds with GM slash commands, once, docked, if their hangar
# lacks them; --gm-setup off makes them say MISSING instead.
#
# **Accounts (ACC2).** tools/up.sh runs --world-accounts on var/world.dat by
# default, where there is no catch-all: each bot is an account of its own
# name, role gm, its password in ~/.config/eve-offline/bots.tsv (mode 600,
# made for you; EVE_BOT_TOKENS=FILE picks another). Before the server starts,
# `tools/account.sh bots` signs up the bots the world does not have yet (a
# one-shot server on 127.0.0.1:26090, ceiling player, sign-up on, the bots'
# only listener; a minute or two, once per new name) and grants them gm,
# offline. You log in with your own login file (tools/account.sh login);
# your character is your account's, so --me is for --catch-all only.
#
# --catch-all keeps the old dev mode: no accounts, a fresh universe unless
# `-- --world FILE`, the roster you + the bots, every bot sends `dev` and is
# the catch-all GM (or its line in EVE_BOT_TOKENS with `-- --accounts FILE`).
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
HOST_IP=${EVE_JAIL_HOST_IP:-10.137.0.1}
BELT_ROCKS=$REPO/out/belt-rocks-30000001.tsv
BOTS=$REPO/server/target/debug/eve-bots
LOG=$REPO/logs/eve-bots.log
CONTENT=${EVE_CONTENT:-$REPO/out/content}   # EVE_CONTENT: another build's bundle (tools/content-lib.sh)
[[ $CONTENT == /* ]] || CONTENT=$REPO/$CONTENT
# Each role: its eve-bots --loop name and its name prefix, in roster order.
ROLES=(mine escort sites builder courier hostile)
declare -A PREFIX_OF=([mine]=Miner [escort]=Escort [sites]=Runner [builder]=Builder
                      [courier]=Courier [hostile]=Hostile)
declare -A COUNT_OF=([mine]=0 [escort]=0 [sites]=0 [builder]=0 [courier]=0 [hostile]=0)

# The same rule as up.sh, checked here first so no bot is started for a run
# that up.sh is about to refuse.
if [[ $EUID -eq 0 ]]; then
    echo "bots.sh: do not run this under sudo. Run it as yourself: sudo -v && tools/bots.sh 3" >&2
    exit 1
fi

ME=Capsuleer
ME_SET=0
CATCH_ALL=0
HOSTILE=0
PENDING=""          # a count waiting for its role word
COUNTED=0           # any count given at all
BUILD=1
KEEP=0
UP_ARGS=()
BOT_ARGS=()
SERVER_EXTRA=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --me)           ME=${2:?--me needs a name}; ME_SET=1; shift ;;
        --catch-all)    CATCH_ALL=1; UP_ARGS+=(--catch-all) ;;
        --signup)       UP_ARGS+=(--signup) ;;
        --no-client)    UP_ARGS+=(--no-client) ;;
        --keep)         KEEP=1; UP_ARGS+=(--keep) ;;
        --no-build)     BUILD=0 ;;
        --pace)         BOT_ARGS+=(--pace "${2:?--pace needs human or fast}"); shift ;;
        --gm-setup)     BOT_ARGS+=(--gm-setup "${2:?--gm-setup needs on or off}"); shift ;;
        --accept-fleet) BOT_ARGS+=(--accept-fleet "${2:?--accept-fleet needs any, from:NAME or none}"); shift ;;
        --no-chat)      BOT_ARGS+=(--chat off) ;;
        --agent)        BOT_ARGS+=(--agent "${2:?--agent needs an agent id}"); shift ;;
        --hostile)      HOSTILE=1 ;;
        -h|--help)      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d'; exit 0 ;;
        --)             shift; SERVER_EXTRA=("$@"); break ;;
        miner|miners|escort|escorts|site|sites|builder|builders|courier|couriers|hostile|hostiles)
            [[ -n $PENDING ]] || { echo "bots.sh: '$1' needs a count before it (2 $1)" >&2; exit 2; }
            case "$1" in
                miner*)   role=mine ;;
                escort*)  role=escort ;;
                site*)    role=sites ;;
                builder*) role=builder ;;
                courier*) role=courier ;;
                hostile*) role=hostile ;;
            esac
            COUNT_OF[$role]=$((COUNT_OF[$role] + PENDING)); PENDING="" ;;
        ''|*[!0-9]*)    echo "bots.sh: unknown argument $1 (see --help)" >&2; exit 2 ;;
        *)
            # A bare count is miners, as before roles existed.
            [[ -z $PENDING ]] || COUNT_OF[mine]=$((COUNT_OF[mine] + PENDING))
            PENDING=$1; COUNTED=1 ;;
    esac
    shift
done
[[ -z $PENDING ]] || COUNT_OF[mine]=$((COUNT_OF[mine] + PENDING))
if (( ! COUNTED )); then
    COUNT_OF[mine]=3
fi
MINERS=${COUNT_OF[mine]}

TOTAL=0
for role in "${ROLES[@]}"; do
    TOTAL=$((TOTAL + COUNT_OF[$role]))
    if [[ $ME == "${PREFIX_OF[$role]}"[0-9]* ]]; then
        echo "bots.sh: --me $ME looks like a bot's name; pick another" >&2
        exit 2
    fi
done
if (( TOTAL < 1 )); then
    echo "bots.sh: at least one bot" >&2
    exit 2
fi
# The opt-in (CP56): eve-bots refuses `--loop hostile` without `--hostile`
# too, but only into its log, after the server is up.
if (( COUNT_OF[hostile] > 0 && ! HOSTILE )); then
    echo "bots.sh: a hostile bot attacks players; opt in with --hostile (tools/bots.sh --hostile 1 hostile)" >&2
    exit 2
fi
(( HOSTILE )) && BOT_ARGS+=(--hostile)

if (( MINERS == 0 )) && [[ ! -s $BELT_ROCKS ]]; then
    echo "bots.sh: no $BELT_ROCKS -- the bots will play, but there is no belt to mine or rat in" >&2
elif [[ ! -s $BELT_ROCKS ]]; then
    cat >&2 <<MSG
bots.sh: $BELT_ROCKS is missing, and without rocks the bots have nothing to
mine. Make it from your client install (once; out/ is gitignored):

    tools/analysis.sh python3 /work/tools/asteroids.py \\
        --client /client --resfiles /client-root/ResFiles \\
        --system 30000001 --out /work/out/belt-rocks-30000001.tsv

(docs/lanes/asteroids.md, the P3.1 park note)
MSG
    exit 1
fi

if [[ $BUILD -eq 1 ]]; then
    echo "== building (in the container; nothing is installed on this machine)"
    "$REPO/tools/dev.sh" cargo build --locked --workspace --bins
fi
[[ -x $BOTS ]] || { echo "bots.sh: $BOTS missing; run without --no-build" >&2; exit 1; }

# **You first**, then the bots, always in this order: character ids come
# from the roster's order, so a --world file keeps meaning the same people
# only while the order holds (docs/lanes/bots.md §5, risk 5).
# Each role after the ones before it in ROLES (miners, then escorts, ...),
# so adding a later role never renumbers an earlier one.
ROSTER=(--character "$ME")
BOT_NAMES=()
BOT_GROUPS=()
SUMMARY=()
for role in "${ROLES[@]}"; do
    n=${COUNT_OF[$role]}
    (( n > 0 )) || continue
    p=${PREFIX_OF[$role]}
    for ((i = 1; i <= n; i++)); do
        ROSTER+=(--character "$p$i,bot")
        BOT_NAMES+=("$p$i")
    done
    BOT_GROUPS+=(--loop "$role" --prefix "$p" --count "$n")
    SUMMARY+=("$n $role")
done
if (( CATCH_ALL )); then
    [[ -n ${EVE_BOT_TOKENS:-} ]] && BOT_ARGS+=(--tokens "$EVE_BOT_TOKENS")
else
    # World accounts: no roster (each account brings its character), and
    # every bot a gm account before the server starts.
    (( ! ME_SET )) || { echo "bots.sh: --me is for --catch-all; with accounts you are your login (tools/account.sh whoami)" >&2; exit 2; }
    ROSTER=()
    world=$REPO/var/world.dat
    for ((k = 0; k < ${#SERVER_EXTRA[@]}; k++)); do
        [[ ${SERVER_EXTRA[k]} == --world ]] && world=${SERVER_EXTRA[k + 1]:-$world}
    done
    tokens=${EVE_BOT_TOKENS:-$HOME/.config/eve-offline/bots.tsv}
    EVE_BOT_TOKENS=$tokens bash "$REPO/tools/account.sh" bots --no-build --world "$world" "${BOT_NAMES[@]}"
    BOT_ARGS+=(--tokens "$tokens")
fi
# The content bundle, read by the host process at this host path (CP46).
# up.sh re-extracts a stale one after the bots have started; they read it
# once at startup, which a fresh boot's extraction can race, so the bots
# only get it when it is already there.
[[ -s $CONTENT/MANIFEST.tsv ]] && BOT_ARGS+=(--content "$CONTENT")
ROCKS_ARGS=()
[[ -s $BELT_ROCKS ]] && ROCKS_ARGS=(--belt-rocks "$BELT_ROCKS")

mkdir -p "$REPO/logs"
echo "== $(IFS=,; echo "${SUMMARY[*]}" | sed 's/,/, /g') -> $HOST_IP:26000 (log: $LOG)"
"$BOTS" --connect "$HOST_IP:26000" "${BOT_GROUPS[@]}" \
    "${BOT_ARGS[@]}" >"$LOG" 2>&1 &
BOTS_PID=$!

stop_bots() {
    local code=$?
    if [[ $KEEP -eq 1 ]]; then
        echo "   bots left up (--keep): stop them with: kill $BOTS_PID"
    else
        kill "$BOTS_PID" 2>/dev/null || true
        wait "$BOTS_PID" 2>/dev/null || true
        echo "   bots stopped; their log: $LOG"
    fi
    exit $code
}
trap stop_bots EXIT INT TERM

# Not `exec`: the trap above has to outlive up.sh to stop the bots.
"$REPO/tools/up.sh" --no-build ${UP_ARGS[@]+"${UP_ARGS[@]}"} -- \
    ${ROSTER[@]+"${ROSTER[@]}"} ${ROCKS_ARGS[@]+"${ROCKS_ARGS[@]}"} ${SERVER_EXTRA[@]+"${SERVER_EXTRA[@]}"}
