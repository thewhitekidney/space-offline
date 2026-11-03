#!/usr/bin/env bash
# Everything a failed run leaves behind, in one place.
#
#   tools/evidence.sh              # the newest run
#   tools/evidence.sh --calls      # ... and the decoded wire capture too
#   tools/evidence.sh --watch      # re-print whenever either log grows
#
# WHY THIS EXISTS, AND WHY IT IS NOT A DAEMON.
#
# For several debugging rounds the logs were being read by the user and pasted
# into the conversation, and the obvious fix looked like a log-shipping
# service in a container. It is not needed: every artifact below is a plain
# file on this machine, owned by the launching user, and an agent with shell
# access can read all of them directly. A daemon would add a process, an
# image and a port to re-expose files that were never hidden.
#
# What was actually missing was knowing WHERE to look and in WHICH ORDER.
# That is a script, not a service.
#
# The four instruments, in the order they are worth reading --- see
# docs/guides/debugging-the-client.md section 3 for what each one proves:
#
#   1. [svc] UNANSWERED in our server log      -> a call we never answered,
#                                                 which HANGS the client
#   2. EXCEPTION / STACKTRACE in the client log -> we answered with the wrong
#                                                 shape, and it said so
#   3. service::method args=... retval=...      -> the client's own DECODED
#                                                 view of what we sent, which
#                                                 is the difference between
#                                                 "it did not complain" and
#                                                 "it read what we meant"
#   4. LoadView called on view <name>           -> how far the UI actually got
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
CLIENT_LOGS="$HOME/.local/share/eve-offline/prefix/pfx/drive_c/eve-logs"
SERVER_LOG="$REPO/logs/eve-server.log"
CAPTURES=${EVE_CAPTURE_DIR:-$REPO/captures}

WITH_CALLS=0
WATCH=0
for a in "$@"; do
    case "$a" in
        --calls) WITH_CALLS=1 ;;
        --watch) WATCH=1 ;;
        -h|--help) sed -n '2,6p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "evidence.sh: unknown argument $a" >&2; exit 2 ;;
    esac
done

# The newest log is NOT reliably the interesting one. A launch produces more
# than one client process -- a run that drew a full hangar left a 16164-line
# log beside a 7485-line one, and the stillborn second process had the newer
# mtime. Picking by mtime showed the character-selection screen and claimed
# the hangar had never loaded.
#
# So: among the logs touched in the same ten-minute window as the newest,
# take the LARGEST. The process that got furthest wrote the most.
newest_client_log() {
    local newest; newest=$(ls -1t "$CLIENT_LOGS"/*.log 2>/dev/null | head -1) || return 1
    [[ -n ${newest:-} ]] || return 1
    local cutoff=$(( $(stat -c %Y "$newest") - 600 ))
    # size<TAB>path, newest-window only, largest first.
    find "$CLIENT_LOGS" -maxdepth 1 -name '*.log' -newermt "@$cutoff" \
         -printf '%s\t%p\n' 2>/dev/null | sort -rn | head -1 | cut -f2
}

rule() { printf '\n== %s\n' "$1"; }

report() {
    local clog; clog=$(newest_client_log || true)

    # 1. Ours first. An unanswered call explains a silent client, and nothing
    #    in the client's own log will mention it -- that is the whole point of
    #    the rule in docs/guides/debugging-the-client.md section 1.
    rule "server: $SERVER_LOG"
    if [[ -f $SERVER_LOG ]]; then
        grep -a '^\[svc\]' "$SERVER_LOG" || echo "   (no [svc] lines yet)"
        if grep -qa 'UNANSWERED' "$SERVER_LOG"; then
            # A worklist, not a diagnosis. We answer None rather than hanging,
            # so the client walks past most of these; the ones that MATTER are
            # the ones that then show up as a traceback in the client's log
            # below. Read the two lists together.
            rule "UNANSWERED -- the worklist, in the order the client asked"
            grep -a 'UNANSWERED' "$SERVER_LOG"
        fi
    else
        echo "   no server log; bring the stack up with tools/up.sh"
    fi

    if [[ -z ${clog:-} || ! -f ${clog:-} ]]; then
        rule "client log"
        echo "   none in $CLIENT_LOGS -- launch once with tools/run-client.sh"
        return
    fi

    # 2. Then what the client complained about. Both markers: LogException
    #    writes EXCEPTION, LogTraceback writes STACKTRACE, and dropping the
    #    latter once hid a _MakeShipActive traceback for an entire round.
    rule "client: $clog"
    local n
    n=$(grep -ac 'EXCEPTION #\|STACKTRACE #' "$clog" || true)
    if [[ ${n:-0} -gt 0 ]]; then
        awk '/EXCEPTION #|STACKTRACE #/{p=1} p{print} /EXCEPTION END|STACKTRACE END/{p=0}' "$clog"
    else
        echo "   (no exceptions or tracebacks)"
    fi

    # 3. The client's decoded view of our replies. Worth more than the
    #    capture: it is what the client's unmarshaller MADE of our bytes.
    rule "calls as the client decoded them"
    grep -a 'retval=' "$clog" | cut -c1-400 || echo "   (none)"

    # 4. How far the UI got. No 'LoadView called on view hangar' means the
    #    station view never even started loading.
    rule "views the client loaded"
    grep -a 'LoadView called on view' "$clog" || echo "   (none -- the UI never got that far)"

    # The jail's own answer to "did it try to phone home during that run?".
    #
    # up.sh writes this on its way out, because the counters live in the
    # namespace's nftables rules and teardown destroys them. Reading them
    # live needs root, so prefer the saved copy and only point at the tool.
    # Every name the client tried to resolve. Our stub resolver is the only
    # one the jail has, so this is the complete list of services the client
    # wanted -- including the attempts that died at routing and so never
    # reached the firewall at all. Needs no root and outlives the jail.
    rule "phone-home attempts"
    "$REPO/tools/jail-drops.sh" 2>/dev/null | sed -n '/names the client/,/^$/p' \
        || echo "   (tools/jail-drops.sh failed)"

    if [[ $WITH_CALLS -eq 1 ]]; then
        rule "wire capture"
        local cap; cap=$(ls -1t "$CAPTURES"/*.evecap 2>/dev/null | head -1 || true)
        if [[ -n ${cap:-} ]]; then
            echo "   $cap"
            "$REPO/tools/dev.sh" cargo run -q -p eve-server --bin evecap -- "$cap" --calls
        else
            echo "   no captures in $CAPTURES"
        fi
    fi
}

if [[ $WATCH -eq 0 ]]; then
    report
    exit 0
fi

# --watch: cheap change detection on size alone. Both logs are append-only
# while a run is live (up.sh truncates the server log at startup, and the
# client opens a fresh file per process), so a size that stopped growing
# means the client has stopped talking -- which is itself the signal that
# something is blocked.
echo "watching; ctrl-c to stop" >&2
last=""
while true; do
    clog=$(newest_client_log || true)
    now="$(stat -c %s "$SERVER_LOG" 2>/dev/null || echo 0):${clog:-}:$(stat -c %s "${clog:-/nonexistent}" 2>/dev/null || echo 0)"
    if [[ $now != "$last" ]]; then
        printf '\n\n######## %s ########\n' "$(date -Is)"
        report
        last=$now
    fi
    sleep 2
done
