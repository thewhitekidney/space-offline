#!/usr/bin/env bash
# Build a SECOND Proton prefix and client copy, so two players can be two
# clients on one machine.
#
#   tools/client2-setup.sh            # build it
#   tools/client2-setup.sh --check    # say whether it is already there
#   tools/client2-setup.sh --remove   # delete it (the copy and the prefix)
#
# Then:  sudo tools/run-client.sh --second
#
# Why a whole second prefix and not just a second process
# ------------------------------------------------------
# `run-client.sh` refuses a second client in one prefix, and it is right to.
# One prefix means one wineserver, one registry, and one install directory --
# whose `cache:` subdirectory the client writes to. Two clients sharing that
# is not a configuration, it is a race.
#
# What it is NOT about is the server. Two connections have been fine since
# Round 0.3: `Session.client_id` is minted per connection, `session_table`
# holds both, and the loopback suite drives two sockets against one server.
# `run-client.sh`'s old message said "two clients would share one jail and
# one session", and the second half of that was wrong.
#
# The cost
# --------
# 403M. The client copy is 403M; `ResFiles` is 48G and is **symlinked**, not
# copied, because it is content-addressed and read-only -- the client only
# ever reads it, by hash, through `resfileindex.txt`. Sharing it is safe in a
# way that sharing the install directory is not.
#
# Nothing here touches prefix 8500, the real `tq` install, or Steam's own
# configuration. Everything it creates lives under compatdata/8501. Its
# source is the offline client's own prefix (O19, tools/offline-prefix.sh),
# never 8500: the client copy comes from there, and ResFiles links there, so
# the second client sees nothing of the online prefix either. A 8501 built
# before O19 links ResFiles into 8500; running this again repoints it, and
# the launchers refuse the second client until it has.
#
# One thing to do by hand afterwards
# ----------------------------------
# **In the second client, set Display & Graphics -> Post Processing to "low".**
# A fresh prefix has no saved graphics profile, and at the client's own
# first-run default the post-process chain draws the 3D scene in greyscale on
# this hardware while the UI stays in colour. It is a shader/driver
# interaction in the client, not the server; `docs/port-tracker.md` §28 has
# the evidence and `docs/guides/human-checks.md` §2 repeats this where the recipe
# lives.
set -euo pipefail

RUN_AS=${SUDO_USER:-$(id -un)}
RUN_HOME=$(getent passwd "$RUN_AS" | cut -d: -f6)
: "${STEAM_ROOT:=$RUN_HOME/.local/share/Steam}"
: "${PROTON:=$STEAM_ROOT/compatibilitytools.d/GE-Proton10-34/proton}"

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"
SRC_PREFIX=$(eve_offline_prefix "$RUN_HOME")
DST_PREFIX="$STEAM_ROOT/steamapps/compatdata/8501"
SRC_EVE="$SRC_PREFIX/pfx/drive_c/CCP/EVE"
DST_EVE="$DST_PREFIX/pfx/drive_c/CCP/EVE"
# The install dirs are settable so a frozen build (tools/import-build.sh) can
# have a second client too: EVE_SECOND_SRC_DIR=tq-1109 EVE_SECOND_CLIENT_DIR=tq-1109-2.
# The defaults are the shipping pair; EVE_SECOND_CLIENT_DIR is the name
# tools/client-ci.sh --with-second looks for.
SRC_CLIENT="$SRC_EVE/${EVE_SECOND_SRC_DIR:-tq-offline}"
DST_CLIENT="$DST_EVE/${EVE_SECOND_CLIENT_DIR:-tq-offline-2}"
for _d in "${EVE_SECOND_SRC_DIR:-tq-offline}" "${EVE_SECOND_CLIENT_DIR:-tq-offline-2}"; do
    [[ $_d =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ && $_d != tq && $_d != sisi ]] \
        || { echo "client2-setup: bad install dir name '$_d'" >&2; exit 1; }
done

ok()   { printf '  ok    %s\n' "$1"; }
bad()  { printf '  MISS  %s\n' "$1"; }

