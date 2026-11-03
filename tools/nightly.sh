#!/usr/bin/env bash
# The nightly verification run for one commit (V4 T5, P6; IP35). It builds
# the commit, soaks it, runs every client-CI scenario against it, and writes
# what happened. Verification only: it NEVER pushes, commits, or edits a
# tracked file, and it never blocks a push. The coordinator or a /loop reads
# the summary.
#
#   tools/nightly.sh                    # HEAD
#   tools/nightly.sh a789726            # any commit
#   tools/nightly.sh HEAD --soak-secs 60 --no-client    # the short proof
#
#   --soak-secs N   soak length (default 300; soak.sh --secs)
#   --no-client     skip the client-CI step (each scenario row says so)
#   --rootless      pass --rootless to client-ci.sh. Opt-in until IP29's bar
#                   (5 consecutive green ballpark_smoke runs) is met; then it
#                   becomes the default here (docs/lanes/ip29.md).
#   --only NAME     run just this scenario (repeatable)
#   --keep          keep the exported tree after the run
#
# Steps, one summary row each, in order:
#   export     `git archive SHA` into $NIGHTLY_WORK/<sha> (default
#              $HOME/code/eve-offline-lanes/nightly, on disk), plus
#              a copy of the untracked inputs in out/ (the content bundle,
#              the celestial tables). The main tree is only read.
#   build      tools/dev.sh cargo build --locked --workspace --bins (debug,
#              what soak.sh and client-ci.sh's default path both run)
#   soak       tools/soak.sh --no-build --secs N
#   scenario:X tools/client-ci.sh run X --no-build, one row per scenario in
#              tools/client-ci/scenarios of the exported tree
#
# Each row is pass, fail or skip. A skip is a fact about this machine, not
# about the commit, and is never red:
#   - "skipped: helper stale" / "skipped: helper foreign": the installed jail
#     helper is not the one this tree would install, which client-ci.sh
#     refuses (IP15). Note that the helper is always FOREIGN for an exported
#     tree, because its REPO= line names the main checkout (V4 C2); only the
#     rootless path is checkout-independent.
#   - "skipped: no helper, no cached sudo": the helper path needs one or the
#     other and an unattended run has neither.
#   - "skipped: build failed" for every step after a red build.
# Every other client-ci.sh exit is red, including exit 3 (refused), so a
# stack that will not start is not hidden as a skip.
#
# Output, in the MAIN tree's out/ (gitignored): out/nightly/<sha>/
#   summary.tsv  step, status, exit, secs, note (one row per step)
#   report.txt   the one-screen report, also printed at the end
#   logs/        every step's output (and the soak's server/bot logs)
#
# Exit status: 0 every row passed or skipped; 1 at least one red row;
# 2 usage (an unknown sha, a bad flag).
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SHA_ARG=HEAD SOAK_SECS=300 CLIENT=1 ROOTLESS=0 KEEP=0 ONLY=()
seen_sha=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --soak-secs) SOAK_SECS="${2:-}"; shift 2 ;;
        --no-client) CLIENT=0; shift ;;
        --rootless)  ROOTLESS=1; shift ;;
        --only)      ONLY+=("${2:-}"); shift 2 ;;
        --keep)      KEEP=1; shift ;;
        -h|--help)   sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
        -*)          echo "nightly.sh: unknown argument $1" >&2; exit 2 ;;
        *)  (( seen_sha )) && { echo "nightly.sh: one sha only" >&2; exit 2; }
            SHA_ARG=$1; seen_sha=1; shift ;;
    esac
done
[[ $SOAK_SECS =~ ^[0-9]+$ ]] || { echo "nightly.sh: --soak-secs: not a whole number" >&2; exit 2; }

SHA=$(git -C "$REPO" rev-parse --verify --quiet "${SHA_ARG}^{commit}") \
    || { echo "nightly.sh: not a commit: $SHA_ARG" >&2; exit 2; }
SHORT=${SHA:0:7}
SUBJECT=$(git -C "$REPO" log -1 --format=%s "$SHA" | cut -c1-60)

OUT=$REPO/out/nightly/$SHORT
: "${NIGHTLY_WORK:=$HOME/code/eve-offline-lanes/nightly}"
TREE=$NIGHTLY_WORK/$SHORT
rm -rf "$OUT"
mkdir -p "$OUT/logs"
SUMMARY=$OUT/summary.tsv
printf 'step\tstatus\texit\tsecs\tnote\n' >"$SUMMARY"
STARTED=$(date -Is)
RED=0

# row STEP STATUS EXIT SECS NOTE
row() {
    printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "${5//$'\t'/ }" >>"$SUMMARY"
    [[ $2 == fail ]] && RED=1
    printf 'nightly: %-24s %-4s %s\n' "$1" "$2" "$5" >&2
}

