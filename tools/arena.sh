#!/usr/bin/env bash
# The doctrine arena (DOC-9, R-DOCTRINE gate G8): the matrix's predicted
# counters fought out in the real combat engine, headless.
#
#   tools/arena.sh                          # 3 matchups x 10 seeds, 6 ships a side
#   tools/arena.sh --pairs 8 --seeds 20 --jobs 4
#   tools/arena.sh --a KEY --b KEY          # one matchup (b is the predicted winner)
#   tools/arena.sh --list                   # the catalog
#   tools/arena.sh --range off              # the matrix without its range term
#   tools/arena.sh --content DIR            # another bundle (default out/content)
#   tools/arena.sh --no-build               # use target/release/eve-arena as it is
#   tools/arena.sh --report                 # also print the gate report afterwards
#
# Every other flag goes to `eve-arena` (see its header: --seeds, --cap,
# --jobs, --sep-m, --limit-s, --trace). Each skirmish is a private in-process
# server on a loopback port: the container has no network at all, the bundle
# is mounted read-only, nothing is written but the log
# out/arena/arena-<time>.log, which `tools/doctrine-report.py` reads.
#
# The matchups are the matrix's own: for the most decisive doctrines A, the
# fleet B with the best score against A, at equal spend. The question the
# arena answers is whether the engine agrees (G8: the counter's median ISK
# kill ratio above 1.0). Run it on a quiet machine: it is real-time combat.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="eve-offline-build"
CONTENT="$REPO/out/content"
build=1
report=0
args=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --content) CONTENT="$(cd "$2" && pwd)"; shift 2 ;;
        --no-build) build=0; shift ;;
        --report) report=1; shift ;;
        *) args+=("$1"); shift ;;
    esac
done

[[ -d "$CONTENT/types" ]] || { echo "arena.sh: no content bundle at $CONTENT (tools/up.sh makes one)" >&2; exit 2; }
if [[ $build == 1 ]]; then
    "$REPO/tools/dev.sh" cargo build --locked --release -p eve-bot --bin eve-arena
fi
BIN="$REPO/server/target/release/eve-arena"
[[ -x "$BIN" ]] || { echo "arena.sh: $BIN is missing (run without --no-build)" >&2; exit 2; }

mkdir -p "$REPO/out/arena"
LOG="$REPO/out/arena/arena-$(date +%Y%m%d-%H%M%S).log"
: "${EVE_MAX_JOBS:=6}"
status=0
docker run --rm --init \
    --network none \
    --cpus "$EVE_MAX_JOBS" \
    --cap-drop ALL \
    --security-opt no-new-privileges \
    --read-only \
    --tmpfs /tmp:exec,size=1g \
    --user "$(id -u):$(id -g)" \
    --memory "${EVE_DEV_MEMORY:-8g}" \
    -v "$BIN:/eve-arena:ro" \
    -v "$CONTENT:/content:ro" \
    "$IMAGE" timeout --signal=KILL "${EVE_ARENA_TIMEOUT:-7200}" \
    /eve-arena --content /content ${args[@]+"${args[@]}"} 2>&1 | tee "$LOG" || status=$?
echo "arena.sh: log in $LOG" >&2
if [[ $report == 1 ]]; then
    "$REPO/tools/analysis.sh" python3 tools/doctrine-report.py --arena "${LOG#"$REPO"/}" || true
fi
exit "$status"
