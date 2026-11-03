#!/usr/bin/env bash
# Host a world that persists: the hosting entry point (tools/up.sh stays the
# debug one). Runs until Ctrl-C.
#
#   tools/serve.sh                     # loopback 127.0.0.1:26000, no client
#   tools/serve.sh --client            # also the jail and the owner's client
#   tools/serve.sh --debug             # a debug eve-server (release is the default, O14)
#   tools/serve.sh --no-build          # skip the container build step
#   tools/serve.sh --capture           # one .evecap per connection, captures/
#   tools/serve.sh --no-chat           # in-game chat off (on by default)
#   tools/serve.sh --friends           # the friends listener 127.0.0.1:26001
#                                      # (ceiling player, sign-up on, chat
#                                      # 127.0.0.1:5223), reached only over
#                                      # the tunnel (O18), and the public
#                                      # gateway 127.0.0.1:8443 (--gateway)
#                                      # EVE_FRIEND_ADVERTISE=ADDR: what the
#                                      # listener tells a client (default 10.137.0.1)
#   tools/serve.sh --bundle-frozen    # the content bundle as it stands: no stale check,
#                                      # no --check, so the host needs no client; the
#                                      # bundle comes by rsync from your dev box and
#                                      # never goes into a kit, a repo or a friend's hands
#   tools/serve.sh --no-owner-listener # with --friends, leave out your own remote GM
#                                      # listener (below)
#   tools/serve.sh --signup            # the first run: sign-up on YOUR
#                                      # listeners too, so your login can sign up
#   tools/serve.sh --accounts FILE     # the legacy token file instead of
#                                      # world accounts (no sign-up anywhere)
#   tools/serve.sh -- --status F       # everything after -- goes to eve-server
#
# **Your own remote GM listener.** With --friends there is a third listener,
# 127.0.0.1:26002 (ceiling gm, cap 2, chat 127.0.0.1:5224), so you can be GM
# from a client on another machine. Nothing forwards it but YOUR OWN sshd:
# tools/tunnel.sh never opens it to a friend's key (its PermitOpen stays the
# three friend ports), and friend-tunnel.sh --owner maps it. It is as exposed
# as 26000 is: loopback, reached by local users and by your ssh login.
#
# Accounts live in the world (--world-accounts, ACC2, O9/O10): a new name
# signs up as a player on a sign-up listener; make a GM with the server
# stopped, tools/account.sh grant NAME gm. Your client (--client) logs in
# from ~/.config/eve-offline/login (tools/account.sh login).
#
# The world is var/world.dat. At every start (restarts included) it is copied
# to var/backups/<utc>.dat first. A crashed server is restarted, at most 3
# times in 10 minutes; then serve.sh stops. Logs: logs/<utc>-eve-server.log,
# the newest 20 kept. Exit codes: 0 stopped, 1 failed, 2 refused usage.
#
# **Stopping is clean** (lane shutdown): Ctrl-C touches var/world.dat.stop,
# which eve-server polls; it hangs up every session, writes the world and
# exits 0 (SIGTERM only if it has not after EVE_STOP_TIMEOUT, default 30 s).
# A GM's in-game /shutdown ends it the same way, and serve.sh then stops
# rather than restarting it: only a non-zero exit is a crash.
# The A && B || C chains below mean "any step failed"; the log is ours, not root's.
# shellcheck disable=SC2015,SC2024,SC2012
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SRV=${EVE_SERVER_BIN:-eve-offline-server}   # the server binary; EVE_SERVER_BIN=eve-server for the plain core one
VAR=$REPO/var
WORLD=$VAR/world.dat
STOP_FILE=$WORLD.stop   # eve-server's default stop file for --world $WORLD
# shellcheck source=tools/stop-server.sh
source "$REPO/tools/stop-server.sh"
BACKUPS=$VAR/backups
LOGS=$REPO/logs
HOST_IP=${EVE_JAIL_HOST_IP:-10.137.0.1}
JAIL_HELPER=/usr/local/sbin/eve-offline-jail

