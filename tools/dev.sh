#!/usr/bin/env bash
# Run a command inside the build image, isolated from this machine.
#
#   tools/dev.sh cargo test --locked --workspace
#   tools/dev.sh --lock cargo generate-lockfile
#   tools/dev.sh --lock cargo fmt --all
#   tools/dev.sh --image        (re)build the image; the only networked step
#
# The container gets: no network, no capabilities, a read-only root, the repo
# mounted READ-ONLY, and one writable directory (server/target). A build
# script or macro in a dependency therefore cannot reach the network, read
# your home directory, or modify anything the host later executes (hooks,
# tools/). `--lock` additionally makes server/ writable, for the two commands
# that must write source files: regenerating Cargo.lock and `cargo fmt`.
#
# `--init` forwards signals so stopping this script stops the container; and
# every command runs under a hard in-container time limit so a SIGKILLed
# wrapper still leaves no runaway container.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="eve-offline-build"

if [[ "${1:-}" == "--image" ]]; then
    exec docker build -t "$IMAGE" -f "$REPO/containers/build.Dockerfile" "$REPO/containers"
fi

server_mount="ro"
if [[ "${1:-}" == "--lock" ]]; then
    server_mount="rw"
    shift
fi
[[ $# -gt 0 ]] || { echo "usage: tools/dev.sh [--lock] <command...> | --image" >&2; exit 2; }

docker image inspect "$IMAGE" >/dev/null 2>&1 \
    || { echo "image '$IMAGE' missing: run tools/dev.sh --image" >&2; exit 2; }

# EVE_TARGET_DIR: a build cache outside the tree (tools/land.sh keeps one
# on disk for its gates); default the tree's own server/target.
target_dir=${EVE_TARGET_DIR:-$REPO/server/target}
mkdir -p "$target_dir"

# **Half the machine, not all of it.** Cargo defaults to one job per core and
# the test harness to one thread per core, which on a 32-thread box makes an
# ordinary `cargo test` indistinguishable from a fork bomb from the owner's
# point of view -- their desktop stutters and the fans spin up for a 4 s gate.
# `--cpus` is the hard ceiling (the container cannot exceed it however many
# threads it spawns) and the two cargo variables stop it trying.
#
# One knob, EVE_MAX_JOBS, because three that can disagree is worse than one.
: "${EVE_MAX_JOBS:=16}"

# **Wait for memory before starting.** The box has no swap; parallel lanes,
# the gate, benches and the owner's client together ran it out of RAM
# (2026-10-29) and the kernel killed whatever it picked. A build only starts
# while MemAvailable is at least EVE_MIN_FREE_GB (default 12); otherwise it
# waits, saying so once a minute. EVE_MIN_FREE_GB=0 turns the wait off.
min_free_kb=$(( ${EVE_MIN_FREE_GB:-12} * 1024 * 1024 ))
waited=0
while avail=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo) && ((avail < min_free_kb)); do
    if ((waited % 60 == 0)); then
        echo "dev.sh: waiting for memory: $((avail / 1048576)) GB available, want ${EVE_MIN_FREE_GB:-12} GB" >&2
    fi
    sleep 5
    waited=$((waited + 5))
done

exec docker run --rm --init \
    --network none \
    --cpus "$EVE_MAX_JOBS" \
    --cap-drop ALL \
    --security-opt no-new-privileges \
    --read-only \
    --tmpfs /tmp:exec,size=2g \
    --user "$(id -u):$(id -g)" \
    --memory "${EVE_DEV_MEMORY:-10g}" \
    -e HOME=/tmp/home \
    -e CARGO_HOME=/tmp/home/cargo \
    -e CARGO_TERM_COLOR=never \
    -e EVE_SERVER_BIN="${EVE_SERVER_BIN:-eve-offline-server}" \
    -e CARGO_BUILD_JOBS="$EVE_MAX_JOBS" \
    -e RUST_TEST_THREADS="$EVE_MAX_JOBS" \
    -e CARGO_PROFILE_DEV_DEBUG="${EVE_DEBUG_INFO:-line-tables-only}" \
    -e CARGO_PROFILE_TEST_DEBUG="${EVE_DEBUG_INFO:-line-tables-only}" \
    -v "$REPO:/repo:ro" \
    -v "$REPO/server:/repo/server:$server_mount" \
    -v "$target_dir:/repo/server/target:rw" \
    -w /repo/server \
    "$IMAGE" timeout --signal=KILL "${EVE_DEV_TIMEOUT:-3600}" "$@"
