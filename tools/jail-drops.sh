#!/usr/bin/env bash
# What did the client try to reach that we refused?
#
#   tools/jail-drops.sh            # everything we know (no root needed)
#   sudo tools/jail-drops.sh       # ... plus live nftables counters
#   tools/jail-drops.sh --reset    # zero the live counters (needs root)
#
# THE PRIMARY EVIDENCE IS DNS, AND THAT IS NOT WHERE I FIRST LOOKED.
#
# The obvious place to count phone-home attempts is the jail's nftables
# output chain, and this script started there. It is close to useless on its
# own, for a reason that is a property of the jail's own strength: the
# namespace has **no default route**, so a packet to any address outside
# 10.137.0.0/30 is rejected by *routing* -- `connect()` returns ENETUNREACH
# immediately -- and never reaches the output hook at all. The counter only
# ever sees traffic aimed at the one address that IS routable, on a port we
# did not allow. That is a narrow slice, and a zero there means much less
# than it looks.
#
# The resolver sees everything instead. The client has to turn a hostname
# into an address before it can try anything, and the only resolver the jail
# has is ours. So `logs/eve-dns.log` is a complete list of every service the
# client wanted to talk to, by name, including every attempt that died at
# routing. It also outlives the jail, because it is our own file.
#
# There is no kernel log to read either. The jail's chains once had a
# rate-limited `log` rule, but nf_log is suppressed in every network
# namespace except the init one (net.netfilter.nf_log_all_netns=0), so it
# never wrote a line; it is gone, and the counters are the record (R-CENSUS
# §0). An IP literal or an IPv6 address dies at routing and leaves no trace
# here at all: only the census (tools/census.sh, docs/guides/census.md) sees those.
# While a census is up, this also prints its per-flow counters (R2).
#
# A hit is NOT a breach. Every one of these was answered NXDOMAIN and the
# packet was never sent. It is a to-do: the name says which subsystem still
# believes it has an internet connection.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
NS=${EVE_JAIL_NS:-eveoffline}
DNS_LOG=$REPO/logs/eve-dns.log

if [[ ${1:-} == --reset ]]; then
    [[ $EUID -eq 0 ]] || { echo "--reset needs root" >&2; exit 1; }
    ip netns exec "$NS" nft reset counters table inet eve_offline_out >/dev/null
    echo "live counters zeroed"
    exit 0
fi
[[ $# -eq 0 ]] || { echo "jail-drops.sh: unknown argument $1" >&2; exit 2; }

echo "== names the client tried to resolve  ($DNS_LOG)"
if [[ -s $DNS_LOG ]]; then
    # One line per distinct name, commonest first, labelled by what the
    # resolver ACTUALLY answered rather than by guessing from the name --
    # our own hostname is in the same domain as several we refuse, so
    # pattern-matching it got this wrong on the first try.
    #
    # eve-dns logs one line per query:
    #   query tranquility.servers.eveonline.com. A -> 10.137.0.1
    #   query sentry.io. A NXDOMAIN
    awk '
        $1 == "query" {
            name = $2; sub(/\.$/, "", name)
            seen[name]++
            if ($0 ~ /-> /) answered[name] = 1
        }
        END {
            for (n in seen)
                printf "%d\t%s\t%s\n", seen[n], n, (n in answered ? "resolved to us" : "REFUSED (NXDOMAIN)")
        }
    ' "$DNS_LOG" | sort -rn | while IFS=$'\t' read -r n name verdict; do
        printf '  %5d  %-45s  %s\n' "$n" "$name" "$verdict"
    done
else
    echo "   no resolver log yet; it is written by tools/up.sh"
fi

# Secondary, and live-only: the nftables counters. Worth reading when the
# jail is up because they catch anything that got as far as a routable
# address -- but see the header for why that is a narrow slice.
echo
echo "== jail counters (live only; see this script's header for the caveat)"
if ! grep -q "^$NS\b" <<<"$(ip netns list 2>/dev/null)"; then
    echo "   the jail is down, so its counters are gone with it."
    echo "   They are live-only by nature; the DNS log above is the durable record."
elif [[ $EUID -ne 0 ]]; then
    echo "   run with sudo to read them while the jail is up"
else
    counters=$(ip netns exec "$NS" nft list table inet eve_offline_out 2>/dev/null || true)
    if grep -qE 'counter packets .* (egress|ingress) the client was refused' <<<"$counters"; then
        grep -E 'counter packets .* (egress|ingress) the client was refused' <<<"$counters" \
            | sed -E 's/^[[:space:]]*/  /'
    else
        echo "   (no counter rules -- re-run sudo tools/jail-setup.sh)"
    fi
    # R2, the census table, when a census is up: one line per flow, every
    # output packet counted before the jail's drop.
    census=$(ip netns exec "$NS" nft list table inet eve_census 2>/dev/null || true)
    if [[ -n $census ]]; then
        echo
        echo "== census flows (R2, table inet eve_census; the full report is tools/census.sh's)"
        grep -E 'counter packets [0-9]+' <<<"$census" | tr ',' '\n' \
            | sed -E 's/^[[:space:]]*(elements = \{)?[[:space:]]*/  /; s/[[:space:]]*\}?[[:space:]]*$//' \
            | grep -E 'counter packets' || true
    fi
fi