# [A] BACKUPS_KEPT: per-start copies kept. Falsified if the owner needs a
# world older than the last 30 starts; store.rs keeps its own format backups.
BACKUPS_KEPT=30
LOGS_KEPT=20
# A bounded restart loop (plan-review L4): RESTART_MAX crashes inside
# RESTART_WINDOW seconds, then stop.
RESTART_MAX=3
RESTART_WINDOW=600
# [A] A server that dies this soon after the FIRST start is a configuration
# error, not a crash, and is not restarted. Falsified if a real crash ever
# lands this early and wants a restart.
STARTUP_GRACE=5
# What the friends' listener tells a client to dial. The jailed friend client
# (jail-relay) reaches the server at the jail host address, so that is the
# default; a client with no jail, which reaches the tunnel on loopback, needs
# EVE_FRIEND_ADVERTISE=127.0.0.1 (docs/lanes/friend-gateway.md).
FRIEND_ADVERTISE=${EVE_FRIEND_ADVERTISE:-$HOST_IP}
FRIENDS_LISTENER="127.0.0.1:26001,ceiling=player,cap=16,chat=127.0.0.1:5223,advertise=$FRIEND_ADVERTISE"
# The owner's remote GM listener (see the header): loopback only, never in tunnel.sh.
OWNER_LISTENER="127.0.0.1:26002,ceiling=gm,cap=2,chat=127.0.0.1:5224,advertise=$FRIEND_ADVERTISE"

# A chat ping, only if YOU put a webhook URL in that file on this box (H5);
# the URL is read inline, never printed. Without the file, or without curl, it
# is silent: a stopped server is also on stderr and in the log.
WEBHOOK_FILE=${EVE_WEBHOOK_FILE:-$HOME/.config/eve-offline/webhook}
ping() {
    [[ -r $WEBHOOK_FILE ]] && command -v curl >/dev/null || return 0
    # The URL is a secret: curl reads it from its config on stdin, so it is
    # never in curl's argv (ps shows argv to every local user).
    local url; url=$(tr -d '\r\n"\\' <"$WEBHOOK_FILE")
    [[ -n $url ]] || return 0
    printf 'url = "%s"\n' "$url" | curl -fsS -m 10 -K - -H 'Content-Type: application/json' \
        -d "{\"content\":\"serve.sh on $(hostname): $1\"}" >/dev/null 2>&1 || true
}
die()  { echo "serve.sh: $*" >&2; exit 1; }
deny() { echo "serve.sh: $*" >&2; exit 2; }

[[ $EUID -ne 0 ]] || deny "do not run this as root; run it as yourself"

PROFILE=release BUILD=1 CLIENT=0 CAPTURE=0 CHAT=1 FRIENDS=0 ACCOUNTS='' SIGNUP=0
FROZEN=0 OWNER_LISTEN=1
SERVER_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)  PROFILE=release ;;
        --debug)    PROFILE=debug ;;
        --no-build) BUILD=0 ;;
        --client)   CLIENT=1 ;;
        --capture)  CAPTURE=1 ;;
        --no-chat)  CHAT=0 ;;
        --friends)  FRIENDS=1 ;;
        --bundle-frozen) FROZEN=1 ;;
        --no-owner-listener) OWNER_LISTEN=0 ;;
        --accounts) [[ $# -ge 2 ]] || deny "--accounts needs a FILE"; ACCOUNTS=$2; shift ;;
        --signup)   SIGNUP=1 ;;
        -h|--help)  sed -n '2,50p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        --) shift; SERVER_ARGS=("$@"); break ;;
        *) deny "unknown argument $1 (see --help)" ;;
    esac
    shift
done

# Fail closed before anything is built or started (plan-review H3). Either
# world accounts (the default) or the legacy --accounts FILE; never the
# catch-all here, so no friend is ever the GM by default.
for a in ${SERVER_ARGS[@]+"${SERVER_ARGS[@]}"}; do
    case $a in
        --world|--world-accounts|--accounts|--user-id|--character|--signup)
            deny "$a after -- is decided by serve.sh itself (see --help)" ;;
    esac
done
if [[ -n $ACCOUNTS ]]; then
    (( ! SIGNUP )) || deny "--signup needs world accounts, not --accounts FILE"
    [[ -f $ACCOUNTS && -r $ACCOUNTS ]] || deny "--accounts $ACCOUNTS: no such readable file"
    ACCOUNTS=$(cd "$(dirname "$ACCOUNTS")" && pwd)/$(basename "$ACCOUNTS")
