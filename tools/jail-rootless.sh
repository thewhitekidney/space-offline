#!/usr/bin/env bash
# The rootless jail (IP29): the CI client's jail, with no sudo at all.
#
#   tools/jail-rootless.sh probe                  can this host make one? (0, or 3)
#   tools/jail-rootless.sh up RUNDIR -- CMD...    the namespace, eve-dns, and CMD
#                                                 (the server); prints "stack is up."
#                                                 and lives until CMD exits or it
#                                                 is killed; the namespace dies
#                                                 with it
#   tools/jail-rootless.sh check                  build the client's view inside the
#                                                 running namespace and run the gate;
#                                                 launch nothing
#   tools/jail-rootless.sh client                 the same, then tools/jail-client.sh
#   tools/jail-rootless.sh selftest RUNDIR        up with no server, then check
#   tools/jail-rootless.sh paths                  print the state paths this slot uses
#                                                 (EVE_RL_SLOT=N: a parallel slot's own)
#   tools/jail-rootless.sh --relay client         A FRIEND'S CLIENT (IP22-b): the
#                                                 namespace with tools/jail-relay.py
#                                                 in place of the server, then the
#                                                 client; needs tools/friend-tunnel.sh
#                                                 running; no sudo
#   tools/jail-rootless.sh --relay check          the same, launching nothing
#   tools/jail-rootless.sh --census up RUNDIR -- CMD...
#                                                 up, with the census (CEN-3): a
#                                                 default route into census0 and
#                                                 the recorders; needs
#                                                 out/census/live (client-ci.sh
#                                                 --census makes it)
#   tools/jail-rootless.sh census-dump            R2, the census nft table, as JSON
#   tools/jail-rootless.sh census-stop            SIGTERM the recorders and wait
#   tools/jail-rootless.sh --census selftest RUNDIR
#
# CI, and a friend's client. The owner's own play keeps the root helper.
#
# --relay (ruling 6; RR1 H2). A friend has no server here: tools/friend-tunnel.sh
# keeps `ssh -L` running to two unix sockets in $XDG_RUNTIME_DIR (eve-game.sock,
# eve-chat.sock), and tools/jail-relay.py, run INSIDE the namespace as the
# holder's command, listens on 10.137.0.1:26000 and :5222 and bridges each
# connection to its socket. A pathname socket is reached through the
# filesystem, not the network, so the namespace keeps its one interface (lo)
# and no route; there is no veth, no nft and no sudo. The client's view masks
# the runtime dir like any other, so the client reaches the sockets only
# through the relay (the gate's connect test covers them: they are host
# sockets). Without a built eve-dns (a friend's kit has no server), the relay
# answers DNS with eve-dns's jail policy.
#
# THE NETWORK. A user + network + mount + pid namespace whose one interface is
# `lo`, holding 127.0.0.1/8 and 10.137.0.1/32 (the helper jail's server
# address, so nothing else changes). There is no route to anywhere else, so
# "no phoning home" is structural: there is no interface to send through
# (V4 §6.3 A). eve-dns and eve-server run inside it, as you, with the
# namespace's capabilities and no others.
#
# THE CLIENT'S VIEW mirrors IP14's allowlist (tools/eve-offline-jail.in,
# build_view; ruling 4): an empty tmpfs over /run, /var/run, /tmp, $HOME and
# the runtime dir, the same bind-backs at their own host paths, a fresh
# /dev/shm. Three additions (docs/lanes/ip29.md, "Leaks"):
#   - /media and /mnt are masked too: a write to a network share (CIFS, an
#     automount) is carried by the host kernel, outside this netns;
#   - every host socket outside the masked trees gets an empty read-only file
#     bound over it (/var/snap holds live, connectable ones);
#   - the client has its own pid namespace and /proc, so it cannot walk into
#     the server's unmasked mount namespace through /proc/PID/root.
#
# THE CENSUS (--census; CEN-3, docs/guides/census.md). The holder also makes census0,
# a TUN device owned by you, gives the namespace default routes (v4 and v6)
# into it, loads the R2 counter table, and starts tools/census_sink.py and
# tools/census_proxy.py with every capability dropped. A TUN device has no
# wire: what is routed into it is bytes in the sink's fd, so the namespace
# still reaches nothing, and now every attempt is recorded. The gate then
# expects census0 and its routes instead of lo alone, and skips its egress
# probes and lookups (they would be recorded as the client's). Same steps,
# same table, as the helper's census-up (tools/eve-offline-jail.in);
# tools/test_jail.py holds the two nft texts equal.
#
# RULING 11. An unprivileged user namespace cannot call setgroups, so the
# client keeps its supplementary groups (docker, libvirt, sudo), unmapped but
# still honoured by the kernel. The defence is masking, and the gate below,
# run as the client user after every capability is dropped, must FAIL TO
# CONNECT to every host socket, by path and by abstract name, inventoried just
# before the launch. One connect that succeeds refuses the launch. The gate is
# the launch's precondition, not a log line.
set -euo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SELF=$REPO/tools/jail-rootless.sh
# PARALLEL SLOTS (lane ci-parallel, #139). EVE_RL_SLOT=N gives this namespace its own
# state directory and launch-env under out/client-ci/slots/N, so several jails of one
# checkout can be up at once; each is a namespace of its own, so 10.137.0.1 and the
# ports inside it do not collide. Unset, the paths are the ones this script always had.
# The slot is a number and nothing else: no path comes in from the environment. The
# launch-env is bound into the client's view at the canonical path, where
# jail-client.sh reads it; the gate and every refusal below are the same for every slot.
SLOT=${EVE_RL_SLOT:-}
[[ -z $SLOT || $SLOT =~ ^[1-9][0-9]{0,2}$ ]] || { echo "jail-rootless: EVE_RL_SLOT must be a number from 1 to 999, not '$SLOT'" >&2; exit 3; }
CANON_ENV_FILE=$REPO/out/client-ci/launch-env
if [[ -n $SLOT ]]; then
    STATE=$REPO/out/client-ci/slots/$SLOT/rootless
    ENV_FILE=$REPO/out/client-ci/slots/$SLOT/launch-env
else
    STATE=$REPO/out/client-ci/rootless
    ENV_FILE=$CANON_ENV_FILE
