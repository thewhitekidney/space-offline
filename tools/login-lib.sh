# shellcheck shell=bash
# The player's login file: where a world-accounts login (ACC2, O9) keeps its
# user name and password, so neither is ever typed on a command line.
#
#   ~/.config/eve-offline/login          the first client (EVE_LOGIN_FILE)
#   ~/.config/eve-offline/login-second   run-client.sh --second
#
# Sourced by tools/run-client.sh, tools/up.sh, tools/serve.sh and
# tools/account.sh; defines functions only. The file is two lines:
#
#   user=Some Name
#   password=a-long-password
#
# mode 600, owned by you, outside every git work tree. The client gets it as
# `/ssoToken=USER:PASSWORD` (docs/lanes/acc2.md, "Which login form"), which
# is why a user name cannot hold `:` and a password cannot hold `=` (the
# client splits its own argument on `=`).
#
# **Hygiene.** The token travels: this file -> a shell variable ->
# out/client-ci/launch-env (mode 600, the token line removed again once the
# client has started) -> jail-client.sh -> the client's own argv. The last is
# unavoidable (the client reads it there) and is visible to `ps` on this
# machine while the client runs. It is never an argument of any other process
# and never printed: errors name the file, not its contents.

# The rules, mirrored from the server's sign-up (account_book.rs NAME_LEN,
# MIN_PASSWORD_LEN, MAX_PASSWORD_LEN, accounts.rs TOOL_TOKENS) so a typo is
# caught before a two-minute boot. The server stays the authority.
EVE_LOGIN_NAME_RE="^[A-Za-z0-9 _'-]{3,24}$"
EVE_LOGIN_TOOL_TOKENS=" offline dev token a-token probe-123 "

# eve_login_path HOME [second]: the login file for that home.
eve_login_path() {
    local home=$1 which=${2:-}
    if [[ -n ${EVE_LOGIN_FILE:-} ]]; then
        printf '%s\n' "$EVE_LOGIN_FILE"
    elif [[ $which == second ]]; then
        printf '%s\n' "$home/.config/eve-offline/login-second"
    else
        printf '%s\n' "$home/.config/eve-offline/login"
    fi
}

# eve_login_check USER PASSWORD: why the pair would be refused, or nothing.
eve_login_check() {
    local user=$1 pw=$2
    [[ $user =~ $EVE_LOGIN_NAME_RE ]] \
        || { echo "the user name must be 3-24 letters, digits, spaces or - _ '"; return 1; }
    [[ $user == "${user# }" && $user == "${user% }" && $user != *"  "* ]] \
        || { echo "the user name must not start or end with a space, or hold two in a row"; return 1; }
    (( ${#pw} >= 8 && ${#pw} <= 256 )) \
        || { echo "the password must be 8-256 characters"; return 1; }
    [[ $pw != *=* ]] || { echo "the password cannot contain '=' (the client splits its argument on it)"; return 1; }
    [[ $pw != *[[:space:]]* ]] || { echo "the password cannot contain spaces"; return 1; }
    [[ ${pw,,} != "${user,,}" ]] || { echo "the password cannot be the user name"; return 1; }
    [[ $EVE_LOGIN_TOOL_TOKENS != *" ${pw,,} "* ]] || { echo "that password is one of the tools' fixed tokens"; return 1; }
    return 0
}

# eve_login_token FILE UID: print USER:PASSWORD from FILE, or say on stderr
# why not and return 1. UID is who must own it.
eve_login_token() {
    local file=$1 uid=$2 mode owner key value user='' pw='' d
    [[ -f $file ]] || { echo "no login file $file" >&2; return 1; }
    owner=$(stat -L -c %u -- "$file") mode=$(stat -L -c %a -- "$file")
    [[ $owner == "$uid" ]] || { echo "$file is not yours (owner uid $owner)" >&2; return 1; }
    (( (8#$mode & 8#077) == 0 )) || { echo "$file is mode $mode; others may read it: chmod 600 '$file'" >&2; return 1; }
    d=$(cd "$(dirname -- "$file")" && pwd -P)
    while [[ -n $d && $d != / ]]; do
        [[ ! -e $d/.git ]] || { echo "$file is inside the git work tree $d; keep it outside every repo" >&2; return 1; }
        d=$(dirname -- "$d")
    done
    while IFS='=' read -r key value || [[ -n $key ]]; do
        value=${value%$'\r'}
        case $key in
            user)     user=$value ;;
            password) pw=$value ;;
        esac
    done < "$file"
    local why
    why=$(eve_login_check "$user" "$pw") || { echo "$file: $why (tools/account.sh login rewrites it)" >&2; return 1; }
    printf '%s:%s\n' "$user" "$pw"
}

# eve_login_user FILE: the user name only (safe to print).
eve_login_user() {
    sed -n 's/^user=//p' -- "$1" 2>/dev/null | head -n 1 | tr -d '\r'
}

# eve_login_prompt FILE: ask for a user name and a password on the terminal
# and write FILE (mode 600, its directory 700). Needs a terminal; run as the
# file's owner, never as root.
eve_login_prompt() {
    local file=$1 user pw pw2 why dir tmp
    [[ -t 0 ]] || { echo "no terminal to ask on; run tools/account.sh login first" >&2; return 1; }
    (( EUID != 0 )) || { echo "not as root: run tools/account.sh login as yourself" >&2; return 1; }
    echo "== login for the eve-offline server (saved in $file, mode 600)"
    echo "   A new name signs up as a player on a listener with sign-up on;"
    echo "   a known name needs its password. The password is never shown."
    while :; do
        read -r -p "   user name: " user || return 1
        read -r -s -p "   password:  " pw || return 1; echo
        read -r -s -p "   again:     " pw2 || return 1; echo
        if [[ $pw != "$pw2" ]]; then echo "   the two passwords differ; again"; continue; fi
        why=$(eve_login_check "$user" "$pw") && break
        echo "   $why; again"
    done
    dir=$(dirname -- "$file")
    ( umask 077 && mkdir -p -- "$dir" ) || return 1
    tmp=$(umask 077 && mktemp "$dir/.login.XXXXXX") || return 1
    printf '# eve-offline login (tools/account.sh login). Mode 600; never in a repo.\nuser=%s\npassword=%s\n' \
        "$user" "$pw" >"$tmp" && chmod 600 "$tmp" && mv -f -- "$tmp" "$file" \
        || { rm -f -- "$tmp"; return 1; }
    echo "   saved: $file (user $user)"
}
