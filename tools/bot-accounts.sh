#!/usr/bin/env bash
# Give bots GM accounts in a world (ACC2: --world-accounts has no catch-all).
#
#   tools/bot-accounts.sh --bin DIR --world FILE --tokens FILE --logs DIR \
#       [--port N] [--wait SECS] NAME... [-- SERVER_ARGS...]
#
# Called by tools/account.sh bots (host) and tools/soak.sh (its default)
# (inside tools/dev.sh's container); not usually run by hand. DIR holds
# eve-server and eve-bots. SERVER_ARGS (the content bundle, the authored
# tables) go to the one-shot server below, so a bot's character is placed by
# the same data the real server uses.
#
# For each NAME:
#   1. a line `NAME<TAB>NAME:PASSWORD` in the tokens file, the password 32
#      random hex digits, made once and kept (the file is mode 600, outside
#      every repo; eve-bots --tokens refuses anything else);
#   2. if the world has no account of that name: a one-shot eve-server on
#      127.0.0.1:PORT with ceiling=player and signup=on, the only listener,
#      and eve-bots logging in once (--gm-setup off), which signs each bot up
#      as a player with its character; the server is stopped as soon as every
#      row is in the world file (it saves after every turn that changed
#      something), so nothing but the bots ever reaches that listener;
#   3. every bot account that is not gm yet: eve-server --grant NAME gm,
#      offline, which is refused while any server holds FILE.lock;
#   4. every bot account not marked yet: eve-server --mark-bot NAME, offline
#      the same way (BOTMARK), so each of its characters is a bot to
#      liveness and the economy. The mark is a separate field: it never
#      raises a role, and no in-game command sets it.
#
# Why not a catch-all listener for bots: the three account modes are never
# mixed (docs/lanes/acc2.md), and with world accounts the select screen shows
# an account only its own characters, so a catch-all bot would see none. A
# bot is an account with role gm and the bot mark; the listener ceiling still decides
# what a session may do, so a bot that logs in on the friends' listener is a
# player there.
#
# Exit: 0 every NAME is a bot-marked gm account; 1 failed (the logs say why); 2 usage.
set -euo pipefail

BIN='' WORLD='' TOKENS='' LOGS='' PORT=26090 WAIT=300
NAMES=() SERVER_ARGS=()
usage() { sed -n '2,8p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2; exit 2; }
while [[ $# -gt 0 ]]; do
    case "$1" in
        --bin)    BIN=${2:?}; shift ;;
        --world)  WORLD=${2:?}; shift ;;
        --tokens) TOKENS=${2:?}; shift ;;
        --logs)   LOGS=${2:?}; shift ;;
        --port)   PORT=${2:?}; shift ;;
        --wait)   WAIT=${2:?}; shift ;;
        --)       shift; SERVER_ARGS=("$@"); break ;;
        -*)       usage ;;
        *)        NAMES+=("$1") ;;
    esac
    shift
