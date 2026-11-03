#!/usr/bin/env bash
# A friend's one-time setup, in steps (F-KIT, issue #140). Idempotent: run a
# step again and it says what is done and changes nothing that is.
#
#   tools/friend-setup.sh check    # read only: build, disk, Proton, ResFiles, jail probe
#   tools/friend-setup.sh copy     # tq -> tq-offline, then your own prefix (offline-prefix.sh)
#   tools/friend-setup.sh patch    # pin check, plan, apply, verify on tq-offline
#
# The client is not in the kit: you need a full EVE install (Steam or the
# launcher, with all resource files downloaded) at the pinned build.
# `copy` writes one new directory, `tq-offline`, beside `tq` in your online
# prefix; it never changes `tq`. It refuses while Wine runs in that prefix.
#
# Paths: STEAM_ROOT (default ~/.local/share/Steam; the Ubuntu .deb's is
# ~/.steam/debian-installation, Flatpak's is under ~/.var/app), EVE_ONLINE_PREFIX
# (the Steam prefix your EVE lives in, default $STEAM_ROOT/steamapps/compatdata/8500;
# set it when your library is elsewhere), EVE_OFFLINE_PREFIX (where your own
# prefix goes), PROTON, as in tools/offline-prefix.sh. The launch scripts
# (tools/jail-rootless.sh) read the offline prefix from EVE_PREFIX, not
# EVE_OFFLINE_PREFIX: if you set the latter, set EVE_PREFIX to the same path.
#
# `copy` makes your own prefix with hard-linked resource files (about 10G) when
# they and the prefix are on one filesystem. `copy --full` makes a real copy
# instead (about 60G), for a prefix on another disk.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"

say() { printf '%s\n' "$*"; }
die() { printf 'friend-setup: %s\n' "$*" >&2; exit 1; }

(( EUID != 0 )) || die "run this as yourself, not as root"
: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
: "${EVE_ONLINE_PREFIX:=$STEAM_ROOT/steamapps/compatdata/$EVE_ONLINE_PREFIX_ID}"
: "${EVE_OFFLINE_PREFIX_SET:=}"
[[ -z ${EVE_OFFLINE_PREFIX:-} ]] || EVE_OFFLINE_PREFIX_SET=1
: "${EVE_OFFLINE_PREFIX:=$(eve_offline_prefix "$HOME")}"
: "${PROTON:=$STEAM_ROOT/compatibilitytools.d/GE-Proton10-34/proton}"
EVE_REL=pfx/drive_c/CCP/EVE
ON=$EVE_ONLINE_PREFIX/$EVE_REL
OFF=$EVE_OFFLINE_PREFIX/$EVE_REL

# The pinned build: the `pinned` row of tools/builds.tsv.
pinned=$(awk -F'\t' '!/^#/ && NF == 5 && $3 == "pinned" { print $1; exit }' "$REPO/tools/builds.tsv" 2>/dev/null || true)
[[ -n $pinned ]] || die "tools/builds.tsv has no pinned build (is this the kit?)"

ini_get() {   # FILE KEY
    sed -n "s/^[[:space:]]*$2[[:space:]]*=[[:space:]]*//p" "$1" 2>/dev/null | tr -d '\r' | head -1 || true
}

# Is a Wine or Proton process using this prefix? (prefix-lib.sh: eve_prefix_busy)
prefix_busy() { eve_prefix_busy "$1" >/dev/null; }

