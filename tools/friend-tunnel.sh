#!/usr/bin/env bash
# A friend's end of the tunnel (IP22-b, ruling 6; RR1 H2). Runs on the
# FRIEND's machine, as the friend, no sudo.
#
#   tools/friend-tunnel.sh HOST PORT --host-key 'ssh-ed25519 AAAA...'   # the first time
#   tools/friend-tunnel.sh HOST PORT                                    # after that
#   tools/friend-tunnel.sh HOST PORT --dry-run                          # print the ssh line
#   tools/friend-tunnel.sh HOST 22 --owner [--host-key ...]            # YOU, as GM, from another machine
#
# Keeps `ssh -N` to the owner's sshd container (tools/tunnel.sh) in the
# foreground until Ctrl-C, forwarding three UNIX sockets in your runtime dir
# (mode 700, yours alone) to the owner's three loopback ports:
#
#   $XDG_RUNTIME_DIR/eve-game.sock  ->  127.0.0.1:26001  (the friends' listener)
#   $XDG_RUNTIME_DIR/eve-chat.sock  ->  127.0.0.1:5223   (its chat)
#   $XDG_RUNTIME_DIR/eve-gw.sock    ->  127.0.0.1:8443   (the public gateway: without
#                                                         it the fitting window is slow)
#
# Then, in another terminal: tools/jail-rootless.sh --relay client.
#
# No TCP port is opened on your machine, so nothing on your LAN (or another
# local user) can use the tunnel; never GatewayPorts. `-F none` ignores your
# ~/.ssh/config, so nothing there can widen this (agent or X11 forwarding,
# a ProxyCommand, GatewayPorts). ExitOnForwardFailure: if either socket
# cannot be made, ssh exits instead of half-working.
#
# The owner's host key is pinned in ~/.config/eve-offline/known_hosts the
# first time (--host-key, the line `tools/tunnel.sh host-key` prints on the
# owner's machine; with --owner, your own sshd's key: the first two fields of
# /etc/ssh/ssh_host_ed25519_key.pub on the host); after that a different key is refused, never asked
# about. Your key: ~/.ssh/eve-offline (EVE_TUNNEL_KEY overrides).
#
# **--owner** (F-HOST H3) is for the owner on a remote client. It does not go
# through tools/tunnel.sh: it logs in to the host's OWN sshd as your own account
# there (EVE_OWNER_SSH_USER, default the name you have here), with your own key,
# and maps the same three sockets to the owner listener of `tools/serve.sh
# --friends`: game 127.0.0.1:26002 (ceiling gm), chat 127.0.0.1:5224, and the
# same gateway 8443. A friend's key cannot reach 26002 (tools/tunnel.sh
# PermitOpen is the three friend ports and nothing else).
#
# Exit codes: ssh's own; 2 refused usage.
set -euo pipefail

CONF=${XDG_CONFIG_HOME:-$HOME/.config}/eve-offline
KNOWN=$CONF/known_hosts
KEY=${EVE_TUNNEL_KEY:-$HOME/.ssh/eve-offline}
USER_NAME=eve
GAME=127.0.0.1:26001
CHAT=127.0.0.1:5223
GW=127.0.0.1:8443
OWNER=0

deny() { echo "friend-tunnel.sh: $*" >&2; exit 2; }

