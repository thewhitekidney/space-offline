#!/usr/bin/env bash
# Regenerate server/crates/eve-marshal/src/string_table.rs from a client's
# blue.dll, in the analysis container (pefile + capstone), client mounted
# read-only.
#
#   tools/extract_string_table.sh            # rewrite string_table.rs
#   tools/extract_string_table.sh --print    # list the table, change nothing
#
# EVE_CLIENT selects the client; defaults to the tq-offline copy.
set -euo pipefail
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
IMAGE=eve-offline/analysis:latest
: "${EVE_CLIENT:=$HOME/.local/share/Steam/steamapps/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline}"
[[ -f "$EVE_CLIENT/bin64/blue.dll" ]] || { echo "no blue.dll under $EVE_CLIENT" >&2; exit 2; }
docker image inspect "$IMAGE" >/dev/null 2>&1 || { echo "run tools/analysis.sh --build first" >&2; exit 2; }

OUT=/out/string_table.rs
[[ "${1:-}" == "--print" ]] && OUT=""

exec docker run --rm --network none --user "$(id -u):$(id -g)" \
    -v "$REPO/tools:/tools:ro" \
    -v "$EVE_CLIENT:/client:ro" \
    -v "$REPO/server/crates/eve-marshal/src:/out:rw" \
    "$IMAGE" python3 -I /tools/extract_string_table.py /client/bin64/blue.dll $OUT "$@"
