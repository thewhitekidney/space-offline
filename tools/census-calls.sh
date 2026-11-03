#!/usr/bin/env bash
# Regenerate findings/client-calls.tsv -- the census of every machoNet call
# site in the client, with what the client does with each answer.
#
#   tools/census-calls.sh            # rewrite findings/client-calls.tsv
#   tools/census-calls.sh --diff     # show what changed, write nothing
#   tools/census-calls.sh --pyc DIR --out FILE
#                                    # census another build's tree (DIR is a
#                                    # pyc tree, read-only), write FILE, touch
#                                    # nothing in findings/. Both flags go
#                                    # together; tools/port-surface.sh uses it.
#
# Three stages, because the middle one reads *our* code and the other two read
# the client's:
#
#   scan_calls.py         -> sites.json    the bytecode census (a symbolic-stack
#                                          machine over every code object; it
#                                          sweeps the tree twice, once to learn
#                                          which accessors hold a service handle
#                                          and once to collect sites, so this
#                                          stage is ~10 minutes, not two)
#   scan_calls_served.py  -> served.json   which (service, method) pairs our
#                                          dispatcher actually answers, parsed
#                                          out of service.rs and service/*.rs
#   scan_calls_emit.py    -> the TSV       the join, deterministically sorted
#
# **Why this exists rather than the census alone:** a census you cannot
# regenerate is a snapshot, and most of its value is in the diff after a batch
# of service work -- which rows moved from `no` to `yes`, and whether a new
# `native` consumer appeared (there is exactly one today, and it is the one
# that caused the undock black screen).
set -euo pipefail

# --pyc/--out: see the header. Parsed first so `--diff` keeps working as $1.
pyc="" ; outfile="" ; mode=""
while (($#)); do
    case $1 in
        --pyc) pyc=${2:?--pyc needs a directory}; shift 2 ;;
        --out) outfile=${2:?--out needs a file}; shift 2 ;;
        --diff) mode=--diff; shift ;;
        *) echo "census-calls: unknown argument $1" >&2; exit 2 ;;
    esac
done
if [[ -n $pyc || -n $outfile ]]; then
    [[ -n $pyc && -n $outfile ]] || { echo "census-calls: --pyc and --out go together" >&2; exit 2; }
    [[ -z $mode ]] || { echo "census-calls: --diff does not combine with --pyc" >&2; exit 2; }
    [[ -d $pyc ]] || { echo "census-calls: no such tree: $pyc" >&2; exit 2; }
    pyc=$(cd "$pyc" && pwd)
fi

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE=eve-offline/analysis:latest
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# The tree mounts at /tree and SCAN_PYC names it; the reader's own outputs go to
# the scratch dir so a run on another tree never writes into findings/.
extra=()
if [[ -n $pyc ]]; then
    extra=(-v "$pyc:/tree:ro" -e SCAN_PYC=/tree -e SCAN_CALLS_OUT=/scratch/calls.tsv)
fi
run() {
    docker run --rm -i --network none --user "$(id -u):$(id -g)" \
        -v "$REPO:/work" -v "$SCRATCH:/scratch" "${extra[@]}" -w /scratch \
        "$IMAGE" "$@"
}

docker image inspect "$IMAGE" >/dev/null 2>&1 \
    || { echo "census-calls: image missing, run tools/analysis.sh --build" >&2; exit 2; }

echo "== scanning the client's bytecode (two sweeps; ~10 minutes)"
run python3 /work/tools/scan_calls.py > "$SCRATCH/sites.json"
echo "== reading our own dispatcher"
run python3 /work/tools/scan_calls_served.py > "$SCRATCH/served.json"
out="$REPO/findings/client-calls.tsv"

# Blind spots this census is known to still have, with measured sizes, are in
# `docs/lanes/census-blindspots.md`. The short version: a missing row does not
# mean "not called".
#
# `scan_calls_emit.py` writes the TSV itself, to the repo, and prints the
# summary tables to stdout. For `--diff` we keep the old file aside and put it
# back, so a diff run leaves the tree exactly as it found it.
if [[ $mode == --diff ]]; then
    cp "$out" "$SCRATCH/before.tsv"
fi
echo "== joining"
run python3 /work/tools/scan_calls_emit.py
if [[ -n $outfile ]]; then
    cp "$SCRATCH/calls.tsv" "$outfile"
    echo "wrote $outfile ($(($(wc -l < "$outfile") - 1)) call sites)"
    exit 0
fi
if [[ $mode == --diff ]]; then
    cp "$out" "$SCRATCH/after.tsv"
    cp "$SCRATCH/before.tsv" "$out"
    diff -u "$SCRATCH/before.tsv" "$SCRATCH/after.tsv" && echo "no change"
    exit 0
fi
echo "wrote $out ($(($(wc -l < "$out") - 1)) call sites)"
