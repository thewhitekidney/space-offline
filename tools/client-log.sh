#!/usr/bin/env bash
# Show the client's own log -- the one with the Python tracebacks in it.
#
#   tools/client-log.sh            # follow the newest log
#   tools/client-log.sh --list     # list the logs, newest first
#   tools/client-log.sh --errors   # exceptions AND tracebacks, newest log
#
# The client writes this because tools/run-client.sh passes
# /fileLogDirectory=C:\eve-logs; see the comment there for why. A new file is
# started per client process (and rotated hourly), named client-<stamp>-<pid>.log.
#
# This is the log to read when the client misbehaves. The Gamelog under
# Documents/EVE/logs only has user-facing message-box text; the tracebacks are
# here.
set -euo pipefail
: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
LOGDIR="$HOME/.local/share/eve-offline/prefix/pfx/drive_c/eve-logs"

[[ -d "$LOGDIR" ]] || { echo "no client log directory at $LOGDIR" >&2
                        echo "launch once with tools/run-client.sh to create it" >&2; exit 2; }

mapfile -t logs < <(ls -1t "$LOGDIR"/*.log 2>/dev/null || true)
[[ ${#logs[@]} -gt 0 ]] || { echo "no logs yet in $LOGDIR" >&2; exit 2; }

case "${1:-}" in
    --list) ls -lt "$LOGDIR"/*.log ;;
    --errors)
        # The client brackets each one with EXCEPTION #n ... EXCEPTION END,
        # so the whole traceback comes out as one block rather than a grep hit.
        #
        # STACKTRACE blocks matter just as much and are a DIFFERENT marker.
        # `log.LogTraceback` writes those -- a traceback with no exception --
        # and for a while this script dropped every one of them. That is how
        # a `_MakeShipActive` traceback sat unread in the log while the
        # numbering jumped from EXCEPTION #3 to #5 and nothing said why.
        # If you ever see a gap in the EXCEPTION numbers, the missing one is
        # a STACKTRACE.
        awk '/EXCEPTION #|STACKTRACE #/{p=1} p{print} /EXCEPTION END|STACKTRACE END/{p=0}' \
            "${logs[0]}"
        ;;
    "") echo "== ${logs[0]}" >&2; tail -f -n +1 "${logs[0]}" ;;
    *)  echo "usage: tools/client-log.sh [--list|--errors]" >&2; exit 2 ;;
esac