[[ $EUID -ne 0 ]] || deny "do not run this as root; run it as yourself"
[[ $# -ge 2 ]] || deny "usage: friend-tunnel.sh HOST PORT [--host-key 'TYPE KEY'] [--dry-run]"
HOST=$1 PORT=$2; shift 2
HOSTKEY='' DRY=0
while (( $# )); do
    case $1 in
        --host-key) [[ $# -ge 2 ]] || deny "--host-key needs the host key line"
                    HOSTKEY=$2; shift ;;
        --dry-run)  DRY=1 ;;
        --owner)    OWNER=1 ;;
        *) deny "unknown argument: $1" ;;
    esac
    shift
done
if (( OWNER )); then
    USER_NAME=${EVE_OWNER_SSH_USER:-$(id -un)}
    GAME=127.0.0.1:26002
    CHAT=127.0.0.1:5224
fi
[[ $USER_NAME =~ ^[a-z_][a-z0-9_-]*$ ]] || deny "EVE_OWNER_SSH_USER is not a plain account name"
[[ $HOST =~ ^[A-Za-z0-9.:-]+$ && $HOST != -* ]] || deny "HOST must be a host name or address"
[[ $PORT =~ ^[0-9]+$ ]] && (( PORT >= 1 && PORT <= 65535 )) || deny "PORT must be a number"

RUNTIME=${XDG_RUNTIME_DIR:-}
[[ -n $RUNTIME && -d $RUNTIME && -O $RUNTIME ]] \
    || deny "XDG_RUNTIME_DIR is unset or not yours; log in to a desktop session (it is /run/user/$(id -u) there)"
[[ $(stat -c %a "$RUNTIME") == 700 ]] || deny "$RUNTIME is not mode 700; refusing to put the sockets there"
GAME_SOCK=$RUNTIME/eve-game.sock
CHAT_SOCK=$RUNTIME/eve-chat.sock
GW_SOCK=$RUNTIME/eve-gw.sock

# The name ssh looks up in known_hosts for a non-22 port.
if (( PORT == 22 )); then KH_NAME=$HOST; else KH_NAME="[$HOST]:$PORT"; fi

if [[ -n $HOSTKEY ]]; then
    [[ $HOSTKEY =~ ^(ssh-ed25519|ecdsa-sha2-nistp(256|384|521)|ssh-rsa)\ [A-Za-z0-9+/]+={0,2}$ ]] \
        || deny "--host-key must be exactly 'TYPE BASE64', as tools/tunnel.sh host-key prints it"
fi

if (( ! DRY )); then
    [[ -r $KEY ]] || deny "no key at $KEY: ssh-keygen -t ed25519 -f ~/.ssh/eve-offline, and send the owner the .pub"
    mkdir -p "$CONF" && chmod 700 "$CONF"
    if [[ -n $HOSTKEY ]]; then
        touch "$KNOWN" && chmod 600 "$KNOWN"
        # One pinned key per host:port; a new --host-key replaces the old.
        grep -vF "$KH_NAME " "$KNOWN" > "$KNOWN.tmp" || true
        printf '%s %s\n' "$KH_NAME" "$HOSTKEY" >> "$KNOWN.tmp"
        chmod 600 "$KNOWN.tmp" && mv "$KNOWN.tmp" "$KNOWN"
        echo "== pinned the owner's host key for $KH_NAME in $KNOWN"
    fi
    grep -qF "$KH_NAME " "$KNOWN" 2>/dev/null \
        || deny "no pinned host key for $KH_NAME: pass it once with --host-key (a friend: the line from tools/tunnel.sh host-key; the owner: the host's real sshd key, the first two fields of /etc/ssh/ssh_host_ed25519_key.pub)"
    for s in "$GAME_SOCK" "$CHAT_SOCK" "$GW_SOCK"; do
        if [[ -e $s && ! -S $s ]]; then deny "$s exists and is not a socket; move it away"; fi
    done
fi

cmd=(ssh -F none -N -T
     -o ExitOnForwardFailure=yes
     -o GatewayPorts=no
     -o StreamLocalBindUnlink=yes
     -o StreamLocalBindMask=0177
     -o ClearAllForwardings=no
     -o ForwardAgent=no -o ForwardX11=no -o IdentityAgent=none
     -o IdentitiesOnly=yes -o IdentityFile="$KEY"
     -o PasswordAuthentication=no -o KbdInteractiveAuthentication=no
     -o PreferredAuthentications=publickey
     -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$KNOWN" -o GlobalKnownHostsFile=/dev/null
     -o UpdateHostKeys=no -o CheckHostIP=no
     -o ServerAliveInterval=30 -o ServerAliveCountMax=3
     -o ConnectTimeout=20
     -L "$GAME_SOCK:$GAME"
     -L "$CHAT_SOCK:$CHAT"
     -L "$GW_SOCK:$GW"
     -p "$PORT" "$USER_NAME@$HOST")

if (( DRY )); then
    printf '%q ' "${cmd[@]}"; echo
    exit 0
fi

cleanup() { rm -f "$GAME_SOCK" "$CHAT_SOCK" "$GW_SOCK"; }
trap cleanup EXIT
echo "== tunnel to $HOST:$PORT: $GAME_SOCK, $CHAT_SOCK and $GW_SOCK (Ctrl-C to stop)"
echo "   now, in another terminal: tools/jail-rootless.sh --relay client"
"${cmd[@]}"
