#!/usr/bin/env bash
# Delete the cargo build output of lane copies nobody is using.
#
#   tools/lanes-gc.sh              # delete idle lanes' target/ dirs
#   tools/lanes-gc.sh --dry-run    # only list what would go, with sizes
#   EVE_LANES_IDLE_MIN=120 tools/lanes-gc.sh   # stricter idle test
#
# WHY THIS EXISTS.
#
# Every lane copy at ~/code/eve-offline-lanes/<lane>/ builds its own
# server/target, 8-11 GB once tests and clippy have run. Lanes rule 9 says a
# lane deletes its copy before it reports, but a lane that is stopped, times
# out or crashes never reaches that step. On 2026-10-29 the sweep found 71
# abandoned target dirs, about 350 GB, and the root disk at 99%: the host's
# SSD then stalls on every write and the whole desktop freezes.
#
# A lane is idle when no process has its cwd inside the lane, no running
# container mounts it, and nothing in its target/ changed in the last
# EVE_LANES_IDLE_MIN minutes (default 60). Only target/ dirs are deleted;
# sources, notes and handbacks stay. Deletion runs at idle IO priority.
set -euo pipefail

LANES=${EVE_LANES_DIR:-$HOME/code/eve-offline-lanes}
IDLE_MIN=${EVE_LANES_IDLE_MIN:-60}
DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1

busy=$(
    {
        for p in /proc/[0-9]*; do readlink "$p/cwd" 2>/dev/null || true; done
        docker ps -q 2>/dev/null | xargs -r docker inspect \
            --format '{{range .Mounts}}{{.Source}}{{"\n"}}{{end}}' 2>/dev/null || true
    } | grep -o "^$LANES/[^/]*" | sort -u
)

freed=0
while IFS= read -r t; do
    lane=$(cut -d/ -f1 <<<"${t#"$LANES"/}")
    if grep -qxF "$LANES/$lane" <<<"$busy"; then
        echo "busy    $lane"
        continue
    fi
    if [ -n "$(find "$t" -mmin "-$IDLE_MIN" -print -quit 2>/dev/null)" ]; then
        echo "recent  $lane"
        continue
    fi
    kb=$(du -sk "$t" 2>/dev/null | cut -f1)
    freed=$((freed + kb))
    if [ "$DRY" = 1 ]; then
        printf 'would   %s (%d MB)\n' "${t#"$LANES"/}" $((kb / 1024))
    else
        ionice -c3 nice -n19 rm -rf -- "$t"
        printf 'deleted %s (%d MB)\n' "${t#"$LANES"/}" $((kb / 1024))
    fi
done < <(find "$LANES" -mindepth 2 -maxdepth 3 -type d -name target \
    \( -path "*/server/target" -o -path "$LANES/*/target" \) -prune 2>/dev/null)

printf '%s %d GB\n' "$([ "$DRY" = 1 ] && echo would-free || echo freed)" $((freed / 1048576))
