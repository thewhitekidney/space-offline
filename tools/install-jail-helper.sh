#!/usr/bin/env bash
# Install (or reinstall) the root-owned jail helper.
#
#   sudo tools/install-jail-helper.sh            # install / update
#   tools/install-jail-helper.sh --diff          # show what would change, no root
#   tools/install-jail-helper.sh --check         # is the installed copy current? no root
#   tools/install-jail-helper.sh --print         # the generated helper, to stdout
#   sudo tools/install-jail-helper.sh --uninstall
#
# `--check` exits 0 when the installed copy is exactly what this checkout
# would install, 3 when it is stale (generated from an older template) or
# foreign (generated for another checkout: its REPO= line names a different
# tree, so its `client` verb would launch THAT tree's shim and launch-env,
# V4 C2), and 4 when nothing is installed. It needs no root: the installed
# file is world-readable. `EVE_JAIL_HELPER` points `--check` and `--diff` at
# another file (tests use a fake); it never moves what an install writes.
#
# **Read the generated file before you install it.** `--diff` prints exactly
# what will land in /usr/local/sbin, and that file is the thing your sudoers
# entry lets run as root without a password. Reviewing it is the step the
# whole design rests on -- `docs/reference/unattended-root.md` §4 property 1 is "root
# never executes anything the invoking user can write", and the moment you
# stop reading this diff, that property is only as good as whoever last
# edited tools/eve-offline-jail.in.
#
# The helper is a SNAPSHOT of tools/jail-setup.sh and tools/jail-teardown.sh,
# not a wrapper around them. The two are kept in step by hand, deliberately:
# a jail change that does not reach the helper is a jail change you have not
# reviewed.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SRC=$REPO/tools/eve-offline-jail.in
DST=/usr/local/sbin/eve-offline-jail
# What --check and --diff compare against. Never where an install writes.
HELPER=${EVE_JAIL_HELPER:-$DST}
: "${EVE_JAIL_NS:=eveoffline}"
: "${EVE_JAIL_HOST_IP:=10.137.0.1}"
: "${EVE_JAIL_NS_IP:=10.137.0.2}"

[[ -f $SRC ]] || { echo "install-jail-helper.sh: no $SRC" >&2; exit 2; }

# generate [REPO]: the helper as installed for REPO (default: this checkout).
generate() {
    sed -e "s|@REPO@|${1:-$REPO}|g" \
        -e "s|@NS@|$EVE_JAIL_NS|g" \
        -e "s|@HOST_IP@|$EVE_JAIL_HOST_IP|g" \
        -e "s|@NS_IP@|$EVE_JAIL_NS_IP|g" \
        "$SRC"
}

# The REPO= line of an installed helper (empty if there is none).
installed_repo() { sed -n "s/^REPO='\\(.*\\)'\$/\\1/p" "$1" | head -n 1; }

fix_lines() {
    echo "  fix it (the helper runs as root, so read the diff first):"
    echo "      $REPO/tools/install-jail-helper.sh --diff | less"
    echo "      sudo $REPO/tools/install-jail-helper.sh"
}

case "${1:-}" in
--check)
    if [[ ! -e $HELPER ]]; then
        echo "install-jail-helper.sh: no helper at $HELPER (not installed)"
        fix_lines
        exit 4
    fi
    [[ -r $HELPER ]] || { echo "install-jail-helper.sh: $HELPER is not readable; treating it as stale"; fix_lines; exit 3; }
    rc=0
    there=$(installed_repo "$HELPER")
    if [[ $there != "$REPO" ]]; then
        echo "install-jail-helper.sh: FOREIGN: $HELPER serves the checkout '${there:-<none>}', not this one ($REPO)."
        echo "  Its client verb would launch that tree's shim with that tree's launch-env."
        echo "  Run from '${there:-<none>}', or reinstall from here, which moves the helper to this checkout."
        rc=3
    fi
    # Stale is judged against the checkout the helper names, so a foreign
    # helper that is otherwise current says only "foreign".
    if ! generate "${there:-$REPO}" | cmp -s - "$HELPER"; then
        echo "install-jail-helper.sh: STALE: $HELPER differs from what tools/eve-offline-jail.in generates."
        rc=3
    fi
    if (( rc )); then fix_lines; exit "$rc"; fi
    echo "install-jail-helper.sh: $HELPER is current for $REPO"
    exit 0
    ;;
--print)
    generate
    exit 0
    ;;
--diff)
    tmp=$(mktemp); trap 'rm -f "$tmp"' EXIT
    generate > "$tmp"
    if [[ -f $HELPER ]]; then
        diff -u "$HELPER" "$tmp" && echo "install-jail-helper.sh: installed copy is up to date"
    else
        echo "== $HELPER does not exist yet; it would be:"
        cat "$tmp"
    fi
    exit 0
    ;;
--uninstall)
    [[ $EUID -eq 0 ]] || { echo "install-jail-helper.sh: --uninstall needs root" >&2; exit 1; }
    rm -f "$DST" && echo "removed $DST"
    echo "the sudoers entry at /etc/sudoers.d/eve-offline is left alone; it is"
    echo "inert without this file, and removing it is your call."
    exit 0
    ;;
"") ;;
*)  echo "install-jail-helper.sh: unknown argument $1" >&2; exit 2 ;;
esac

[[ $EUID -eq 0 ]] || { echo "install-jail-helper.sh: must run as root (sudo tools/install-jail-helper.sh)" >&2; exit 1; }

# The helper refuses to run from an untrusted path, so fail here rather than
# installing something that will refuse itself later.
[[ -d /usr/local/sbin ]] || { echo "no /usr/local/sbin" >&2; exit 1; }
owner=$(stat -c '%U' /usr/local/sbin); mode=$(stat -c '%a' /usr/local/sbin)
[[ $owner == root ]] || { echo "/usr/local/sbin is owned by $owner, not root" >&2; exit 1; }
(( (0$mode & 022) == 0 )) || { echo "/usr/local/sbin is mode $mode (writable by non-root)" >&2; exit 1; }

tmp=$(mktemp); trap 'rm -f "$tmp"' EXIT
generate > "$tmp"
bash -n "$tmp" || { echo "generated helper does not parse; not installing" >&2; exit 1; }
install -o root -g root -m 0755 "$tmp" "$DST"
echo "installed $DST"
echo "   repo snapshot: $REPO"
echo "   namespace:     $EVE_JAIL_NS   ($EVE_JAIL_HOST_IP <-> $EVE_JAIL_NS_IP)"
echo
echo "check it with:"
echo "   sudo -n $DST status"
echo "   sudo -n ip netns exec $EVE_JAIL_NS /bin/sh   # MUST still ask for a password"