fi
HOLDER_PID=$STATE/holder.pid
INVENTORY=$STATE/host-sockets
RESOLV=$STATE/resolv.conf
HOST_IP=10.137.0.1
SERVER_NAME=tranquility.servers.eveonline.com
DNS_BIN=$REPO/server/target/debug/eve-dns
CLIENT_SHIM=$REPO/tools/jail-client.sh
# Sockets that are root by another name (ruling 3). Probed by name whether or
# not the host has them today, at both spellings, beside the inventory.
PRIVILEGED=(/run/docker.sock /var/run/docker.sock
            /run/libvirt/libvirt-sock /run/libvirt/libvirt-sock-ro
            /run/libvirt/libvirt-admin-sock /run/libvirt/virtqemud-sock
            /var/run/libvirt/libvirt-sock /var/run/libvirt/libvirt-sock-ro
            /run/dbus/system_bus_socket /run/snapd.socket /run/cups/cups.sock
            /run/systemd/resolve/io.systemd.Resolve /run/avahi-daemon/socket)
NETFS='cifs smb3 nfs nfs4 autofs 9p ceph glusterfs afs fuse.sshfs davfs fuse.rclone'
# The census, as the helper's (tools/eve-offline-jail.in).
CENSUS_DEV=census0
CENSUS_V4=198.18.0.2/32
CENSUS_V6=fd00:ce:5::2/64
CENSUS_LIVE=$REPO/out/census/live
CENSUS_SINK=$REPO/tools/census_sink.py
CENSUS_PROXY=$REPO/tools/census_proxy.py
CENSUS_PROXY_PORT=9

say() { printf '%s\n' "$*"; }
die() { printf 'jail-rootless: %s\n' "$*" >&2; exit 3; }

# The namespace's whole creation, one line: user (mapped to you, capabilities
# kept for the setup), net, mount, pid. --kill-child: when this dies, so does
# everything inside.
NS_CREATE=(unshare --user --map-current-user --keep-caps --net --mount
           --propagation private --pid --fork --kill-child --mount-proc --)

# Does what the jail's setup does, inside the namespace: loopback up (the
# holder's first step) and a tmpfs mount (the client's view). Creating the
# namespace is not enough: on Ubuntu 23.10+ with
# kernel.apparmor_restrict_unprivileged_userns=1 an unconfined process gets a
# namespace without capabilities, and it is `ip` and `mount` that fail.
v_probe() {
    local why='' sysctl=''
    "${NS_CREATE[@]}" bash -c 'ip link set lo up && d=$(mktemp -d) && mount -t tmpfs tmpfs "$d"; rc=$?; umount "$d" 2>/dev/null; rmdir "$d" 2>/dev/null; exit $rc' \
        >/dev/null 2>&1 && { say "ok    an unprivileged user+net+mount+pid namespace can be made, with loopback and a tmpfs"; return 0; }
    sysctl=$(cat "${EVE_RL_APPARMOR_SYSCTL:-/proc/sys/kernel/apparmor_restrict_unprivileged_userns}" 2>/dev/null || true)
    if [[ $sysctl == 1 ]]; then
        why="AppArmor restricts unprivileged user namespaces here (kernel.apparmor_restrict_unprivileged_userns=1).
  The friend side needs one of these, once, with sudo:
    sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
  That lasts until reboot and lowers protection for every program on the machine, not just this one.
  A narrower fix is an AppArmor profile that names /usr/bin/unshare only and allows 'userns,'; it is not
  set up here, and the owner has not settled which to recommend. Ask before you change either.
  Debian, Fedora, Arch and SteamOS are not affected."
    elif "${NS_CREATE[@]}" true >/dev/null 2>&1; then
        why="a namespace can be made, but loopback or a tmpfs mount failed inside it (is 'ip' installed? iproute2)"
    else
        why="unprivileged user namespaces are unavailable here (kernel.unprivileged_userns_clone, user.max_user_namespaces, or a container?)"
    fi
    die "$why"
}

# --- the holder ---------------------------------------------------------------

v_up() {
    local rundir=${1:?up RUNDIR -- CMD...}; shift
    [[ ${1:-} == -- ]] && shift
    mkdir -p "$STATE" "$rundir"
    [[ ${EVE_RL_DNS:-} == relay || -x $DNS_BIN ]] || die "no resolver at $DNS_BIN (build it first)"
    if [[ -s $HOLDER_PID ]] && kill -0 "$(cat "$HOLDER_PID")" 2>/dev/null; then
        die "a rootless jail from this checkout is already up (pid $(cat "$HOLDER_PID"))"
    fi
    printf 'nameserver %s\noptions timeout:1 attempts:1\n' "$HOST_IP" > "$RESOLV"
    "${NS_CREATE[@]}" "$SELF" _holder "$rundir" "$@" &
    local outer=$! h='' i rc=0
    # unshare blocks TERM while it waits for its child, so the signal goes to
    # the holder (pid 1 inside; it traps TERM): its exit takes the namespace
    # and everything in it down. Killing only $outer left the whole stack up.
    trap 'kill ${h:+"$h"} "$outer" 2>/dev/null; rm -f "$HOLDER_PID"; exit 143' TERM INT
    for i in $(seq 1 50); do
        h=$(pgrep -P "$outer" 2>/dev/null | head -1) || h=''
        [[ -n $h ]] && break
        sleep 0.1
    done
    [[ -n $h ]] || die "the namespace did not start"
    echo "$h" > "$HOLDER_PID"
    wait "$outer" || rc=$?
    rm -f "$HOLDER_PID"
    return "$rc"
}

