#!/usr/bin/env bash
# Where am I? (AGENTS.md step 0). Read-only; always exits 0.
# Prints one key=value per line and ends with next=<step>.
cd "$(dirname "$0")/.." 2>/dev/null || true
py() { python3 -I tools/claims.py "$@" 2>/dev/null; }

if docker info >/dev/null 2>&1; then docker=yes; else docker=no; fi
image=no
if [ "$docker" = yes ] && docker image inspect eve-offline-build >/dev/null 2>&1; then image=yes; fi
echo "docker=$docker"
echo "image=$image"

client=unknown
if grep -q -- '--where-pinned' tools/new-build.sh 2>/dev/null; then
  if tools/new-build.sh --where-pinned >/dev/null 2>&1; then client=yes; else client=no; fi
fi
echo "client=$client"
jail=unknown
if grep -q -- '--installed' tools/install-jail-helper.sh 2>/dev/null; then
  if tools/install-jail-helper.sh --installed >/dev/null 2>&1; then jail=yes; else jail=no; fi
fi
echo "jail=$jail"
if [ -d out/content ]; then echo "bundle=yes"; else echo "bundle=no"; fi

branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo none)
echo "branch=${branch:-none}"
if [ -n "$(git status --porcelain 2>/dev/null)" ]; then dirty=yes; else dirty=no; fi
echo "dirty=$dirty"
ahead=$(git rev-list --count origin/master..HEAD 2>/dev/null || echo unknown)
echo "ahead=$ahead"

who=$(python3 -I tools/claims.py whoami 2>"${TMPDIR:-/tmp}/agent-status.$$"); rc=$?
err=$(cat "${TMPDIR:-/tmp}/agent-status.$$" 2>/dev/null); rm -f "${TMPDIR:-/tmp}/agent-status.$$"
case $rc in
  0) tracker=ok ;;
  *) case $err in
       *config*) tracker=no-config ;;
       *) if [ -z "${EVE_TRACKER_TOKEN:-}${EVE_TRACKER_TOKEN_FILE:-}" ]; then tracker=no-token; else tracker=unreachable; fi ;;
     esac ;;
esac
echo "tracker=$tracker"
[ -n "$who" ] && echo "login=$who"

claim=none
n=$(printf '%s\n' "$branch" | sed -n 's#^[^/]*/\([0-9][0-9]*\)-.*#\1#p')
if [ -n "$n" ] && [ "$tracker" = ok ] && py show "$n" --holder-is-me >/dev/null; then claim=$n; fi
echo "claim=$claim"
pr=none
if [ "$tracker" != unreachable ]; then pr=$(py pr-for-branch) || pr=none; fi
echo "pr=${pr:-none}"

if [ "$docker" = no ] || [ "$image" = no ]; then next=step1
elif [ "$claim" != none ]; then next=step4
elif [ "$dirty" = yes ]; then next=stop-dirty
else next=step2; fi
echo "next=$next"
exit 0
