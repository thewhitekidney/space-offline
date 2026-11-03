#!/usr/bin/env bash
# The friend kit (IP42; ruling 7; RR1 H5): what a friend needs to play on the
# owner's server, and nothing else. Friends get this tar, never the repo
# (O2: the repo's history holds client-derived tables).
#
#   tools/friend-kit.sh                  # write out/friend-kit.tar from HEAD
#   tools/friend-kit.sh --out FILE       # somewhere else
#   tools/friend-kit.sh --list           # the allowlist, one path per line
#   tools/friend-kit.sh --vet < PATHS    # non-zero if any path is forbidden
#
# The tar is `git archive HEAD` of KIT_PATHS below, under `eve-offline-kit/`,
# so it holds committed files only: never var/, out/, ~/.config, a login
# file, a key, or anything untracked. Uncommitted edits to a kit file are
# NOT in the kit; the script says so. Every entry of the finished tar is
# vetted again against FORBIDDEN before the script reports success.
#
# The kit never holds the client. A friend needs their own full install of the
# pinned build (tools/friend-setup.sh check says whether they have it);
# tools/pin-client.sh and tools/eve_patch.py verify the copy they make.
#
# The tar also holds one generated file, KIT_VERSION (the commit and the pinned
# build), added to the staging tree after `git archive`. It is not on the allowlist, so the
# test and --list never see it; the vet below covers it like any entry.
#
# tools/test_friend_kit.py checks the allowlist: nothing forbidden, every
# path exists, and every tools/ file an allowlisted script sources or runs
# is in the kit (or named there as owner-only, with the reason).
set -euo pipefail
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# What a friend runs (docs/guides/playing-with-friends.md, "A friend"), and what
# those scripts source or run. A trailing / ships a directory.
KIT_PATHS=(
    tools/friend-setup.sh           # check / copy / patch: the one-time setup in steps
    tools/friend-tunnel.sh          # ssh -L to the owner's sshd container
    tools/jail-rootless.sh          # --relay client|check, probe
    tools/jail-relay.py             # run inside the namespace by jail-rootless.sh
    tools/jail-client.sh            # the shim jail-rootless.sh launches the client with
    tools/eve_patch.py              # plan/apply/verify; launch-env for jail-client.sh
    tools/patches/                  # the patcher's per-build manifests
    tools/builds.tsv                # the pinned build (friend-setup.sh, friend-kit.sh)
    tools/pin-client.sh             # verify the copy is the pinned build
    tools/client-pin.sha256         # pin-client.sh's manifest (hashes, no client data)
    tools/account.sh                # `login` writes the login file
    tools/login-lib.sh              # sourced by account.sh and jail-rootless.sh
    tools/prefix-lib.sh             # sourced by jail-rootless.sh and offline-prefix.sh
    tools/offline-prefix.sh         # a friend's own offline prefix (O19)
    docs/guides/playing-with-friends.md
    docs/guides/patcher.md
    docs/guides/jail.md
)

# Never in a kit, whatever the allowlist says. Matched against each path
# (and each tar entry with the kit prefix removed).
vet() {
    local p rc=0
    while IFS= read -r p; do
        [[ -n $p ]] || continue
        case "$p" in
            server/*|out/*|var/*|captures/*|findings/*|content/*|.git/*|*/.git/*) ;;
            *tables.rs|*.config/*|.config|*secrets*|*.pem|*.key|*/id_rsa*|*/id_ed25519*|id_*) ;;
            *authorized_keys*|*known_hosts*|*/login|login|*.env|*bots.tsv) ;;
            *.exe|*.dll|*.stuff|*ResFiles*|*code.ccp|*start.ini|*tq-offline*) ;;
            *) continue ;;
        esac
        echo "friend-kit: FORBIDDEN $p" >&2
        rc=1
    done
    return "$rc"
}

die() { echo "friend-kit: $*" >&2; exit 1; }

OUT=$REPO/out/friend-kit.tar
while (($#)); do
    case "$1" in
        --out) [[ $# -ge 2 ]] || die "--out needs a file"; OUT=$2; shift ;;
        --list) for p in "${KIT_PATHS[@]}"; do printf '%s\n' "$p"; done; exit 0 ;;
        --vet) vet; exit $? ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
        *) echo "friend-kit: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

printf '%s\n' "${KIT_PATHS[@]}" | vet || die "the allowlist names a forbidden path; refusing"