fi

# One serve.sh per world: two servers saving one file would interleave.
mkdir -p "$VAR" "$BACKUPS" "$LOGS"
exec 9>>"$VAR/serve.lock"   # >> : a plain > would change the mtime report_world compares
flock -n 9 || die "another serve.sh holds $VAR/serve.lock"

if (( BUILD )); then
    echo "== building eve-server ($PROFILE, in the container)"
    flags=(--locked -p "$SRV" --bin "$SRV")
    [[ $PROFILE == release ]] && flags+=(--release)
    "$REPO/tools/dev.sh" cargo build "${flags[@]}"
fi
SERVER=$REPO/server/target/$PROFILE/$SRV
[[ -x $SERVER ]] || die "$SERVER missing; run without --no-build"
echo "== eve-server build: $PROFILE"

# The accounts gate needs the server's half too: a build that cannot read the
# flag would run the friends listener as GMs (or refuse it at boot).
want=--world-accounts
[[ -n $ACCOUNTS ]] && want=--accounts
"$SERVER" --help 2>&1 | grep -q -- "$want" \
    || deny "this eve-server has no $want yet; refusing to serve without accounts"

# Your own client's login, checked before the boot (run-client.sh reads it).
if (( CLIENT )) && [[ -z $ACCOUNTS && -z ${EVE_SSO_TOKEN+set} ]]; then
    # shellcheck source=tools/login-lib.sh
    source "$REPO/tools/login-lib.sh"
    login=$(eve_login_path "$HOME")
    [[ -e $login ]] || eve_login_prompt "$login" || die "no login file; make it with tools/account.sh login"
    eve_login_token "$login" "$(id -u)" >/dev/null || exit 1
    echo "== your login: $(eve_login_user "$login")"
fi

# Your client's own Wine prefix (O19), checked before the content build: the
# jail refuses the online prefix 8500 only at its verify, minutes later.
# A launch-env left from before O19 fails that verify too, so it goes.
if (( CLIENT )); then
    # shellcheck source=tools/prefix-lib.sh
    source "$REPO/tools/prefix-lib.sh"
    [[ -d $(eve_offline_prefix "$HOME")/pfx ]] \
        || die "your client has no prefix of its own yet; make it once with: tools/offline-prefix.sh --link-resfiles"
    eve_drop_stale_launch_env "$REPO"
fi

# The content bundle: the same staleness rule as up.sh, then --check.
CONTENT=$REPO/out/content
MANIFEST=$CONTENT/MANIFEST.tsv
content_tool() {
    "$REPO/tools/analysis.sh" python3 -I /work/tools/content.py \
        --client /client --resfiles /client-root/ResFiles --out /work/out/content "$@"
}
content_stale() {
    [[ -s $MANIFEST ]] || { echo "no $MANIFEST"; return 0; }
    local want have rel sha
    want=$(python3 -I -c '
import importlib.util, sys
spec = importlib.util.spec_from_file_location("content", sys.argv[1])
mod = importlib.util.module_from_spec(spec); spec.loader.exec_module(mod)
print(mod.extractor_hash())' "$REPO/tools/content.py" 2>/dev/null || true)
    have=$(awk -F'\t' '$1=="extractor"{print $2; exit}' "$MANIFEST")
    [[ -n $want && $have == "$want" ]] || echo "the extractor changed or cannot be hashed"
    while IFS=$'\t' read -r rel sha; do
        [[ -f $REPO/$rel && $(sha256sum "$REPO/$rel" | cut -d' ' -f1) == "$sha" ]] \
            || echo "the authored input $rel changed"
    done < <(awk -F'\t' '$1=="input"{print $2 "\t" $3}' "$MANIFEST")
}
if (( FROZEN )); then
    # A host with no client: the bundle came by rsync
    # (docs/guides/hosting.md). Staleness needs python and --check needs the
    # client, so neither runs; the manifest must be there and is printed.
    [[ -s $MANIFEST ]] || die "--bundle-frozen: no $MANIFEST; rsync out/content/ from your dev box first"
    echo "== content bundle frozen: build $(awk -F'\t' '$1=="client_build"{print $2; exit}' "$MANIFEST"), data_hash $(awk -F'\t' '$1=="data_hash"{print $2; exit}' "$MANIFEST")"
else
    stale=$(content_stale)
    if [[ -n $stale ]]; then
        echo "== extracting the content bundle"; printf '   because: %s\n' "$stale"
        content_tool || die "tools/content.py failed; not serving a stale bundle"
    fi
    echo "== checking the content bundle (content.py --check)"
    content_tool --check >"$LOGS/content-check.log" 2>&1 \
    || { tail -n 20 "$LOGS/content-check.log" >&2; die "content.py --check failed (logs/content-check.log)"; }
fi

# The build's sha names store.rs's format backups (IP11); read at run time.
EVE_GIT_SHA=$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo unknown)
export EVE_GIT_SHA

