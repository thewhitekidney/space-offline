#!/usr/bin/env bash
# Gate, then push -- and push ONLY if the gate actually passed.
#
# This exists because red reached `origin/master` twice, and both times the
# command looked right. `docs/port-tracker.md` §48.5 was
#
#     tools/gate.sh ; git push          # `;` runs the push on failure
#
# and the recorded fix was "use && not ;". That fix was obeyed, and §54.6
# happened anyway:
#
#     tools/gate.sh 2>&1 | tail -4 && git push
#
# which uses `&&` and still always pushes, because **a pipeline's exit status
# is its last command's** and the last command is `tail`. The gate had exited
# 101.
#
# The lesson §54.6 drew is that a fix naming a mechanism does not cover the
# class: the real rule is "the thing you branch on must be the thing you care
# about". A script is the version of that rule which does not depend on
# anybody remembering it at 2am.
#
# So: the gate's output goes to a FILE, never through a pipe, and the branch is
# on `$?` alone.
#
#   tools/land.sh                  # gate, then push the current branch
#   tools/land.sh --no-push        # gate only, keep the log
#   tools/land.sh --remote NAME    # default: origin
#   tools/land.sh --head           # gate a clean checkout of HEAD, push that
#
# **`--head` is how the coordinator lands while lanes are mid-edit.** The bare
# gate refuses a dirty tree, and during a lane round the tree is always dirty
# with somebody's half-written file, so pushes used to wait for every lane to
# report (`docs/lanes/README.md` rule 8). `--head` checks HEAD out into a
# throwaway `git worktree`, runs the *bare* gate there -- a real clean tree,
# not `--dirty`, which is scoped by design and authorises nothing -- and
# pushes exactly that commit. What is gated is what is pushed, by sha.
#
# **The stamp** (review V4 G1, RR1 L2). On a pass, `land.sh` writes
# `out/landed/<sha>` (`tree <hash>` and `log-sha256 <hex>`) next to the gate
# log `out/landed/<sha>.log`. `tools/hooks/pre-push` accepts a pushed sha
# without re-gating only if its stamp's tree is `git rev-parse <sha>^{tree}`
# and the log still hashes to the stamp and holds `== gate passed`; anything
# else, it runs the gate. Without this the installed hook would gate every
# `--head` landing a second time, in the dirty main tree, and fail it. The
# stamp is not a signature: it stops an accident and a stale file, not a
# person who writes all three lines by hand.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

remote=origin
push=1
head=0
while (($#)); do
    case "$1" in
        --no-push) push=0 ;;
        --remote) remote=$2; shift ;;
        --head) head=1 ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
        *) echo "land: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

log=$(mktemp -t eve-land-XXXXXX.log)

# **`set -e` must not fire here.** The whole point is to inspect the gate's
# status rather than die on it, so the call is guarded and the status captured
# before anything else can overwrite `$?`.
status=0
# Read before the gate starts: the bare gate refuses a dirty tree, so this
# commit's tree is the one it checks, and a commit made meanwhile is not.
sha=$(git rev-parse HEAD)
if ((head)); then
    # **On disk, never in /tmp**: /tmp is RAM on the dev box, and a gate's
    # build tree there held ~7 GB of it until the box ran out (2026-10-29).
    # The build cache is kept between gates (one target dir, EVE_TARGET_DIR), so
    # a gate compiles what changed instead of the whole workspace.
    land_dir=${EVE_LAND_DIR:-$HOME/code/eve-offline-lanes/land}
    mkdir -p "$land_dir/target"
    wt=$(mktemp -d -p "$land_dir" wt-XXXXXX)
    rmdir "$wt"
    git worktree add -q --detach "$wt" "$sha"
    export EVE_TARGET_DIR="$land_dir/target"
    # The mount point (an empty, ignored dir) the cache is mounted over.
    mkdir -p "$wt/server/target"
    # Removed on every exit path: a stale worktree is registered in `.git` and
    # would refuse the next `worktree add` of the same path.
    trap 'git worktree remove --force "$wt" 2>/dev/null || true' EXIT
    (cd "$wt" && tools/gate.sh) >"$log" 2>&1 || status=$?
else
    tools/gate.sh >"$log" 2>&1 || status=$?
fi

if ((status != 0)); then
    echo "land: GATE FAILED (exit $status) -- nothing pushed" >&2
    echo "land: last 20 lines of $log" >&2
    tail -20 "$log" >&2
    echo "land: full log kept at $log" >&2
    exit "$status"
fi

# A pass is asserted positively, not inferred from a zero exit: a gate that
# silently stopped running its test step would also exit 0.
if ! grep -q '^== gate passed$' "$log"; then
    echo "land: gate exited 0 but never printed its pass line -- nothing pushed" >&2
    echo "land: log kept at $log" >&2
    exit 1
fi

tests=$(grep -oE 'test result: ok\. [0-9]+ passed' "$log" \
        | grep -oE '[0-9]+' | awk '{s+=$1} END {print s+0}' || true)
echo "land: gate passed, $tests tests"

# Stamp the gated commit (see the header). The stamp is written last and by
# rename, so a half-written one never exists. The newest STAMPS_KEPT stay;
# older shas are long pushed.
STAMPS_KEPT=50
mkdir -p out/landed
mv "$log" "out/landed/$sha.log"
read -r digest _ < <(sha256sum "out/landed/$sha.log")
printf 'tree %s\nlog-sha256 %s\n' "$(git rev-parse "$sha^{tree}")" "$digest" >"out/landed/$sha.tmp"
mv "out/landed/$sha.tmp" "out/landed/$sha"
echo "land: stamped out/landed/$sha"
mapfile -t old < <(ls -1t out/landed/*.log 2>/dev/null | tail -n +$((STAMPS_KEPT + 1)))
for f in "${old[@]}"; do rm -f "$f" "${f%.log}"; done

if ((push == 0)); then
    echo "land: --no-push, stopping here"
    exit 0
fi

branch=$(git rev-parse --abbrev-ref HEAD)
if ((head)); then
    echo "land: pushing $sha (gated in a clean worktree) to $remote/$branch"
    git push "$remote" "$sha:refs/heads/$branch"
else
    echo "land: pushing $branch to $remote"
    git push "$remote" "$branch"
fi
echo "land: pushed"

# **The maintainer's after-push step.** A maintainer's checkout may hold an
# executable `private/post-land.sh` (never exported); it gets the pushed sha and
# the remote. A contributor's checkout has none, so nothing runs. Its failure is
# loud and exits 3: the push above already happened, the follow-up did not.
hook=private/post-land.sh
if [[ -x $hook && -z ${EVE_NO_POST_LAND:-} ]]; then
    echo "land: running $hook"
    hstatus=0
    "$hook" "$sha" "$remote" || hstatus=$?
    if ((hstatus != 0)); then
        echo "land: $hook FAILED (exit $hstatus) -- $sha is pushed to $remote, the follow-up is not done" >&2
        exit 3
    fi
fi
