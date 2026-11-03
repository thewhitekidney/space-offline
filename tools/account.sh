#!/usr/bin/env bash
# Accounts in the world file (ACC2, owner decisions O9 and O10).
#
#   tools/account.sh login [--second]     # write your login file (asks; mode 600)
#   tools/account.sh whoami [--second]    # the user name in it (never the password)
#   tools/account.sh list                 # the world's accounts: id, name, role
#   tools/account.sh grant NAME gm        # make NAME a GM (or: player), offline
#   tools/account.sh bots NAME...         # give bots gm accounts (tools/bots.sh does this)
#
# Options: --world FILE (default var/world.dat, the world tools/serve.sh and
# tools/up.sh run), --no-build.
#
# **grant needs the server stopped.** A running eve-server holds FILE.lock
# for its whole life, and grant takes the same lock: the server keeps the
# world in memory and saves it after every turn, so a grant written under it
# would be overwritten by its next save. Stop the server (Ctrl-C), grant,
# start it again; a new role takes effect at the next login. While it runs, a
# GM can do the same in game: /grant NAME gm.
#
# The first run (the owner), once:
#
#   tools/account.sh login                # pick a user name and a password
#   sudo -v && tools/up.sh --signup       # log in: the new name signs up as a player
#   (quit the client; up.sh stops)
#   tools/account.sh grant 'Your Name' gm
#   sudo -v && tools/up.sh                # log in again: GM (/whoami answers)
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SRV=${EVE_SERVER_BIN:-eve-offline-server}   # the server binary; EVE_SERVER_BIN=eve-server for the plain core one
# shellcheck source=tools/login-lib.sh
source "$REPO/tools/login-lib.sh"
WORLD=$REPO/var/world.dat
BUILD=1 SECOND=''
die()  { echo "account.sh: $*" >&2; exit 1; }
deny() { echo "account.sh: $*" >&2; exit 2; }
usage() { sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

[[ $# -gt 0 ]] || { usage >&2; exit 2; }
CMD=$1; shift
ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --world)    [[ $# -ge 2 ]] || deny "--world needs a FILE"; WORLD=$2; shift ;;
        --no-build) BUILD=0 ;;
        --second)   SECOND=second ;;
        -h|--help)  usage; exit 0 ;;
        *)          ARGS+=("$1") ;;
    esac
    shift
done
[[ $EUID -ne 0 ]] || deny "run this as yourself, not as root"
[[ $WORLD == /* ]] || WORLD=$PWD/$WORLD

# The host binary, built in the container like every other tool here.
server_bin() {
    local dir=$REPO/server/target/debug
    if (( BUILD )); then
        "$REPO/tools/dev.sh" cargo build --locked -p "$SRV" -p eve-bot --bins >&2
    fi
    [[ -x $dir/$SRV ]] || die "no $dir/$SRV; run without --no-build"
    printf '%s\n' "$dir"
}

# Refuse, with the reason, while a server holds the world's lock.
check_unlocked() {
    local lock=$WORLD.lock
    [[ -e $lock ]] || return 0
    if ! flock -n "$lock" true 2>/dev/null; then
        cat >&2 <<MSG
account.sh: an eve-server is running with $WORLD (it holds $lock).
  A running server keeps the world in memory and saves it after every turn,
  so a grant written now would be lost at its next save; that is why the
  server holds the lock for its whole life. Stop it (Ctrl-C in serve.sh or
  up.sh), run this again, then start it. Or, as a GM in game: /grant NAME ROLE
MSG
        exit 1
    fi
}

case $CMD in
    login)
        file=$(eve_login_path "$HOME" "$SECOND")
        eve_login_prompt "$file"
        ;;
    whoami)
        file=$(eve_login_path "$HOME" "$SECOND")
        eve_login_token "$file" "$(id -u)" >/dev/null || exit 1
        echo "$(eve_login_user "$file")   ($file)"
        ;;
    list)
        [[ -f $WORLD ]] || die "no world file $WORLD yet"
        printf 'user_id\trole\tname\n'
        awk -F'\t' '$1 == "delta" && $2 == "account" && $3 == 1 { print $4 "\t" $6 "\t" $5 }' "$WORLD" | sort -n
        ;;
    grant)
        [[ ${#ARGS[@]} -eq 2 ]] || deny "usage: tools/account.sh grant NAME player|gm"
        [[ ${ARGS[1]} == gm || ${ARGS[1]} == player ]] || deny "role ${ARGS[1]}: player or gm"
        [[ -f $WORLD ]] || die "no world file $WORLD yet: start the server once and sign up first"
        check_unlocked
        bin=$(server_bin)
        "$bin/$SRV" --world "$WORLD" --grant "${ARGS[0]}" "${ARGS[1]}"
        echo "   takes effect at ${ARGS[0]}'s next login, on a listener whose ceiling allows it"
        echo "   (the friends' listener is ceiling=player: a GM is a player there)."
        ;;
    bots)
        [[ ${#ARGS[@]} -gt 0 ]] || deny "usage: tools/account.sh bots NAME..."
        check_unlocked
        content=$REPO/out/content
        [[ -s $content/MANIFEST.tsv ]] \
            || die "no content bundle $content: run 'sudo -v && tools/up.sh --no-client' once (Ctrl-C when the stack is up), then this"
        bin=$(server_bin)
        bash "$REPO/tools/bot-accounts.sh" --bin "$bin" --world "$WORLD" \
            --tokens "${EVE_BOT_TOKENS:-$HOME/.config/eve-offline/bots.tsv}" \
            --logs "$REPO/logs" "${ARGS[@]}" -- \
            --content "$content" --authored "$REPO/content"
        ;;
    -h|--help|help) usage ;;
    *) deny "unknown command $CMD (see --help)" ;;
esac
