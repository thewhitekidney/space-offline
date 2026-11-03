#!/usr/bin/env bash
# The Wine prefix of one parallel client-CI slot (lane ci-parallel, issue #139).
#
#   tools/client-ci/slot-prefix.sh ensure SLOT [--second] [--client DIR]
#   tools/client-ci/slot-prefix.sh check  SLOT [--second] [--client DIR]
#   tools/client-ci/slot-prefix.sh remove SLOT|all
#   tools/client-ci/slot-prefix.sh size
#
# Two clients cannot share a prefix: one prefix is one wineserver, one registry,
# one settings file, one verdict directory and one install directory whose
# cache the client writes (tools/client2-setup.sh says so). So every slot has a
# prefix of its own, made from the offline client's prefix (tools/offline-prefix.sh):
#
#   - a COPY of the Wine prefix (registry, fonts, the user directory), without the
#     other installs, their settings folders, the old logs, the launcher's
#     `downloads` and the `c_program_*` shared caches of installs this slot never runs;
#   - the client install, copied and RENAMED to <DIR>-s<SLOT> (<DIR>-s<SLOT>-2 for
#     the --with-second client), because client-ci.sh finds and stops its client
#     by the install directory's name, and two slots must not match each other;
#   - the client's settings folder (prefs.ini with the chat setting, the cache)
#     copied under the new name, so the first launch is not a first launch;
#   - ResFiles as a symlink to the offline prefix's, as tools/client2-setup.sh does:
#     the store is content-addressed and read-only, and the jail binds it
#     read-only. No copy, no hard links.
#
# `ensure` makes the prefix when it is missing and, every time, replaces the client
# install with a fresh copy, so a slot never tests a build older than the
# offline prefix's. It never touches the offline prefix, and the online prefix
# (compatdata/8500) is refused as a source and as a target. Run it while no client
# runs in the offline prefix: the registry is copied as it stands.
#
# Disk, measured on the owner's prefix (59 GB, of which ResFiles 49 GB): the copy
# without ResFiles and the other installs but WITH `downloads` (4.7 GB) and the
# `c_program_*` caches (1.2 GB and more) is 7.0 GB, so 7.5 GB a slot with the client
# (0.45 GB) and its settings folder. Without those two it is 1.2 GB, so about
# 1.7 GB a slot and prefix: 5 GB for three, 10 GB with a second prefix each. [A] the
# client needs neither: they belong to the launcher and to installs under Program
# Files, and the slot's client is started directly. Falsifier: a slot's client
# log shows it fetching or missing a file the offline prefix's client had; put the
# directory back in the exclude list below. `size` prints what the slots hold now.
#
# Safety: the copy is made as prefix.tmp and moved into place when it is whole, so a
# copy that was interrupted is made again, never used; `ensure` and `remove` refuse
# while a client runs from the source (the registry is copied as it stands) or while
# a runner holds out/client-ci/slots/runner.lock (client-ci-regress.sh holds it
# for its whole run; it sets EVE_CI_RUNNER_LOCK_HELD for the calls it makes itself).
#
# EVE_SLOT_HOME (default $HOME) and EVE_SLOT_SRC_PREFIX (default the offline
# prefix) exist for the tests.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"
# shellcheck source=tools/client-ci/slot-lib.sh
source "$REPO/tools/client-ci/slot-lib.sh"

HOME_DIR=${EVE_SLOT_HOME:-$HOME}
SRC_PREFIX=${EVE_SLOT_SRC_PREFIX:-$(eve_offline_prefix "$HOME_DIR")}
SRC_DIR=${EVE_CLIENT_DIR:-tq-offline}

die() { echo "slot-prefix: $*" >&2; exit 1; }

usage() { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "${BASH_SOURCE[0]}"; exit 2; }

cmd=${1:-}; [[ -n $cmd ]] || usage
shift
slot=''; second=0
if [[ $cmd != size ]]; then
    slot=${1:-}; [[ -n $slot ]] || usage
    shift
fi
while [[ $# -gt 0 ]]; do
    case $1 in
        --second) second=1 ;;
        --client) SRC_DIR=${2:?--client needs a directory name}; shift ;;
        *) die "unknown argument $1" ;;
    esac
    shift
