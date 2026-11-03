#!/usr/bin/env bash
# How do two builds' client surfaces differ, and which of it do we serve?
#
#   tools/port-surface.sh OLD_PYC NEW_PYC [--out DIR]
#                         [--old-client DIR] [--new-client DIR] [--no-cache]
#
# OLD_PYC and NEW_PYC are unpacked bytecode trees (out/pyc, out/pyc-3619879,
# out/new-build/<build>/pyc). Both are read-only. Nothing is installed and
# nothing leaves the analysis container, which has no network.
#
# Per tree it runs, with ONE commit of the tools on both sides (a difference is
# then the client's, not the extractor's):
#
#   census-calls.sh --pyc   client-calls.tsv      (scan_calls, ~10 minutes)
#   scan_bound              bound-objects.json
#   scan_remote             remote-services.txt
#   scan_services           client-services.json
#   scan_client_callable    client-callable.tsv
#   scan_push               push-events.tsv
#   scan_session_keys       session-keys.tsv      (seeded by findings/)
#   scan_user_errors        user-errors.tsv       (seeded by findings/)
#   scan_proto_messages     proto-messages.txt
#   gw_dump                 gw-protos.txt
#
# then joins both sets against what we serve (scan_calls_served.py, and a text
# search of server/crates) with tools/port_surface.py. Output, in DIR:
#
#   surface.md         one section per step G1-G6, SERVED-AND-CHANGED FIRST
#   surface-diff.tsv   one row per change
#   old/ new/          each side's tables, meta.txt, per-tool logs
#   served.json        what our dispatcher answers
#
# DIR defaults to <dir of NEW_PYC>/surface when NEW_PYC is .../pyc (so
# out/new-build/<build>/surface, which is where tools/new-build.sh --report
# looks), else out/port-surface/<name of NEW_PYC>.
#
# Exit 0: nothing we serve moved. Exit 3: something did (read surface.md).
# Exit 2: usage or a missing tree. Exit 1: a tool failed (see old/*.log).
#
# **The cache.** The ten scanners take about ten minutes per tree, nearly all in
# scan_calls. A side is cached under out/port-surface-cache/ by a key made of the
# tree's content, the tools, the seeds, our dispatcher's source and the install it
# was read against, so the OLD side of a port day costs one run, ever, and any
# change that could alter the answer makes a new key. --no-cache ignores it.
#
# **Resources.** user-errors has columns that come from an install (dialog and
# label ids, loc_args). Give --old-client/--new-client (or EVE_SURFACE_OLD_CLIENT,
# EVE_SURFACE_NEW_CLIENT) a full install dir such as
# ~/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-3619879 to read them.
# Without one that side is read from bytecode only, meta.txt says
# `resources=no`, and G6 compares only the bytecode columns.
#
# The release line in code.ccp file names (`release\V24.01\`) is read from the
# install's start.ini when there is one, else the V24.01 default.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
IMAGE=eve-offline/analysis:latest

usage() { sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2; exit 2; }

old="" new="" out="" oldclient=${EVE_SURFACE_OLD_CLIENT:-} newclient=${EVE_SURFACE_NEW_CLIENT:-} cache=1
pos=()
while (($#)); do
    case $1 in
        --out) out=${2:?--out needs a directory}; shift 2 ;;
        --old-client) oldclient=${2:?--old-client needs a directory}; shift 2 ;;
        --new-client) newclient=${2:?--new-client needs a directory}; shift 2 ;;
        --no-cache) cache=0; shift ;;
        -h|--help) usage ;;
        -*) echo "port-surface: unknown flag $1" >&2; usage ;;
        *) pos+=("$1"); shift ;;
    esac
