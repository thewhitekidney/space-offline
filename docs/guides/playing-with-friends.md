# Playing with friends

How a friend reaches the owner's server, and how the owner runs accounts.
Accounts (ACC2), the login file (ACC2-TOOLS), the sshd container (IP22-a),
the friend's tunnel and relay (IP22-b) and the friend kit (IP42) have
landed.

**Read this first.** The jail stops the client from reaching anything but
this server. It does not protect your files from this server. The server
can run code in your client (`docs/guides/jail.md`). Play only on a server whose
owner you trust.

**Platforms (O11).** Friends run **Linux**. The friend's side needs no
sudo: an unprivileged user namespace (`tools/jail-rootless.sh probe` says
whether yours allows one), `ssh`, `python3` and Proton. Windows is not
handled by this guide (the owner decides that separately, and the
no-jail packets are not built yet); nothing here claims "no phoning home"
for a Windows client. Ubuntu 23.10 and newer restrict unprivileged user
namespaces with AppArmor; `jail-rootless.sh probe` names that case and the two
ways round it. Neither is settled advice yet: the sysctl lasts until reboot and
lowers protection for every program on the machine, and a narrower AppArmor
profile that names `unshare` only is not set up here. The owner has not
decided which to recommend; ask before changing either.

## How it fits together

- The game port is never exposed (O18). The only way in from outside is
  SSH into the owner's sshd container (`tools/tunnel.sh`). A key is needed,
  not a password, there is no shell, and the container forwards to two
  loopback ports only: the friends' listener `127.0.0.1:26001`, its chat
  `127.0.0.1:5223` and the public-gateway stub `127.0.0.1:8443`
  (`serve.sh --friends` starts it; without it the fitting window is slow).
- On the friend's machine, `tools/friend-tunnel.sh` keeps that SSH
  connection open and turns the three ports into three unix sockets in your
  runtime dir (`eve-game.sock`, `eve-chat.sock`, `eve-gw.sock`; yours alone, no TCP port
  on your machine). `tools/jail-rootless.sh --relay client` starts the
  client in a namespace with no network but `lo`, where
  `tools/jail-relay.py` answers on `10.137.0.1:26000` and `:5222` and
  passes each connection to its socket. The client can reach nothing else.

```
your client -> 10.137.0.1:26000 (relay, in the jail) -> eve-game.sock
  -> ssh -> owner's sshd container -> 127.0.0.1:26001 (friends' listener)
```
- The friends' listener has `ceiling=player` and `signup=on`. Your first
  login with a new name and password creates your account as a **player**,
  with one character of the same name (O9, O10). From then on that name
  needs that password. A GM grant does not apply on this listener: the
  ceiling caps every session there at player.
- Your client logs in with `/ssoToken=NAME:PASSWORD`, taken from your
  login file `~/.config/eve-offline/login` (mode 600). Do not type it on a
  command line. The server keeps only a salted PBKDF2 hash of the password.
  While the client runs, the password is in its command line, so `ps` on
  your own machine can see it. Use a password you use nowhere else.
- Rules: a name is 3-24 letters, digits, spaces, `-`, `_` or `'`. A
  password is 8-256 characters, with no `=` and no spaces, and is not the
  name. A wrong password shows a "wrong user name or password" dialog.
  Repeated failures back off, from 2 s doubling up to 5 min.

## The owner

The first run makes you a GM. Your own listeners are `127.0.0.1:26000` and
the jail's `10.137.0.1:26000`. Both have `ceiling=gm`, and sign-up is off
there unless you pass `--signup`.

```sh
tools/account.sh login                    # pick your name and password (asks; mode 600)
sudo -v && tools/up.sh --signup           # log in: you sign up as a player; quit the client
tools/account.sh grant 'Your Name' gm     # with the server stopped
sudo -v && tools/up.sh                    # log in again: /whoami says GM
```

`tools/serve.sh --client --signup` works for the first run too. Both
scripts use the world `var/world.dat`.

Hosting:

```sh
tools/serve.sh --client --friends         # you on the jail, friends on 127.0.0.1:26001
tools/tunnel.sh up                        # the sshd container, port 26022 (--port N)
tools/tunnel.sh add-key ann.pub ann       # one per friend: their ~/.ssh/eve-offline.pub
tools/tunnel.sh host-key                  # the line each friend pins, once
tools/tunnel.sh status                    # up or down, the keys, the listener
tools/tunnel.sh remove-key ann            # revoke one friend
tools/tunnel.sh down                      # stop it
tools/friend-kit.sh                       # out/friend-kit.tar: what a friend needs, never the repo
```

- **The router:** forward TCP **26022** (or your `--port`) to this machine
  and nothing else: never 26000, 26001, 5222 or 5223. On a dedicated server
  with a public address, open only that port in its firewall.
- **What to send a friend:** `out/friend-kit.tar`, your address, the port,
  and the `host-key` line. The kit is `git archive HEAD` of an allowlist
  (`tools/friend-kit.sh --list`): the friend's scripts, the libs they
  source, the patcher and its manifests, the client pin and three docs.
  It never holds the client, `var/`, `out/`, `server/`, a key or a login
  file, and uncommitted edits are not in it. Never send the repo (O2).
  The client is not in it either: a friend needs their own full install of
  the pinned build (every resource file downloaded) and makes the offline
  copy with `tools/friend-setup.sh`; `pin-client.sh` and the patcher check
  it. The launcher updates in place, so tell friends to hold the build until
  you say otherwise. The tar's `KIT_VERSION` says which commit it is. Send the host-key line over a channel you trust (not the same
  e-mail as the address if you can help it): it is how their ssh knows it
  is talking to you.