# run STEP NOTE-ON-FAIL CMD... : run with output to logs/STEP.log, one row.
# Sets LAST_RC.
run() {
    local step=$1 failnote=$2; shift 2
    local log=$OUT/logs/${step//[:\/]/_}.log t0=$SECONDS rc=0
    "$@" >"$log" 2>&1 </dev/null || rc=$?
    LAST_RC=$rc
    if (( rc == 0 )); then
        row "$step" pass 0 $((SECONDS - t0)) ""
    else
        local last
        last=$(grep -v '^[[:space:]]*$' "$log" | tail -n 1 | cut -c1-100)
        row "$step" fail "$rc" $((SECONDS - t0)) "$failnote${last:+: $last}"
    fi
}

cleanup() {
    if (( ! KEEP )) && [[ -d $TREE ]]; then
        chmod -R u+w "$TREE" 2>/dev/null || true
        rm -rf "$TREE"
    fi
}
trap cleanup EXIT

# ------------------------------------------------------------------ export
t0=$SECONDS
export_tree() {
    chmod -R u+w "$TREE" 2>/dev/null || true
    rm -rf "$TREE"
    mkdir -p "$TREE"
    git -C "$REPO" archive "$SHA" | tar -x -C "$TREE"
    # The untracked inputs the server reads (the bundle, the celestial
    # tables). Run outputs and earlier nightlies stay behind.
    if [[ -d $REPO/out ]]; then
        rsync -a --exclude=/nightly --exclude=/land-wt --exclude=/landed \
            --exclude=/client-ci --exclude=/chat --exclude=/ci-display \
            --exclude='*.log' "$REPO/out/" "$TREE/out/"
    fi
}
if export_tree >"$OUT/logs/export.log" 2>&1; then
    row export pass 0 $((SECONDS - t0)) "$TREE"
    EXPORTED=1
else
    row export fail 1 $((SECONDS - t0)) "could not export $SHORT (logs/export.log)"
    EXPORTED=0
fi

# ------------------------------------------------------------------ build
BUILT=0
if (( EXPORTED )); then
    run build "cargo build" "$TREE/tools/dev.sh" cargo build --locked --workspace --bins
    (( LAST_RC == 0 )) && BUILT=1
else
    row build skip - 0 "skipped: export failed"
fi

# ------------------------------------------------------------------ soak
if (( BUILT )); then
    run soak "soak verdict" "$TREE/tools/soak.sh" --no-build --secs "$SOAK_SECS"
    # Keep the soak's own logs (server, bots, rss.tsv) beside the summary.
    [[ -d $TREE/server/target/soak ]] && cp -r "$TREE/server/target/soak" "$OUT/logs/soak-files" 2>/dev/null || true
    soak_line=$(grep -E '^soak: (verdict|RSS peak|\[turns\])' "$OUT/logs/soak.log" | cut -c1-90 || true)
    [[ -n $soak_line ]] && echo "$soak_line" >"$OUT/logs/soak.verdict"
else
    row soak skip - 0 "skipped: build failed"
fi

# ------------------------------------------------------------------ client CI
scenarios=()
if (( EXPORTED )); then
    if (( ${#ONLY[@]} )); then
        scenarios=("${ONLY[@]}")
    else
        for f in "$TREE"/tools/client-ci/scenarios/*.py; do
            [[ -e $f ]] || continue
            n=$(basename "$f" .py)
            [[ $n == _* ]] && continue      # _prelude.py is not a scenario
            # The census scenarios fire canaries, hang or crash the client on
            # purpose; they run under tools/census.sh, or here by --only.
            [[ $n == census_* ]] && continue
            scenarios+=("$n")
        done
    fi
fi

# Why client CI cannot run on this machine right now, or "" if it can.
client_skip=""
if (( ! CLIENT )); then
    client_skip="skipped: --no-client"
elif (( ! BUILT )); then
    client_skip="skipped: build failed"
elif (( ! ROOTLESS )); then
    hrc=0
    hout=$("$TREE/tools/install-jail-helper.sh" --check 2>&1) || hrc=$?
    printf '%s\n' "$hout" >"$OUT/logs/helper-check.log"
    if (( hrc == 3 )); then
        if grep -qi stale <<<"$hout"; then client_skip="skipped: helper stale"
        else client_skip="skipped: helper foreign"; fi
        grep -qi foreign <<<"$hout" && grep -qi stale <<<"$hout" \
            && client_skip="skipped: helper stale (and foreign)"
    elif (( hrc == 4 )) && ! sudo -n true 2>/dev/null; then
        client_skip="skipped: no helper, no cached sudo"
    elif (( hrc != 0 && hrc != 4 )); then
        client_skip="skipped: helper check exit $hrc"
    fi
fi

if (( ${#scenarios[@]} == 0 )); then
    if (( EXPORTED && CLIENT )); then
        row "scenario:*" fail - 0 "no scenarios in tools/client-ci/scenarios"
    fi
fi
for s in "${scenarios[@]}"; do
    if [[ -n $client_skip ]]; then
        row "scenario:$s" skip - 0 "$client_skip"
        continue
    fi
    args=(run "$s" --no-build)
    (( ROOTLESS )) && args+=(--rootless)
    run "scenario:$s" "client-ci" "$TREE/tools/client-ci.sh" "${args[@]}"
done

# ------------------------------------------------------------------ report
{
    n_pass=$(awk -F'\t' 'NR>1 && $2=="pass"' "$SUMMARY" | wc -l)
    n_fail=$(awk -F'\t' 'NR>1 && $2=="fail"' "$SUMMARY" | wc -l)
    n_skip=$(awk -F'\t' 'NR>1 && $2=="skip"' "$SUMMARY" | wc -l)
    if (( RED )); then verdict=RED; else verdict=GREEN; fi
    echo "nightly $SHORT  $verdict  ($n_pass pass, $n_fail fail, $n_skip skip)"
    echo "  $SUBJECT"
    echo "  started $STARTED, finished $(date -Is)"
    echo "  soak ${SOAK_SECS}s; client CI $( (( ROOTLESS )) && echo rootless || echo helper)"
    echo
    awk -F'\t' 'NR>1 { printf "  %-26s %-4s %5ss  %s\n", $1, $2, $4, $5 }' "$SUMMARY" | head -n 18
    [[ -f $OUT/logs/soak.verdict ]] && { echo; sed "s/^/  /" "$OUT/logs/soak.verdict"; }
    echo
    echo "  summary: out/nightly/$SHORT/summary.tsv   logs: out/nightly/$SHORT/logs/"
    (( KEEP )) && echo "  tree kept: $TREE"
} >"$OUT/report.txt"
cat "$OUT/report.txt"

(( RED )) && exit 1
exit 0