# Sign-up: always on the friends' listener (ceiling player; only SSH key
# holders reach it, O18), on yours only with --signup. A sign-up is always a
# player (O10); the ceiling caps what any account may do on each listener.
OWN='' OWNER_BIND=$OWNER_LISTENER
# Your own listener too, or a remote owner could never make his account.
(( SIGNUP )) && { OWN=",signup=on"; OWNER_BIND+=",signup=on"; }
ARGS=(--world "$WORLD" --content "$CONTENT" --authored "$REPO/content"
      --bind "127.0.0.1:26000$OWN")
(( CLIENT )) && ARGS+=(--bind "$HOST_IP:26000$OWN")
if [[ -n $ACCOUNTS ]]; then
    (( FRIENDS )) && ARGS+=(--bind "$FRIENDS_LISTENER")
    (( FRIENDS && OWNER_LISTEN )) && ARGS+=(--bind "$OWNER_BIND")
    ARGS+=(--accounts "$ACCOUNTS")
else
    (( FRIENDS )) && ARGS+=(--bind "$FRIENDS_LISTENER,signup=on")
    (( FRIENDS && OWNER_LISTEN )) && ARGS+=(--bind "$OWNER_BIND")
    ARGS+=(--world-accounts)
fi
(( CHAT )) && ARGS+=(--chat)
# The public-gateway stub on 8443 beside each local listener; the tunnel
# forwards 127.0.0.1:8443 to the friends. Without it their gateway requests
# wait out their deadlines (slow fitting window, right-click menu).
(( FRIENDS )) && ARGS+=(--gateway)
if (( CAPTURE )); then mkdir -p "$REPO/captures"; ARGS+=(--capture "$REPO/captures"); fi
ARGS+=("${SERVER_ARGS[@]}")

# The jail, only for a local client. Only through the installed helper with
# no password: a hosting script must not sit at a sudo prompt.
JAIL=0
jail_down() { (( JAIL )) && sudo -n "$JAIL_HELPER" down >/dev/null 2>&1 || true; }
if (( CLIENT )); then
    sudo -n "$JAIL_HELPER" status >/dev/null 2>&1 \
        || die "--client needs the jail helper ($JAIL_HELPER) runnable without a password; see docs/guides/jail.md"
    echo "== jail"
    sudo -n "$JAIL_HELPER" down >/dev/null 2>&1 || true
    JAIL=1
    sudo -n "$JAIL_HELPER" up && sudo -n "$JAIL_HELPER" verify || { jail_down; die "the jail did not come up"; }
    sudo -n "$JAIL_HELPER" dns >"$LOGS/eve-dns.log" 2>&1 || { jail_down; die "eve-dns did not start"; }
fi