- **The address.** A home connection's address usually changes now and
  then. A stable name (dynamic DNS) depends on your router and ISP and is
  not set up here (plan-review L4); until it is, tell friends the current
  address when it changes.
- `tools/tunnel.sh up` builds the image the first time (Docker, nothing on
  the host), makes the container's host key once in `var/tunnel/`, and
  checks the config with `sshd -t` before it starts. It does not come back
  after a reboot: run it again. sshd runs as you, not root, with every
  capability dropped.
- A friend's key added while the tunnel is up works at their next login. A
  removed key cannot log in again, but a session that is already open
  lasts until it drops (`down` then `up` cuts it).

Admin:

```sh
tools/account.sh list                     # user id, role, name; never a hash
tools/account.sh grant NAME gm            # or player; refused while the server runs
```

A grant is refused while a server runs. A running server holds
`var/world.dat.lock` for its whole life, because it saves the world after
every turn and would overwrite an offline grant. Stop it, grant, then start
it. In game, a GM on a `ceiling=gm` listener can type `/grant NAME gm`
instead. A new role takes effect at the next login.

Bots: `sudo -v && tools/bots.sh 3` gives each bot a GM account of its own
before the server starts. Their passwords go in
`~/.config/eve-offline/bots.tsv`. Bots connect to your own listener, and on
the friends' listener they would be players.

## A friend

Once:

```sh
ssh-keygen -t ed25519 -f ~/.ssh/eve-offline       # send the owner ~/.ssh/eve-offline.pub
tar -xf friend-kit.tar && cd eve-offline-kit      # the kit from the owner; every command below runs here
tools/friend-setup.sh check                        # build, resource files, disk, Proton, the jail probe
tools/friend-setup.sh copy                         # tq-offline, and your own prefix (close EVE first)
tools/friend-setup.sh patch                        # pin check, plan, apply, verify
tools/account.sh login                             # your name and password (asks; mode 600)
```

`check` is read only. It needs your full EVE install at the pinned build
(Steam or the launcher, every resource file downloaded); if your launcher
has already moved on, it says you cannot play on that copy. `copy` adds one
directory, `tq-offline`, next to `tq` in your online prefix and then makes
your own prefix from it; your real install is never changed, and the jail
never sees the online prefix. A Steam that is not in
`~/.local/share/Steam` (the Ubuntu .deb, Flatpak) needs `STEAM_ROOT=...`, and
an EVE prefix that is not `$STEAM_ROOT/steamapps/compatdata/8500` needs
`EVE_ONLINE_PREFIX=...`. If your own prefix would land on another disk than
your resource files, `check` says so and `copy --full` makes a real copy
instead of hard links (about 60G). Each step can be run again; a half copy
from a stopped run is removed, not built on.

Each time you play, two terminals:

```sh
tools/friend-tunnel.sh HOST PORT --host-key 'ssh-ed25519 AAAA...'   # the first time only
tools/friend-tunnel.sh HOST PORT                   # after that; leave it running (Ctrl-C ends it)
tools/jail-rootless.sh --relay client              # the second terminal: the client, no sudo
```

- `HOST` and `PORT` are what the owner sent you, and the `--host-key` line
  is the one from their `tools/tunnel.sh host-key`. It is kept in
  `~/.config/eve-offline/known_hosts`; if the owner's key ever changes, ssh
  refuses to connect instead of asking. Ask the owner why before you pass a
  new `--host-key`.
- `friend-tunnel.sh` ignores your `~/.ssh/config`, never offers a password
  and never forwards your agent or X11. Your key is `~/.ssh/eve-offline`
  (`EVE_TUNNEL_KEY` picks another).
- `--relay client` refuses to start unless the tunnel's two sockets answer.
  It runs the jail's gate first (no network but `lo`, no host socket
  reachable); any LEAK line stops the launch. `--relay check` runs the gate
  and launches nothing. Logs: `out/friend/<utc>/`.
- It uses your own prefix `~/.local/share/eve-offline/prefix` and client
  `.../CCP/EVE/tq-offline`, and Proton `GE-Proton10-34` under
  `~/.local/share/Steam/compatibilitytools.d`. `EVE_PREFIX`, `EVE_CLIENT`
  and `PROTON` override them. It never uses the prefix your online EVE runs
  in.
- The login comes from your login file, as on the owner's side. It is in
  `out/client-ci/launch-env` (mode 600) only until the client is running.

Your first login signs you up. Choose the name you want to be seen by: it
is also your character's name. If you lose the password, ask the owner;
there is no reset yet.

**Not yet proven:** no client has been launched through `--relay client`
yet (the rootless jail itself runs the client in the owner's regression).
If the client never appears, send the owner `out/friend/<utc>/` (it holds
`client.log` when the client wrote one) and run once with `PROTON_LOG=1`.

If it does not connect: `tools/tunnel.sh status` on the owner's side says
whether the tunnel is up and whether `serve.sh --friends` is listening; on
yours, `friend-tunnel.sh` prints ssh's own error (a refused key says
`Permission denied (publickey)`: the owner has not added it yet).
