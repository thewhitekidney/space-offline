#!/usr/bin/env bash
# Is the client's resource set actually complete?
#
#   tools/check-resources.sh              # summary
#   tools/check-resources.sh --missing    # ... and list what is absent
#   tools/check-resources.sh --prefix res:/dx9/model/hangar   # scope it
#
# WHY THIS EXISTS.
#
# EVE streams its art on demand: `resfileindex.txt` names every resource, but
# the blobs live in a SHARED `ResFiles/` store one level above the install,
# and only the ones some client actually asked for are on disk. A copy that
# has never rendered a hangar has never downloaded one.
#
# That produced a failure with no server component at all. `hangarView.
# LoadScene` does
#
#     scene, _ = self.sceneManager.LoadScene(stationGraphicFile, ...)
#
# and `sceneManager.LoadScene` returns None for a file it cannot open, so the
# unpack raises `TypeError: 'NoneType' object is not iterable` and LoadView
# aborts -- looking exactly like a shape we got wrong. The first measurement
# said 41% of 122754 files were missing, 17.5 GiB, including every byte of
# `res:/dx9/model/hangar/minmatar/mh1/`.
#
# A red herring to know about: the error names a `.red` file, but there are
# ZERO `.red` entries in the index and ~18700 `.black` ones. The client
# translates the extension on the way to the cache, so a `.red` path in a
# traceback is normal and is not the problem.
#
# Run this after a launcher download to confirm it really finished, rather
# than trusting that it said so.
set -euo pipefail

: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
: "${EVE_CLIENT:=$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline}"
# The store is shared between installs (tq, sisi, tq-offline), so it sits
# beside them, not inside the one we launch.
: "${EVE_RESFILES:=$(dirname "$EVE_CLIENT")/ResFiles}"

PREFIX=""
LIST=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --missing) LIST=1 ;;
        --prefix)  PREFIX=${2:?--prefix needs a value}; shift ;;
        -h|--help) sed -n '2,6p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "check-resources.sh: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

INDEX=$EVE_CLIENT/resfileindex.txt
[[ -f "$INDEX" ]] || { echo "no resfileindex.txt at $INDEX" >&2; exit 2; }
[[ -d "$EVE_RESFILES" ]] || { echo "no ResFiles store at $EVE_RESFILES" >&2; exit 2; }

echo "index : $INDEX"
echo "store : $EVE_RESFILES"
[[ -n $PREFIX ]] && echo "prefix: $PREFIX"

LIST=$LIST PREFIX=$PREFIX python3 - "$INDEX" "$EVE_RESFILES" <<'PY'
import os, sys
index, root = sys.argv[1], sys.argv[2]
prefix = os.environ.get("PREFIX", "")
show = os.environ.get("LIST") == "1"

total = missing = 0
total_bytes = missing_bytes = 0
absent = []
with open(index, encoding="utf-8", errors="replace") as fh:
    for line in fh:
        # res:/path,<aa/aaaa..._md5>,<md5>,<size>,<compressed size>
        parts = line.rstrip("\n").split(",")
        if len(parts) < 5:
            continue
        path, blob = parts[0], parts[1]
        if prefix and not path.startswith(prefix):
            continue
        try:
            size = int(parts[4])      # the on-disk, compressed size
        except ValueError:
            continue
        total += 1
        total_bytes += size
        if not os.path.exists(os.path.join(root, blob)):
            missing += 1
            missing_bytes += size
            if show:
                absent.append(path)

if total == 0:
    print("\nno indexed resources matched")
    raise SystemExit(2)

gib = 2 ** 30
print(f"\nindexed : {total:>8}  {total_bytes / gib:7.2f} GiB")
print(f"missing : {missing:>8}  {missing_bytes / gib:7.2f} GiB   "
      f"({100 * missing / total:.1f}% of files)")

if show:
    for path in absent:
        print("  MISSING", path)

if missing:
    print("\nThe client cannot draw what it does not have. Let the official "
          "launcher\nfinish downloading, then run this again.")
    raise SystemExit(1)
print("\ncomplete.")
PY