# Resolve one real resource the way the CLIENT resolves it, and not the way
# this script happens to have arranged things.
#
# **This is the check that was wrong the first time.** The original version
# resolved a path through `$DST_EVE/ResFiles` -- the symlink it had just
# created -- and reported ok. The client does not use that path. It reads
# `HKCU\Software\CCP\EVEONLINE\CACHEFOLDER` and appends `ResFiles`, and
# with no such key it falls back to `C:\ProgramData\CCP\EVE\SharedCache`,
# which does not exist in either prefix. So the rig passed its own check and
# the client died on
#
#     RuntimeError: Couldn't open file: res:/staticdata/schematics.fsdbinary
#
# A check that verifies what the builder built rather than what the consumer
# consumes is worth less than no check: it converts a failure into a
# surprise. This one starts from the registry.
check_resfile() {
    local reg="$DST_PREFIX/pfx/user.reg"
    [[ -f "$reg" ]] || { bad "no user.reg"; return 1; }
    local cache
    cache=$(sed -n 's/^"CACHEFOLDER"="\(.*\)"$/\1/p' "$reg" | head -1)
    [[ -n "$cache" ]] || { bad "CACHEFOLDER unset -- the client will look in ProgramData"; return 1; }
    ok "CACHEFOLDER = $cache"

    # C:\CCP\EVE\ -> <prefix>/pfx/drive_c/CCP/EVE
    local unix="${cache//\\//}"
    unix="${unix#C:/}"
    unix="$DST_PREFIX/pfx/drive_c/${unix%/}"
    [[ -d "$unix/ResFiles" ]] || { bad "no ResFiles under $unix"; return 1; }

    # One real entry, by hash, from the index the client reads.
    local line rel want
    line=$(head -1 "$DST_CLIENT/resfileindex.txt")
    rel=$(cut -d, -f2 <<<"$line")
    want=$(cut -d, -f4 <<<"$line")
    local f="$unix/ResFiles/$rel"
    [[ -f "$f" ]] || { bad "index entry $rel does not resolve"; return 1; }
    local got; got=$(stat -c %s "$f")
    if [[ "$got" == "$want" ]]; then
        ok "res file resolves by hash, $got bytes as the index says"
    else
        bad "res file resolves but is $got bytes, index says $want"
        return 1
    fi
    return 0
}

check() {
    local missing=0
    echo "== second client prefix ($DST_PREFIX)"
    [[ -d "$DST_PREFIX/pfx" ]]            && ok "prefix initialised"      || { bad "prefix"; missing=1; }
    [[ -f "$DST_CLIENT/bin64/exefile.exe" ]] && ok "client copy"          || { bad "client copy"; missing=1; }
    local pack
    pack=$(sed -n 's/^[[:space:]]*cryptoPack[[:space:]]*=[[:space:]]*//p' "$DST_CLIENT/start.ini" 2>/dev/null | tr -d '\r')
    [[ "$pack" == "Placebo" ]] && ok "cryptoPack = Placebo" || { bad "cryptoPack = ${pack:-unset}"; missing=1; }
    [[ -L "$DST_EVE/ResFiles" ]]          && ok "ResFiles symlink"        || { bad "ResFiles symlink"; missing=1; }
    if [[ -L "$DST_EVE/ResFiles" ]] && eve_is_online_path "$(readlink -m -- "$DST_EVE/ResFiles")"; then
        bad "ResFiles links into the online prefix; run tools/client2-setup.sh again (O19)"; missing=1
    fi
    check_resfile || missing=1
    [[ -d "$DST_PREFIX/pfx/drive_c/eve-logs" ]] && ok "log directory" || { bad "log directory"; missing=1; }
    return $missing
}

if [[ "${1:-}" == "--check" ]]; then
    check && echo "second client is ready: sudo tools/run-client.sh --second" || {
        echo "not ready; run tools/client2-setup.sh" >&2; exit 1; }
    exit 0
fi

if [[ "${1:-}" == "--remove" ]]; then
    # The symlink goes first: an `rm -rf` that followed it would walk into the
    # 48G original. `rm` does not follow symlinks for the directory it is
    # given, but removing it explicitly first makes that independent of
    # anyone's `rm` flags.
    [[ -L "$DST_EVE/ResFiles" ]] && rm "$DST_EVE/ResFiles"
    [[ -d "$DST_PREFIX" ]] && rm -rf "$DST_PREFIX"
    echo "removed $DST_PREFIX"
    exit 0
fi