# resfileindex.txt and resfileindex_Windows.txt: rows are
# path,file,md5,size,compressed-size; the file is under ResFiles by its second
# field and has the first size. Every index must have rows, and every row's
# file must exist at that size. Prints "checked N missing M wrong-size S" (a
# few names go to stderr); exit 0 only when all is well.
res_rows() {   # CLIENT_DIR RESFILES_DIR
    python3 -I -c '
import os, sys
cd, res = sys.argv[1], sys.argv[2]
n = miss = bad = 0
ok = True
for name in ("resfileindex.txt", "resfileindex_Windows.txt"):
    rows = 0
    try:
        f = open(os.path.join(cd, name), encoding="utf-8", errors="replace")
    except OSError:
        print("  no " + name, file=sys.stderr)
        ok = False
        continue
    with f:
        for line in f:
            p = line.rstrip("\r\n").split(",")
            if len(p) < 4:
                continue
            rows += 1
            path = os.path.join(res, p[1])
            try:
                size = os.stat(path).st_size
            except OSError:
                miss += 1
                if miss <= 3:
                    print("  missing " + p[1], file=sys.stderr)
                continue
            if p[3].isdigit() and size != int(p[3]):
                bad += 1
                if bad <= 3:
                    print("  wrong size " + p[1], file=sys.stderr)
    n += rows
    if rows == 0:
        print("  " + name + " has no rows", file=sys.stderr)
        ok = False
print("checked %d missing %d wrong-size %d" % (n, miss, bad))
sys.exit(0 if ok and miss == 0 and bad == 0 else 1)
' "$1" "$2"
}

# The nearest directory that exists at or above PATH.
existing_dir() { local d=$1; while [[ ! -d $d && $d != / ]]; do d=$(dirname -- "$d"); done; printf '%s' "$d"; }
dev_of() { stat -c %d -- "$(existing_dir "$1")"; }
same_fs() { [[ $(dev_of "$1") == "$(dev_of "$2")" ]]; }

# The way forward for a tq-offline that is not the pinned build.
stale_help() {
    say "        Make it again from your install (it must be build $pinned):"
    say "          rm -rf '$ON/tq-offline'"
    say "          tools/offline-prefix.sh --force --link-resfiles   # replaces your own prefix, which holds the old copy"
    say "          tools/friend-setup.sh copy"
}

# Run the ResFiles test, print its result; 0 when the install is complete.
# (shared by check and copy)
res_ok() {
    local out r=0
    out=$(res_rows "$ON/tq" "$ON/ResFiles" 2>&1) || r=1
    if (( r == 0 )); then say "  ok    ResFiles complete (${out##*$'\n'})"; return 0; fi
    say "  FAIL  ResFiles incomplete: ${out##*$'\n'}"
    [[ $out != *$'\n'* ]] || say "${out%$'\n'*}"
    say "        Let the launcher or Steam finish the full download, then run this again."
    return 1
}

step_check() {
    local rc=0 b ob free need_k=0 res_k=0
    say "== friend-setup check (pinned build $pinned)"
    ob=$(ini_get "$ON/tq-offline/start.ini" build)
    [[ -n $ob ]] || ob=$(ini_get "$OFF/tq-offline/start.ini" build)
    if [[ -f $ON/tq/start.ini ]]; then
        b=$(ini_get "$ON/tq/start.ini" build)
        if [[ $b == "$pinned" ]]; then say "  ok    your install is build $b"
        elif [[ $ob == "$pinned" ]]; then
            say "  ok    your install moved on to build ${b:-unknown}, but your tq-offline copy is the pinned $pinned: play on that one"
            say "        Do not let the launcher touch tq-offline; the copy step is done."
        else
            say "  FAIL  your install is build ${b:-unknown}, the server needs $pinned."
            say "        You cannot play on this copy. Do you hold a frozen $pinned copy (a backup, a second library)?"
            say "        The launcher updates in place, so ask the owner before you update again."
            rc=1
        fi
    elif [[ -f $OFF/tq-offline/start.ini ]]; then
        say "  ok    no online install, but tq-offline exists (the copy step is done)"
    else
        say "  FAIL  no EVE install at $ON/tq (set STEAM_ROOT or EVE_ONLINE_PREFIX; Flatpak and the Ubuntu .deb keep Steam elsewhere)"
        rc=1
    fi
    if [[ -n $ob && $ob != "$pinned" ]]; then
        say "  FAIL  your tq-offline copy is build $ob, not the pinned $pinned (left from before a re-pin?)"
        stale_help
        rc=1
    fi
    if [[ -f $ON/tq/resfileindex.txt && -d $ON/ResFiles ]]; then
        res_ok || rc=1
    elif [[ -d $ON/tq ]]; then
        say "  FAIL  no ResFiles or resfileindex.txt: the install is not fully downloaded"; rc=1
    fi
    if [[ -d $ON/tq ]]; then
        need_k=$(du -sk -- "$ON/tq" | cut -f1)
        [[ ! -d $ON/ResFiles ]] || res_k=$(du -sk -- "$ON/ResFiles" | cut -f1)
        free=$(df -k --output=avail -- "$HOME" | tail -1 | tr -d ' ')
        if same_fs "$ON/ResFiles" "$EVE_OFFLINE_PREFIX"; then
            if (( free > need_k * 3 + 12582912 )); then say "  ok    disk: $(( free / 1048576 ))G free; ResFiles can be hard-linked (about 10G)"
            else say "  WARN  disk: $(( free / 1048576 ))G free; the copy needs about 10G"; fi
        else
            say "  WARN  your resource files and $EVE_OFFLINE_PREFIX are on different filesystems: they cannot be hard-linked."
            say "        Use: tools/friend-setup.sh copy --full   (a real copy, about $(( (need_k + res_k) / 1048576 + 8 ))G; $(( free / 1048576 ))G free here)"
            (( free > (need_k + res_k) + 8388608 )) || { say "  FAIL  not enough room for a full copy"; rc=1; }
        fi
    fi
    if [[ -x $PROTON ]]; then say "  ok    Proton at $PROTON"
    else say "  FAIL  no Proton at $PROTON (GE-Proton10-34 under compatibilitytools.d, or set PROTON)"; rc=1; fi
    if "$REPO/tools/jail-rootless.sh" probe; then :; else rc=1; fi
    (( rc == 0 )) && say "ready for: tools/friend-setup.sh copy"
    return "$rc"
}

