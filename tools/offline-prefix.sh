#!/usr/bin/env bash
# Give the offline client a Wine prefix of its own (owner decision O19).
#
#   tools/offline-prefix.sh                   # make it, once: a full copy
#   tools/offline-prefix.sh --link-resfiles   # the same, ResFiles hard-linked
#   tools/offline-prefix.sh --check           # what is there; changes nothing
#   tools/offline-prefix.sh --force [...]     # replace a copy this script made
#
# Why
# ---
# The jailed client used to run in prefix 8500, the Wine prefix the owner's
# ONLINE, Steam-launched EVE uses, and the jail binds that prefix in
# read-write. A compromised jailed client, or a hostile server driving it
# (the server can run Python in the client: docs/guides/jail.md), could plant a DLL
# there that the online client later loads, outside the jail (ruling 14).
# So the offline client gets a copy, made once, and every jailed launch path
# uses the copy. The online prefix is never bound into the jail, not even
# read-only, and the helper refuses a launch that names it.
#
# What it copies
# --------------
# Everything in compatdata/8500 -- the bottle, the registry, the Wine user
# profile, `tq-offline` with its cryptoPack and blue.dll patches, and the
# client's settings -- EXCEPT the online game installs `CCP/EVE/tq` and
# `CCP/EVE/sisi`, which the offline client never runs (and which no jailed
# process should be able to touch, even as copies), and the launcher's login
# state (everything of the launcher's and the online client's that holds a
# session: `AppData/Roaming/EVE Online`, all of `AppData/Local/CCP` but the
# offline client's own `c_ccp_eve_tq-offline_*`, `.ccp`, `AppData/Local/Temp`,
# `CCP/EVE/{settings.dat,history.txt,downloads,metadata}` and the CEF caches),
# matched without regard to case, and checked again by name afterwards. Then
# ResFiles:
#
#   default            a real copy (`cp -a --reflink=auto`). On ext4, where
#                      reflinks do not exist, that is the whole store: about
#                      48G on one test machine (2026-10-26), plus about 8G
#                      for the rest of the prefix. On btrfs or xfs it is a
#                      reflink and costs almost nothing until a file changes.
#   --link-resfiles    hard links (`cp -al`): about 8G in all. The files are
#                      the online store's own inodes, so the jail MUST NOT be
#                      able to write them, and it cannot: both jails bind
#                      `<client>/../ResFiles` read-only on top of the
#                      read-write prefix, and tools/jail-client.sh refuses to
#                      launch if ResFiles is writable from inside. Needs the
#                      copy and Steam on one filesystem.
#
# It never writes to the online prefix: it reads it, and every write goes to
# `<dest>.partial`, renamed to `<dest>` only once the copy is whole. It
# refuses while a Wine or Proton process is using the online prefix, since a
# registry copied mid-write is a broken one.
#
# Where: ~/.local/share/eve-offline/prefix (tools/prefix-lib.sh says why).
# Override with EVE_OFFLINE_PREFIX; the source with EVE_ONLINE_PREFIX (tests).
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"

say() { printf '%s\n' "$*"; }
die() { printf 'offline-prefix.sh: %s\n' "$*" >&2; exit 1; }

FORCE=0 LINK=0 CHECK=0
for a in "$@"; do
    case $a in
        --force) FORCE=1 ;;
        --link-resfiles) LINK=1 ;;
        --check) CHECK=1 ;;
        -h | --help) sed -n '2,7p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument $a (see --help)" ;;
    esac
done

