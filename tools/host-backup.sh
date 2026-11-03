#!/usr/bin/env bash
# Backups of a hosted world (F-HOST H6), safe while the server runs.
#
#   tools/host-backup.sh                 # one backup now (hourly from cron or a timer)
#   tools/host-backup.sh list            # the backups, newest first
#   tools/host-backup.sh restore DIR     # put a backup back; serve.sh must be STOPPED
#
# A backup is a dated directory var/host-backups/<utc>/ holding:
#   world.dat         the world (the saver writes by rename, so a copy is whole)
#   world.dat.ledger/ the economy ledger
#   bots/state/       the bots' minds (var/bots/state)
#   tunnel/           var/tunnel: the host key, so a friend's pin survives a lost disk
#   bots.tsv          the bots' passwords (mode 600, the dir is mode 700)
# The newest EVE_BACKUPS_KEPT (default 48) stay. tools/serve.sh's copy at every
# start (var/backups/) is a different, per-start safety net and is left alone.
#
# **Off this disk.** Set EVE_BACKUP_RSYNC=user@host:dir/ (or a local path on
# another disk) and each backup is also copied there with rsync, the whole
# var/host-backups/ tree (into eve-host-backups/ there), deleting what you pruned. The target is yours: the
# backup holds password hashes and the host key. An rsync that fails makes this
# exit 1, so cron mails you.
#
# The target is a DIRECTORY NAME OF OUR OWN: backups go to
# "$EVE_BACKUP_RSYNC/eve-host-backups/", and only that directory is ever
# synced with --delete, so a bare "user@host:" can never wipe a home directory.
#
# Env: EVE_VAR_DIR (default var/), EVE_BOT_TOKENS (default
# ~/.config/eve-offline/bots.tsv), EVE_BACKUPS_KEPT, EVE_BACKUP_RSYNC.
# Exit codes: 0 ok, 1 failed, 2 refused usage.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VAR=${EVE_VAR_DIR:-$REPO/var}
WORLD=$VAR/world.dat
OUT=$VAR/host-backups
TOKENS=${EVE_BOT_TOKENS:-$HOME/.config/eve-offline/bots.tsv}
# [A] KEPT: 48 hourly backups is two days. Falsified by the owner needing a
# world older than two days (then raise it, or rsync somewhere that keeps more).
KEPT=${EVE_BACKUPS_KEPT:-48}

die()  { echo "host-backup.sh: $*" >&2; exit 1; }
deny() { echo "host-backup.sh: $*" >&2; exit 2; }
[[ $EUID -ne 0 ]] || deny "do not run this as root; run it as yourself"
[[ $KEPT =~ ^[0-9]+$ ]] && (( KEPT >= 1 )) || deny "EVE_BACKUPS_KEPT must be a number of 1 or more"

CMD=backup
case ${1:-} in
    ''|backup) ;;
    list) CMD=list ;;
    restore) CMD=restore; [[ $# -eq 2 ]] || deny "usage: host-backup.sh restore DIR"; FROM=$2
        [[ -d $FROM && -s $FROM/world.dat ]] || deny "$FROM is not a backup (no world.dat)" ;;
    -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) deny "unknown command $1 (see --help)" ;;
esac

# Locks, held for the whole operation (a check that holds nothing races a
# server starting a moment later). serve.lock is opened with >>: a plain >
# would change the mtime serve.sh's report_world compares.
mkdir -p "$VAR"
exec 6>>"$VAR/host-backup.lock"
flock -n 6 || die "another host-backup.sh or host-wipe.sh is running"
if [[ $CMD == restore ]]; then
    exec 9>>"$VAR/serve.lock"
    flock -n 9 || die "a serve.sh holds $VAR/serve.lock: stop it first"
    exec 8>>"$WORLD.lock"
    flock -n 8 || die "a server holds $WORLD.lock: stop serve.sh first"
    exec 7>>"$VAR/bots.lock"
    flock -n 7 || die "host-bots.sh is running: stop the bots first"
fi

case $CMD in
list)
    [[ -d $OUT ]] || exit 0
    ls -1d "$OUT"/20*T*Z*/ 2>/dev/null | sed 's|/$||' | sort -r
    ;;
