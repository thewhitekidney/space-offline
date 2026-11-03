#!/usr/bin/env bash
# Prove the jail actually holds. Run after jail-setup.sh.
#   sudo tools/jail-verify.sh
#
# Every check is an assertion. Exit status is non-zero if ANY egress is
# possible, so this is safe to run as a gate before launching the client.
set -uo pipefail

NS=${EVE_JAIL_NS:-eveoffline}
HOST_IP=${EVE_JAIL_HOST_IP:-10.137.0.1}
[[ $EUID -eq 0 ]] || { echo "must run as root" >&2; exit 1; }

fail=0
nsx() { ip netns exec "$NS" "$@"; }

# Can a TCP connect to $1:$2 be established within 3s?
can_connect() {
    nsx timeout 3 bash -c "exec 3<>/dev/tcp/$1/$2" 2>/dev/null
}

deny() {  # deny <label> <ip> <port>
    if can_connect "$2" "$3"; then
        echo "  FAIL  reached $1 ($2:$3) -- jail is leaking"; fail=1
    else
        echo "  ok    blocked $1 ($2:$3)"
    fi
}

# Probe targets are deliberately all on THIS machine or THIS LAN.
#
# To prove a jail is sealed you have to try to escape it, so these are real
# connection attempts from inside the namespace. That makes the choice of
# target a real decision, not a detail: a probe that leaks is a packet
# genuinely sent. So we aim every one of them at the owner's own
# infrastructure and never at a third party -- and above all never at CCP.
#
# This is also the *stronger* test. The host has ip_forward=1 (docker,
# libvirt), which means the realistic leak is the namespace being routed out
# through the host and onto the LAN. These targets probe exactly that path.
# The namespace has no default route and no LAN route, so a public address and
# a LAN address fail at the same place -- testing the LAN tests the same
# property with nothing leaving the building.
#
# An earlier version of this script probed 1.1.1.1, 8.8.8.8, example.com and
# Sentry's address. Sentry is CCP's crash-reporting endpoint, so a leak there
# was a leak to CCP. Replaced, deliberately.
#
# The host's own LAN address comes from `ip route get` on TEST-NET-1
# (192.0.2.1, RFC 5737): a route lookup, which sends nothing, about an
# address nobody owns (ruling 10).
LAN_GW=${EVE_JAIL_PROBE_GW:-$(ip -4 route show default | awk '{print $3; exit}')}
HOST_LAN_IP=${EVE_JAIL_PROBE_SELF:-$(ip -4 route get 192.0.2.1 2>/dev/null \
    | awk '{for (i = 1; i < NF; i++) if ($i == "src") { print $(i + 1); exit }}')}
PREFIX=30

# The structure first. The probes below can pass for the wrong reason -- a
# target that does not exist on this LAN fails to connect whatever the jail
# does -- so the guarantee itself is asserted directly: the host refuses to
# forward for the jail, both jail chains default to drop, and the jail has
# exactly one route, the connected /30 (V6 F2; a `0.0.0.0/1` would slip past
# a check for `default` alone).
echo "== the jail's own structure"
if nft list table inet eve_offline_jail >/dev/null 2>&1; then
    echo "  ok    host table inet eve_offline_jail (no forwarding for the jail)"
else
    echo "  FAIL  host table inet eve_offline_jail is missing: the host may forward for the jail"; fail=1
fi
for chain in output input; do
    if grep -q 'policy drop;' <<<"$(nsx nft list chain inet eve_offline_out "$chain" 2>/dev/null)"; then
        echo "  ok    jail chain $chain has policy drop"
    else
        echo "  FAIL  jail chain inet eve_offline_out $chain is missing or not policy drop"; fail=1
    fi
done
routes=$(nsx ip -4 route show 2>/dev/null | awk 'NF {print $1}')
if [[ "$routes" == "${HOST_IP%.*}.0/$PREFIX" ]]; then
    echo "  ok    the only route is ${HOST_IP%.*}.0/$PREFIX"
else
    echo "  FAIL  routes in the jail are '$(echo $routes)', expected exactly ${HOST_IP%.*}.0/$PREFIX"; fail=1
fi
if grep -q . <<<"$(nsx ip -6 route show default 2>/dev/null)"; then
    echo "  FAIL  an IPv6 default route exists"; fail=1
else
    echo "  ok    no IPv6 default route"
fi