(( EUID != 0 )) || die "run this as yourself, not as root: the prefix is yours"
: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
: "${EVE_ONLINE_PREFIX:=$STEAM_ROOT/steamapps/compatdata/$EVE_ONLINE_PREFIX_ID}"
: "${EVE_OFFLINE_PREFIX:=$(eve_offline_prefix "$HOME")}"
SRC=$(realpath -m -- "$EVE_ONLINE_PREFIX")
DST=$(realpath -m -- "$EVE_OFFLINE_PREFIX")
STAGE=$DST.partial
MARK=eve-offline-prefix.txt
EVE_REL=pfx/drive_c/CCP/EVE
# What is left out, by lowercased path relative to the prefix (Windows names
# are case-insensitive, and so is a hostile one). `Application Data` and
# `Local Settings` are Wine's old names for AppData; when one is a real
# directory its path is folded onto the AppData one first (norm_rel).
#   CCP/EVE/{tq,sisi}            the online installs (ResFiles is handled below)
#   CCP/EVE/{settings.dat,history.txt,downloads,metadata}, CCP/EVE/c_*/cache/cef
#                                the online launcher's and client's own state
#   users/*/AppData/Local/CCP/*  the launcher's login state; only the offline
#                                client's EVE/c_ccp_eve_tq-offline_* settings stay
#   users/*/AppData/Roaming/EVE Online, users/*/AppData/Local/Temp, .ccp, .eve-online
# F3: the launcher's login state (cookies, tokens) lives in the Wine profile. The
# jailed client's prefix is read-write for code the server can drive, so it must
# not hold a real CCP session. After the copy a sweep (sweep_secrets) refuses on
# any name that looks like one that got through, wherever it is.
norm_rel() {
    local l=$1 u='^(pfx/drive_c/users/[^/]+)/'
    if [[ $l =~ $u"local settings/application data"(.*)$ ]]; then l="${BASH_REMATCH[1]}/appdata/local${BASH_REMATCH[2]}"
    elif [[ $l =~ $u"local settings/temp"(.*)$ ]]; then l="${BASH_REMATCH[1]}/appdata/local/temp${BASH_REMATCH[2]}"
    elif [[ $l =~ $u"application data"(.*)$ ]]; then l="${BASH_REMATCH[1]}/appdata/roaming${BASH_REMATCH[2]}"
    fi
    printf '%s' "$l"
}
excluded_rel() {   # lowercased, normalised path -> 0 if it is left out
    case $1 in
        pfx/drive_c/users/*/appdata/local/ccp/c_ccp_eve_tq-offline_*) return 1 ;;
        pfx/drive_c/users/*/appdata/local/ccp/eve | pfx/drive_c/users/*/appdata/local/ccp/eve/c_ccp_eve_tq-offline_*) return 1 ;;
        pfx/drive_c/ccp/eve/tq | pfx/drive_c/ccp/eve/sisi | pfx/drive_c/ccp/eve/resfiles) return 0 ;;
        pfx/drive_c/ccp/eve/settings.dat | pfx/drive_c/ccp/eve/history.txt) return 0 ;;
        pfx/drive_c/ccp/eve/downloads | pfx/drive_c/ccp/eve/metadata) return 0 ;;
        pfx/drive_c/ccp/eve/c_*/cache/cef) return 0 ;;
        pfx/drive_c/users/*/appdata/local/ccp/* | pfx/drive_c/users/*/appdata/local/temp) return 0 ;;
        pfx/drive_c/users/*/appdata/roaming/eve\ online) return 0 ;;
        .ccp | .eve-online | pfx/drive_c/users/*/.ccp | pfx/drive_c/users/*/.eve-online) return 0 ;;
        pfx/drive_c/users/*/local\ settings/application\ data | pfx/drive_c/users/*/local\ settings/temp) return 0 ;;
    esac
    return 1
}
walk_rel() {   # a directory that holds something excluded: copy it entry by entry
    local l=$1 w='^pfx/drive_c/users/[^/]+(/(appdata|appdata/local|appdata/local/ccp|appdata/local/ccp/eve|appdata/roaming|local settings|application data))?$'
    [[ -z $l || $l == pfx || $l == pfx/drive_c || $l == pfx/drive_c/users || $l == pfx/drive_c/ccp || $l == pfx/drive_c/ccp/eve ]] && return 0
    [[ $l =~ $w ]] && return 0
    [[ $l =~ ^pfx/drive_c/ccp/eve/c_[^/]+(/cache)?$ ]]
}
# Names that mean a login session or a launcher's state, anywhere in a tree
# (ResFiles is hashes and is pruned). Prints what it finds.
sweep_secrets() {   # DIR
    find "$1" -path "$1/$EVE_REL/ResFiles" -prune -o \( -iname 'cookies*' -o -iname 'local storage' \
        -o -iname 'session storage' -o -iname 'login data*' -o -iname 'eve online' \
        -o -iname 'user_information*' \) -print 2>/dev/null
}

check() {
    local rc=0 pack f
    say "== offline prefix: $DST"
    if [[ ! -d $DST/pfx ]]; then
        say "  MISS  no prefix here; make it with tools/offline-prefix.sh"
        return 1
    fi
    if [[ -f $DST/$MARK ]]; then
        sed 's/^/  ok    /' "$DST/$MARK"
    else
        say "  MISS  no $MARK: not made by this script"; rc=1
    fi
    if [[ -f $DST/$EVE_REL/tq-offline/bin64/exefile.exe ]]; then
        say "  ok    client copy tq-offline"
        pack=$(sed -n 's/^[[:space:]]*cryptoPack[[:space:]]*=[[:space:]]*//p' \
            "$DST/$EVE_REL/tq-offline/start.ini" 2>/dev/null | tr -d '\r')
        if [[ $pack == Placebo ]]; then say "  ok    cryptoPack = Placebo"
        else say "  MISS  cryptoPack = ${pack:-unset} (tools/client-prepare.sh)"; rc=1; fi
    else
        say "  MISS  no client at $DST/$EVE_REL/tq-offline"; rc=1
    fi
    if [[ -d $DST/$EVE_REL/ResFiles && ! -L $DST/$EVE_REL/ResFiles ]]; then
        f=$(find "$DST/$EVE_REL/ResFiles" -type f -print -quit 2>/dev/null || true)
        if [[ -n $f && $(stat -c %h -- "$f") -gt 1 ]]; then
            say "  ok    ResFiles (hard-linked: the jail binds it read-only)"
        else
            say "  ok    ResFiles (its own copy)"
        fi
    else
        say "  MISS  no ResFiles directory"; rc=1
    fi
    for f in tq sisi; do
        [[ ! -e $DST/$EVE_REL/$f ]] || { say "  WARN  the online install $f is in the copy"; rc=1; }
    done
    f=$(sweep_secrets "$DST" | head -5)
    if [[ -n $f ]]; then say "  FAIL  login state in the copy:"; say "$f" | sed 's/^/          /'; rc=1; fi
    return $rc
}

