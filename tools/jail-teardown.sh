#!/usr/bin/env bash
# Remove the jail. Safe to run repeatedly.
#   sudo tools/jail-teardown.sh
set -euo pipefail
NS=${EVE_JAIL_NS:-eveoffline}
[[ $EUID -eq 0 ]] || { echo "must run as root" >&2; exit 1; }
ip netns del "$NS" 2>/dev/null && echo "removed namespace $NS" || echo "no namespace $NS"
ip link del eve-host 2>/dev/null && echo "removed veth eve-host" || true
nft delete table inet eve_offline_jail 2>/dev/null && echo "removed host nft table" || true
nft delete table inet eve_offline_hostin 2>/dev/null && echo "removed host input-allow table" || true
HOST_IF=${EVE_JAIL_HOST_IF:-eve-host}
if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q "$HOST_IF"; then
    ufw delete allow in on "$HOST_IF" >/dev/null 2>&1 && echo "removed ufw allow for $HOST_IF" || true
fi
rm -rf "/etc/netns/$NS"
echo "done."