backup)
    [[ -s $WORLD ]] || die "no world at $WORLD; nothing to back up"
    ( umask 077; mkdir -p "$OUT" )
    chmod 700 "$OUT"
    name=$(date -u +%Y%m%dT%H%M%SZ)
    dst=$OUT/$name
    [[ -e $dst ]] && dst=$dst-$$
    tmp=$OUT/.tmp-$name-$$
    rm -rf "$tmp"
    # A failed or interrupted copy leaves nothing behind until the rename.
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    ( umask 077; mkdir -p "$tmp" )
    cp -p "$WORLD" "$tmp/world.dat"
    [[ -d $WORLD.ledger ]] && cp -a "$WORLD.ledger" "$tmp/world.dat.ledger"
    if [[ -d $VAR/bots/state ]]; then mkdir -p "$tmp/bots"; cp -a "$VAR/bots/state" "$tmp/bots/state"; fi
    [[ -d $VAR/tunnel ]] && cp -a "$VAR/tunnel" "$tmp/tunnel"
    [[ -f $TOKENS ]] && { cp -p "$TOKENS" "$tmp/bots.tsv"; chmod 600 "$tmp/bots.tsv"; }
    sync
    # A directory appears whole or not at all.
    mv "$tmp" "$dst"
    trap - EXIT INT TERM
    echo "== backup ${dst#"$REPO"/}"
    # Prune: all but the newest KEPT directories.
    ls -1d "$OUT"/20*T*Z*/ 2>/dev/null | sed 's|/$||' | sort -r | tail -n +$((KEPT + 1)) | xargs -r -d '\n' rm -rf --
    if [[ -n ${EVE_BACKUP_RSYNC:-} ]]; then
        command -v rsync >/dev/null || die "EVE_BACKUP_RSYNC is set and rsync is not installed"
        target=${EVE_BACKUP_RSYNC%/}/eve-host-backups/
        rsync -a --delete --exclude '.tmp-*' -- "$OUT/" "$target" \
            || die "rsync to $target failed; the backup is still in $OUT"
        echo "== copied off this disk to $target"
    else
        echo "   (same disk only: set EVE_BACKUP_RSYNC to keep one elsewhere)"
    fi
    ;;
restore)
    FROM=$(cd "$FROM" && pwd)
    # Stage the copy first under a temp name: a failed copy (a full disk)
    # must leave the live world where it is.
    stage=$VAR/.restore-$$
    trap 'rm -rf "$stage"' EXIT
    rm -rf "$stage"
    ( umask 077; mkdir -p "$stage/bots" )
    cp -p "$FROM/world.dat" "$stage/world.dat"
    [[ -d $FROM/world.dat.ledger ]] && cp -a "$FROM/world.dat.ledger" "$stage/world.dat.ledger"
    [[ -d $FROM/bots/state ]] && cp -a "$FROM/bots/state" "$stage/bots/state"
    sync
    # What is there now is kept, not deleted: a restore can itself be undone.
    keep=$VAR/wiped/restore-$(date -u +%Y%m%dT%H%M%SZ)
    ( umask 077; mkdir -p "$keep" )
    for f in "$WORLD" "$WORLD.ledger" "$VAR/bots/state"; do
        [[ -e $f ]] && mv "$f" "$keep/$(basename "$f")"
    done
    mv "$stage/world.dat" "$WORLD"
    [[ -e $stage/world.dat.ledger ]] && mv "$stage/world.dat.ledger" "$WORLD.ledger"
    if [[ -d $stage/bots/state ]]; then mkdir -p "$VAR/bots"; mv "$stage/bots/state" "$VAR/bots/state"; fi
    # The tunnel's host key and the bots' passwords are put back only if they
    # are gone: a restore of the world must not swap a live host key.
    [[ -d $FROM/tunnel && ! -e $VAR/tunnel ]] && cp -a "$FROM/tunnel" "$VAR/tunnel"
    [[ -f $FROM/bots.tsv && ! -e $TOKENS ]] && { mkdir -p "$(dirname "$TOKENS")"; cp -p "$FROM/bots.tsv" "$TOKENS"; }
    if [[ -f $FROM/bots.tsv && -f $TOKENS ]] && ! cmp -s "$FROM/bots.tsv" "$TOKENS"; then
        echo "host-backup.sh: WARNING: the bots' passwords in $TOKENS differ from the backup's ($FROM/bots.tsv);" >&2
        echo "   the live file was kept. Bots in the restored world may have other passwords: compare them." >&2
    fi
    echo "== restored $FROM; the world it replaced is in ${keep#"$REPO"/}"
    echo "   start tools/serve.sh again"
    ;;
esac
