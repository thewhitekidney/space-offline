#!/usr/bin/env bash
# Run already-built tests as concurrent `cargo test` jobs; called by
# tools/gate.sh INSIDE the build image (tools/dev.sh), never on the host.
#
#   tools/test-parallel.sh 'ARGS 1' 'ARGS 2' ...
#
# Each argument is one job: the words after `cargo test --locked`. **Why**
# (gate-speed lane, 2026-11-03): `cargo test` runs its test binaries one after
# another, so the gate paid eve-bot's socket tests (~290 s, mostly waits on
# game clocks), eve-bot's lib (~85 s) and the rest (~135 s) in series. The
# gate builds everything once first (`cargo test --no-run`), so the jobs here
# only take cargo's build lock long enough to see the build is fresh, then run
# side by side. The caller chooses jobs whose union is exactly the targets one
# `cargo test --workspace` runs.
#
# Every job runs to the end. Outputs print in the order given, each under a
# `   -- cargo test <args>` line, so a log reads like a serial run; the exit
# code is the first failing job's, and every failing job is named.
set -uo pipefail

(($#)) || { echo "usage: tools/test-parallel.sh 'cargo test args' ..." >&2; exit 2; }

dir=$(mktemp -d -t eve-test-par-XXXXXX)
trap 'rm -rf "$dir"' EXIT
i=0
for job in "$@"; do
    (
        read -ra args <<<"$job"
        s=0
        cargo test --locked "${args[@]}" >"$dir/$i.out" 2>&1 || s=$?
        echo "$s" >"$dir/$i.rc"
    ) &
    i=$((i + 1))
done
wait

status=0
failed=()
i=0
for job in "$@"; do
    echo "   -- cargo test --locked $job"
    cat "$dir/$i.out" 2>/dev/null || true
    s=$(cat "$dir/$i.rc" 2>/dev/null || echo 1)
    if ((s != 0)); then
        failed+=("$job")
        ((status != 0)) || status=$s
    fi
    i=$((i + 1))
done
for job in "${failed[@]}"; do
    echo "test-parallel: cargo test --locked $job FAILED" >&2
done
exit "$status"
