#!/usr/bin/env bash
# Create the network jail the modified client runs inside.
#
#   sudo tools/jail-setup.sh
#
# This is the NETWORK half of the jail. The filesystem half -- the client's
# mount namespace, an allowlist of bind-backs over empty /run, /tmp, $HOME,
# runtime dir and /dev/shm (ruling 4) -- exists only in the root helper's
# `client` verb (tools/eve-offline-jail.in, build_view). The nft tables below
# are kept identical to the helper's; tools/test_jail.py fails if they drift.
#
# Design: the client runs in its own network namespace that has NO default
# route. The only reachable address is the host end of a veth pair, and even
# that is restricted to the ports our own server listens on. Nothing else is
# routable, so there is no egress path to block in the first place -- the
# allow-list is belt and braces on top of an absence of routes.
#
# This is isolation, not evasion: the namespace cannot reach CCP's
# infrastructure at all, which is also why a client in it must never be
# pointed at a live server.
set -euo pipefail

NS=${EVE_JAIL_NS:-eveoffline}
HOST_IF=eve-host
NS_IF=eve-ns
HOST_IP=${EVE_JAIL_HOST_IP:-10.137.0.1}
NS_IP=${EVE_JAIL_NS_IP:-10.137.0.2}
PREFIX=30
# Ports on HOST_IP the jailed client may talk to.
#   26000 machoNet, 53 the stub resolver,
#   5222 in-game chat (XMPP, `eve-server --chat`, docs/lanes/chat.md).
# (8443: the gRPC gateway, answered by eve-server --gateway. UI-LAG.)
# Every one is accepted for `ip daddr $HOST_IP` only -- our own server's end
# of the veth -- so adding a port here opens nothing beyond our server.
# 5222 is allowed even when chat is off: the client dials it only when our
# server hands it an address (`XmppChatMgr.GetDeprecatedPrefsFallback`), and
# a SYN this jail dropped would be the full TCP timeout and then a modal
# retry dialog, where an open port fails or succeeds at once.
GAME_PORTS=${EVE_JAIL_PORTS:-26000,5222,8443}

[[ $EUID -eq 0 ]] || { echo "jail-setup.sh: must run as root" >&2; exit 1; }

echo "== namespace $NS"
# Compare the FIRST FIELD, not the whole line.
#
# `ip netns list` prints a bare name for a namespace with no nsid and
# `eveoffline (id: 0)` once one has been assigned -- and moving the veth in
# below with `ip link set ... netns` is exactly what assigns it. So
# `grep -qx "$NS"` matched on a fresh namespace and stopped matching after
# the first successful run, which made `ip netns add` fail with
# "Cannot create namespace file ...: File exists" and took the whole launch
# down. A guard that works until the thing it guards has been used once is
# worse than no guard, because the failure arrives on a later run with
# nothing about it changed.
if ! grep -qx "$NS" <<<"$(ip netns list | awk '{print $1}')"; then
    ip netns add "$NS"
fi

echo "== veth $HOST_IF <-> $NS_IF"
if ! ip link show "$HOST_IF" >/dev/null 2>&1; then
    ip link add "$HOST_IF" type veth peer name "$NS_IF"
    ip link set "$NS_IF" netns "$NS"
fi
ip addr replace "$HOST_IP/$PREFIX" dev "$HOST_IF"
ip link set "$HOST_IF" up
ip -n "$NS" addr replace "$NS_IP/$PREFIX" dev "$NS_IF"
ip -n "$NS" link set "$NS_IF" up
ip -n "$NS" link set lo up

# No default route is ever added inside the namespace. Remove one if something
# else put it there.
ip -n "$NS" route del default 2>/dev/null || true

echo "== resolver (stub only; our server answers, nothing else is reachable)"
mkdir -p "/etc/netns/$NS"
printf 'nameserver %s\noptions timeout:1 attempts:1\n' "$HOST_IP" > "/etc/netns/$NS/resolv.conf"

# The host very likely has ip_forward=1 (docker, libvirt). Without this the
# namespace could route out THROUGH the host. Drop it early, before docker's
# own forward rules, in both address families.
echo "== host: refuse to forward anything for the jail subnet"
nft -f - <<NFT
table inet eve_offline_jail {
    chain forward {
        type filter hook forward priority -300; policy accept;
        ip  saddr ${HOST_IP%.*}.0/$PREFIX drop
        ip  daddr ${HOST_IP%.*}.0/$PREFIX drop
        iifname "$HOST_IF" drop
        oifname "$HOST_IF" drop
    }
}
NFT