# Inside, as you, with the namespace's capabilities; pid 1 of its pid ns.
v_holder() {
    local rundir=$1 srv i; shift
    trap 'census_stop; exit 0' TERM INT
    ip link set lo up
    ip addr add "$HOST_IP/32" dev lo
    # The census first, so it records from the very start (the server too).
    if [[ ${EVE_RL_CENSUS:-} == 1 ]]; then census_build || die "the census did not come up"; fi
    # --relay without eve-dns: the relay (the command below) answers DNS.
    if [[ ${EVE_RL_DNS:-} != relay ]]; then
        "$DNS_BIN" --bind "$HOST_IP:53" --target "$HOST_IP" --log "$rundir/eve-dns.log" \
            </dev/null >"$rundir/eve-dns.out" 2>&1 &
    fi
    if (( $# == 0 )); then say "stack is up."; while :; do sleep 3600 & wait $!; done; fi
    "$@" </dev/null >"$rundir/${EVE_RL_CMD_LOG:-eve-server.log}" 2>&1 &
    srv=$!
    for ((i = 0; i < ${EVE_BOOT_WAIT:-120}; i++)); do
        kill -0 "$srv" 2>/dev/null || { tail -n 5 "$rundir/${EVE_RL_CMD_LOG:-eve-server.log}" >&2 || true; die "the server died while booting"; }
        timeout 1 bash -c "exec 3<>/dev/tcp/$HOST_IP/26000" 2>/dev/null && break
        sleep 1
    done
    (( i < ${EVE_BOOT_WAIT:-120} )) || die "the server did not accept on $HOST_IP:26000"
    say "stack is up."
    wait "$srv"
}

# --- the census (--census) ------------------------------------------------------

# R2, kept equal to the helper's census_nft by tools/test_jail.py.
census_nft() {
    cat <<'NFT'
table inet eve_census {
    set tcp4 { type ipv4_addr . inet_service; flags dynamic; counter; size 65536; }
    set udp4 { type ipv4_addr . inet_service; flags dynamic; counter; size 65536; }
    set tcp6 { type ipv6_addr . inet_service; flags dynamic; counter; size 65536; }
    set udp6 { type ipv6_addr . inet_service; flags dynamic; counter; size 65536; }
    set oth4 { type ipv4_addr . inet_proto; flags dynamic; counter; size 4096; }
    set oth6 { type ipv6_addr . inet_proto; flags dynamic; counter; size 4096; }
    chain output {
        type filter hook output priority -10; policy accept;
        meta nfproto ipv4 tcp flags & (syn | ack) == syn update @tcp4 { ip daddr . tcp dport }
        meta nfproto ipv6 tcp flags & (syn | ack) == syn update @tcp6 { ip6 daddr . tcp dport }
        meta nfproto ipv4 meta l4proto udp update @udp4 { ip daddr . udp dport }
        meta nfproto ipv6 meta l4proto udp update @udp6 { ip6 daddr . udp dport }
        meta nfproto ipv4 meta l4proto != { tcp, udp } update @oth4 { ip daddr . meta l4proto }
        meta nfproto ipv6 meta l4proto != { tcp, udp } update @oth6 { ip6 daddr . meta l4proto }
    }
}
NFT
}

census_here() { ip link show "$CENSUS_DEV" >/dev/null 2>&1; }

# A recorder, with no capability in any set and no_new_privs: it attaches to
# a device you own and binds a port the sysctl below made unprivileged.
census_start() {   # SCRIPT ARGS...
    setpriv --inh-caps=-all --ambient-caps=-all --bounding-set=-all --no-new-privs -- \
        python3 -I "$@" </dev/null >>"$CENSUS_LIVE/recorders.err" 2>&1 &
}

# In the holder, as you, with the namespace's capabilities. [A] Each step
# works in a user-namespace-owned netns: the TUN device, the two per-netns
# sysctls, and nft dynamic sets with per-element counters. Falsifier:
# `jail-rootless.sh --census selftest RUNDIR` fails at that step.
census_build() {
    local mode i
    [[ -d $CENSUS_LIVE && -O $CENSUS_LIVE && ! -L $CENSUS_LIVE ]] \
        || { say "no directory $CENSUS_LIVE of yours (tools/client-ci.sh --census makes it)" >&2; return 1; }
    mode=$(cat "$CENSUS_LIVE/mode" 2>/dev/null || true)
    case $mode in '') mode=observe ;; observe|handshake) ;; *) say "bad census mode '$mode'" >&2; return 1 ;; esac
    ip tuntap add dev "$CENSUS_DEV" mode tun user "$(id -u)" group "$(id -g)" || return 1
    sysctl -qw "net.ipv6.conf.$CENSUS_DEV.router_solicitations=0" || return 1
    sysctl -qw "net.ipv6.conf.$CENSUS_DEV.accept_ra=0" || return 1
    ip addr add "$CENSUS_V4" dev "$CENSUS_DEV" || return 1
    ip -6 addr add "$CENSUS_V6" dev "$CENSUS_DEV" nodad || return 1
    ip link set "$CENSUS_DEV" up || return 1
    ip route add default dev "$CENSUS_DEV" || return 1
    ip -6 route add default dev "$CENSUS_DEV" || return 1
    # Only your own gid is mapped here, so that is the ping range.
    sysctl -qw "net.ipv4.ping_group_range=$(id -g) $(id -g)" || return 1
    sysctl -qw "net.ipv4.ip_unprivileged_port_start=$CENSUS_PROXY_PORT" || return 1
    census_nft | nft -c -f - || return 1
    census_nft | nft -f - || return 1
    census_start "$CENSUS_SINK" --dev "$CENSUS_DEV" --out "$CENSUS_LIVE/tun.jsonl" --mode "$mode"
    census_start "$CENSUS_PROXY" --out "$CENSUS_LIVE/proxy.jsonl" --port "$CENSUS_PROXY_PORT"
    for i in $(seq 1 50); do
        if ! grep -q NO-CARRIER <<<"$(ip link show "$CENSUS_DEV")" \
           && grep -q . <<<"$(ss -Hltn "sport = :$CENSUS_PROXY_PORT" 2>/dev/null)"; then
            say "census up: default routes -> $CENSUS_DEV; sink ($mode), proxy 127.0.0.1:$CENSUS_PROXY_PORT"
            return 0
        fi
        sleep 0.2
    done
    tail -n 5 "$CENSUS_LIVE/recorders.err" >&2 2>/dev/null || true
    return 1
}

# The recorders of THIS namespace: our command line, and the net namespace of
# the holder (or our own, inside it).
census_pids() {   # NETNS-LINK
    local script pid
    for script in "$CENSUS_SINK" "$CENSUS_PROXY"; do
        for pid in $(pgrep -u "$(id -u)" -f "^python3 -I $script " 2>/dev/null || true); do
            [[ $(readlink "/proc/$pid/ns/net" 2>/dev/null) == "$1" ]] && printf '%s\n' "$pid"
        done
    done
    return 0
}

# SIGTERM, then wait up to 10 s: each writes its stop line on SIGTERM. From
# the holder's trap (before the pid namespace takes everything down with
# SIGKILL) or from outside (`census-stop`, before the dump is read).
census_stop() {
    local ns pids i
    ns=$(readlink "/proc/${1:-self}/ns/net")
    pids=$(census_pids "$ns")
    [[ -n $pids ]] || return 0
    kill -TERM $pids 2>/dev/null || true
    for i in $(seq 1 50); do
        pids=$(census_pids "$ns")
        [[ -z $pids ]] && return 0
        sleep 0.2
    done
    kill -KILL $pids 2>/dev/null || true
}

v_census_dump() {
    local h
    h=$(holder)
    nsenter -t "$h" -U -n --preserve-credentials --keep-caps -- nft -j list table inet eve_census
}

v_census_stop() {
    local h
    h=$(holder)
    census_stop "$h"
}

# --- the client's view --------------------------------------------------------

holder() {
    local h
    h=$(cat "$HOLDER_PID" 2>/dev/null) || die "no rootless jail is up (no $HOLDER_PID)"
    [[ $h =~ ^[0-9]+$ ]] && tr '\0' ' ' < "/proc/$h/cmdline" 2>/dev/null | grep -q "jail-rootless.sh _holder" \
        || die "pid $h is not this checkout's rootless jail"
    printf '%s' "$h"
}