done
[[ $SRC_DIR =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ && $SRC_DIR != tq && $SRC_DIR != sisi ]] || die "bad client directory name '$SRC_DIR'"

if [[ $cmd != size && ! ( $cmd == remove && $slot == all ) ]]; then
    ci_slot_vars "$REPO" "$HOME_DIR" "$slot" "$SRC_DIR" || die "SLOT must be a number from 1 to 999, not '$slot'"
    if (( second )); then dst=$SLOT_PREFIX2 dst_client=$SLOT_CLIENT_DIR2; else dst=$SLOT_PREFIX dst_client=$SLOT_CLIENT_DIR; fi
    eve_refuse_online slot-prefix.sh "the source prefix=$SRC_PREFIX" "the slot prefix=$dst"
fi

SRC_EVE=$SRC_PREFIX/pfx/drive_c/CCP/EVE
SETTINGS_REL=pfx/drive_c/users/steamuser/AppData/Local/CCP/EVE

# Is a client of install directory DIR running? Decided by the process's own comm, as
# client-ci.sh's client_pids does: a pgrep is not a client.
client_live() {
    local pid comm
    for pid in $(pgrep -f "$1.bin64.exefile" 2>/dev/null || true); do
        comm=$(cat "/proc/$pid/comm" 2>/dev/null || true)
        case $comm in pgrep | pkill | '') continue ;; esac
        return 0
    done
    return 1
}

# Take the runner lock for this call unless a runner made it (it already holds it).
take_lock() {
    [[ -z ${EVE_CI_RUNNER_LOCK_HELD:-} ]] || return 0
    local lock; lock=$(ci_runner_lock "$REPO")
    mkdir -p "$(dirname "$lock")"
    exec 9>"$lock"
    flock -n 9 || die "a client-ci-regress run holds $lock; its slots are in use"
}

settings_folders() {   # DIR NAME: the settings folders "c_ccp_eve_NAME_*" under DIR
    local d
    for d in "$1"/c_ccp_eve_"$2"_*; do [[ -d $d ]] && printf '%s\n' "$d"; done
    return 0
}