backup_world() {
    [[ -s $WORLD ]] || return 0
    local dst; dst=$BACKUPS/$(date -u +%Y%m%dT%H%M%SZ).dat
    [[ -e $dst ]] && dst=${dst%.dat}-$$-$RANDOM.dat
    cp -p "$WORLD" "$dst.tmp" && sync "$dst.tmp" && mv "$dst.tmp" "$dst" \
        || die "could not back up $WORLD; not starting over it"
    echo "== world backed up to ${dst#"$REPO"/}"
    sync "$BACKUPS" 2>/dev/null || true
    prune "$BACKUPS"/*.dat "$BACKUPS_KEPT"
}
# prune GLOB... N: delete all but the newest N files the glob matched.
prune() {
    local n=${!#} files=("${@:1:$#-1}")
    [[ -e ${files[0]} ]] || return 0
    ls -1t -- "${files[@]}" | tail -n +$((n + 1)) | xargs -r -d '\n' rm -f --
}
prune_logs() { prune "$LOGS"/*-eve-server.log "$LOGS_KEPT"; }

# The taint and downgrade state (store.rs diverts; plan-review M5).
report_world() {
    echo "== world state"
    if [[ -e $WORLD.tainted && $WORLD.tainted -nt $VAR/serve.lock ]]; then
        echo "   TAINTED: a panic poisoned the world this run. $WORLD was NOT overwritten;"
        echo "   the session is in $WORLD.tainted. Inspect it before keeping it."
    elif [[ -e $WORLD.downgrade && $WORLD.downgrade -nt $VAR/serve.lock ]]; then
        echo "   DOWNGRADE REFUSED: this build cannot save over $WORLD;"
        echo "   the session is in $WORLD.downgrade (or rerun with -- --accept-downgrade)."
    else
        echo "   clean: ${WORLD#"$REPO"/} $( [[ -e $WORLD ]] && stat -c '%s bytes' "$WORLD" || echo 'not written yet')"
    fi
}

STOPPING=0 PID='' CLIENT_PID=''
stop() {
    STOPPING=1
    [[ -n $PID ]] && { echo "== stopping: the world is written first"; stop_server "$PID" "$STOP_FILE" || true; }
}
on_exit() {
    local code=$?
    trap - EXIT INT TERM
    (( code == 0 )) || ping "serve.sh stopped (exit $code); friends are disconnected"
    [[ -n $PID ]] && { stop_server "$PID" "$STOP_FILE" || true; wait "$PID" 2>/dev/null || true; }
    [[ -n $CLIENT_PID ]] && kill "$CLIENT_PID" 2>/dev/null || true
    jail_down
    report_world
    exit "$code"
}
trap stop INT TERM
trap on_exit EXIT
touch "$VAR/serve.lock"   # this run's start; report_world compares against it

crashes=()   # epoch seconds of each crash inside the window
first=1
while :; do
    backup_world
    prune_logs
    LOG=$LOGS/$(date -u +%Y%m%dT%H%M%SZ)-eve-server.log
    echo "== eve-server on 127.0.0.1:26000$( (( CLIENT )) && echo " and $HOST_IP:26000")$( (( FRIENDS )) && echo ", friends 127.0.0.1:26001$( (( FRIENDS && OWNER_LISTEN )) && echo " and 26002")"); log ${LOG#"$REPO"/}"
    started=$(date +%s)
    "$SERVER" "${ARGS[@]}" >"$LOG" 2>&1 9>&- &
    PID=$!
    if (( first && CLIENT )); then
        for _ in $(seq 120); do
            timeout 1 bash -c "exec 3<>/dev/tcp/$HOST_IP/26000" 2>/dev/null && break
            kill -0 "$PID" 2>/dev/null || break; sleep 1
        done
        "$REPO/tools/run-client.sh" & CLIENT_PID=$!
    fi
    (( first )) && echo "serving; Ctrl-C to stop"
    # `wait` returns early when a trapped signal arrives; wait again for the exit.
    status=0; wait "$PID" || status=$?
    while kill -0 "$PID" 2>/dev/null; do status=0; wait "$PID" || status=$?; done
    PID=''
    (( STOPPING )) && { echo "== stopped"; exit 0; }
    # Exit 0 is the clean stop: a GM's /shutdown, or someone touching the
    # stop file. Not a crash; do not restart.
    (( status == 0 )) && { echo "== eve-server stopped cleanly (/shutdown or its stop file)"; exit 0; }
    now=$(date +%s)
    echo "serve.sh: eve-server exited $status after $((now - started))s; last lines:" >&2
    tail -n 5 "$LOG" | sed 's/^/   /' >&2
    (( first && now - started < STARTUP_GRACE )) && die "it died on startup; not restarting"
    first=0
    kept=()
    for t in "${crashes[@]}"; do (( now - t < RESTART_WINDOW )) && kept+=("$t"); done
    crashes=("${kept[@]}" "$now")
    (( ${#crashes[@]} <= RESTART_MAX )) \
        || die "$RESTART_MAX restarts inside $((RESTART_WINDOW / 60)) minutes; stopping"
    echo "== restart ${#crashes[@]} of $RESTART_MAX in this window"
done
