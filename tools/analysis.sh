#!/usr/bin/env bash
# Run a command in the analysis container.
#
#   tools/analysis.sh --build                 # build (or rebuild) the image
#   tools/analysis.sh python3 tools/x.py      # run something
#   tools/analysis.sh uncompyle6 out/pyc/autoexec.pyc
#   tools/analysis.sh shell                   # interactive
#
# The repo is mounted at /work. The client installation, if EVE_CLIENT is set,
# is mounted read-only at /client. Containers get no network.
set -euo pipefail

IMAGE=eve-offline/analysis:latest
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# **`tq-offline`, which is the pinned build -- 3662168 since the re-pin of
# 2026-10-31 (it was 3619879, now `tq-3619879`) -- the build we actually ship
# against.** This defaulted to `tq` for a long time, and `tq` is a DIFFERENT
# install: 3628513. So a lane told "only 3619879 counts" was silently
# analysing another build's `exefile.exe`, `python27.dll`, `code.ccp`,
# `manifest.dat` and `resfileindex.txt` -- and marking what it found `[V]`.
#
# The blast radius was measured rather than guessed, which is the only reason
# this was not worse: of 12,794 `.pyj` only 7 differ past the `.pyc` header,
# all of them in the protobuf gateway (`eveProto/*`, `publicGateway/*`,
# `stackless_response_router`), and `blue.dll` and `_destiny.dll` differ ONLY
# in the PE checksum, the Authenticode blob and our own manifest patch -- not
# one byte of code or data. `exefile.exe` (117 ranges) and `python27.dll`
# looked as if they genuinely differed; re-measured 2026-10-26 with a PE-aware
# compare (checksum, security directory entry and certificate table removed,
# our `.eveoffline-orig` backups used) they are SIGNATURE-ONLY too: the ~5.7 KB
# is the size of a certificate table. So the native claims about them hold for
# both builds (tools/build-delta.py reproduces the whole comparison).
#
# `tools/run-client.sh` and `tools/client-ci.sh` have always used
# `tq-offline`, so the tests were right and only the analysis was wrong.
: "${EVE_CLIENT_DIR:=tq-offline}"
# The offline prefix copy (tools/offline-prefix.sh). There is NO fallback to
# the online prefix (compatdata/8500) any more: a typo'd EVE_CLIENT_DIR must
# not read the online install, even read-only. Name an install elsewhere with
# EVE_CLIENT (a full path), as tools/new-build.sh does.
_eve_off=$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE
# A name that was asked for and is missing is an error; the default missing
# only warns below (clientless runs, and tests, still work).
if [ -z "${EVE_CLIENT:-}" ] && [ "$EVE_CLIENT_DIR" != tq-offline ] && [ ! -d "$_eve_off/$EVE_CLIENT_DIR" ]; then
    echo "analysis.sh: no install '$EVE_CLIENT_DIR' in $_eve_off (set EVE_CLIENT_DIR to a dir there, or EVE_CLIENT to a path)" >&2
    exit 2
fi
: "${EVE_CLIENT:=$_eve_off/$EVE_CLIENT_DIR}"

build() {
    docker build --network default \
        -t "$IMAGE" \
        -f "$REPO/containers/analysis.Dockerfile" \
        "$REPO/containers"
}

if [[ "${1:-}" == "--build" ]]; then
    build
    exit 0
fi

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "analysis.sh: image missing, building once..." >&2
    build
fi

mounts=(-v "$REPO:/work")
if [[ -d "$EVE_CLIENT" ]]; then
    # Say which build is mounted, every time. The whole point of the note
    # above is that the wrong one is indistinguishable from the right one
    # once you are inside the container.
    if [[ -r "$EVE_CLIENT/start.ini" ]]; then
        echo "analysis.sh: /client = $(basename "$EVE_CLIENT"), $(grep -h '^build' \
            "$EVE_CLIENT/start.ini" | head -1 | tr -d ' ')" >&2
    else
        echo "analysis.sh: /client = $(basename "$EVE_CLIENT") (no start.ini; build unknown)" >&2
    fi
    mounts+=(-v "$EVE_CLIENT:/client:ro")
    # `ResFiles/` is a *sibling* of the build directory, not a child of it, so
    # mounting $EVE_CLIENT alone gives you the manifest without the data:
    # `resfileindex.txt` lists every resource as `res:/...` and maps it to a
    # path under `../ResFiles/`, and all 203 static-data tables and the whole
    # localisation corpus live there. A lane hit this and taught its reader to
    # cope with the wrong root instead; mounting the parent is the actual fix.
    mounts+=(-v "$(cd "$EVE_CLIENT/.." && pwd):/client-root:ro")
else
    echo "analysis.sh: warning: EVE_CLIENT not found at $EVE_CLIENT" >&2
fi

# Always keep stdin attached so `... python3 - <<EOF` and other piped input
# reach the container; add -t only for a real terminal.
tty_args=(-i)
[[ -t 0 ]] && tty_args=(-i -t)

cmd=("$@")
if [[ ${#cmd[@]} -eq 0 || "${cmd[0]}" == "shell" ]]; then
    cmd=(bash)
fi

exec docker run --rm "${tty_args[@]}" \
    --network none \
    --user "$(id -u):$(id -g)" \
    "${mounts[@]}" \
    -w /work \
    "$IMAGE" "${cmd[@]}"