COPY_PART=""
cleanup_copy() { [[ -z $COPY_PART ]] || rm -rf -- "$COPY_PART"; }

step_copy() {
    local full=0 a b ob need_k free
    for a in "$@"; do
        case $a in
            --full) full=1 ;;
            *) die "copy: unknown argument $a (only --full)" ;;
        esac
    done
    local part=$ON/tq-offline.partial
    ob=$(ini_get "$ON/tq-offline/start.ini" build)
    if [[ -f $ON/tq-offline/bin64/exefile.exe && $ob == "$pinned" ]]; then
        say "== tq-offline already exists in the online prefix (build $ob); not copying it again"
    else
        if [[ -e $ON/tq-offline && -n $ob && $ob != "$pinned" ]]; then
            say "  FAIL  $ON/tq-offline is build $ob, not the pinned $pinned"; stale_help; exit 1
        fi
        [[ -f $ON/tq/start.ini ]] || die "no install at $ON/tq (run check)"
        b=$(ini_get "$ON/tq/start.ini" build)
        [[ $b == "$pinned" ]] || die "your install is build ${b:-unknown}, not the pinned $pinned: you cannot play on it (run check)"
        [[ -f $ON/tq/resfileindex.txt && -d $ON/ResFiles ]] || die "no ResFiles or resfileindex.txt: the install is not fully downloaded (run check)"
        res_ok || exit 1
        if prefix_busy "$EVE_ONLINE_PREFIX"; then die "Wine is running in $EVE_ONLINE_PREFIX (pids $(eve_prefix_busy "$EVE_ONLINE_PREFIX")); close EVE and the launcher first"; fi
        [[ ! -e $ON/tq-offline ]] || die "$ON/tq-offline exists but is incomplete; remove it (rm -rf '$ON/tq-offline') and run again"
        need_k=$(du -sk -- "$ON/tq" | cut -f1)
        free=$(df -k --output=avail -- "$ON" | tail -1 | tr -d ' ')
        (( free > need_k + 1048576 )) || die "not enough room next to $ON/tq: it needs $(( need_k / 1048576 ))G and 1G more, $(( free / 1048576 ))G free"
        # A half copy from an earlier, stopped run is not a copy: remove it so the
        # new one does not land inside it.
        rm -rf -- "$part"
        COPY_PART=$part
        trap cleanup_copy EXIT
        trap 'exit 130' INT TERM
        say "== copying tq to tq-offline (a real copy; tq itself is not changed)"
        cp -aT --reflink=auto -- "$ON/tq" "$part"
        # The launcher or a Wine may have started while it copied.
        if prefix_busy "$EVE_ONLINE_PREFIX"; then die "Wine started in $EVE_ONLINE_PREFIX during the copy; nothing was kept. Close EVE and run again"; fi
        [[ $(ini_get "$ON/tq/start.ini" build) == "$pinned" && $(ini_get "$part/start.ini" build) == "$pinned" ]] \
            || die "the build changed during the copy; nothing was kept. Run check"
        [[ -f $part/bin64/exefile.exe ]] || die "the copy has no bin64/exefile.exe; nothing was kept"
        mv -T -- "$part" "$ON/tq-offline"
        COPY_PART=""
        trap - EXIT INT TERM
    fi
    local -a opts=(--link-resfiles)
    if (( full )); then opts=()
    elif ! same_fs "$ON/ResFiles" "$EVE_OFFLINE_PREFIX"; then
        die "your resource files and $EVE_OFFLINE_PREFIX are on different filesystems, so they cannot be hard-linked.
  Run: tools/friend-setup.sh copy --full    (a real copy: about 60G; check says how much room there is)"
    fi
    if [[ -f $EVE_OFFLINE_PREFIX/eve-offline-prefix.txt ]]; then
        say "== your own prefix exists already: $EVE_OFFLINE_PREFIX"
        # Display only, so a rerun is safe: it changes nothing, and before the
        # patch step its cryptoPack line is expected to say MISS.
        "$REPO/tools/offline-prefix.sh" --check || say "  (a MISS before 'patch' is expected for cryptoPack; if the prefix itself is damaged: tools/offline-prefix.sh --force ${opts[*]})"
        [[ -f $OFF/tq-offline/bin64/exefile.exe ]] || { say "  FAIL  your own prefix has no tq-offline client; make it again: tools/offline-prefix.sh --force ${opts[*]}"; exit 1; }
        ob=$(ini_get "$OFF/tq-offline/start.ini" build)
        if [[ $ob != "$pinned" ]]; then
            say "  FAIL  your own prefix holds tq-offline build ${ob:-unknown}, not the pinned $pinned"; stale_help; exit 1
        fi
    elif [[ -e $EVE_OFFLINE_PREFIX.partial ]]; then
        say "== a half-made prefix from an earlier run is at $EVE_OFFLINE_PREFIX.partial; making it again"
        "$REPO/tools/offline-prefix.sh" --force "${opts[@]}"
    else
        "$REPO/tools/offline-prefix.sh" "${opts[@]}"
    fi
    if [[ -n ${EVE_OFFLINE_PREFIX_SET:-} ]]; then say "note: also set EVE_PREFIX='$EVE_OFFLINE_PREFIX' when you launch (tools/jail-rootless.sh reads that name)"; fi
    say "next: tools/friend-setup.sh patch"
}

