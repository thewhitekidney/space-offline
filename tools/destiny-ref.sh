#!/usr/bin/env bash
# Deploy the Destiny reference dumper into the client's log directory, and
# collect what it wrote.
#
#   tools/destiny-ref.sh deploy    # put the script where the client can see it
#   tools/destiny-ref.sh collect   # copy the blobs back into the repo tree
#   tools/destiny-ref.sh remove    # delete the script from the prefix
#
# Why a script for two `cp`s: the paths are the whole difficulty. The dumper
# goes into the Proton prefix and NOT into the client installation -- nothing
# under $EVE_CLIENT is touched, `manifest.dat` is not rewritten, and no
# `--restore` is needed afterwards. `C:\eve-logs` is the client's own log
# directory (`run-client.sh` passes `/fileLogDirectory=C:\eve-logs`), so it
# already exists and the client can already write to it.
#
# Once deployed, trigger it from inside the client -- Insider -> Tools ->
# Slash Console:
#
#     /execfile c:\eve-logs\destiny_dump.py
#
# `cmd_execfile` is a **client-side** command (`eve/devtools/script/svc_console.py`)
# gated on `session.role & ROLE_PROGRAMMER`, which our session mask sets
# (`roles::SANDBOX_DOCKED`, bit 51). It never reaches our dispatcher, so there
# is nothing to implement on this side.
#
# See docs/reference/destiny-reference-blob.md for what the blobs are for.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${EVE_PREFIX:=$HOME/.local/share/eve-offline/prefix/pfx/drive_c}"
LOGS="$EVE_PREFIX/eve-logs"
SCRIPT="$REPO/tools/client-overrides/destiny_dump.py"
OUT="$REPO/findings/destiny-ref"

case "${1:-}" in
deploy)
    [[ -d "$LOGS" ]] || { echo "destiny-ref: $LOGS does not exist; launch the client once first" >&2; exit 1; }
    cp "$SCRIPT" "$LOGS/destiny_dump.py"
    echo "deployed: $LOGS/destiny_dump.py"
    echo "in the client: Insider -> Tools -> Slash Console, then"
    echo "    /execfile c:\\eve-logs\\destiny_dump.py"
    ;;
collect)
    src="$LOGS/destiny-ref"
    [[ -d "$src" ]] || { echo "destiny-ref: nothing at $src -- has the dump run?" >&2; exit 1; }
    mkdir -p "$OUT"
    cp -a "$src/." "$OUT/"
    echo "collected into $OUT:"
    ls -la "$OUT"
    ;;
remove)
    rm -f "$LOGS/destiny_dump.py"
    echo "removed: $LOGS/destiny_dump.py"
    ;;
*)
    sed -n '2,27p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
