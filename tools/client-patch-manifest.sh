#!/usr/bin/env bash
# Run client-patch-manifest.py against a client copy, in the analysis container
# (which has pefile + capstone) with the copy mounted read-write.
#
#   tools/client-patch-manifest.sh [--check|--restore] [--allow-real]
#
# EVE_CLIENT selects the client; defaults to the tq-offline copy.
set -euo pipefail
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
IMAGE=eve-offline/analysis:latest
: "${EVE_CLIENT:=$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline}"
[[ -f "$EVE_CLIENT/bin64/blue.dll" ]] || { echo "no blue.dll under $EVE_CLIENT" >&2; exit 2; }
docker image inspect "$IMAGE" >/dev/null 2>&1 || { echo "run tools/analysis.sh --build first" >&2; exit 2; }
exec docker run --rm --network none --user "$(id -u):$(id -g)" \
    -v "$REPO/tools:/tools:ro" -v "$EVE_CLIENT:/client:rw" \
    "$IMAGE" python3 -I /tools/client-patch-manifest.py /client "$@"