ensure() {
    [[ -f $SRC_EVE/$SRC_DIR/bin64/exefile.exe ]] || die "no client at $SRC_EVE/$SRC_DIR (tools/offline-prefix.sh makes the prefix)"
    [[ -d $SRC_EVE/ResFiles ]] || die "no ResFiles at $SRC_EVE/ResFiles"
    eve_refuse_online slot-prefix.sh "the source ResFiles=$SRC_EVE/ResFiles"
    local dst_eve=$dst/pfx/drive_c/CCP/EVE f made=0
    if [[ ! -d $dst/pfx ]]; then
        client_live "$SRC_DIR" && die "a client runs from $SRC_DIR in the source prefix; the registry would be copied mid-write. Stop it first"
        echo "== making $dst (a copy of the offline prefix, without ResFiles, the other installs, the launcher's downloads and the old logs)"
        mkdir -p "$(dirname "$dst")"
        made=1
        # Into prefix.tmp, then one rename: a copy that dies half way leaves no $dst/pfx
        # for the next ensure to take as done.
        local tmp=$dst.tmp
        rm -rf -- "${tmp:?}"
        mkdir -p "$tmp"
        # tar, because cp has no exclude. Everything under EVE/ except the directory
        # itself, every settings folder, the launcher's downloads, the shared caches of
        # other installs and the log directory's contents stay behind.
        tar -C "$SRC_PREFIX" -cf - \
            --exclude='./pfx/drive_c/CCP/EVE/*' \
            --exclude='./pfx/drive_c/users/*/AppData/Local/CCP/EVE/c_ccp_eve_*' \
            --exclude='./pfx/drive_c/users/*/AppData/Local/CCP/EVE/c_program_*' \
            --exclude='./pfx/drive_c/users/*/AppData/Local/CCP/EVE/downloads' \
            --exclude='./pfx/drive_c/eve-logs/*' \
            --exclude='./pfx/drive_c/users/*/AppData/Local/CCP/EVE/Crashes/*' \
            . | tar -C "$tmp" -xf - || { rm -rf -- "${tmp:?}"; die "the copy of $SRC_PREFIX failed (disk full?)"; }
        mkdir -p "$tmp/pfx/drive_c/eve-logs"
        rm -rf -- "${dst:?}"
        mv -T -- "$tmp" "$dst"
    fi
    mkdir -p "$dst_eve"
    # The client, fresh every time (0.45 GB), so the slot tests today's build.
    rm -rf -- "${dst_eve:?}/$dst_client"
    cp -a -- "$SRC_EVE/$SRC_DIR" "$dst_eve/$dst_client"
    # ResFiles: a link to the offline prefix's store, never a copy. -n replaces the
    # link itself and never writes through it.
    ln -sfn -- "$SRC_EVE/ResFiles" "$dst_eve/ResFiles"
    # The settings folder under the new name, only when the slot has none yet.
    local from_root=$SRC_PREFIX/$SETTINGS_REL to_root=$dst/$SETTINGS_REL
    mkdir -p "$to_root"
    while IFS= read -r f; do
        local suffix=${f##*/c_ccp_eve_"${SRC_DIR,,}"_} to=$to_root/c_ccp_eve_${dst_client,,}_
        [[ -d ${to}${suffix} ]] || cp -a -- "$f" "${to}${suffix}"
    done < <(settings_folders "$from_root" "${SRC_DIR,,}")
    mkdir -p "$dst/pfx/drive_c/eve-logs"
    printf 'slot %s%s of %s; client %s copied from %s on %s\n' "$slot" "$( ((second)) && echo ' (second)')" \
        "$SRC_PREFIX" "$dst_client" "$SRC_DIR" "$(date -u +%FT%TZ)" > "$dst/eve-offline-slot.txt"
    (( made )) && echo "   made"
    echo "   client $dst_client refreshed from $SRC_DIR"
}

check() {
    local bad=0 dst_eve=$dst/pfx/drive_c/CCP/EVE
    chk() { if "${@:2}"; then printf '  ok    %s\n' "$1"; else printf '  MISS  %s\n' "$1"; bad=1; fi; }
    echo "== slot $slot prefix ($dst)"
    chk "prefix initialised" test -d "$dst/pfx"
    chk "client $dst_client" test -f "$dst_eve/$dst_client/bin64/exefile.exe"
    chk "ResFiles link" test -L "$dst_eve/ResFiles"
    chk "ResFiles resolves, and is not the online prefix's" bash -c '! [[ -L $1 ]] || { r=$(readlink -m -- "$1"); [[ -d $r ]] && [[ $r != *compatdata/8500* ]]; }' _ "$dst_eve/ResFiles"
    chk "log directory" test -d "$dst/pfx/drive_c/eve-logs"
    chk "a settings folder for $dst_client" bash -c 'for d in "$1"/c_ccp_eve_"$2"_*; do [[ -d $d ]] && exit 0; done; exit 1' _ "$dst/$SETTINGS_REL" "${dst_client,,}"
    return $bad
}

remove_slot() {   # N
    ci_slot_vars "$REPO" "$HOME_DIR" "$1" "$SRC_DIR" || die "SLOT must be a number from 1 to 999, not '$1'"
    # The ResFiles links go first: nothing below may follow them into the store.
    local d
    for d in "$SLOT_PREFIX" "$SLOT_PREFIX2" "$SLOT_PREFIX.tmp"; do
        [[ -L $d/pfx/drive_c/CCP/EVE/ResFiles ]] && rm -- "$d/pfx/drive_c/CCP/EVE/ResFiles"
    done
    rm -rf -- "$SLOT_BASE"
    echo "removed $SLOT_BASE"
}

case $cmd in
    ensure) take_lock; ensure ;;
    check)  check ;;
    remove)
        take_lock
        if [[ $slot == all ]]; then
            for d in "$HOME_DIR"/.local/share/eve-offline/slots/[1-9]*; do
                [[ -d $d ]] && remove_slot "${d##*/}"
            done
            echo "no slots left"
        else
            remove_slot "$slot"
        fi ;;
    size)   du -sh -- "$HOME_DIR/.local/share/eve-offline/slots"/* 2>/dev/null || echo "no slots" ;;
    *)      usage ;;
esac