echo "== egress must be impossible (targets are all local to this machine/LAN)"
[[ -n "$LAN_GW" ]] && deny "the LAN gateway"  "$LAN_GW"   53
[[ -n "$LAN_GW" ]] && deny "the LAN gateway"  "$LAN_GW"   80
deny "docker0 on this host"       172.17.0.1    53
deny "libvirt bridge"             192.168.122.1 53

echo "== the host's OTHER addresses must be unreachable"
# The allowlist names exactly one address. The host has many; reaching any
# other one means the allowlist is not doing what it claims.
if [[ -n "$HOST_LAN_IP" && "$HOST_LAN_IP" != "$HOST_IP" ]]; then
    deny "the host's LAN address" "$HOST_LAN_IP" 22
    deny "the host's LAN address" "$HOST_LAN_IP" 26000
fi

echo "== udp egress must be impossible too"
# The TCP checks above all fail at routing. UDP to the one address that IS
# routable, on a port that is not allowlisted, tests the nftables policy
# rather than the routing table -- a different failure mode, worth its own
# check. 123 is NTP; only 53 is permitted.
if nsx timeout 3 bash -c "exec 3<>/dev/udp/$HOST_IP/123 && printf x >&3" 2>/dev/null; then
    echo "  FAIL  sent udp to $HOST_IP:123 -- the port allowlist is not holding"; fail=1
else
    echo "  ok    blocked udp $HOST_IP:123 (only 53 is allowed)"
fi

# These resolve through our own stub on $HOST_IP:53, which has no upstream at
# all -- it answers from its table or returns NXDOMAIN. So a query here never
# leaves the machine even if it names a CCP host.
echo "== dns must not resolve anything outside our stub"
if nsx timeout 3 getent hosts sentry.io >/dev/null 2>&1; then
    echo "  FAIL  resolved sentry.io"; fail=1
else
    echo "  ok    cannot resolve sentry.io"
fi
if nsx timeout 3 getent hosts clientresources.eveonline.com >/dev/null 2>&1; then
    echo "  FAIL  resolved clientresources.eveonline.com"; fail=1
else
    echo "  ok    cannot resolve clientresources.eveonline.com"
fi

echo "== there must be no default route"
if grep -q . <<<"$(nsx ip route show default)"; then
    echo "  FAIL  default route present:"; nsx ip route show default; fail=1
else
    echo "  ok    no default route"
fi

echo "== no global IPv6 address"
if grep -q inet6 <<<"$(nsx ip -6 addr show scope global)"; then
    echo "  FAIL  global IPv6 address present"; fail=1
else
    echo "  ok    no global IPv6"
fi

# The negative DNS checks above pass for two different reasons, and only one
# of them is good: our stub answering NXDOMAIN, or resolution being broken
# entirely inside the namespace. Asserting that a name which SHOULD resolve
# does resolve is what tells the two apart -- and its absence is why the first
# client launch failed with "Could not connect to the specified address" while
# every check here reported ok.
echo "== the one name that MUST resolve, must resolve to us"
got=$(nsx timeout 3 getent hosts tranquility.servers.eveonline.com 2>/dev/null | awk '{print $1; exit}')
if [[ "$got" == "$HOST_IP" ]]; then
    echo "  ok    tranquility.servers.eveonline.com -> $got"
elif [[ -z "$got" ]]; then
    echo "  --    tranquility.servers.eveonline.com did not resolve (expected until eve-dns is running)"
else
    echo "  FAIL  tranquility.servers.eveonline.com -> $got, expected $HOST_IP"; fail=1
fi

echo "== our own server must be reachable (when it is running)"
if can_connect "$HOST_IP" 26000; then
    echo "  ok    reached $HOST_IP:26000"
else
    echo "  --    $HOST_IP:26000 refused (expected until the server exists)"
fi

# What this script cannot see: the client's FILESYSTEM view. The socket
# allowlist (ruling 4) is built inside the helper's `client` verb, in a mount
# namespace that exists only for the client's lifetime, so only the helper can
# list what is in it: `sudo -n /usr/local/sbin/eve-offline-jail verify`.
echo "== unix sockets"
echo "  --    the socket allowlist is checked by the helper's verify, not here"

echo
if [[ $fail -eq 0 ]]; then
    echo "JAIL OK - no egress path found."
else
    echo "JAIL LEAKING - do not launch the client." >&2
fi
exit $fail
