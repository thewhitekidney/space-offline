#!/usr/bin/env bash
# Wipe a hosted world for a new start (F-HOST; the 11-09 wipe, owner decision
# P0d item 2). Nothing is deleted: it is moved, so a wipe can be undone.
#
#   tools/host-wipe.sh --yes            # move the world away
#   tools/host-wipe.sh                  # say what it would move
#
# Moves, into var/wiped/<utc>/: var/world.dat and everything beside it
# (world.dat.*, the ledger, a tainted or downgrade copy, format backups; not
# the lock), var/bots/state, and the bots' passwords (bots.tsv: their accounts
# go with the world, so new ones are made). It refuses while a server holds the
# world's lock. var/backups, var/host-backups and var/tunnel stay.
#
# Afterwards (printed too): friends do nothing, their login file signs them up
# again on first login with the same name and password; you sign up first and
# grant yourself gm; then bot accounts and the world settings.
#
# Env: EVE_VAR_DIR, EVE_BOT_TOKENS. Exit codes: 0 ok, 1 failed, 2 refused usage.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VAR=${EVE_VAR_DIR:-$REPO/var}
WORLD=$VAR/world.dat
TOKENS=${EVE_BOT_TOKENS:-$HOME/.config/eve-offline/bots.tsv}
die()  { echo "host-wipe.sh: $*" >&2; exit 1; }
deny() { echo "host-wipe.sh: $*" >&2; exit 2; }
[[ $EUID -ne 0 ]] || deny "do not run this as root; run it as yourself"

YES=0
case ${1:-} in
    '') ;;
    --yes) YES=1; [[ $# -eq 1 ]] || deny "unexpected argument $2" ;;
    -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) deny "unknown argument $1 (see --help)" ;;
esac

# Held for the whole wipe, so a server cannot start under it (see
# host-backup.sh). serve.lock is opened with >> to keep its mtime.
mkdir -p "$VAR"
exec 6>>"$VAR/host-backup.lock"
flock -n 6 || die "a host-backup.sh is running"
exec 9>>"$VAR/serve.lock"
flock -n 9 || die "a serve.sh holds $VAR/serve.lock: stop it first"
exec 8>>"$WORLD.lock"
flock -n 8 || die "a server holds $WORLD.lock: stop serve.sh first"
exec 7>>"$VAR/bots.lock"
flock -n 7 || die "host-bots.sh is running: stop the bots first"

things=()
for f in "$WORLD" "$WORLD".*; do
    [[ -e $f && $f != "$WORLD.lock" ]] && things+=("$f")
done
[[ -e $VAR/bots/state ]] && things+=("$VAR/bots/state")
[[ -e $TOKENS ]] && things+=("$TOKENS")
(( ${#things[@]} )) || { echo "nothing to wipe"; exit 0; }

if (( ! YES )); then
    echo "would move to $VAR/wiped/<utc>/:"
    printf '   %s\n' "${things[@]}"
    echo "run again with --yes"
    exit 0
fi

dst=$VAR/wiped/$(date -u +%Y%m%dT%H%M%SZ)
[[ -e $dst ]] && dst=$dst-$$
( umask 077; mkdir -p "$dst" )
for f in "${things[@]}"; do
    # The bots' state dir and the world's own files keep their names; the
    # tokens file is named for what it is.
    mv -- "$f" "$dst/$(basename "$f")"
done
echo "== wiped: moved ${#things[@]} item(s) to ${dst#"$REPO"/}"
cat <<MSG
next:
  1. tools/serve.sh --friends --bundle-frozen --signup   (your login signs up on 26000 or, from far away, on 26002)
  2. quit; server/target/release/eve-offline-server --world var/world.dat --grant 'YourName' gm   (server stopped; the release
     binary serve.sh built: account.sh would build a debug one)
  3. tools/host-bots.sh accounts, then serve.sh again, then tools/host-bots.sh run
  4. world settings as GM in game (training speed, ISK, SP)
  Friends do nothing: their next login signs them up again with the same name and password.
MSG