step_patch() {
    local c=$OFF/tq-offline
    [[ -f $c/start.ini ]] || die "no $c (run: tools/friend-setup.sh copy)"
    [[ $(ini_get "$c/start.ini" build) == "$pinned" ]] \
        || die "this copy is build $(ini_get "$c/start.ini" build), not the pinned $pinned: you cannot play on it"
    # pin-client.sh accepts the stock files and the patcher's patched ones, so a
    # failure here is a real difference: a damaged or foreign copy.
    "$REPO/tools/pin-client.sh" --path "$c" || die "$c does not match the pinned build (pin-client.sh, above); make it again: tools/friend-setup.sh copy"
    say "== plan"
    python3 -I "$REPO/tools/eve_patch.py" plan --client "$c"
    say "== apply"
    python3 -I "$REPO/tools/eve_patch.py" apply --client "$c"
    say "== verify"
    python3 -I "$REPO/tools/eve_patch.py" verify --client "$c"
    say "next: tools/account.sh login, then the tunnel (docs/guides/playing-with-friends.md)"
}

case "${1:-}" in
    check) step_check ;;
    copy)  shift; step_copy "$@" ;;
    patch) step_patch ;;
    -h|--help|'') sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; [[ -n ${1:-} ]] || exit 2 ;;
    *) die "unknown step $1 (check, copy, patch)" ;;
esac