# The host's sockets, from the host side, just before the launch: every
# pathname socket on every local filesystem and tmpfs, and every abstract
# name in the host's network namespace (`@name`). The gate must fail to
# connect to each. Also the two LAN addresses the egress probe aims at,
# found with `ip route get` on TEST-NET, which sends nothing (ruling 10).
inventory() {
    local m
    {
        while read -r m; do
            find "$m" -xdev -type s -print 2>/dev/null || true
        done < <(findmnt -rn -o TARGET -t ext4,ext3,xfs,btrfs,f2fs,zfs,vfat,tmpfs | sed 's/\\x20/ /g')
        awk 'NR > 1 && $8 ~ /^@/ { print $8 }' /proc/net/unix
        printf '%s\n' "${PRIVILEGED[@]}"
    } | sort -u > "$INVENTORY"
    GW=$(ip -4 route show default 2>/dev/null | awk '{print $3; exit}')
    LAN=$(ip -4 route get 192.0.2.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") { print $(i + 1); exit }}')
}

v_enter() {   # check | client
    local mode=$1 h
    h=$(holder)
    [[ $mode == check || -x $CLIENT_SHIM ]] || die "no client shim at $CLIENT_SHIM"
    inventory
    say "inventory: $(wc -l < "$INVENTORY") host sockets and names the client must fail to reach"
    exec nsenter -t "$h" -U -n -m -p --preserve-credentials --keep-caps -- \
        unshare --mount --propagation private --pid --fork --kill-child --mount-proc -- \
        env EVE_RL_GW="${GW:-}" EVE_RL_LAN="${LAN:-}" "$SELF" _view "$mode"
}

declare -A LENV=()
read_launch_env() {
    local key value
    [[ -r $ENV_FILE ]] || return 0
    while IFS='=' read -r key value; do
        case $key in
            XDG_RUNTIME_DIR|WAYLAND_DISPLAY|DISPLAY|XAUTHORITY|STEAM_COMPAT_DATA_PATH|EVE_CLIENT|PROTON)
                LENV[$key]=$value ;;
        esac
    done < "$ENV_FILE"
}

