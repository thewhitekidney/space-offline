# Which Wine prefix the jailed client runs in (owner decision O19). SOURCED,
# never run: tools/run-client.sh, tools/client-ci.sh, tools/client-headless.sh
# and tools/offline-prefix.sh read it, so the path is decided in one place.
#
# The client runs in a prefix of its own, a one-time copy of the online
# prefix that tools/offline-prefix.sh makes. It never runs in, and never
# sees, the prefix the owner plays online in (Steam appid 8500): a jailed
# client, or a hostile server driving it, could otherwise plant a DLL there
# that the online client loads later, outside the jail (ruling 14).
#
# The root helper (tools/eve-offline-jail.in) and the rootless jail
# (tools/jail-rootless.sh) cannot source this file -- the helper is a
# root-owned snapshot, and the view they build contains only the shim -- so
# each carries its own copy of the check. tools/test_jail.py holds the four
# copies to one id.

# The online EVE install's Steam appid, and so its compatdata directory.
EVE_ONLINE_PREFIX_ID=8500

# eve_offline_prefix HOME: the offline client's own prefix. Outside Steam's
# compatdata on purpose, so Steam's own prefix management (cleanup on
# uninstall, a Proton upgrade, "delete compatibility data") never reaches it,
# and on the same filesystem as ~/.local/share/Steam, so the ResFiles store
# can be hard-linked rather than copied (tools/offline-prefix.sh
# --link-resfiles).
eve_offline_prefix() { printf '%s/.local/share/eve-offline/prefix' "$1"; }

# eve_prefix_of ID HOME STEAM_ROOT: `offline` is the copy; a number is
# STEAM_ROOT/steamapps/compatdata/ID (8501 is the second player's prefix,
# tools/client2-setup.sh). Anything else is an error.
eve_prefix_of() {
    case $1 in
        offline) eve_offline_prefix "$2" ;;
        '' | *[!0-9]*) return 1 ;;
        *) printf '%s/steamapps/compatdata/%s' "$3" "$1" ;;
    esac
}

# eve_is_online_path PATH: PATH, as written or with its symlinks resolved,
# is the online prefix or lies inside it, in any Steam library.
eve_is_online_path() {
    local p=$1 r
    r=$(realpath -m -- "$p" 2>/dev/null) || r=$p
    for p in "$p" "$r"; do
        case $p in
            */[sS]team[aA]pps/compatdata/"$EVE_ONLINE_PREFIX_ID" | */[sS]team[aA]pps/compatdata/"$EVE_ONLINE_PREFIX_ID"/*)
                return 0 ;;
        esac
    done
    return 1
}

# eve_refuse_online WHO NAME=PATH...: exit 1, saying why, at the first PATH
# that is the online prefix or inside it. Empty values are skipped.
eve_refuse_online() {
    local who=$1 kv
    shift
    for kv in "$@"; do
        [[ -n ${kv#*=} ]] || continue
        if eve_is_online_path "${kv#*=}"; then
            {
                echo "$who: ${kv%%=*} is inside the online EVE prefix (compatdata/$EVE_ONLINE_PREFIX_ID):"
                echo "    ${kv#*=}"
                echo "  The jailed client never runs in, or sees, the prefix you play online in (O19)."
                echo "  Make its own prefix once with tools/offline-prefix.sh and use the default."
            } >&2
            exit 1
        fi
    done
}

# eve_drop_stale_launch_env REPO: remove REPO/out/client-ci/launch-env when it
# names the online prefix. A launch-env from before O19 does, and the jail's
# verify reads it and refuses before any launch writes a fresh one
# (tools/jail-client.sh writes one at every launch, so nothing is lost).
eve_drop_stale_launch_env() {
    local lenv=$1/out/client-ci/launch-env
    if [[ -f $lenv ]] && grep -q "/compatdata/$EVE_ONLINE_PREFIX_ID\(/\|$\)" "$lenv"; then
        echo "== removing a stale $lenv that names the online prefix"
        rm -f -- "$lenv"
    fi
}

# eve_prefix_busy PREFIX: print the pids of processes using PREFIX and return 0
# if there are any. A process counts when a STEAM_COMPAT_DATA_PATH or WINEPREFIX
# in its environment, once resolved with realpath (a symlink or a trailing
# slash or a ".." does not hide it), is the prefix or inside it; when its cwd
# is inside; or when it is called exefile.exe or eve-online.exe (the game, in
# any prefix: close it before copying). Unreadable /proc entries (other
# users') are skipped. EVE_PROC_DIR is /proc; the tests give it a fake.
eve_prefix_busy() {
    local p d pid c kv v pids="" proc=${EVE_PROC_DIR:-/proc}
    p=$(realpath -m -- "$1")
    for d in "$proc"/[0-9]*; do
        pid=${d##*/}
        c=$(cat -- "$d/comm" 2>/dev/null || true)
        if [[ ${c,,} == exefile.exe || ${c,,} == eve-online.exe ]]; then pids+=" $pid"; continue; fi
        c=$(readlink -- "$d/cwd" 2>/dev/null || true)
        if [[ -n $c ]]; then
            c=$(realpath -m -- "$c")
            if [[ $c == "$p" || $c == "$p"/* ]]; then pids+=" $pid"; continue; fi
        fi
        while IFS= read -r -d '' kv; do
            case $kv in
                STEAM_COMPAT_DATA_PATH=* | WINEPREFIX=*)
                    v=$(realpath -m -- "${kv#*=}")
                    if [[ $v == "$p" || $v == "$p"/* ]]; then pids+=" $pid"; break; fi ;;
            esac
        done 2>/dev/null < "$d/environ" || true
    done
    [[ -n $pids ]] || return 1
    printf '%s\n' "${pids# }"
}