done
[[ -n $BIN && -n $WORLD && -n $TOKENS && -n $LOGS && ${#NAMES[@]} -gt 0 ]] || usage
[[ $PORT =~ ^[0-9]+$ && $WAIT =~ ^[0-9]+$ ]] || usage
SERVER=$BIN/${EVE_SERVER_BIN:-eve-offline-server} BOTS=$BIN/eve-bots
for b in "$SERVER" "$BOTS"; do [[ -x $b ]] || { echo "bot-accounts.sh: no $b" >&2; exit 1; }; done
say() { echo "== bot accounts: $*"; }

# 1. The tokens file. Lines are appended, never rewritten, so a bot keeps its
# password across runs; a name is never its own token (ruling 2).
( umask 077 && mkdir -p -- "$(dirname -- "$TOKENS")" && touch -- "$TOKENS" )
chmod 600 -- "$TOKENS"
has_token() { awk -v n="$1" '!/^#/ { t = $NF; sub(/[ \t]+[^ \t]+$/, ""); if ($0 == n) f = 1 } END { exit !f }' "$TOKENS"; }
made=0
for n in "${NAMES[@]}"; do
    has_token "$n" && continue
    pw=$(od -An -tx1 -N16 /dev/urandom | tr -d ' \n')
    [[ ${#pw} -eq 32 ]] || { echo "bot-accounts.sh: /dev/urandom gave no password" >&2; exit 1; }
    printf '%s\t%s:%s\n' "$n" "$n" "$pw" >>"$TOKENS"
    made=$((made + 1))
done
(( made == 0 )) || say "$made new password(s) in $TOKENS"

# The world's account rows, `lowercased-name<TAB>role<TAB>bot` (store.rs:
# `delta account 1 user_id username role created scheme iterations salt hash
# [bot]`; a name holds no tab or %, so no unescaping; the third column is
# `bot` on a marked row, else empty). Never the salt or the hash.
accounts() {
    [[ -f $WORLD ]] || return 0
    awk -F'\t' '$1 == "delta" && $2 == "account" && $3 == 1 { print tolower($5) "\t" $6 "\t" $12 }' "$WORLD"
}
role_of() { accounts | awk -F'\t' -v n="${1,,}" '$1 == n { print $2; exit }'; }
marked() { accounts | awk -F'\t' -v n="${1,,}" '$1 == n && $3 == "bot" { f = 1 } END { exit !f }'; }
missing() {
    local n have
    have=$(accounts)
    for n in "${NAMES[@]}"; do
        awk -F'\t' -v n="${n,,}" '$1 == n { f = 1 } END { exit !f }' <<<"$have" || printf '%s\n' "$n"
    done
}

# 2. Sign-up, for the names the world does not have.
mapfile -t new < <(missing)
if ((${#new[@]})); then
    mkdir -p "$LOGS"
    slog=$LOGS/bot-accounts-server.log blog=$LOGS/bot-accounts-bots.log
    say "signing up ${#new[@]} (${new[*]}) on 127.0.0.1:$PORT (ceiling player); logs ${slog}, ${blog}"
    "$SERVER" --world "$WORLD" --world-accounts \
        --bind "127.0.0.1:$PORT,ceiling=player,signup=on,botnames=on,cap=4096" --status "$LOGS/bot-accounts-status.tsv" \
        "${SERVER_ARGS[@]}" >"$slog" 2>&1 &
    spid=$!
    bargs=()
    for n in "${new[@]}"; do bargs+=(--name "$n"); done
    "$BOTS" --connect "127.0.0.1:$PORT" "${bargs[@]}" --tokens "$TOKENS" \
        --gm-setup off --pace fast --chat off --run "$WAIT" >"$blog" 2>&1 &
    bpid=$!
    stop() { kill "$bpid" "$spid" 2>/dev/null || true; wait "$bpid" 2>/dev/null || true; wait "$spid" 2>/dev/null || true; }
    trap stop EXIT
    ok=0
    for ((i = 0; i < WAIT; i += 2)); do
        [[ -z $(missing) ]] && { ok=1; break; }
        kill -0 "$spid" 2>/dev/null || break
        sleep 2
    done
    stop
    trap - EXIT
    if ((!ok)); then
        echo "bot-accounts.sh: not signed up after ${i}s: $(missing | tr '\n' ' ')" >&2
        tail -n 5 "$slog" | sed 's/^/   server: /' >&2
        grep -i 'refused\|error' "$blog" | tail -n 5 | sed 's/^/   bots:   /' >&2 || true
        echo "   A name already held by a character from catch-all play cannot sign up (docs/lanes/acc2.md, Not done)." >&2
        exit 1
    fi
    say "signed up after ~${i}s"
fi

# 3. Grant gm, offline (the one-shot server is gone; a running one refuses).
for n in "${NAMES[@]}"; do
    [[ $(role_of "$n") == gm ]] && continue
    "$SERVER" --world "$WORLD" --grant "$n" gm >/dev/null \
        || { echo "bot-accounts.sh: --grant $n gm failed (is a server running with $WORLD?)" >&2; exit 1; }
done

# 4. The bot mark, offline, the same way.
for n in "${NAMES[@]}"; do
    marked "$n" && continue
    "$SERVER" --world "$WORLD" --mark-bot "$n" >/dev/null \
        || { echo "bot-accounts.sh: --mark-bot $n failed (is a server running with $WORLD?)" >&2; exit 1; }
done
say "${#NAMES[@]} bot(s) are bot-marked gm accounts in $WORLD"
