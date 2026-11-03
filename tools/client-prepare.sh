#!/usr/bin/env bash
# Run client-prepare.py against a client copy, in the analysis container with
# that copy mounted read-write. The client lives outside the repo, so it gets
# its own bind mount; nothing else is writable.
#
#   tools/client-prepare.sh [--check|--restore|--pack PACK] [--allow-real]
#
# EVE_CLIENT selects the client; it defaults to the tq-offline copy, never the
# live install.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
IMAGE=eve-offline/analysis:latest
: "${EVE_CLIENT:=$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline}"

[[ -f "$EVE_CLIENT/start.ini" ]] || { echo "client-prepare.sh: no client at $EVE_CLIENT" >&2; exit 2; }
docker image inspect "$IMAGE" >/dev/null 2>&1 || { echo "client-prepare.sh: run tools/analysis.sh --build first" >&2; exit 2; }

exec docker run --rm \
    --network none \
    --user "$(id -u):$(id -g)" \
    -v "$REPO/tools:/tools:ro" \
    -v "$EVE_CLIENT:/client:rw" \
    "$IMAGE" python3 -I /tools/client-prepare.py /client "$@"
