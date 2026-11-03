#!/usr/bin/env bash
# Pin the client build this project is reverse-engineered against, and notice
# when something replaces it.
#
# **Why this exists.** Every `[V]` mark in this repository -- every verified
# constant, every wire layout, every formula read out of CCP's own bytecode --
# is measured against the pinned build (**3662168** since the re-pin of
# 2026-10-31, issue #19; it was 3619879). Steam had already moved its live copy
# to 3628513 (`docs/archive/plan-subsystems.md` §2a). Our two jailed installs are still
# 3619879, and nothing was guarding them: a Steam verify, a reinstall, or a
# disk sweep would replace the one artefact the whole corpus is anchored to,
# and the first symptom would be a `[V]` fact quietly ceasing to be true.
#
# It would also wipe our own patches. `start.ini` and `manifest.dat` are
# modified in place, with `.eveoffline-orig` backups beside them, so a verify
# costs the build *and* the patching.
#
#   tools/pin-client.sh --write     # record the manifest (do this once)
#   tools/pin-client.sh             # verify; non-zero if anything moved
#   tools/pin-client.sh --quick     # verify only the load-bearing files
#   tools/pin-client.sh --path DIR  # another install (e.g. tq-offline-2)
#
# The manifest is hashes, not client data, so it belongs in the repository --
# the no-client-data rule is about CCP's content, and a sha256 is not content.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# O19: the offline prefix copy; until it exists, verify reads the online
# prefix (verify only reads).
_eve_off=$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE
[ -d "$_eve_off/tq-offline" ] || _eve_off=$HOME/.local/share/Steam/steamapps/compatdata/8500/pfx/drive_c/CCP/EVE
PREFIX=${PREFIX:-$_eve_off}
client=$PREFIX/tq-offline
manifest=tools/client-pin.sha256
mode=verify
quick=0

# The files that decide whether our RE still applies. `code.ccp` is the whole
# Python corpus `out/pyc` was extracted from; `bin64` holds `_destiny.dll`,
# `blue.dll` and the rest of what the native lanes disassemble; `start.ini`
# carries the build number itself.
QUICK_FILES=(start.ini code.ccp manifest.dat carbon.json
             resfileindex.txt resfileindex_Windows.txt
             bin64/exefile.exe bin64/_destiny.dll bin64/blue.dll
             bin64/_trinity.dll bin64/_evelocalization.dll)

while (($#)); do
    case "$1" in
        --write) mode=write ;;
        --quick) quick=1 ;;
        --path) client=$2; shift ;;
        --manifest) manifest=$2; shift ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
        *) echo "pin-client: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

[[ -d $client ]] || { echo "pin-client: no client at $client" >&2; exit 2; }

build=$(sed -n 's/^build *= *//p' "$client/start.ini" | tr -d '\r')
echo "pin-client: $client"
echo "pin-client: start.ini says build = ${build:-<unreadable>}"

# Hashed from inside the install so the manifest holds relative paths and is
# portable between the two prefixes.
hash_all() { (cd "$client" && find . -type f -print0 | sort -z | xargs -0 sha256sum); }
hash_quick() {
    (cd "$client" && for f in "${QUICK_FILES[@]}"; do
        [[ -f $f ]] && sha256sum "./$f"
    done)
}

if [[ $mode == write ]]; then
    tmp=$(mktemp)
    { echo "# eve-offline client pin"
      echo "# install: $client"
      echo "# build:   $build"
      echo "# written: $(date -u -Is)"
      echo "# NOTE: start.ini and manifest.dat are OUR patched copies; the"
      echo "#       .eveoffline-orig files beside them are CCP's originals."
      hash_all
    } >"$tmp"
    mv "$tmp" "$manifest"
    echo "pin-client: wrote $manifest ($(grep -cv '^#' "$manifest") files)"
    exit 0
fi

[[ -f $manifest ]] || { echo "pin-client: no manifest; run --write first" >&2; exit 2; }

want_build=$(sed -n 's/^# build: *//p' "$manifest")
if [[ -n $want_build && $want_build != "$build" ]]; then
    echo "pin-client: BUILD MOVED -- manifest pins $want_build, install says $build" >&2
    exit 1
fi

# Compared as files, never through a pipe whose exit status would be the
# comparator's rather than the comparison's -- the same trap `tools/land.sh`
# exists for.
now=$(mktemp); grep -v '^#' "$manifest" | sort -k2 >"$now.want"
if ((quick)); then hash_quick | sort -k2 >"$now"; else hash_all | sort -k2 >"$now"; fi

# **Stock or patched** (docs/guides/patcher.md). The phone-home patcher rewrites
# whole files between two known hashes, so for every file its manifest for
# this build lists, either hash is the pinned file; the `.eveoffline-orig`
# backups beside them are `tools/eve_patch.py verify`'s to check, not ours.
patches=tools/patches/$build.json
if [[ -f $patches ]]; then
    for f in "$now" "$now.want"; do
        python3 -I - "$patches" "$f" "$client" "$manifest" <<'PY'
import json, os, sys
sys.path.append(os.path.join(os.path.dirname(os.path.abspath(sys.argv[1])), ".."))
import eve_patch  # noqa: E402  (tools/, last on the path)
files = json.load(open(sys.argv[1]))["files"]
alias = {}
for rel, e in files.items():
    alias[(e["sha256_stock"], "./" + rel)] = "stock-or-patched"
    if e["kind"] == "ccp":
        # A ccp file's patched bytes depend on the host's zlib, so there is no
        # patched whole-file hash to list: whatever hash the install's file has
        # counts when it reads as the manifest's patched form.
        p = os.path.join(sys.argv[3], rel)
        if os.path.isfile(p) and eve_patch.is_patched(e, open(p, "rb").read()):
            alias[(eve_patch.sha256_file(p), "./" + rel)] = "stock-or-patched"
        # The pin's own row for this file was written from some install, so it
        # holds that host's raw hash of a patched code.ccp. It counts as
        # stock-or-patched when the pin also holds the file's `.orig` row and
        # that row is the manifest's stock: the pin then provably came from
        # a patched copy of this build. Without the `.orig` row it does not.
        pin = {}
        for ln in open(sys.argv[4]):
            if not ln.startswith("#"):
                ph, _, pp = ln.rstrip("\n").partition("  ")
                pin[pp] = ph
        if pin.get("./" + rel + ".eveoffline-orig") == e["sha256_stock"] and ("./" + rel) in pin:
            alias[(pin["./" + rel], "./" + rel)] = "stock-or-patched"
    else:
        alias[(e["sha256_patched"], "./" + rel)] = "stock-or-patched"
backups = {"./" + rel + ".eveoffline-orig" for rel in files}
out = []
for line in open(sys.argv[2]):
    h, _, p = line.rstrip("\n").partition("  ")
    if p not in backups:
        out.append(f"{alias.get((h, p), h)}  {p}\n")
open(sys.argv[2], "w").writelines(out)
PY
    done
fi

status=0
if ((quick)); then
    # Only the named subset is checked, so missing rows are expected; compare
    # the intersection.
    join -j2 -o 1.1,2.1,0 "$now" "$now.want" 2>/dev/null \
        | awk '$1 != $2 { print "CHANGED " $3; bad=1 } END { exit bad?1:0 }' || status=1
    checked=$(wc -l <"$now")
else
    diff_out=$(diff "$now.want" "$now" || true)
    if [[ -n $diff_out ]]; then
        echo "$diff_out" | head -40 >&2
        status=1
    fi
    checked=$(wc -l <"$now")
fi
rm -f "$now" "$now.want"

if ((status != 0)); then
    echo "pin-client: FAILED -- the pinned install has changed" >&2
    echo "pin-client: the RE corpus is anchored to build $want_build; see" >&2
    echo "pin-client: docs/archive/plan-subsystems.md §2a before trusting any [V] mark" >&2
    exit 1
fi
echo "pin-client: ok, $checked files match build $build"