# A host firewall filters INPUT to the host's own addresses, and the veth's
# host end ($HOST_IP) is one of them. So when the jailed client sends a DNS
# query or a game-server connection to $HOST_IP, it arrives as *input to the
# host* and a default-deny firewall (ufw, firewalld) drops it before our own
# services ever see it -- the client then reports "could not connect" or fails
# to resolve, while eve-dns and eve-server sit there working perfectly.
#
# We punch exactly the holes the jail needs, on the veth interface only, and
# jail-teardown.sh removes them. Traffic on any other interface is untouched.
if systemctl is-active --quiet ufw 2>/dev/null && command -v ufw >/dev/null; then
    echo "== host firewall: ufw is active, allowing jail traffic in on $HOST_IF"
    # `in on $HOST_IF` scopes this to the veth; nothing else reaches the host
    # this way. ufw rules are idempotent (re-adding is a no-op).
    ufw allow in on "$HOST_IF" >/dev/null && echo "   ufw allow in on $HOST_IF"
elif command -v nft >/dev/null && grep -qiE 'hook input' <<<"$(nft list chains 2>/dev/null)"; then
    # A raw-nftables host firewall with an input hook. We cannot override its
    # drop from a separate chain (drop is terminal across chains), so we add a
    # high-priority input chain that accepts jail traffic BEFORE the firewall's
    # chain runs -- priority -150 is before ufw (0) and most firewalld rules.
    echo "== host firewall: nftables input hook present, accepting jail traffic early"
    nft -f - <<NFT
table inet eve_offline_hostin {
    chain input {
        type filter hook input priority -150; policy accept;
        iifname "$HOST_IF" accept
    }
}
NFT
    echo "   nft accept iifname $HOST_IF (priority -150)"
else
    echo "== host firewall: none detected on INPUT; nothing to open"
fi

echo "== jail: default-deny egress, allow only $HOST_IP ports $GAME_PORTS + dns"
#
# The trailing `counter` in each chain is the reason this jail can be
# *audited* and not merely trusted. Everything that reaches it has already
# failed every allow rule, so it is, by construction, the client trying to
# talk to something we never agreed to -- a phone-home that our patching
# missed. It is not terminal, so the chain policy still drops the packet.
#
# There is no `log` rule, and that is not an oversight. The kernel suppresses
# nf_log in every network namespace but the init one
# (net.netfilter.nf_log_all_netns=0, the default), so a `log` here would never
# write a line; an earlier version had one and promised "a sample in the
# kernel ring buffer" that could not appear (R-CENSUS §0). Turning the sysctl
# on is host-wide, and would still show only what reaches this hook.
#
# What reaches this hook is a narrow slice: the namespace has no default
# route, so a packet to any address outside 10.137.0.0/30 (and every IPv6
# packet) fails with ENETUNREACH at routing, before any netfilter hook. The
# counter sees only traffic to 10.137.0.1 on a port we did not allow. The
# census (docs/guides/census.md) is what records everything else. Read the counters
# with tools/jail-drops.sh.
RULES=$(mktemp)
trap 'rm -f "$RULES"' EXIT
cat > "$RULES" <<NFT
table inet eve_offline_out {
    chain output {
        type filter hook output priority 0; policy drop;
        oifname "lo" accept
        ip daddr $HOST_IP tcp dport { $GAME_PORTS } accept
        ip daddr $HOST_IP udp dport 53 accept
        ip daddr $HOST_IP tcp dport 53 accept
        ip daddr $HOST_IP icmp type echo-request accept
        ct state established,related accept
        counter comment "egress the client was refused"
    }
    chain input {
        type filter hook input priority 0; policy drop;
        iifname "lo" accept
        ip saddr $HOST_IP accept
        ct state established,related accept
        counter comment "ingress the client was refused"
    }
}
NFT
# Check before applying. A syntax error in a ruleset that is applied
# incrementally can leave the jail half-built -- allow rules in place, policy
# not yet set -- which is the one failure mode that fails OPEN.
ip netns exec "$NS" nft -c -f "$RULES"
ip netns exec "$NS" nft -f "$RULES"

echo
echo "jail '$NS' is up."
echo "  client sees only: $HOST_IP (tcp $GAME_PORTS, dns 53)"
echo "  no default route, no IPv6 address, forwarding refused on the host"
echo "verify with: sudo tools/jail-verify.sh"
