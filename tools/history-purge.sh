#!/usr/bin/env bash
# History purge, DRY RUN tooling for owner decision O2 (docs/guides/history-purge.md).
#
#   tools/history-purge.sh SOURCE_REPO DEST_PATH [--path EXTRA]...
#
# Makes a fresh `git clone --no-local` of SOURCE into DEST, rewrites DEST's
# history so the listed paths exist in no commit, expires reflogs, gcs, then
# verifies. It NEVER touches SOURCE (read-only: clone, rev-list, ls-tree) and
# NEVER pushes: DEST's `origin` is removed before anything is rewritten.
#
# Why plain `git filter-branch --index-filter`, not git-filter-repo: filter-repo
# is a third-party tool we would have to install; filter-branch ships with git,
# and for "delete these N paths from every commit" it is adequate. It is slow
# in general (one index rewrite per commit) but ~800 commits take a minute.
#
# Commits are NOT pruned when they become empty: the commit count and every
# message must survive. Commit hashes all change (parents are rewritten).
set -euo pipefail

# Paths that held extracted client data (docs/port-tracker.md section 66).
PATHS=(
  server/crates/eve-server/src/reprocess/tables.rs
  server/crates/eve-server/src/industry/tables.rs
)

die() { echo "history-purge: $*" >&2; exit 1; }