[[ "$RUN_AS" != "root" ]] || { echo "client2-setup.sh: run as yourself, not root" >&2; exit 1; }
[[ -x "$PROTON" ]] || { echo "client2-setup.sh: proton not found at $PROTON" >&2; exit 1; }
[[ -f "$SRC_CLIENT/bin64/exefile.exe" ]] || {
    echo "client2-setup.sh: no client to copy at $SRC_CLIENT" >&2
    echo "  the first client has to exist before the second one can;" >&2
    echo "  its prefix is made once by tools/offline-prefix.sh (O19)." >&2
    exit 1
}
[[ -d "$SRC_EVE/ResFiles" ]] || {
    echo "client2-setup.sh: no ResFiles at $SRC_EVE/ResFiles" >&2; exit 1; }

# Space: the copy only, since ResFiles is shared.
need_mb=$(( $(du -sm "$SRC_CLIENT" | cut -f1) + 2048 ))
free_mb=$(df -m --output=avail "$STEAM_ROOT" | tail -1 | tr -d ' ')
if (( free_mb < need_mb )); then
    echo "client2-setup.sh: need ~${need_mb}M free, have ${free_mb}M" >&2
    exit 1
fi
echo "== space: need ~${need_mb}M (client copy + prefix), have ${free_mb}M"

echo "== initialising prefix 8501"
mkdir -p "$DST_PREFIX"
if [[ ! -d "$DST_PREFIX/pfx" ]]; then
    # `proton run` on a trivial command is enough to create the bottle.
    # Proton writes to STEAM_COMPAT_DATA_PATH and needs the client install
    # path for its own runtime bits.
    STEAM_COMPAT_DATA_PATH="$DST_PREFIX" \
    STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_ROOT" \
        "$PROTON" run cmd /c exit >/dev/null 2>&1 || true
fi
[[ -d "$DST_PREFIX/pfx" ]] || { echo "client2-setup.sh: proton did not create $DST_PREFIX/pfx" >&2; exit 1; }

echo "== copying the client (403M-ish; ResFiles is NOT copied)"
mkdir -p "$DST_EVE"
if [[ ! -f "$DST_CLIENT/bin64/exefile.exe" ]]; then
    cp -a "$SRC_CLIENT" "$DST_CLIENT"
fi

echo "== symlinking ResFiles (48G, read-only, content-addressed)"
if [[ ! -L "$DST_EVE/ResFiles" ]]; then
    ln -s "$SRC_EVE/ResFiles" "$DST_EVE/ResFiles"
elif [[ $(readlink -- "$DST_EVE/ResFiles") != "$SRC_EVE/ResFiles" ]]; then
    # Built before O19, it links into the online prefix 8500. -n replaces the
    # link itself and never writes through it.
    echo "   repointing it from $(readlink -- "$DST_EVE/ResFiles")"
    ln -sfn "$SRC_EVE/ResFiles" "$DST_EVE/ResFiles"
fi

# **The step whose absence killed the first attempt.** The client finds
# ResFiles through `HKCU\Software\CCP\EVEONLINE\CACHEFOLDER`, which prefix
# 8500 has as `C:\CCP\EVE\` and a fresh prefix does not have at all. With
# it missing the client falls back to `C:\ProgramData\CCP\EVE\SharedCache`,
# finds nothing, tries to download the file from clientresources.eveonline.com
# -- correctly blocked by the jail -- and dies on
# `RuntimeError: Couldn't open file: res:/staticdata/schematics.fsdbinary`.
#
# Registry state is prefix state, so copying the install directory could never
# carry it. Set through wine's own `reg` rather than by appending to
# `user.reg`, so wine owns the file it is going to rewrite.
echo "== pointing the client at ResFiles (HKCU CCP\\EVEONLINE\\CACHEFOLDER)"
STEAM_COMPAT_DATA_PATH="$DST_PREFIX" \
STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_ROOT" \
    "$PROTON" run reg add 'HKCU\Software\CCP\EVEONLINE' \
        /v CACHEFOLDER /t REG_SZ /d 'C:\CCP\EVE\' /f >/dev/null 2>&1 || true

echo "== log directory"
mkdir -p "$DST_PREFIX/pfx/drive_c/eve-logs"

echo
check
echo
echo "ready. With the stack already up (tools/up.sh --keep):"
echo "    sudo tools/run-client.sh --second"
echo "and pick the second character."