git -C "$REPO" rev-parse --git-dir >/dev/null 2>&1 || die "$REPO is not a git checkout (the kit is git archive HEAD)"
head=$(git -C "$REPO" rev-parse --short HEAD)
for p in "${KIT_PATHS[@]}"; do
    git -C "$REPO" cat-file -e "HEAD:${p%/}" 2>/dev/null || die "$p is not in HEAD ($head)"
done
dirty=$(git -C "$REPO" status --porcelain -- "${KIT_PATHS[@]}")
if [[ -n $dirty ]]; then
    echo "friend-kit: uncommitted changes are NOT in the kit (it is HEAD, $head):" >&2
    printf '  %s\n' "$dirty" >&2
fi

# The pinned build comes first: a kit that cannot say which build it is for is no kit.
pinned=$(awk -F'\t' '!/^#/ && NF == 5 && $3 == "pinned" { print $1; exit }' "$REPO/tools/builds.tsv")
[[ -n $pinned ]] || die "tools/builds.tsv has no pinned build; refusing"

mkdir -p -- "$(dirname -- "$OUT")"
tmp=$OUT.tmp.$$
stage=$(mktemp -d)
trap 'rm -f -- "$tmp"; rm -rf -- "$stage"' EXIT

# The tar is made from a staging tree, not by appending to git archive's: the
# pin manifest's `# install:` line names the owner's home directory and is
# dropped, KIT_VERSION is added, and every entry gets owner 0, no names, and
# the commit's time, so the same commit gives the same bytes and nothing of
# this machine (user, group, clock) is in it.
git -C "$REPO" archive --format=tar --prefix=eve-offline-kit/ HEAD -- "${KIT_PATHS[@]}" | tar -xf - -C "$stage"
pinfile=$stage/eve-offline-kit/tools/client-pin.sha256
[[ ! -f $pinfile ]] || sed -i '/^# install:/d' "$pinfile"
printf 'commit %s\nbuild  %s\n' "$(git -C "$REPO" rev-parse HEAD)" "$pinned" > "$stage/eve-offline-kit/KIT_VERSION"
ctime=$(git -C "$REPO" show -s --format=%ct HEAD)
tar -cf "$tmp" -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$ctime" eve-offline-kit

# Defence in depth: every entry is under the prefix, allowlisted, and clean.
entries=$(tar -tf "$tmp")
bad=0
while IFS= read -r e; do
    [[ $e == eve-offline-kit/* ]] || { echo "friend-kit: entry outside the kit dir: $e" >&2; bad=1; continue; }
    rel=${e#eve-offline-kit/}
    [[ -z $rel || $rel == */ ]] && continue
    ok=0
    [[ $rel == KIT_VERSION ]] && ok=1
    for p in "${KIT_PATHS[@]}"; do
        if [[ $p == */ ]]; then [[ $rel == "$p"* ]] && ok=1
        else [[ $rel == "$p" ]] && ok=1; fi
    done
    ((ok)) || { echo "friend-kit: entry not on the allowlist: $rel" >&2; bad=1; }
done <<<"$entries"
# shellcheck disable=SC2001  # one prefix per line, not one string
sed 's|^eve-offline-kit/||' <<<"$entries" | vet || bad=1
((bad == 0)) || die "the archive failed its own check; not written"
mv -f -- "$tmp" "$OUT"
trap - EXIT
rm -rf -- "$stage"

files=$(grep -vc '/$' <<<"$entries" || true)
cat <<MSG
friend-kit: wrote $OUT ($files files, HEAD $head, $(du -k "$OUT" | cut -f1) KB)
  Send it with the address, the port and your \`tools/tunnel.sh host-key\` line
  (the host-key line over a channel you trust). Never send the repo.

The client is not in the kit. A friend needs their own full install of build
$pinned (every resource file downloaded) and makes tq-offline from it with
tools/friend-setup.sh; tools/pin-client.sh checks the copy against the pin, and
tools/eve_patch.py refuses any build it has no manifest for and verifies the
copy after patching. If their launcher has moved past $pinned they cannot play.

What the friend does (docs/guides/playing-with-friends.md, in the kit):
MSG
# The friend's section of the doc, verbatim: one source for the steps.
sed -n '/^## A friend$/,/^## /{/^## /!p}' "$REPO/docs/guides/playing-with-friends.md"