[ $# -ge 2 ] || die "usage: $0 SOURCE_REPO DEST_PATH [--path EXTRA]..."
SRC_ARG=$1; DEST_ARG=$2; shift 2
while [ $# -gt 0 ]; do
  case $1 in
    --path) [ $# -ge 2 ] || die "--path needs a value"; PATHS+=("$2"); shift 2 ;;
    *) die "unknown argument $1" ;;
  esac
done

[ -d "$SRC_ARG/.git" ] || [ -f "$SRC_ARG/HEAD" ] || die "SOURCE is not a git repo: $SRC_ARG"
SRC=$(cd "$SRC_ARG" && pwd -P)
[ ! -e "$DEST_ARG" ] || die "DEST exists, refusing: $DEST_ARG"
DEST_PARENT=$(cd "$(dirname "$DEST_ARG")" 2>/dev/null && pwd -P) || die "DEST's parent directory does not exist"
DEST=$DEST_PARENT/$(basename "$DEST_ARG")
[ "$DEST" != "$SRC" ] || die "DEST equals SOURCE"
case "$DEST/" in "$SRC"/*) die "DEST is inside SOURCE" ;; esac
case "$SRC/" in "$DEST"/*) die "SOURCE is inside DEST" ;; esac

export FILTER_BRANCH_SQUELCH_WARNING=1
G_SRC() { git -C "$SRC" "$@"; }
G_DST() { git -C "$DEST" "$@"; }

SRC_BRANCH=$(G_SRC symbolic-ref --short HEAD) || die "SOURCE HEAD is detached"
echo "== source: $SRC (branch $SRC_BRANCH)"
echo "== dest:   $DEST"
echo "== purging: ${PATHS[*]}"

# Blob ids of every version of the purged paths, taken from SOURCE (read-only),
# so the verifier can look for them by id, not just by name.
BLOBS=$(mktemp); trap 'rm -f "$BLOBS"' EXIT
for p in "${PATHS[@]}"; do
  for c in $(G_SRC rev-list --all -- "$p"); do
    G_SRC rev-parse --verify -q "$c:$p" || true
  done
done | sort -u > "$BLOBS"
echo "== distinct blob versions to eliminate: $(wc -l < "$BLOBS")"

git clone --no-local --quiet "$SRC" "$DEST"
BEFORE_KB=$(du -sk "$DEST/.git" | cut -f1)

# Keep every remote branch as a local branch, then drop the remote entirely:
# no origin means nothing to push to, and no refs/remotes keeping old objects.
for r in $(G_DST for-each-ref --format='%(refname:strip=3)' refs/remotes/origin | grep -v '^HEAD$'); do
  G_DST rev-parse --verify -q "refs/heads/$r" >/dev/null || G_DST branch --quiet "$r" "origin/$r"
done
G_DST remote remove origin
NCOMMITS_BEFORE=$(G_DST rev-list --all --count)

# Rewrite every branch and tag. The paths are single-quoted into the filter.
QUOTED=""
for p in "${PATHS[@]}"; do QUOTED="$QUOTED '$p'"; done
G_DST filter-branch -f --index-filter \
  "git rm --cached --ignore-unmatch -q -r --$QUOTED" \
  --tag-name-filter cat -- --all >"$DEST.filter.log" 2>&1 \
  || { tail -20 "$DEST.filter.log" >&2; die "filter-branch failed (log: $DEST.filter.log)"; }
tr "\r" "\n" < "$DEST.filter.log" | tail -2

# Drop filter-branch's backup refs, expire reflogs, collect garbage.
for ref in $(G_DST for-each-ref --format='%(refname)' refs/original); do G_DST update-ref -d "$ref"; done
G_DST reflog expire --expire=now --expire-unreachable=now --all
G_DST gc --prune=now --aggressive --quiet
AFTER_KB=$(du -sk "$DEST/.git" | cut -f1)

# ---- verification -----------------------------------------------------------
FAIL=0
bad() { echo "VERIFY FAIL: $*"; FAIL=1; }

# 1. no purged path in any commit, no purged blob id reachable or present.
for p in "${PATHS[@]}"; do
  n=$(G_DST log --all --oneline -- "$p" | wc -l)
  [ "$n" -eq 0 ] || bad "$p still touched by $n commits"
done
G_DST rev-list --all --objects > "$DEST.objs"
n=$(cut -d' ' -f1 "$DEST.objs" | sort -u | comm -12 - "$BLOBS" | wc -l)
[ "$n" -eq 0 ] || bad "$n purged blob ids still reachable"
n=0
while read -r b; do G_DST cat-file -e "$b" 2>/dev/null && n=$((n+1)); done < "$BLOBS"
[ "$n" -eq 0 ] || bad "$n purged blob ids still in the object store"
rm -f "$DEST.objs"
echo "check 1: purged paths/blobs gone ........ $([ $FAIL -eq 0 ] && echo ok || echo FAILED)"

# 2. HEAD tree byte-identical to SOURCE HEAD (same tree id).
ts=$(G_SRC rev-parse "$SRC_BRANCH^{tree}"); td=$(G_DST rev-parse "$SRC_BRANCH^{tree}")
[ "$ts" = "$td" ] || bad "HEAD tree differs: source $ts dest $td"
echo "check 2: HEAD tree $td == source ........ $([ "$ts" = "$td" ] && echo ok || echo FAILED)"

# 3. commit count, and every commit's author, date, message in order.
ns=$(G_SRC rev-list --count "$SRC_BRANCH"); nd=$(G_DST rev-list --count "$SRC_BRANCH")
[ "$ns" = "$nd" ] || bad "commit count differs: source $ns dest $nd"
meta() { git -C "$1" log --reverse --format='%an <%ae>%x09%ad%x09%cn%x09%cd%x09%B%x00' --date=raw "$SRC_BRANCH" | sha256sum; }
[ "$(meta "$SRC")" = "$(meta "$DEST")" ] || bad "commit metadata or messages differ"
echo "check 3: $nd commits, messages identical . $([ "$ns" = "$nd" ] && echo ok || echo FAILED)"

# 4. every commit's tree, with the purged paths removed, matches the original's.
#    (Compares file lists with blob ids, commit by commit, in order.)
EXCL=$(printf '%s\n' "${PATHS[@]}")
lt() { git -C "$1" ls-tree -r "$2" | awk -F'\t' -v ex="$EXCL" 'BEGIN{n=split(ex,a,"\n");for(i=1;i<=n;i++)x[a[i]]=1} !($2 in x)' | sha256sum; }
mapfile -t CS < <(G_SRC rev-list --reverse "$SRC_BRANCH")
mapfile -t CD < <(G_DST rev-list --reverse "$SRC_BRANCH")
diffs=0
for i in "${!CS[@]}"; do
  [ "$(lt "$SRC" "${CS[$i]}")" = "$(lt "$DEST" "${CD[$i]}")" ] || diffs=$((diffs+1))
done
[ "$diffs" -eq 0 ] || bad "$diffs commits differ in non-purged files"
echo "check 4: other files identical in all ${#CS[@]} commits ... $([ "$diffs" -eq 0 ] && echo ok || echo FAILED)"

# 5. no remote, so a push is impossible from DEST.
[ -z "$(G_DST remote)" ] || bad "DEST still has a remote"

echo "== size .git: before $((BEFORE_KB/1024)) MiB (kB $BEFORE_KB), after $((AFTER_KB/1024)) MiB (kB $AFTER_KB)"
echo "== commits: $NCOMMITS_BEFORE reachable, all rewritten (new ids); commits whose tree changed: $(
  n=0; for i in "${!CS[@]}"; do [ "$(G_SRC rev-parse "${CS[$i]}^{tree}")" = "$(G_DST rev-parse "${CD[$i]}^{tree}")" ] || n=$((n+1)); done; echo $n)"
rm -f "$DEST.filter.log"
if [ $FAIL -eq 0 ]; then echo "PURGE DRY RUN OK: $DEST (not pushed; no remote)"; else echo "PURGE FAILED VERIFICATION"; exit 1; fi
