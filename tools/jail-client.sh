#!/usr/bin/env bash
# Launch the client inside the jail. **Runs as you, never as root.**
#
# Both launch paths end here (IP15). `eve-offline-jail client` (the helper)
# and `run-client.sh`'s direct branch both enter the network namespace as
# root, build the client's mount allowlist (IP14's `build_view`, the same
# code on both paths), and `setpriv --clear-groups` down to the invoking user
# before exec'ing this file -- so this script, which lives in a user-writable
# repo, never runs with privilege.
#
# Because sudo scrubs the environment, the caller cannot pass anything in
# through it. The settings come from a file instead:
#
#     out/client-ci/launch-env     KEY=VALUE, one per line
#
# which `tools/client-ci.sh` writes immediately before asking for the client.
# **This file is read here, as you, and never by root** -- which is why it is
# allowed to be user-writable at all. Only a fixed set of keys is honoured,
# and anything else in the file is ignored rather than exported, so a stray
# `LD_PRELOAD=` cannot ride in on it.
set -euo pipefail

# **The in-jail gate (ruling 3), first, as the user who will run the client.**
# Docker, libvirt, sudo and lxd membership is root by another name, and a
# unix socket crosses a network namespace (RR1 C3): a client holding one of
# those gids could `start /unix docker run --network host`. Both launch paths
# drop supplementary groups; this checks, from the process that would use
# them, that the drop happened. It also guards a stale installed helper,
# which predates the allowlist and the helper's own gate. Exit 3: refused.
gate() { echo "jail-client.sh: $*; refusing to launch" >&2; exit 3; }
(( EUID != 0 )) || gate "running as root"
denied=()
for g in docker libvirt sudo lxd; do
    gid=$(getent group "$g" | cut -d: -f3) || gid=""   # absent group: nothing to hold
    [[ -n $gid ]] && denied+=("$gid")
done
for g in $(id -G); do
    for d in ${denied[@]+"${denied[@]}"}; do
        [[ $g == "$d" ]] && gate "the client would hold gid $g (docker, libvirt, sudo or lxd)"
    done
done
for s in /run/docker.sock /var/run/docker.sock /run/libvirt/libvirt-sock /var/run/libvirt/libvirt-sock; do
    [[ -w $s ]] && gate "$s is writable by the client"
done

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ENV_FILE=$REPO/out/client-ci/launch-env
[[ -f $ENV_FILE ]] || { echo "jail-client.sh: no $ENV_FILE" >&2; exit 2; }