done
((${#pos[@]} == 2)) || usage
old=${pos[0]} new=${pos[1]}
for t in "$old" "$new"; do
    [[ -d $t ]] || { echo "port-surface: no such tree: $t" >&2; exit 2; }
done
for c in "$oldclient" "$newclient"; do
    [[ -z $c || -d $c ]] || { echo "port-surface: no such install: $c" >&2; exit 2; }
done
old=$(cd "$old" && pwd); new=$(cd "$new" && pwd)
[[ $old != "$new" ]] || { echo "port-surface: OLD and NEW are the same tree" >&2; exit 2; }

if [[ -z $out ]]; then
    if [[ $(basename "$new") == pyc ]]; then out=$(dirname "$new")/surface
    else out=$REPO/out/port-surface/$(basename "$new"); fi
fi
mkdir -p "$out"; out=$(cd "$out" && pwd)

docker image inspect "$IMAGE" >/dev/null 2>&1 \
    || { echo "port-surface: image missing, run tools/analysis.sh --build" >&2; exit 2; }

# A name for a side: the build directory when the tree is .../<build>/pyc.
# (out/pyc is the pinned build's own tree: its parent says nothing, so it is `pyc`.)
label_of() {
    if [[ $(basename "$1") == pyc && $(basename "$(dirname "$1")") != out ]]; then basename "$(dirname "$1")"
    else basename "$1"; fi
}

# A content hash of a tree or a set of files, independent of where it lives.
hash_tree() { (cd "$1" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum | sha256sum | cut -c1-16); }
hash_files() { (cd "$REPO" && { sha256sum "$@" 2>/dev/null || true; } | sha256sum | cut -c1-16); }

tools_hash() {
    hash_files tools/scan_calls.py tools/scan_calls_emit.py tools/scan_calls_served.py tools/scan_bound.py \
        tools/scan_remote.py tools/scan_services.py tools/scan_client_callable.py tools/scan_push.py \
        tools/scan_session_keys.py tools/scan_user_errors.py tools/scan_proto_messages.py tools/gw_dump.py \
        tools/pyc27.py tools/findings_lib.py tools/protos.py tools/fsd.py tools/census-calls.sh tools/port-surface.sh \
        findings/session-keys.tsv findings/user-errors.tsv findings/bound-objects.json \
        $(cd "$REPO" && ls server/crates/eve-server/src/service.rs server/crates/eve-server/src/service/*.rs 2>/dev/null)
}

# One docker run in the analysis image. DK_TREE is mounted read-only at /tree,
# the side's output directory at /o, an install (if any) at /client.
dk() {
    local args=(--rm -i --network none --user "$(id -u):$(id -g)" -e PYTHONDONTWRITEBYTECODE=1
                -v "$REPO:/work:ro" -v "$DK_OUT:/o" -w /o)
    [[ -z ${DK_TREE:-} ]] || args+=(-v "$DK_TREE:/tree:ro")
    if [[ -n ${DK_CLIENT:-} ]]; then
        args+=(-v "$DK_CLIENT:/client:ro" -v "$(cd "$DK_CLIENT/.." && pwd):/client-root:ro")
        [[ ! -r $DK_CLIENT/start.ini ]] || args+=(-e SCAN_START_INI=/client/start.ini)
    fi
    docker run "${args[@]}" "$IMAGE" "$@"
}

scan_side() {  # LABEL TREE CLIENT DEST
    local label=$1 tree=$2 client=$3 dest=$4
    local thash khash key cdir
    thash=$(hash_tree "$tree")
    khash=none
    [[ -z $client ]] || khash=$(cd "$client" && sha256sum start.ini 2>/dev/null | cut -c1-16)-$(basename "$client")
    key=$label-$thash-$(tools_hash)-$khash
    cdir=$REPO/out/port-surface-cache/$key
    rm -rf "$dest"
    if ((cache)) && [[ -f $cdir/DONE ]]; then
        mkdir -p "$dest"; cp -a "$cdir/." "$dest/"; rm -f "$dest/DONE"
        echo "== $label: cached ($key)"
        return 0
    fi
    mkdir -p "$dest"
    DK_TREE=$tree DK_OUT=$dest DK_CLIENT=$client
    local s=$SECONDS
    echo "== $label: scanning $tree ($([[ -n $client ]] && echo "resources from $client" || echo "bytecode only"))"
    {
        echo "label=$label"
        echo "tree=$tree"
        echo "tree_hash=$thash"
        echo "resources=$([[ -n $client ]] && echo yes || echo no)"
        echo "client=${client:-none}"
        [[ -z $client ]] || echo "client_$(grep -h '^build' "$client/start.ini" 2>/dev/null | head -1 | tr -d ' \r')"
    } > "$dest/meta.txt"
    # census-calls.sh runs its own containers; it needs no install, only SCAN_START_INI
    # for the release line, which it does not get, so the V24.01 default holds there.
    "$REPO/tools/census-calls.sh" --pyc "$tree" --out "$dest/client-calls.tsv" > "$dest/client-calls.log" 2>&1 \
        || { echo "port-surface: $label: census-calls failed:" >&2; tail -5 "$dest/client-calls.log" >&2; return 1; }
    local ok=1
    step() {  # NAME LOGFILE CMD...
        local name=$1 log=$2; shift 2
        if ! "$@" > "$dest/$log" 2>&1; then echo "port-surface: $label: $name failed (see $dest/$log)" >&2; tail -4 "$dest/$log" >&2; ok=0; fi
    }
    step scan_bound scan_bound.log dk python3 /work/tools/scan_bound.py --pyc-root /tree -o /o/bound-objects.json
    step scan_remote remote-services.txt dk bash -c 'ln -s /tree pyc && python3 /work/tools/scan_remote.py; rc=$?; rm -f pyc; exit $rc'
    step scan_services scan_services.log dk bash -c 'ln -s /tree pyc && python3 /work/tools/scan_services.py; rc=$?; rm -f pyc; mv services.json client-services.json; exit $rc'
    step scan_client_callable scan_client_callable.log dk python3 -I /work/tools/scan_client_callable.py --pyc /tree --out /o/client-callable.tsv
    step scan_push scan_push.log dk python3 -I /work/tools/scan_push.py --pyc /tree --out /o/push-events.tsv
    step scan_session_keys session-keys.log dk python3 -I /work/tools/scan_session_keys.py --pyc /tree --seed /work/findings/session-keys.tsv --out /o/session-keys.tsv
    local ue=(python3 -I /work/tools/scan_user_errors.py --pyc /tree --seed /work/findings/user-errors.tsv --out /o/user-errors.tsv)
    if [[ -n $client ]]; then ue+=(--client /client); else ue+=(--no-resources); fi
    step scan_user_errors user-errors.log dk "${ue[@]}"
    step scan_proto_messages scan_proto_messages.log dk python3 -I /work/tools/scan_proto_messages.py --pyc /tree --out /o/proto-messages.txt
    step gw_dump gw_dump.log dk python3 -I /work/tools/gw_dump.py --pyc /tree --out /o/gw-protos.txt
    ((ok)) || return 1
    echo "== $label: done in $((SECONDS - s)) s"
    if ((cache)); then
        mkdir -p "$cdir"; cp -a "$dest/." "$cdir/"; : > "$cdir/DONE"
    fi
}

ol=$(label_of "$old"); nl=$(label_of "$new")
[[ $ol != "$nl" ]] || { ol=old-$ol; nl=new-$nl; }

# Both sides at once: they share nothing, and scan_calls is one core each.
scan_side "$ol" "$old" "$oldclient" "$out/old" > "$out/old.log" 2>&1 & p1=$!
scan_side "$nl" "$new" "$newclient" "$out/new" > "$out/new.log" 2>&1 & p2=$!
rc1=0; rc2=0
wait "$p1" || rc1=$?
wait "$p2" || rc2=$?
cat "$out/old.log" "$out/new.log"
if ((rc1 || rc2)); then echo "port-surface: a side failed" >&2; exit 1; fi

echo "== our dispatcher's served set"
DK_TREE="" DK_OUT=$out DK_CLIENT="" dk env SCAN_SERVED=/o/served.json python3 /work/tools/scan_calls_served.py > /dev/null

echo "== joining"
rc=0
DK_TREE="" DK_OUT=$out DK_CLIENT="" dk python3 -I /work/tools/port_surface.py /o/old /o/new --out /o --repo /work || rc=$?
case $rc in
    0|3) ;;
    *) echo "port-surface: the join failed (exit $rc)" >&2; exit 1 ;;
esac
echo "== $out/surface.md ($([[ $rc == 3 ]] && echo "SERVED IMPACT" || echo "no served impact"))"
exit "$rc"
