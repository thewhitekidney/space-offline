# Hosting: the world on a box with friends and bots

How to run `tools/serve.sh` on a dedicated machine (a 12-core desktop-class box) with friends
through the tunnel, you as GM from your own client, and the society bots
beside them. It is the host half of `docs/guides/playing-with-friends.md`.
Status: written from the scripts and their stub tests (`tools/test_serve_host.py`);
**no real host has run it yet**, so every step is the dry run's first draft.

## What runs

| piece | how | listens on |
|---|---|---|
| the server | `tools/serve.sh --friends --bundle-frozen` | `127.0.0.1:26000` (you, GM), `26001` (friends, player, sign-up), `26002` (your remote GM), chat `5222/5223/5224`, gateway `8443` |
| the friends' tunnel | `tools/tunnel.sh up` | one public port, 26022; forwards only 26001, 5223, 8443 |
| your remote GM | `tools/friend-tunnel.sh HOST 22 --owner` on your desk | your own sshd, not the tunnel |
| the bots | `tools/host-bots.sh run` | connect to `127.0.0.1:26000` |
| backups | `tools/host-backup.sh` hourly | `var/host-backups/` and, if set, rsync |

## 1. The content bundle: no client on the host

`serve.sh` normally checks the bundle and re-extracts it when stale, and both
need the client. On a host that has none:

```sh
rsync -a out/content/ HOST:eve-offline/out/content/     # from your dev box, your own machines only
tools/serve.sh --friends --bundle-frozen                # on HOST
```

`--bundle-frozen` skips the stale check and `content.py --check`, requires
`out/content/MANIFEST.tsv`, and prints its build and `data_hash` so you can see
what you serve. The bundle is CCP data: it goes to your own machines, never
into a kit, a repo or a friend's hands. Authored tables (`content/`) come from
the checkout as always; a changed authored file is not noticed in this mode, so
rsync again after a content change.

## 2. You as GM from another machine

`--friends` adds a third listener, `127.0.0.1:26002` (ceiling gm, cap 2, chat
`5224`), unless `--no-owner-listener`. Nothing forwards it except **your own
sshd**: `tools/tunnel.sh` keeps exactly three ports in its `PermitOpen`
(`tools/test_serve_host.py` asserts 26002 and 5224 appear nowhere in it). On
your desk, with your key in your account on the host:

```sh
tools/friend-tunnel.sh HOST 22 --owner --host-key "$(cut -d' ' -f1,2 /etc/ssh/ssh_host_ed25519_key.pub)"   # the key is the host's real sshd one, read ON the host; then the same relay as a friend
EVE_OWNER_SSH_USER=name tools/friend-tunnel.sh HOST 22 --owner             # if your account differs
tools/jail-rootless.sh --relay client
```

Your login file is the owner account; it is GM on 26002 and a player on a
friend's listener. [A] chat binds per listener as the friends' does (the
server's code says so); falsifier: the server refuses to boot with the third
listener, or chat is silent for you.

Chat channels: a session on a listener with a Gm ceiling can seize any player channel (`GrantChannelOwnership` grants owner to a GM-roled session, and bans never bind it). Friends must stay on a Player ceiling; never give them a Gm-ceiling listener.

## 3. Bots

```sh
tools/host-bots.sh accounts --miners 40 --escorts 10     # once; serve.sh STOPPED
tools/serve.sh --friends --bundle-frozen                 # tmux window 1
tools/host-bots.sh run --miners 40 --escorts 10          # tmux window 2; Ctrl-C stops the bots
```

The same counts go to both: a name with no password is refused. Release
binaries, `nice -n 10`, society and closed economy as the soak runs them, minds
in `var/bots/state`, passwords in `~/.config/eve-offline/bots.tsv`. The roster
for fifty is your choice; the defaults are 5 miners and 2 escorts.

## 4. Backups, restore, wipe

```sh
tools/host-backup.sh                       # safe while serving; EVE_BACKUP_RSYNC=user@elsewhere:dir copies it into dir/eve-host-backups/ off the disk
tools/host-backup.sh list
tools/host-backup.sh restore var/host-backups/<utc>      # server stopped; what it replaces goes to var/wiped/
tools/host-wipe.sh --yes                   # server stopped; moves the world, bot state and bot passwords to var/wiped/<utc>/
```

Hourly from cron, with the checkout's path: `0 * * * * cd ~/eve-offline && EVE_BACKUP_RSYNC=... tools/host-backup.sh`.
A backup holds password hashes, the bots' passwords and the tunnel host key
(a lost host key breaks every friend's pin), so the rsync target is yours.
After a wipe friends do nothing: their login file signs them up again.

## 5. Staying up

`serve.sh` restarts a crashed server up to 3 times in 10 minutes, then stops. If
`~/.config/eve-offline/webhook` holds a chat webhook URL (or
`EVE_WEBHOOK_FILE` names one), the stop pings it; the URL is read, never
printed. There is no start at boot: first sessions run in tmux
(`tmux new -s eve 'tools/serve.sh --friends --bundle-frozen'`); a user systemd
unit with linger is the later step.

## Login backoff behind the tunnel

Every friend reaches the server as `127.0.0.1` (the sshd container). The
per-address failure limit used to count that one address, so twenty typos from
anyone locked out everyone for up to five minutes. A loopback peer now has no
per-address key; each name still backs off after five failures. A real address
keeps its limit of twenty.