# An allow-list, not a `source`. The file is data.
declare -A allowed=(
    [HOME]=1 [DISPLAY]=1 [WAYLAND_DISPLAY]=1 [XDG_RUNTIME_DIR]=1 [XAUTHORITY]=1
    [PROTON_USE_XALIA]=1 [VK_DRIVER_FILES]=1 [VK_ICD_FILENAMES]=1
    [LP_NUM_THREADS]=1
    [STEAM_COMPAT_DATA_PATH]=1 [STEAM_COMPAT_CLIENT_INSTALL_PATH]=1
    [EVE_CLIENT]=1 [PROTON]=1 [EVE_CLIENT_ARGS]=1
    # The login token (ACC2): `USER:PASSWORD` against a world-accounts
    # server. Its own key, not part of EVE_CLIENT_ARGS, because that is split
    # on spaces and a user name may hold one; run-client.sh writes it into a
    # mode-600 launch-env and removes the line once the client has started.
    [EVE_SSO_TOKEN]=1
    # Diagnostics. Added 2026-10-24, when the client began exiting during
    # Proton startup without writing a byte: `proton run` printed three lines
    # and returned, and there was no way to ask it for more, because this
    # list dropped every variable that would have. None of these three loads
    # code -- they select logging -- so the property this list exists for
    # (`LD_PRELOAD=` cannot ride in) is unchanged.
    [PROTON_LOG]=1 [PROTON_LOG_DIR]=1 [WINEDEBUG]=1
    # The census (CEN-3, docs/guides/census.md). EVE_CENSUS=1 puts the browser
    # stand-ins first on PATH and fires the host and Wine canaries before
    # the client. EVE_CENSUS_CONTROL=1 is the first key that WEAKENS a
    # launch (no patcher env or args, no winebrowser override), so it is
    # refused unless census mode is real: see census_setup.
    [EVE_CENSUS]=1 [EVE_CENSUS_CONTROL]=1
    # EVE_CENSUS_NOENV=1 also weakens a launch (the patcher's env is not
    # applied, its args are), so it is refused outside a census the same way.
    [EVE_CENSUS_NOENV]=1
)
declare -A cfg=()
while IFS='=' read -r key value; do
    [[ -z ${key:-} || $key == \#* ]] && continue
    [[ -n ${allowed[$key]:-} ]] && cfg[$key]=$value
done < "$ENV_FILE"

: "${cfg[EVE_CLIENT]:?jail-client.sh: EVE_CLIENT missing from $ENV_FILE}"
: "${cfg[PROTON]:?jail-client.sh: PROTON missing from $ENV_FILE}"
exe="${cfg[EVE_CLIENT]}/bin64/exefile.exe"

# **O19 (ruling 14): never in, never beside, the online prefix.** The client
# runs in its own Wine prefix (tools/offline-prefix.sh), not in the prefix
# the owner's online EVE uses (Steam appid 8500), and that prefix must not be
# visible in the view at all. Both jails refuse this before they bind
# anything; this repeats it from inside, as the process that would use it,
# so a stale helper or a hand-written launch-env is caught too. Last, the
# client's ResFiles must be read-only here: with
# `offline-prefix.sh --link-resfiles` its files are the online store's own.
ONLINE_PREFIX_ID=8500
refuse_online_prefix() {   # HOME STEAM_ROOT PREFIX CLIENT
    local home=$1 steam=$2 p x
    shift 2
    for p in "$@" "${2:+$2/../ResFiles}"; do
        [[ -n $p ]] || continue
        for x in "$p" "$(realpath -m -- "$p")"; do
            case $x in
                */[sS]team[aA]pps/compatdata/"$ONLINE_PREFIX_ID" | */[sS]team[aA]pps/compatdata/"$ONLINE_PREFIX_ID"/*)
                    gate "$p is in the online EVE prefix (compatdata/$ONLINE_PREFIX_ID); make the client's own with tools/offline-prefix.sh (O19)" ;;
            esac
        done
    done
    for p in ${home:+"$home/.local/share/Steam"} ${steam:+"$steam"}; do
        if [[ -e $p/steamapps/compatdata/$ONLINE_PREFIX_ID ]]; then
            gate "the online EVE prefix $p/steamapps/compatdata/$ONLINE_PREFIX_ID is visible inside the jail"
        fi
    done
    if [[ -n ${2:-} && -d $2/../ResFiles && -w $2/../ResFiles ]]; then
        gate "ResFiles is writable inside the jail; is the jail helper stale? (tools/install-jail-helper.sh --check)"
    fi
    return 0
}
refuse_online_prefix "${cfg[HOME]:-}" "${cfg[STEAM_COMPAT_CLIENT_INSTALL_PATH]:-}" \
    "${cfg[STEAM_COMPAT_DATA_PATH]:-}" "${cfg[EVE_CLIENT]}"
[[ -x ${cfg[PROTON]} ]] || { echo "jail-client.sh: no proton at ${cfg[PROTON]}" >&2; exit 2; }
[[ -f $exe ]] || { echo "jail-client.sh: no client at $exe" >&2; exit 2; }

# **The census (CEN-3).** Census mode is claimed by the launch-env and
# proven by the kernel: census0 must be a TUN device in this namespace and
# both default routes (v4, v6) must go to it and nowhere else -- which only
# the helper's census-up (or jail-rootless.sh --census) can arrange. So the
# control's weakening (EVE_CENSUS_CONTROL=1) cannot happen outside a census,
# where every packet ends in a sink and nothing leaves. The two paths are
# variables so tools/test_jail.py can point them at a fake view.
CENSUS_DEV=census0
CENSUS_SYS=/sys/class/net
CENSUS_PROC=/proc/net
census_setup() {   # EVE_CENSUS EVE_CENSUS_CONTROL REPO [EVE_CENSUS_NOENV]
    local census=$1 control=$2 repo=$3 noenv=${4:-} d4 d6
    CENSUS=0 CENSUS_CONTROL=0 CENSUS_NOENV=0 CENSUS_ENV=()
    case $census in '') ;; 1) CENSUS=1 ;; *) gate "EVE_CENSUS='$census'; it is 1 or absent" ;; esac
    case $control in '') ;; 1) CENSUS_CONTROL=1 ;; *) gate "EVE_CENSUS_CONTROL='$control'; it is 1 or absent" ;; esac
    if (( CENSUS_CONTROL && ! CENSUS )); then
        gate "EVE_CENSUS_CONTROL=1 without EVE_CENSUS=1: a control run is a census run or nothing"
    fi
    case $noenv in '') ;; 1) CENSUS_NOENV=1 ;; *) gate "EVE_CENSUS_NOENV='$noenv'; it is 1 or absent" ;; esac
    if (( CENSUS_NOENV && ! CENSUS )); then
        gate "EVE_CENSUS_NOENV=1 without EVE_CENSUS=1: a no-env run is a census run or nothing"
    fi
    if (( CENSUS_NOENV && CENSUS_CONTROL )); then
        gate "EVE_CENSUS_NOENV=1 with EVE_CENSUS_CONTROL=1: the control is already unpatched"
    fi
    (( CENSUS )) || return 0
    # IFF_TUN|IFF_NO_PI, plus IFF_PERSIST (0x800): `ip tuntap add` makes the device
    # persistent and this kernel shows that bit, so the flags read 0x1801, not 0x1001.
    tf=$(cat "$CENSUS_SYS/$CENSUS_DEV/tun_flags" 2>/dev/null || true)
    [[ $tf == 0x[0-9a-f]* ]] && (( (tf & ~0x800) == 0x1001 )) \
        || gate "EVE_CENSUS=1 but there is no TUN device $CENSUS_DEV here (census-up, or jail-rootless.sh --census)"
    # Default routes from /proc, as census_canary.py's guard reads them:
    # destination 0, mask 0 (v4) or length 0 (v6), not a reject route
    # (RTF_REJECT, 0x200; the kernel parks those on lo).
    local ifc dst len flags x
    d4='' d6=''
    while read -r ifc dst x flags x x x len x; do
        [[ $dst == 00000000 && $len == 00000000 && $flags =~ ^[0-9A-Fa-f]+$ ]] || continue
        if (( (16#$flags & 0x200) == 0 )); then d4+="$ifc "; fi
    done < <(tail -n +2 "$CENSUS_PROC/route" 2>/dev/null || true)
    while read -r dst len x x x x x x flags ifc x; do
        [[ $dst == 00000000000000000000000000000000 && $len == 00 && $flags =~ ^[0-9A-Fa-f]+$ ]] || continue
        if (( (16#$flags & 0x200) == 0 )); then d6+="$ifc "; fi
    done < <(cat "$CENSUS_PROC/ipv6_route" 2>/dev/null || true)
    [[ $d4 == "$CENSUS_DEV " && $d6 == "$CENSUS_DEV " ]] \
        || gate "EVE_CENSUS=1 but the default routes are v4 '${d4:-none}', v6 '${d6:-none}', not exactly $CENSUS_DEV"
    local live=$repo/out/census/live
    [[ -x $repo/tools/census/bin/xdg-open ]] || gate "no browser stand-ins at $repo/tools/census/bin"
    [[ -f $live/exec.log && -w $live/exec.log ]] || gate "$live/exec.log is not writable here"
    [[ -f $live/canary.jsonl && -w $live/canary.jsonl ]] || gate "$live/canary.jsonl is not writable here"
    CENSUS_LIVE=$live
    # The client sees EVE_CENSUS=1 (the CEN-4 scenarios fire nothing
    # without it) and, in the control, EVE_CENSUS_CONTROL=1 (recorded only).
    CENSUS_ENV=("PATH=$repo/tools/census/bin:$PATH" "CENSUS_EXEC_LOG=$live/exec.log" "EVE_CENSUS=1")
    if (( CENSUS_CONTROL )); then CENSUS_ENV+=("EVE_CENSUS_CONTROL=1"); fi
    if (( CENSUS_NOENV )); then CENSUS_ENV+=("EVE_CENSUS_NOENV=1"); fi
    return 0
}
census_setup "${cfg[EVE_CENSUS]:-}" "${cfg[EVE_CENSUS_CONTROL]:-}" "$REPO" "${cfg[EVE_CENSUS_NOENV]:-}"

# Deliberately word-split: these are the client's own `/flag=value`
# arguments, which contain no spaces.
read -r -a args <<< "${cfg[EVE_CLIENT_ARGS]:-}"

# **The phone-home patch's launch layer** (tier A, docs/guides/patcher.md). The
# patcher prints, per build, the env the client must see (a dead loopback
# proxy with NO_PROXY for our server, the gateway address, a loopback Sentry
# DSN), the args it must get, and the args it must never get. Read as you,
# like everything here; only `env` lines whose key is in this fixed list are
# taken, so the "allow-list, never source" property holds. An install whose
# build has no manifest, or a forbidden argument, refuses the launch.
declare -A patch_allowed=(
    [HTTP_PROXY]=1 [HTTPS_PROXY]=1 [NO_PROXY]=1
    [EVE_PUBLIC_GATEWAY_ADDRESS]=1 [EVE_CLIENT_SENTRY_DSN]=1
)
patch_env=()
lenv=$(python3 -I "$REPO/tools/eve_patch.py" launch-env --client "${cfg[EVE_CLIENT]}" \
        --check-args "${args[*]+"${args[*]}"}") \
    || gate "tools/eve_patch.py launch-env refused (no manifest for this build, or a forbidden client argument)"
# The control (EVE_CENSUS_CONTROL=1, census only) launches as the unpatched
# copy would: the patcher still checks the arguments, but its env and args
# are not applied, so every path the patch closes is open and recorded.
(( CENSUS_CONTROL )) && lenv=''
# ... and with crash upload on: /noCrashReportUpload would keep N1's upload
# from ever showing, and N1 would stay INCONCLUSIVE (docs/lanes/cen4.md).
if (( CENSUS_CONTROL )); then
    kept=()
    for a in ${args[@]+"${args[@]}"}; do [[ ${a,,} == /nocrashreportupload ]] || kept+=("$a"); done
    args=(${kept[@]+"${kept[@]}"})
fi
while read -r kind val; do
    case $kind in
        env)
            k=${val%%=*}
            if [[ -n ${patch_allowed[$k]:-} && -n ${val#*=} ]]; then patch_env+=("$val"); fi ;;
        arg)
            have=0
            for a in ${args[@]+"${args[@]}"}; do if [[ ${a,,} == "${val,,}" ]]; then have=1; fi; done
            if ((!have)); then args+=("$val"); fi ;;
    esac
done <<< "$lenv"
# The no-env subject (census only): the patched files and the patcher's args,
# but none of its env, so what the files alone close is what the census sees
# (docs/guides/census.md, "No-env run").
(( CENSUS_NOENV )) && patch_env=()

# **Set or absent, never empty.** This used to pass every key as
# `k=${cfg[$k]:-}`, which turns a variable the caller never set into one set
# to the empty string -- and for the two Vulkan keys those are opposite
# instructions. `VK_DRIVER_FILES=` does not mean "no preference"; it means
# "the complete list of driver manifests is: nothing". The loader then finds
# zero ICDs, `DxvkInstance::createInstance` fails, and the client exits
# during Proton startup without writing a single byte to its log -- which
# looks exactly like "the client did not launch".
#
# It could only bite a launch that leaves them unset, i.e. one on the real
# display with the real GPU: client-CI sets `VK_DRIVER_FILES` to lavapipe's
# ICD, so every automated run papered over it. 2026-10-24.
#
# `WINEDEBUG` has the same shape for a different reason: unset means Wine's
# default channels, `WINEDEBUG=` means silence.
# The token goes first, as one argument, after the patcher's check (which
# never sees it). A key in the file replaces any /ssoToken= in the args.
if [[ -n ${cfg[EVE_SSO_TOKEN]:-} ]]; then
    kept=()
    for a in ${args[@]+"${args[@]}"}; do [[ ${a,,} == /ssotoken=* ]] || kept+=("$a"); done
    args=("/ssoToken=${cfg[EVE_SSO_TOKEN]}" ${kept[@]+"${kept[@]}"})
fi

env_args=()
for k in HOME DISPLAY WAYLAND_DISPLAY XDG_RUNTIME_DIR XAUTHORITY PROTON_USE_XALIA \
         VK_DRIVER_FILES VK_ICD_FILENAMES LP_NUM_THREADS \
         STEAM_COMPAT_DATA_PATH STEAM_COMPAT_CLIENT_INSTALL_PATH \
         PROTON_LOG PROTON_LOG_DIR WINEDEBUG; do
    [[ -n ${cfg[$k]:-} ]] && env_args+=("$k=${cfg[$k]}")
done
# Wine's URL hand-off (winebrowser -> xdg-open -> a host browser that is
# online) never starts, on either launch path (ruling 4). Last, so nothing in
# the launch-env can undo it; Proton appends its own overrides to this.
# No session bus: without this, dbus-launch autolaunches a daemon that outlives
# a client that died at once, and the jail's wait loop then never ends.
env_args+=("DBUS_SESSION_BUS_ADDRESS=disabled:")
env_args+=(${patch_env[@]+"${patch_env[@]}"})
# The control drops the override too (and the helper's inherited one), so
# the browser hand-off reaches the census stand-ins and is recorded.
env_unset=()
if (( CENSUS_CONTROL )); then
    env_unset=(-u WINEDLLOVERRIDES)
else
    env_args+=("WINEDLLOVERRIDES=winebrowser.exe=d")
fi
env_args+=(${CENSUS_ENV[@]+"${CENSUS_ENV[@]}"})

cd "${cfg[EVE_CLIENT]}"
if (( CENSUS )); then
    # The canaries (docs/guides/census.md), each party bracketing its own rows: the
    # host rows (census_canary.py run marks its own window), then Wine's
    # ICMP and resolver through Proton, in the client's own prefix and env.
    # A failure here is not a refusal: a missing canary makes the report
    # INVALID, which is the right verdict for a harness that cannot see.
    canary=(python3 -I "$REPO/tools/census_canary.py")
    "${canary[@]}" run --out "$CENSUS_LIVE/canary.jsonl" || echo "jail-client.sh: the host canaries refused" >&2
    # Each Proton canary keeps its own output and a line of timing and exit
    # status (wine-canary.log, which client-ci copies into the run), so a
    # canary that does not show up explains itself instead of only "missing".
    wine_canary() {   # LABEL PROGRAM ARGS...
        local label=$1 rc=0 t0 t1; shift
        t0=$(date +%s.%N)
        timeout 120 env ${env_unset[@]+"${env_unset[@]}"} -C "${cfg[EVE_CLIENT]}" "${env_args[@]}" \
            "${cfg[PROTON]}" run "$@" </dev/null >"$CENSUS_LIVE/wine-$label.out" 2>&1 || rc=$?
        t1=$(date +%s.%N)
        printf '%s rc=%s t0=%s t1=%s cmd=%s\n' "$label" "$rc" "$t0" "$t1" "$*" >> "$CENSUS_LIVE/wine-canary.log"
    }
    "${canary[@]}" mark start wine --out "$CENSUS_LIVE/canary.jsonl"
    # ICMP: an echo to a TEST-NET address. DNS: the resolver, through a name
    # lookup by ping.exe. GE-Proton10-34's Wine has no nslookup.exe (checked
    # 2026-10-30), which is why wine-dns was missing in every rootless run.
    wine_canary icmp ping.exe -n 1 -w 2000 192.0.2.7
    wine_canary dns ping.exe -n 1 -w 1000 canary-wine.census.invalid
    "${canary[@]}" mark end wine --out "$CENSUS_LIVE/canary.jsonl"
fi
# Through a pipe, not straight to our stdout: when stdout is a regular file
# (client-ci writes client-launch.log) Wine's first process (steam.exe)
# segfaults in libc right after the server hands it the file handle, 2026-10-27,
# and the client never starts. A terminal or a pipe is fine.
rc=0
env ${env_unset[@]+"${env_unset[@]}"} -C "${cfg[EVE_CLIENT]}" "${env_args[@]}" \
    "${cfg[PROTON]}" run "$exe" ${args[@]+"${args[@]}"} 2>&1 | cat || rc=$?
exit "$rc"