# vet OWNER KIND PATH, as IP14's: resolved, the right kind, not /, owned by
# OWNER. Root appears here as the overflow uid (it is not mapped).
vet() {
    local owner=$1 kind=$2 p=$3 r u
    VETTED=''
    [[ -n $p && $p == /* && $p != *[[:cntrl:]]* ]] || return 1
    r=$(realpath -e -- "$p" 2>/dev/null) || return 1
    [[ $r != / ]] || return 1
    case $kind in
        dir) [[ -d $r ]] ;; file) [[ -f $r ]] ;; socket) [[ -S $r && $r == "$p" ]] ;; *) false ;;
    esac || return 1
    u=$(stat -c %u -- "$r") || return 1
    case $owner in
        user) [[ $u == "$UID_C" ]] ;;
        root) [[ $u == "$(cat /proc/sys/kernel/overflowuid)" ]] ;;
        any)  true ;;
        *)    false ;;
    esac || return 1
    VETTED=$r
}

# O19, as IP14's (tools/eve-offline-jail.in): no entry of the view is, lies
# inside, or holds the online EVE prefix (Steam appid 8500), not even
# read-only. The client runs in its own copy (tools/offline-prefix.sh).
ONLINE_PREFIX_ID=8500
online_overlap() {   # PATH (resolved)
    local p=$1
    case $p in
        */[sS]team[aA]pps/compatdata/"$ONLINE_PREFIX_ID" | */[sS]team[aA]pps/compatdata/"$ONLINE_PREFIX_ID"/*)
            return 0 ;;
    esac
    [[ -n ${ONLINE_R:-} ]] || return 1
    [[ $ONLINE_R == "$p" || $ONLINE_R == "$p"/* ]]
}

# As IP14's: covered entries are skipped, except a read-only one inside a
# read-write one, which goes on top (ResFiles inside the prefix, O19).
bind_back() {   # MODE OWNER KIND LABEL SRC [TARGET]
    local mode=$1 owner=$2 kind=$3 label=$4 src=$5 target=${6:-} id n t i
    vet "$owner" "$kind" "$src" || { say "skip  $label (absent, or not yours: ${src:-unset})"; return 0; }
    src=$VETTED
    online_overlap "$src" \
        && die "$label ($src) is, is inside, or holds the online EVE prefix (compatdata/$ONLINE_PREFIX_ID); the client has its own (tools/offline-prefix.sh, O19); refusing"
    target=${target:-$src}
    for i in ${B_TARGET[@]+"${!B_TARGET[@]}"}; do
        t=${B_TARGET[$i]}
        [[ $target == "$t" || $target == "$t"/* ]] || continue
        [[ $mode == ro && ${B_MODE[$i]} == rw && $target != "$t" ]] || return 0
    done
    id=$(stat -c %d:%i -- "$src"); n=${#B_TARGET[@]}
    if [[ $kind == dir ]]; then mkdir -- "$STAGE/$n"; else : > "$STAGE/$n"; fi
    mount --bind -- "$src" "$STAGE/$n"
    [[ $(stat -c %d:%i -- "$STAGE/$n") == "$id" ]] || die "$label changed while it was bound back"
    B_MODE+=("$mode") B_KIND+=("$kind") B_LABEL+=("$label") B_TARGET+=("$target")
}

mask_tree() {   # LABEL PATH MODE
    mkdir -p -- "$2"
    mount -t tmpfs -o "nosuid,nodev,mode=$3" "eve-rootless-$1" "$2"
}

place_all() {
    local i t opts
    for i in ${B_TARGET[@]+"${!B_TARGET[@]}"}; do
        t=${B_TARGET[$i]}
        if [[ ${B_KIND[$i]} == dir ]]; then mkdir -p -- "$t"
        else mkdir -p -- "$(dirname -- "$t")"; [[ -e $t ]] || : > "$t"; fi
        mount --bind -- "$STAGE/$i" "$t"
        opts=remount,bind,nosuid,nodev
        [[ ${B_MODE[$i]} == ro ]] && opts+=,ro
        mount -o "$opts" -- "$t"
        umount -- "$STAGE/$i"
    done
}

# pid 1 of the client's own pid namespace, as you, still holding the
# namespace's capabilities (that is what lets it mount).
v_view() {
    local mode=$1 home wl p t skip rc=0 others census
    local -a want=() prune=()
    UID_C=$(id -u)
    STAGE=/dev/shm
    B_MODE=() B_KIND=() B_LABEL=() B_TARGET=()
    read_launch_env
    home=$(getent passwd "$UID_C" | cut -d: -f6)
    HOME_R=$(realpath -e -- "$home") || die "cannot resolve your home '$home'"
    RUNTIME_R=''
    if vet user dir "${LENV[XDG_RUNTIME_DIR]:-}"; then RUNTIME_R=$VETTED; fi
    ONLINE_R=$(realpath -m -- "$HOME_R/.local/share/Steam/steamapps/compatdata/$ONLINE_PREFIX_ID")
    # Two readers of the inventory, opened before $HOME is masked: one for
    # the masking below, one for the gate.
    exec 3< "$INVENTORY" 4< "$INVENTORY"
    mount -t tmpfs -o nosuid,nodev,mode=0700 eve-rootless-stage "$STAGE"
    : > "$STAGE/empty"; chmod 0444 "$STAGE/empty"

    # THE ALLOWLIST: IP14's lines, in IP14's order, plus this script (the gate).
    bind_back ro root dir    udev       /run/udev
    bind_back ro user file   resolver   "$RESOLV" "$(realpath -m -- /etc/resolv.conf)"
    bind_back rw user socket pulse      "${RUNTIME_R:+$RUNTIME_R/pulse/native}"
    bind_back rw user socket pipewire   "${RUNTIME_R:+$RUNTIME_R/pipewire-0}"
    wl=${LENV[WAYLAND_DISPLAY]:-}
    [[ $wl =~ ^wayland-[0-9]+$ ]] || wl=''
    bind_back rw user socket wayland    "${wl:+${RUNTIME_R:+$RUNTIME_R/$wl}}"
    bind_back ro user file   xauthority "${LENV[XAUTHORITY]:-}"
    if [[ ${LENV[DISPLAY]:-} =~ ^(unix)?:([0-9]+)(\.[0-9]+)?$ ]]; then
        bind_back rw any socket x11     "/tmp/.X11-unix/X${BASH_REMATCH[2]}"
    fi
    bind_back ro user dir    proton     "$(dirname -- "${LENV[PROTON]:-/}")"
    bind_back rw user dir    prefix     "${LENV[STEAM_COMPAT_DATA_PATH]:-}"
    bind_back rw user dir    game       "${LENV[EVE_CLIENT]:-}"
    bind_back ro user dir    resfiles   "${LENV[EVE_CLIENT]:+${LENV[EVE_CLIENT]}/../ResFiles}"
    bind_back ro user file   shim       "$CLIENT_SHIM"
    bind_back ro user file   launch-env "$ENV_FILE" "$CANON_ENV_FILE"
    bind_back ro user file   gate       "$SELF"
    # jail-client.sh runs the patcher's launch-env (PATCH-1) inside the view;
    # $HOME is masked, so a checkout or kit under $HOME needs it put back.
    # Not in IP14's list (docs/lanes/friends.md, "The patcher in the view").
    bind_back ro user file   patcher    "$REPO/tools/eve_patch.py"
    bind_back ro user dir    patches    "$REPO/tools/patches"
    # The census (CEN-3), as IP14's: only while census0 is here.
    census=0
    if census_here; then
        census=1
        bind_back ro user dir    census-bin    "$REPO/tools/census"
        bind_back ro user file   census-canary "$REPO/tools/census_canary.py"
        bind_back ro user file   census-wire   "$REPO/tools/census_wire.py"
        bind_back rw user file   census-exec   "$CENSUS_LIVE/exec.log"
        bind_back rw user file   census-marks  "$CENSUS_LIVE/canary.jsonl"
    fi

    mask_tree run /run 0755
    [[ $(realpath -m -- /var/run) == /run ]] || mask_tree var-run /var/run 0755
    mask_tree tmp /tmp 1777
    mask_tree home "$HOME_R" 0700
    [[ -z $RUNTIME_R ]] || mask_tree runtime "$RUNTIME_R" 0700
    for p in /media /mnt; do [[ -d $p ]] && mask_tree "${p#/}" "$p" 0755; done
    place_all

    # Ruling 11: every host socket still reachable by path gets an empty,
    # read-only file bound over it, unless it is an allowlisted socket.
    while IFS= read -r p <&3; do
        [[ $p == /* && -S $p ]] || continue
        skip=0
        for t in "${B_TARGET[@]}"; do [[ $p == "$t" ]] && skip=1; done
        (( skip )) && continue
        { mount --bind -- "$STAGE/empty" "$p" && mount -o remount,bind,ro,nosuid,nodev -- "$p"; } 2>/dev/null \
            || say "note  could not mask $p (the gate decides)"
    done
    exec 3<&-
    umount -l -- "$STAGE"
    mount -t tmpfs -o nosuid,nodev,mode=1777 eve-rootless-shm /dev/shm
    # /sys is the HOST's sysfs, mounted in the host's net namespace: it lists the
    # host's interfaces and has no census0, so jail-client.sh's census_setup
    # (which reads /sys/class/net/census0/tun_flags) refused every rootless
    # census launch although the namespace had the device (found 2026-10-30).
    # A fresh sysfs, mounted here, shows this namespace's devices. Read-only.
    if (( census )); then
        mount -t sysfs -o ro,nosuid,nodev,noexec eve-rootless-sys /sys \
            || die "cannot mount a sysfs for this namespace; the census launch could not see census0"
    fi
    say "view  bound back: ${B_LABEL[*]:-nothing}"

    for p in "${!B_TARGET[@]}"; do
        case ${B_KIND[$p]} in
            socket) want+=("${B_TARGET[$p]}") ;;
            dir)    prune+=("${B_TARGET[$p]}") ;;
        esac
    done
    # The drop: no capability in any set, no_new_privs, and a scrubbed
    # environment (sudo's env_reset, which this path has no sudo to do).
    # Groups cannot be dropped here (ruling 11); the gate is the defence.
    setpriv --inh-caps=-all --ambient-caps=-all --bounding-set=-all --no-new-privs -- \
        env -i PATH=/usr/bin:/bin LANG="${LANG:-C.UTF-8}" \
            EVE_RL_GW="${EVE_RL_GW:-}" EVE_RL_LAN="${EVE_RL_LAN:-}" EVE_RL_UID="$UID_C" EVE_RL_CENSUS="$census" \
            EVE_RL_WANT="${want[*]:-}" EVE_RL_PRUNE="${prune[*]:-}" \
            EVE_RL_ROOTS="/run /tmp /dev/shm $HOME_R${RUNTIME_R:+ $RUNTIME_R}" \
        bash "$SELF" _gate "$mode" || rc=$?
    exec 4<&-
    # Proton returns once wineserver has the game; stay pid 1 until every
    # process in this namespace has gone (bash reaps the orphans), so the
    # launcher's exit still means "the client is gone".
    while :; do
        others=0
        for p in /proc/[0-9]*; do [[ ${p#/proc/} == 1 ]] || { others=1; break; }; done
        (( others )) || break
        sleep 2
    done
    return "$rc"
}

# --- the gate (ruling 11) -------------------------------------------------------
# As the client user, after the drop. Any LEAK refuses the launch (exit 3).
v_gate() {
    local mode=$1 bad=0 p t w hit addrs links
    local -a prune=()
    leak() { say "LEAK  $*"; bad=1; }
    ok() { say "ok    $*"; }
    [[ $(id -u) == "$EVE_RL_UID" ]] && ok "uid $(id -u)" || leak "running as uid $(id -u), not $EVE_RL_UID"
    [[ $(awk '$1 == "CapEff:" {print $2}' /proc/self/status) == 0000000000000000 ]] \
        && ok "no effective capability" || leak "a capability survived the drop"
    [[ $(awk '$1 == "NoNewPrivs:" {print $2}' /proc/self/status) == 1 ]] \
        && ok "no_new_privs" || leak "no_new_privs is not set"
    say "--    groups $(id -G) (unmapped show as $(cat /proc/sys/kernel/overflowgid); ruling 11: not droppable here, so every socket is masked and probed)"
    for p in /proc/self/fd/*; do
        [[ $(readlink "$p" 2>/dev/null) == socket:* ]] && leak "an inherited socket on fd ${p##*/}"
    done
    links=$(ip -o link show | awk -F': ' '{print $2}' | tr '\n' ' ')
    addrs=$(ip -o -4 addr show | awk '{print $4}' | sort | tr '\n' ' ')
    if [[ ${EVE_RL_CENSUS:-0} == 1 ]]; then
        # The census: census0 and its routes, and nothing else. A packet the
        # route takes there is recorded and goes nowhere (a TUN has no wire).
        [[ $links == "lo $CENSUS_DEV " ]] && ok "the interfaces are lo and $CENSUS_DEV (census)" || leak "interfaces: $links"
        [[ $addrs == "10.137.0.1/32 127.0.0.1/8 $CENSUS_V4 " ]] && ok "addresses: $addrs" || leak "addresses: $addrs"
        p=$(ip -o -6 addr show scope global 2>/dev/null | awk '{print $2 " " $4}' | tr '\n' ' ')
        [[ $p == "$CENSUS_DEV $CENSUS_V6 " ]] && ok "the only global IPv6 is $CENSUS_V6 on $CENSUS_DEV" || leak "global IPv6: $p"
        [[ $(ip route get 192.0.2.1 2>/dev/null) == *" dev $CENSUS_DEV "* ]] \
            && ok "192.0.2.1 routes to $CENSUS_DEV (nothing sent)" || leak "192.0.2.1 does not route to $CENSUS_DEV"
        [[ $(ip -6 route get 2001:db8::1 2>/dev/null) == *" dev $CENSUS_DEV "* ]] \
            && ok "2001:db8::1 routes to $CENSUS_DEV (nothing sent)" || leak "2001:db8::1 does not route to $CENSUS_DEV"
        # What jail-client.sh's census_setup will read, from where it reads it:
        # the gate agreeing with `ip` while the shim saw no census0 (a /sys of
        # the host's netns) was the 2026-10-30 refusal. IFF_PERSIST (0x800) rides along.
        tf=$(cat "/sys/class/net/$CENSUS_DEV/tun_flags" 2>/dev/null || true)
        [[ $tf == 0x[0-9a-f]* ]] && (( (tf & ~0x800) == 0x1001 )) \
            && ok "/sys shows $CENSUS_DEV as a TUN device ($tf)" || leak "/sys/class/net/$CENSUS_DEV/tun_flags is '$tf', not a TUN device: jail-client.sh would refuse"
    else
        [[ $links == 'lo ' ]] && ok "the only interface is lo" || leak "interfaces: $links"
        [[ $addrs == "10.137.0.1/32 127.0.0.1/8 " ]] && ok "addresses: $addrs" || leak "addresses: $addrs"
        [[ -z $(ip -6 addr show scope global 2>/dev/null) ]] && ok "no global IPv6" || leak "a global IPv6 address"
        ip route get 192.0.2.1 >/dev/null 2>&1 && leak "a route to 192.0.2.1 exists" \
            || ok "no route to 192.0.2.1 (nothing sent)"
    fi
    ip addr add 10.137.0.9/32 dev lo 2>/dev/null && leak "ip addr add succeeded as uid $(id -u)" \
        || ok "ip addr add refused as uid $(id -u)"
    if [[ ${EVE_RL_CENSUS:-0} == 1 ]]; then
        # Every probe and lookup here would land in the census as the
        # client's own (a LEAK in the report); the routes above are the check.
        say "--    census: egress probes and lookups skipped (the recorders would count them as the client's)"
    else
        for p in ${EVE_RL_GW:+$EVE_RL_GW:53} ${EVE_RL_LAN:+$EVE_RL_LAN:22} 172.17.0.1:53 192.168.122.1:53; do
            timeout 3 bash -c "exec 3<>/dev/tcp/${p%:*}/${p#*:}" 2>/dev/null && leak "connected to $p" || ok "$p unreachable"
        done
        for p in sentry.io clientresources.eveonline.com; do
            timeout 3 getent hosts "$p" >/dev/null 2>&1 && leak "resolved $p" || ok "cannot resolve $p"
        done
        p=$(timeout 3 getent hosts "$SERVER_NAME" 2>/dev/null | awk '{print $1; exit}') || p=''
        [[ -z $p || $p == "$HOST_IP" ]] && ok "$SERVER_NAME -> ${p:-nothing yet}" || leak "$SERVER_NAME -> $p"
    fi
    # The socket listing must equal the allowlist (IP14's scan_view).
    for p in $EVE_RL_PRUNE; do prune+=(-path "$p" -o); done
    while IFS= read -r -d '' p; do
        hit=0
        for w in $EVE_RL_WANT; do [[ $p == "$w" ]] && hit=1; done
        (( hit )) || leak "socket $p is visible"
    done < <(find $EVE_RL_ROOTS \( ${prune[@]+"${prune[@]}"} -false \) -prune -o -type s -print0 2>/dev/null)
    ok "socket listing checked against the allowlist: ${EVE_RL_WANT:-none}"
    # Network filesystems: a write there leaves the machine by the host kernel.
    while read -r p t; do
        [[ $p == /proc/* || $p == /sys/* ]] && continue
        [[ " $NETFS " == *" $t "* ]] || continue
        [[ -e $p ]] && leak "network filesystem visible at $p ($t)"
    done < <(findmnt -rn -o TARGET,FSTYPE | sed 's/\\x20/ /g')
    ok "no network filesystem visible"
    # Ruling 11, the point of all this: connect to every host socket and name.
    python3 -I -c '
import os, socket, sys
want = set(os.environ.get("EVE_RL_WANT", "").split())
bad = n = 0
for line in os.fdopen(4):
    p = line.rstrip("\n")
    if not p or p in want:
        continue
    n += 1
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect("\0" + p[1:] if p.startswith("@") else p)
        print("LEAK  connected to " + p); bad = 1
    except OSError:
        pass
    finally:
        s.close()
if not bad:
    print("ok    failed to connect to every one of %d host sockets and names" % n)
sys.exit(bad)' || bad=1
    exec 4<&-
    (( bad == 0 )) || { say "GATE REFUSED: the client does not start"; exit 3; }
    say "GATE OK"
    [[ $mode == client ]] || exit 0
    exec "$CLIENT_SHIM"
}

# --- a friend's client: the relay in place of the server (IP22-b) ---------------

# The launch-env for a friend's own copy, as client-ci.sh's rootless_client
# writes it (run-client.sh's keys), plus the login (ACC2-TOOLS): the token
# from ~/.config/eve-offline/login, never a command line; mode 600; the token
# line removed once the client is running (relay_scrub).
relay_env() {
    # shellcheck source=tools/prefix-lib.sh
    source "$REPO/tools/prefix-lib.sh"
    # shellcheck source=tools/login-lib.sh
    source "$REPO/tools/login-lib.sh"
    local steam=${STEAM_ROOT:-$HOME/.local/share/Steam} prefix client proton login token
    prefix=${EVE_PREFIX:-$(eve_offline_prefix "$HOME")}
    client=${EVE_CLIENT:-$prefix/pfx/drive_c/CCP/EVE/${EVE_CLIENT_DIR:-tq-offline}}
    proton=${PROTON:-$steam/compatibilitytools.d/GE-Proton10-34/proton}
    eve_refuse_online jail-rootless.sh "EVE_PREFIX=$prefix" "EVE_CLIENT=$client"
    [[ -d $prefix/pfx ]] || die "no Wine prefix at $prefix (tools/offline-prefix.sh, or set EVE_PREFIX)"
    [[ -d $client ]] || die "no client at $client (set EVE_CLIENT)"
    [[ -x $proton ]] || die "no proton at $proton (set PROTON=/path/to/proton)"
    login=$(eve_login_path "$HOME" "")
    if [[ ! -e $login && -t 0 ]]; then eve_login_prompt "$login" || exit 1; fi
    token=$(eve_login_token "$login" "$(id -u)") || die "no usable login: tools/account.sh login"
    say "login: $(eve_login_user "$login") (from $login)"
    mkdir -p "$(dirname -- "$ENV_FILE")"
    (umask 077 && : > "$ENV_FILE")
    chmod 600 "$ENV_FILE"
    {
        printf 'HOME=%s\n' "$HOME"
        printf 'DISPLAY=%s\n' "${DISPLAY:-}"
        printf 'WAYLAND_DISPLAY=%s\n' "${WAYLAND_DISPLAY:-}"
        printf 'XDG_RUNTIME_DIR=%s\n' "${XDG_RUNTIME_DIR:-}"
        printf 'XAUTHORITY=%s\n' "${XAUTHORITY:-}"
        printf 'PROTON_USE_XALIA=%s\n' "${PROTON_USE_XALIA:-}"
        printf 'VK_DRIVER_FILES=%s\n' "${VK_DRIVER_FILES:-}"
        printf 'VK_ICD_FILENAMES=%s\n' "${VK_ICD_FILENAMES:-}"
        printf 'LP_NUM_THREADS=%s\n' "${LP_NUM_THREADS:-}"
        printf 'PROTON_LOG=%s\n' "${PROTON_LOG:-}"
        printf 'PROTON_LOG_DIR=%s\n' "${PROTON_LOG_DIR:-}"
        printf 'WINEDEBUG=%s\n' "${WINEDEBUG:-}"
        printf 'STEAM_COMPAT_DATA_PATH=%s\n' "$prefix"
        printf 'STEAM_COMPAT_CLIENT_INSTALL_PATH=%s\n' "$steam"
        printf 'EVE_CLIENT=%s\n' "$client"
        printf 'PROTON=%s\n' "$proton"
        printf 'EVE_CLIENT_ARGS=%s\n' '/fileLogDirectory=C:\eve-logs /noCrashReportUpload'
        printf 'EVE_SSO_TOKEN=%s\n' "$token"
    } > "$ENV_FILE"
}

# Remove the token line IN PLACE: the view holds the file by a bind mount,
# so a rename (sed -i) would leave the old inode, token and all, inside it.
relay_scrub() {
    local rest
    [[ -f $ENV_FILE ]] || return 0
    rest=$(grep -v '^EVE_SSO_TOKEN=' "$ENV_FILE" || true)
    printf '%s\n' "$rest" > "$ENV_FILE"
}

# [A] RELAY_SCRUB_WAIT: seconds to wait for the client's exefile.exe before
# the token line goes anyway. jail-client.sh reads the file well before
# Proton starts the game. Falsified by a slow first launch that logs in as
# nobody (no /ssoToken).
RELAY_SCRUB_WAIT=180

# F6: the client writes its log inside the prefix (C:\eve-logs); the folder a
# friend is asked to send the owner is $rundir, so the newest log from this
# run is copied there. Never fatal: a missing log is not a failed launch.
# That directory is written by the client, which a hostile server can drive,
# and this runs on the host as you: so it is called only after the namespace
# is gone, takes regular files only (a symlink to ~/.ssh/... or a FIFO is
# skipped, never followed or waited on), caps the size, and never overwrites.
RELAY_LOG_MAX=$((8 * 1024 * 1024))
relay_keep_client_log() {
    local rundir=$1 dir out
    dir=${EVE_PREFIX:-$(eve_offline_prefix "$HOME")}/pfx/drive_c/eve-logs
    out=$(python3 -I -c '
import os, stat, sys
d, ref, dst, cap = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
try:
    since = os.stat(ref).st_mtime
    names = os.listdir(d)
except OSError:
    sys.exit(0)
best = None
for n in names:
    if not (n.startswith("client-") and n.endswith(".log")):
        continue
    p = os.path.join(d, n)
    try:
        st = os.lstat(p)
    except OSError:
        continue
    if stat.S_ISREG(st.st_mode) and st.st_mtime > since and (best is None or st.st_mtime > best[0]):
        best = (st.st_mtime, p)
if best is None:
    sys.exit(0)
try:
    fd = os.open(best[1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
except OSError:
    sys.exit(0)
try:
    st = os.fstat(fd)
    if not stat.S_ISREG(st.st_mode):
        sys.exit(0)
    out = os.open(dst, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(out, "wb") as o:
        left = cap
        while left > 0:
            chunk = os.read(fd, min(65536, left))
            if not chunk:
                break
            o.write(chunk)
            left -= len(chunk)
        if left == 0 and os.read(fd, 1):
            o.write(b"\n[truncated at %d bytes]\n" % cap)
    print(best[1])
finally:
    os.close(fd)
' "$dir" "$rundir/up.log" "$rundir/client.log" "$RELAY_LOG_MAX" 2>/dev/null) || out=''
    if [[ -n $out ]]; then say "client log: $rundir/client.log"
    else say "no client log from this run under $dir"; fi
}

v_relay() {   # check | client
    local mode=${1:-} rt rundir up rc=0 dnsmode=bin s i
    local -a relay=()
    [[ $mode == check || $mode == client ]] || die "usage: jail-rootless.sh --relay check|client"
    v_probe >/dev/null
    rt=${XDG_RUNTIME_DIR:-}
    [[ -n $rt && -d $rt && -O $rt ]] || die "XDG_RUNTIME_DIR is unset or not yours"
    for s in "$rt/eve-game.sock" "$rt/eve-chat.sock"; do
        [[ -S $s ]] || die "no $s: start tools/friend-tunnel.sh HOST PORT first, and leave it running"
        python3 -I -c 'import socket, sys
s = socket.socket(socket.AF_UNIX); s.settimeout(5); s.connect(sys.argv[1]); s.close()' "$s" 2>/dev/null \
            || die "$s does not answer: is tools/friend-tunnel.sh still running?"
    done
    rundir=$REPO/out/friend/$(date -u +%Y%m%dT%H%M%SZ)
    mkdir -p "$rundir"
    relay=(python3 -I "$REPO/tools/jail-relay.py" run
           --map "$HOST_IP:26000=$rt/eve-game.sock" --map "$HOST_IP:5222=$rt/eve-chat.sock")
    # The gateway is optional: an owner on an older tunnel has no third forward,
    # and game and chat must still work there.
    [[ ! -S $rt/eve-gw.sock ]] || relay+=(--map "$HOST_IP:8443=$rt/eve-gw.sock")
    if [[ ! -x $DNS_BIN ]]; then
        dnsmode=relay
        relay+=(--dns "$HOST_IP:53" --dns-target "$HOST_IP" --dns-log "$rundir/eve-dns.log")
    fi
    [[ $mode == check ]] || relay_env
    EVE_RL_DNS=$dnsmode EVE_RL_CMD_LOG=relay.log \
        "$SELF" up "$rundir" -- "${relay[@]}" </dev/null >"$rundir/up.log" 2>&1 &
    up=$!
    trap 'relay_scrub; kill -TERM "$up" 2>/dev/null; wait "$up" 2>/dev/null; exit 143' TERM INT
    for ((i = 0; i < 300; i++)); do
        grep -q '^stack is up\.' "$rundir/up.log" 2>/dev/null && break
        kill -0 "$up" 2>/dev/null || { relay_scrub; tail -n 5 "$rundir/up.log" >&2 || true; die "the relay's namespace did not start (log $rundir)"; }
        sleep 0.2
    done
    grep -q '^stack is up\.' "$rundir/up.log" || { relay_scrub; kill -TERM "$up" 2>/dev/null; die "the relay did not come up (log $rundir)"; }
    say "relay up: $HOST_IP:26000 and :5222 -> the tunnel; dns: $dnsmode; log $rundir"
    if [[ $mode == client ]]; then
        (
            for ((i = 0; i < RELAY_SCRUB_WAIT; i++)); do
                pgrep -u "$(id -u)" -f 'exefile\.exe' >/dev/null 2>&1 && break
                sleep 1
            done
            relay_scrub
        ) &
    fi
    "$SELF" "$mode" </dev/null || rc=$?
    relay_scrub
    kill -TERM "$up" 2>/dev/null; wait "$up" 2>/dev/null || true
    [[ $mode != client ]] || relay_keep_client_log "$rundir"
    return "$rc"
}

v_selftest() {
    local rundir=${1:?selftest RUNDIR} up rc=0
    v_probe
    mkdir -p "$rundir"
    if [[ ${EVE_RL_CENSUS:-} == 1 ]]; then
        # The census needs its live directory; a selftest's is throwaway.
        rm -rf "$CENSUS_LIVE"; mkdir -p -m 0700 "$CENSUS_LIVE"
        : > "$CENSUS_LIVE/exec.log"; : > "$CENSUS_LIVE/canary.jsonl"
    fi
    "$SELF" up "$rundir" > "$rundir/up.log" 2>&1 &
    up=$!
    for _ in $(seq 1 50); do grep -q '^stack is up\.' "$rundir/up.log" 2>/dev/null && break; sleep 0.2; done
    "$SELF" check || rc=$?
    if [[ ${EVE_RL_CENSUS:-} == 1 ]]; then
        "$SELF" census-dump > "$rundir/nft.json" || rc=1
        "$SELF" census-stop || rc=1
        grep -q '"event": "stop"' "$CENSUS_LIVE/tun.jsonl" 2>/dev/null \
            && say "ok    the sink wrote its stop line" || { say "FAIL  the sink has no stop line"; rc=1; }
    fi
    kill -TERM "$up" 2>/dev/null; wait "$up" 2>/dev/null || true
    return "$rc"
}

# --census goes first and applies to what follows (up, selftest).
if [[ ${1:-} == --census ]]; then export EVE_RL_CENSUS=1; shift; fi
case "${1:-}" in
    probe)    v_probe ;;
    paths)    printf 'STATE=%s\nHOLDER_PID=%s\nINVENTORY=%s\nRESOLV=%s\nENV_FILE=%s\n' "$STATE" "$HOLDER_PID" "$INVENTORY" "$RESOLV" "$ENV_FILE" ;;
    census-dump) v_census_dump ;;
    census-stop) v_census_stop ;;
    --relay)  v_relay "${2:-}" ;;
    up)       shift; v_up "$@" ;;
    check)    v_enter check ;;
    client)   v_enter client ;;
    selftest) shift; v_selftest "$@" ;;
    _holder)  shift; v_holder "$@" ;;
    _view)    v_view "$2" ;;
    _gate)    v_gate "$2" ;;
    *) sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac
