#!/usr/bin/env bash
# Re-pin: make a new client build the pinned one (PORT-REPIN, T5; issue #145).
# The "Re-pinning" steps of docs/guides/build-compat.md as one script.
#
#   tools/re-pin.sh NEW_DIR [--prefix P] [--old-name NAME] [--dry-run]
#
#   NEW_DIR      the new build's install dir under CCP/EVE (the patched copy,
#                e.g. tq-1109); its start.ini `build` must be the registry's
#                PINNED row, so tools/builds.tsv and build.rs land first
#   --prefix P   the Wine prefix root (holds pfx/drive_c/CCP/EVE); default the
#                offline prefix. A scratch prefix is how the drill runs this.
#   --old-name N what the current tq-offline is renamed to. Default
#                tq-<oldbuild>, and when that exists (tq-3662168 does) the
#                first free tq-<oldbuild>-a, -b, ... The old name is never
#                taken blindly: that collision made a plain `mv` nest the
#                copy inside the existing dir.
#   --dry-run    run every check, print every step, change nothing
#
# Refuses, before it changes anything, when: a target exists; the new build is
# the old one; the registry does not pin the new build; the new build has no
# stock control copy tq-<new>-control (census.sh's control, made straight after
# import, before any patch; defect 4 of r-port-1109-1101); out/pyc-<old> or
# out/content-<old> exists; the prefix is the online one (compatdata/8500).
#
# Then, in this order:
#   1. tq-offline -> OLD, NEW_DIR -> tq-offline (renames only)
#   2. eve_patch.py apply on the new tq-offline: the copy is patched, so this
#      writes only the prefs keys, which live under the install name
#   3. pin-client.sh --write (commit it and land: the gate stays red until then)
#   4. out/pyc -> out/pyc-<old>, out/content -> out/content-<old>. The new
#      extract is NOT run here: it is slow and wants a reader (build-compat.md
#      step 5); the script prints the commands
#   5. the second client: client2-setup.sh --remove, then set it up again
#      (skipped with --prefix: client2-setup.sh builds from the offline prefix)
#   6. slot-prefix.sh remove all (`ensure` refreshes the client only, so a
#      stale slot prefix would keep the old settings)
#
# It never launches a client and never touches compatdata/8500.
# EVE_REPIN_REPO points at another checkout (the tests).
set -euo pipefail

REPO=${EVE_REPIN_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"

die() { echo "re-pin: $*" >&2; exit 2; }
usage() { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "${BASH_SOURCE[0]}"; }

NEW= PREFIX= OLD_NAME= DRY=0
while [[ $# -gt 0 ]]; do
    case $1 in
        --prefix) PREFIX=${2:?--prefix needs a directory}; shift ;;
        --old-name) OLD_NAME=${2:?--old-name needs a name}; shift ;;
        --dry-run) DRY=1 ;;
        -h|--help) usage; exit 0 ;;
        -*) usage >&2; exit 2 ;;
        *) [[ -z $NEW ]] || die "one NEW_DIR only"; NEW=$1 ;;
    esac
    shift
done
[[ -n $NEW ]] || { usage >&2; exit 2; }

DEFAULT_PREFIX=0
if [[ -z $PREFIX ]]; then PREFIX=$(eve_offline_prefix "$HOME"); DEFAULT_PREFIX=1; fi
EVE=$PREFIX/pfx/drive_c/CCP/EVE
eve_is_online_path "$PREFIX" && die "$PREFIX is the online prefix; refused"
[[ -d $EVE ]] || die "no $EVE"

name_ok() { [[ $1 =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ && $1 != tq && $1 != sisi && $1 != tq-offline ]]; }
ini_build() { sed -n 's/^build *= *//p' "$1/start.ini" 2>/dev/null | tr -d '\r' | head -n 1; }

name_ok "$NEW" || die "bad NEW_DIR '$NEW'"
[[ -f $EVE/$NEW/start.ini ]] || die "$EVE/$NEW is not an install (no start.ini)"
[[ -f $EVE/tq-offline/start.ini ]] || die "$EVE/tq-offline is not an install (no start.ini)"
NEWB=$(ini_build "$EVE/$NEW"); OLDB=$(ini_build "$EVE/tq-offline")
[[ $NEWB =~ ^[0-9]+$ && $OLDB =~ ^[0-9]+$ ]] || die "cannot read both builds (new '$NEWB', old '$OLDB')"
[[ $NEWB != "$OLDB" ]] || die "$NEW is build $NEWB, the one already in tq-offline: nothing to re-pin"

PINNED=$(awk -F'\t' '!/^#/ && $3 == "pinned" {print $1; exit}' "$REPO/tools/builds.tsv" 2>/dev/null || true)
[[ $PINNED == "$NEWB" ]] || die "tools/builds.tsv pins '${PINNED:-nothing}', not $NEWB: land the registry row and build.rs first (build-compat.md, Re-pinning step 1)"

if [[ -n $OLD_NAME ]]; then
    name_ok "$OLD_NAME" || die "bad --old-name '$OLD_NAME'"
    [[ ! -e $EVE/$OLD_NAME ]] || die "$EVE/$OLD_NAME exists; pick another --old-name"
else
    OLD_NAME=tq-$OLDB
    for sfx in a b c d e f g h i j k l m n o p q r s t u v w x y z; do
        [[ -e $EVE/$OLD_NAME ]] || break
        OLD_NAME=tq-$OLDB-$sfx
    done
    [[ ! -e $EVE/$OLD_NAME ]] || die "no free old name for $OLDB"
fi
[[ $OLD_NAME != "$NEW" ]] || die "--old-name is NEW_DIR"

CONTROL=tq-$NEWB-control
[[ -f $EVE/$CONTROL/start.ini ]] || die "no stock control copy $EVE/$CONTROL: the census control must be the new build with only B0+P0, made from the import BEFORE any patch (r-port-1109-1101 defect 4)"
[[ $(ini_build "$EVE/$CONTROL") == "$NEWB" ]] || die "$CONTROL is not build $NEWB"

for k in pyc content; do
    [[ ! -e $REPO/out/$k-$OLDB ]] || die "$REPO/out/$k-$OLDB exists; move it aside"
done

echo "re-pin: $OLDB -> $NEWB   prefix $PREFIX"
echo "re-pin: tq-offline -> $OLD_NAME, $NEW -> tq-offline, control $CONTROL"
(( DRY )) && echo "re-pin: dry run, nothing is changed"

step() {   # DESCRIPTION CMD...  (printed always, run unless --dry-run)
    local d=$1; shift
    echo "== $d"
    printf '   +'; printf ' %q' "$@"; printf '\n'
    (( DRY )) || "$@"
}

step "rename the old pinned copy" mv -- "$EVE/tq-offline" "$EVE/$OLD_NAME"
step "the new copy becomes tq-offline" mv -- "$EVE/$NEW" "$EVE/tq-offline"
# apply skips patched files and writes the prefs keys that differ; they live
# under the install name (c_ccp_eve_tq-offline_*)
step "restore the prefs under the new tq-offline" python3 -I "$REPO/tools/eve_patch.py" apply --client "$EVE/tq-offline"
step "pin the new install (commit and land the manifest with the registry)" \
    env PREFIX="$EVE" "$REPO/tools/pin-client.sh" --write
if [[ -e $REPO/out/pyc ]]; then step "keep the old corpus" mv -- "$REPO/out/pyc" "$REPO/out/pyc-$OLDB"; fi
if [[ -e $REPO/out/content ]]; then step "keep the old bundle" mv -- "$REPO/out/content" "$REPO/out/content-$OLDB"; fi
if (( DEFAULT_PREFIX )); then
    step "drop the second client" "$REPO/tools/client2-setup.sh" --remove
    step "set the second client up again" "$REPO/tools/client2-setup.sh"
else
    echo "== second client: skipped (--prefix; client2-setup.sh builds from the offline prefix)"
fi
step "drop the slot prefixes (a stale one keeps the old settings)" \
    env EVE_SLOT_SRC_PREFIX="$PREFIX" "$REPO/tools/client-ci/slot-prefix.sh" remove all

cat <<TXT

re-pin: done$( (( DRY )) && echo " (dry run)").  By hand, next (build-compat.md Re-pinning step 5):
  - extract the new code.ccp into out/pyc (from code.ccp.eveoffline-orig if tier C patched it)
  - rebuild out/content with tools/content.py for build $NEWB
  - regenerate the corpus tables, one commit each; commit tools/client-pin.sha256, then tools/land.sh --head
  - census.sh now defaults to the control $CONTROL
TXT