if (( CHECK )); then
    check && say "ready: the jailed client uses $DST" || exit 1
    exit 0
fi

# --- refusals, before anything is written -----------------------------------
[[ -d $SRC/pfx/drive_c ]] || die "no online prefix at $SRC (set STEAM_ROOT or EVE_ONLINE_PREFIX)"
[[ -f $SRC/$EVE_REL/tq-offline/bin64/exefile.exe ]] || die "no client copy at $SRC/$EVE_REL/tq-offline.
  Make tq-offline first (docs/guides/jail.md, \"We run a copy\"); this copies it along with the prefix."
[[ $DST != "$SRC" && $DST != "$SRC"/* && $SRC != "$DST"/* ]] \
    || die "the copy ($DST) and the online prefix ($SRC) overlap"
! eve_is_online_path "$DST" || die "$DST is inside an online prefix (compatdata/$EVE_ONLINE_PREFIX_ID)"

# Only a copy this script made is replaced, and only when asked.
for d in "$DST" "$STAGE"; do
    [[ -e $d || -L $d ]] || continue
    (( FORCE )) || die "$d already exists; not replacing it.
  tools/offline-prefix.sh --check     says what it holds
  tools/offline-prefix.sh --force     replaces it (anything the client saved there is lost)"
    [[ -d $d && ! -L $d ]] || die "$d is not a directory; remove it yourself"
    [[ $d == "$STAGE" || -f $d/$MARK ]] || die "$d was not made by this script (no $MARK); remove it yourself"
done

# Nothing may be running in the online prefix while its registry is copied
# (prefix-lib.sh says what counts).
if busy=$(eve_prefix_busy "$SRC"); then die "processes are using the online prefix (pids $busy). Close EVE first."; fi

mkdir -p -- "$(dirname -- "$DST")"
if (( LINK )); then
    [[ $(stat -c %d -- "$SRC/$EVE_REL/ResFiles") == "$(stat -c %d -- "$(dirname -- "$DST")")" ]] \
        || die "--link-resfiles needs $DST on the same filesystem as $SRC"
fi

# --- space --------------------------------------------------------------------
need_k=$(du -sk -- "$SRC" | cut -f1)
for x in "$EVE_REL/tq" "$EVE_REL/sisi"; do
    [[ -d $SRC/$x ]] && need_k=$(( need_k - $(du -sk -- "$SRC/$x" | cut -f1) ))
done
(( LINK )) && [[ -d $SRC/$EVE_REL/ResFiles ]] && need_k=$(( need_k - $(du -sk -- "$SRC/$EVE_REL/ResFiles" | cut -f1) ))
free_before=$(df -k --output=avail -- "$(dirname -- "$DST")" | tail -1 | tr -d ' ')
say "== space: the copy needs up to $(( need_k / 1048576 ))G (less with reflinks), $(( free_before / 1048576 ))G free"
(( free_before > need_k + 1048576 )) || die "not enough space (need ${need_k}K plus 1G headroom)"

# --- the copy -------------------------------------------------------------------
for d in "$STAGE" "$DST"; do
    if [[ -d $d ]]; then say "== removing the old $d"; rm -rf -- "$d"; fi
done

# copy_except SRC_DIR DST_DIR REL: copy SRC_DIR into DST_DIR, skipping what
# excluded_rel names (REL is SRC_DIR's own path in the prefix). Only the
# ancestors of an excluded path are walked; everything else is one `cp -a`.
copy_except() {
    local from=$1 to=$2 rel=$3 e r l
    mkdir -p -- "$to"
    chmod --reference="$from" -- "$to"
    shopt -s dotglob nullglob
    for e in "$from"/*; do
        r=${rel:+$rel/}${e##*/}
        l=$(norm_rel "${r,,}")
        if excluded_rel "$l"; then continue
        elif [[ -d $e && ! -L $e ]] && walk_rel "$l"; then copy_except "$e" "$to/${e##*/}" "$r"
        else cp -a --reflink=auto -- "$e" "$to/"
        fi
    done
    shopt -u dotglob nullglob
    touch -r "$from" -- "$to"
}

say "== copying $SRC (without CCP/EVE/tq, CCP/EVE/sisi) into $STAGE"
copy_except "$SRC" "$STAGE" ""
if [[ -d $SRC/$EVE_REL/ResFiles ]]; then
    if (( LINK )); then
        say "== hard-linking ResFiles (read-only inside the jail)"
        cp -al -- "$SRC/$EVE_REL/ResFiles" "$STAGE/$EVE_REL/ResFiles"
        resmode="hard-linked from the online prefix (read-only in the jail)"
    else
        say "== copying ResFiles (the big one)"
        cp -a --reflink=auto -- "$SRC/$EVE_REL/ResFiles" "$STAGE/$EVE_REL/ResFiles"
        resmode="its own copy"
    fi
else
    resmode="absent in the source"
fi

# A symlink that leads back into the online prefix would undo the point.
leaks=$(find "$STAGE" -type l -print0 | while IFS= read -r -d '' l; do
    t=$(readlink -m -- "$l")
    if [[ $t == "$SRC" || $t == "$SRC"/* ]] || eve_is_online_path "$t"; then printf '%s -> %s\n' "$l" "$t"; fi
done)
[[ -z $leaks ]] || die "the copy has symlinks into the online prefix; left at $STAGE for inspection:
$leaks"
found=$(sweep_secrets "$STAGE")
[[ -z $found ]] || die "the copy holds names that look like a login session; left at $STAGE for inspection:
$found"

{
    printf 'copied from %s\n' "$SRC"
    printf 'copied on   %s by tools/offline-prefix.sh\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'ResFiles    %s\n' "$resmode"
    printf 'left out    CCP/EVE/tq, CCP/EVE/sisi (the online installs), the launcher login state\n'
} > "$STAGE/$MARK"
mv -T -- "$STAGE" "$DST"

free_after=$(df -k --output=avail -- "$(dirname -- "$DST")" | tail -1 | tr -d ' ')
say "== done: $DST"
say "   disk used: $(( (free_before - free_after) / 1024 ))M; ResFiles: $resmode"
say "   the online prefix was only read: $SRC"
echo
check || true
cat <<EOF

Every jailed launch now uses this prefix (tools/run-client.sh, tools/up.sh,
tools/client-ci.sh). Next:
    tools/pin-client.sh                 # the copy is still the pinned build
    tools/client2-setup.sh              # only if you use a second client:
                                        # points its ResFiles at this copy
The old client copy in the online prefix is no longer used; remove it when
you like (the online install is not touched by this):
    rm -rf '$SRC/$EVE_REL/tq-offline'
EOF
