# Red team: what is wrong with `docs/review/plan.md`

Red-team lane RR1, 2026-10-25, at `c40dfcb`. The lane read `plan.md` in full,
plus V1-V7, `content/plan.md` §6, `lanes/README.md`, `lanes/account-spec.md`
and the code and tools named below. The only commands it ran were read-only:
`grep`, `sed`, `ls`, `find /run`, `id`, and an independent ownership script
kept in scratch. It ran no cargo and no root, and it edited nothing but this
file.

Marks: **[V]** checked here (a `file:line`, a command), **[I]** inferred,
**unknown**.

**Verdict.** The plan's engineering core is sound. It keeps the one lock, the
one crate, small seams and std-only, and its ownership arithmetic mostly
holds (§F). Its security half has three holes, and each one undoes what it
claims to deliver for friends:

- The jail it hardens is still escapable through sockets it does not mask
  (C3, C4).
- The accounts it designs make GM tokens guessable (C2).
- It never asks whether a friend can run the same client build as the
  server (C1).

None of these needs a rewrite if it is fixed now. Two of them would need one
if they are found after IP22 and IP27 land.

---

## Critical: fix before IW0b starts

### C1. Friends' client build. The plan never asks whether a friend can run build 3,619,879

**Evidence:**
- The server targets one build: `BOOT_BUILD = 3_619_879`
  (`eve-marshal/src/constants.rs:208`) [V].
- The handshake refuses only an **older** client
  (`handshake.rs:298-303`, `our_build > build`) [V]. A newer client gets
  through, and then meets:
  - a content bundle extracted from another build;
  - marshalled code objects (`GetSettingsInfo`, `signedFunc`) built for
    this build's bytecode.
- Steam updates a friend's EVE install to whatever build is current. The
  owner avoids this with a private copy (`run-client.sh:67-82`, `tq-offline`).
  A friend has no old build to copy from once CCP patches [I].
- Neither the plan, V6 nor `account-spec.md` mentions a friend's build
  [V grep "client build", "same build"].

**Cost.** This is the end goal itself. Every friend whose Steam has moved
past 3,619,879 cannot play, and a newer build fails in confusing ways rather
than at the handshake. The only alternatives are porting the server to each
new build, which is a rewrite treadmill, or the owner handing friends a
client copy privately.

**Fix:**
- New owner decision **O17**: "How does a friend get build 3,619,879?" The
  options are:
  - (a) the owner shares the `tq-offline` copy privately: never in the repo,
    and the friend kit (H5) only prints the steps;
  - (b) the server follows CCP's builds;
  - (c) friends play only while Steam's build equals ours.

  Recommend (a). Ask it alongside O9-O11, because IP22 is wasted without it.
- In IP27, make `--accounts` mode refuse `client_build != BOOT_BUILD` with
  the existing `StaleClient`-style error, renamed `WrongBuild`, carrying
  both numbers. Test `handshake::tests::a_newer_build_is_refused_in_strict_mode`.
  Size: S, and not behaviour-neutral in strict mode.

### C2. GM tokens are guessable. Bots send their character name as the token, and `--player NAME` does the same

**Evidence:**
- `eve-bot`'s `BotConfig.name` is documented as "The character to play, by
  name, **and the SSO token it sends**" (`eve-bot/src/driver.rs:128`). The
  login sends that token as `user_sso_token` (`eve-bot/src/lib.rs:1180-1237`)
  [V].
- `account-spec.md` §7 step 6, which IP27 builds, defines `--player NAME`
  as "one account with `token: NAME`" [V].
- O10 gives the bots `gm` "with their own tokens". Bot names are character
  names, and every player sees them in Local.

**Cost.** Any friend who launches with `/ssoToken=<a bot's name>` is a GM,
and so is the owner's account if it was made with `--player`. The PvP
guarantee IP21 and IP27 exist to provide is void, and the plan's tests would
not notice. `a_player_session_cannot_run_createitem` passes either way.

**Fix: two independent layers.** Both are needed, because tokens leak
(shell history, screenshots, `ps`).

1. **A role ceiling per listener.** IP8 already makes `--bind`
   repeatable. Give each listener a ceiling:
   `--bind 127.0.0.1:26001,ceiling=player`. The session role becomes
   `min(account role, listener ceiling)`, so friends arrive through a
   listener that caps them at `player`. The bots and the owner's jailed
   client use the loopback or jail listener, whose ceiling is `gm`.
   - Owns: IP8 adds the ceiling to `Config.bind`. IP27 applies it in
     `session.rs`.
   - Test: `server::tests::a_gm_token_on_a_player_listener_gets_player`.
   - Size: S on top of IP8 and IP27.
2. **Random tokens.**
   - `eve-server --new-token` prints 32 bytes from `/dev/urandom` as hex,
     plus the `sha256` line for `accounts.tsv`. This is std-only.
   - `eve-bots` reads per-bot tokens from `--tokens FILE`, which lives
     outside the repo, instead of sending the name. That is a new hunk in
     `eve-bot/src/driver.rs` and `eve-bot/src/bin/eve-bots.rs`, which no IP
     packet owns today. Give it to IP27 as a second-lane packet, **IP27b**,
     after CP39 (W15), which owns `eve-bots.rs`, or ahead of it in IW2.
   - `--player NAME` is accepted only with a listener whose ceiling is `gm`
     and whose address is loopback.
   - Test: `accounts::tests::a_token_equal_to_a_character_name_is_rejected`
     (the accounts loader refuses one).

### C3. A jailed client can reach the Docker socket, so the jail can be escaped through it

**Evidence:**
- `/var/run/docker.sock` is `srw-rw---- root docker`, and the user is in
  `docker` and `libvirt` [V `ls`, `id`].
- **The helper path is safe.** It uses `setpriv ... --clear-groups`
  (`eve-offline-jail.in:297`, `:316`; installed copy `:238`, `:257`) [V].
- **The non-helper path keeps the groups.** It runs the client with
  `sudo -u "$RUN_AS"` (`run-client.sh:335`, `:430`). `sudo` calls
  `initgroups`, so the client keeps `docker` [I, sudo's default].
- **IP29's rootless jail keeps them too** (`unshare --user`). Unprivileged
  user namespaces deny `setgroups`, and unmapped supplementary groups still
  count in permission checks. So `docker.sock` stays usable from inside the
  "lo-only" namespace [I, kernel semantics, which is the reason
  `/proc/self/setgroups` exists].
- **Reaching it is easy.** A pathname unix socket crosses network
  namespaces (V6 F1). Wine's `start /unix` runs native binaries. The server
  can run code in the client (V6 F4, `GetSettingsInfo`). So
  `start /unix /usr/bin/docker run --network host ...` is network egress,
  plus root on the host.
- The plan's O13 treats the docker group as an **agent** problem ("know
  it"). It is also a **jail** problem.

**Cost.** This breaks "no phoning home whatsoever" on two of the three
launch paths. The rootless path, IP29 and later IP35's nightly runs, is the
one the plan calls "structural" and "strictly stronger".

**Fix:**
- **IP15** (`run-client.sh`): the direct branch drops to the user with
  `setpriv --reuid --regid --clear-groups`, not `sudo -u`.
- **IP29** (`jail-rootless.sh`): before dropping to uid 1000, bind
  `/dev/null` over `/run/docker.sock` and `/run/libvirt/libvirt-sock*`.
  This is in addition to C4's allowlist, which also covers it.
- **Every path** gets an assertion inside the jail, as the client user,
  before exec:
  - `id -G` has no `docker`, `libvirt` or `sudo` group, **or**
  - `[ ! -w /run/docker.sock ]`.

  Refuse to launch otherwise.
- Test: an extra probe in `jail-verify.sh` and in IP29's check. It can be
  falsified by running it without the mask. Size: S. Add a line to O13.

### C4. IP14's socket masks are a denylist, and the list is already incomplete. The "Buy PLEX" path is not closed

**Evidence: sockets reachable from the client's mount namespace today**
[V `ls /run/user/1000`, `find /run -type s -perm -o=w`]:
- in `$XDG_RUNTIME_DIR`:
  - `openssh_agent`, which can sign with the owner's SSH keys;
  - `<chat-app>-ipc-0`;
  - `snapd-session-agent.socket` (666);
  - `vscode-*.sock`;
  - `systemd/io.systemd.Manager` (666);
  - `at-spi/`, `gnupg/`, `keyring/`;
- system-wide:
  - `/run/snapd.socket` (666);
  - `/run/cups/cups.sock`, which can print to LAN printers;
  - `/run/systemd/io.systemd.*`, `/run/systemd/resolve.hook/*`,
    `/run/libvirt/libvirt-sock-ro`;
- `/dev/shm`, which is shared;
- `~/.steam`, Steam's IPC, while Proton runs `steam.exe`, which bridges to
  the host's Steam client (`run-client.sh:146`, `STEAM_COMPAT_CLIENT_INSTALL_PATH`
  `:411-423`) [V the env; I that Steam IPC is reachable].

IP14 masks three directories, plus `bus` and `xdg-open`.

**The browser path survives masking `xdg-open`** [I]:
- Wine's `winebrowser` tries a list of browsers in order: `xdg-open`, then
  `firefox`, `konqueror` and others. When `xdg-open` is `/bin/false`, it
  falls through to `firefox`.
- That `firefox` may hand the URL to the host's running instance (X11
  remoting) [I].
- The plan's §6 line "the mount masks cover every browser path with no
  client change" is therefore unverified, and it is used to rule out
  alternatives.

**Cost.**
- Phone-home is possible through Steam or a host browser.
- A malicious or compromised server can use the owner's SSH agent.
- Each new desktop app adds a socket that nobody masks.

**Fix: turn IP14 into an allowlist.** In `v_client`, in the mount namespace,
before `setpriv`:
1. `mount -t tmpfs` over `/run/user/$UID`, then bind back only `pulse/native`,
   `pipewire-0`, `wayland-0` and the Xwayland auth file.
2. Mount `tmpfs` over each socket-bearing dir under `/run` except `/run/udev`.
   Simplest: a tmpfs over `/run`, then bind back `/run/udev` and the netns
   `resolv.conf` source.
3. A fresh `tmpfs` on `/dev/shm`.
4. `tmpfs` over `$HOME/.steam` and `$HOME/.local/share/Steam/*.pipe`, or
   unset `STEAM_COMPAT_CLIENT_INSTALL_PATH` if Proton allows it (unknown).
5. `WINEDLLOVERRIDES=winebrowser.exe=d`, the same form as the common
   `winemenubuilder.exe=d`, so no browser chain starts at all. This adds no
   client patch, and it keeps §6's "do not patch `ShellExecute`".

Mirror the same allowlist in IP15's direct branch and in IP29.

**Test.** `tools/test_jail_mounts.py` (stdlib). It parses the mount script
and asserts that the bind-back set is exactly the allowlist. Plus a
`jail-verify.sh` probe that `find /run /dev/shm -type s` inside the jail
lists only the allowlisted sockets. Falsify it by adding one.

The human check stays: "Buy PLEX" opens no browser **with Steam running**.

Size: IP14 stays M. Correct §6's line to say what was verified.

---

## High: fix before IP22 or IP27 is briefed

### H1. The friend's jail stops phone-home. It is not a sandbox, and IP33 would say it is

**Evidence:**
- The client runs as the friend's own uid, with the friend's whole `$HOME`
  and X11 (`eve-offline-jail.in:297`, `run-client.sh:15-24`, `:52-58`) [V].
- The server can run arbitrary Python in the client (V6 F4), and Wine's
  `start /unix` reaches native binaries.
- So whoever controls the owner's server can:
  - read the friend's `~/.ssh` and documents, and send them back over port
    26000, which the jail allows;
  - type into the friend's other X11 windows.
- IP33's text, "the jail bounds a compromised server", is false for
  confidentiality.

**Cost.** Friends extend the owner more trust than the docs say. One
compromise of the owner's box reaches every friend.

**Fix:**
- **IP33:** say it plainly. "The jail stops the client from reaching
  anything but this server. It does not protect your files from this
  server."
- **IP22's `docs/guides/playing-with-friends.md`:** a short consent paragraph.
- **IP14:** add `tmpfs` over `$HOME` in the client's mount namespace, and
  bind back only:
  - the Proton install;
  - the prefix;
  - `$STEAM_ROOT/steamapps/common/...`, read-only where possible.

  The helper already owns that mount namespace. Test: the mount-list
  assertion of C4. Size: S on top of C4.

### H2. The tunnel's owner end is under-specified: addresses, chat and the jail dependency

**Evidence:**
- V6's `permitopen="10.137.0.1:26000"` reaches the owner's **jail veth**
  address. That address exists only while the owner's jail is up. IP10's
  `serve.sh` starts the jail "only if a local client is wanted" [V plan
  IP10]. So a dedicated server with no local client has nothing for the
  tunnel to reach.
- Chat runs "on port 5222 of the `--bind` address"
  (`bin/eve-server.rs:67-71`) [V]. IP8 makes machoNet multi-bind but says
  nothing about chat. Chat also advertises the address the connection
  arrived on (`service/base.rs:400`). If the server binds `127.0.0.1`, a
  friend's client is told to dial `127.0.0.1:5222`, which inside the
  friend's namespace is its own loopback.
- `-o GatewayPorts=yes` on the friend side is unnecessary, and it is
  dangerous if the bind address is ever left empty.

**Fix:**
- IP8: chat listens beside every machoNet listener, or takes
  `--chat-bind`, repeatable.
- IP10: `serve.sh` binds a **friends listener** on `127.0.0.1:26001`
  (ceiling `player`, C2) and chat on `127.0.0.1:5223`, and passes
  `--chat-advertise 10.137.0.1`.
- IP22: `permitopen` targets those two ports.
- The friend side forwards `10.137.0.1:26000` and `:5222` to them, with
  `ExitOnForwardFailure=yes` and no `GatewayPorts`.
- Test: `server::tests::chat_listens_on_every_bind`. Human check: the
  owner's server with no local jail, plus one tunnelled friend, gets chat.

**Better still: re-base the friend side on IP29's rootless jail.**
- `ssh -L /run/user/$UID/eve.sock:127.0.0.1:26001` can forward a local
  unix socket.
- A ~100-line std relay inside the friend's lo-only namespace bridges it
  to `10.137.0.1:26000` (V6 F13 already sketches the relay).
- The friend then needs **no sudo, no root helper, no veth and no nft**,
  and gets the stronger, structural jail.

That also answers O15 for remote play. Size: M. It moves IP22 after IP29.

### H3. Fail-open window: the friends' path lands two waves before accounts

**Evidence:**
- IP10 (`serve.sh`, IW0b) and IP22 (the tunnel, IW1) only need O11.
- IP27 (accounts, IW2) needs O9 and O10.
- Without `--accounts`, IP27 keeps one catch-all `gm` account. Until then,
  every session is `SANDBOX_DOCKED` (`session.rs:69`) [V].

**Cost.** In the IW1-IW2 gap, every friend is a GM. The plan's own item 2
calls that "PvP is meaningless", and nothing stops the owner from using the
documented path in that window.

**Fix:**
- IP22: after **IP27**, not just O11.
- IP10: `serve.sh` refuses any `ceiling=player` or non-loopback listener
  unless `--accounts` is given.
- The catch-all account is allowed only on a loopback or jail listener.
- Check: `serve.sh --bind 127.0.0.1:26001,ceiling=player` without
  `--accounts` exits 2. Size: S.

### H4. sshd is a new internet-facing service on the owner's desktop, and it may need a host install

**Evidence:**
- V6 F5 says that whether sshd already runs is **unknown**.
- The plan adds a router port forward to it, and it says nothing about
  `sshd_config`: password auth, agent and X11 forwarding, TTY.
- The `authorized_keys` options (`restrict,port-forwarding,permitopen,command`)
  are per key. They do not stop password logins to other accounts, or
  probes against the real user.
- An `eve` system user needs `useradd` as root.

**Cost.**
- An internet-exposed login service on the machine that also holds the
  `docker` group (root-equivalent, O13).
- If `openssh-server` is absent, it breaks the "Docker only, never install
  on the host" rule.

**Fix:**
- New file `containers/tunnel.Dockerfile`: a pinned, digest-locked image
  with only `sshd`.
- A generated `sshd_config` with:
  - `PasswordAuthentication no`, `KbdInteractiveAuthentication no`;
  - `PermitTTY no`, `X11Forwarding no`, `AllowAgentForwarding no`;
  - `AllowTcpForwarding local`, `PermitOpen` set to the two friend ports;
  - `ForceCommand /bin/false`;
  - one forced user.
- It runs with `--network host` on a high port. `tools/tunnel.sh up|down|add-key`.
- Test: `tools/test_tunnel_config.py` asserts each directive. Falsify it by
  dropping one.
- Owner decision **O18**: container sshd (recommended) or host sshd.
- Size: M. IP22 then owns four files: `friend-tunnel.sh`,
  `docs/guides/playing-with-friends.md`, `tunnel.sh` and `tunnel.Dockerfile`.

### H5. Giving friends the repo gives them client-derived data

**Evidence:**
- Friends need the jail scripts, `eve-dns` and `friend-tunnel.sh`, which
  all live in this repo.
- The repo holds client-derived `reprocess/tables.rs` and
  `industry/tables.rs` at HEAD until CP32b and CP33b (W15), and in its
  history forever under O2's recommendation [V `ls`, tracker §66].

**Cost.** It breaks the "no client data in the repo" rule the moment the
first friend gets read access.

**Fix:**
- `tools/friend-kit.sh` writes `out/friend-kit.tar` with `git archive HEAD`
  of an **allowlist** of paths:
  - `tools/jail-*.sh`, `tools/run-client.sh`, `tools/friend-tunnel.sh`;
  - `server/crates/eve-dns/`, the workspace stub and `containers/build.Dockerfile`.
- `tools/test_friend_kit.py` asserts that no path in the kit matches
  `tables.rs`, `out/`, `captures/` or `findings/`.
- Amend O2's text: "rewrite history before any friend gets repo access;
  the kit avoids the need."
- Size: S. Put it in IW1 beside IP22.

### H6. Account-to-character ownership is undefined, and the obvious place for it is wrong

**Evidence:**
- O9's file is `sha256(token) user_id role`, which has no characters
  [V plan O9].
- IP27's hunk reads "the account's `character_ids`" [V plan IP27], and
  `account-spec.md` §3 keeps them in `SandboxConfig`, which is boot
  configuration.
- The client cannot create characters (`roles.rs:136`: "not implemented:
  `ccSvc.CreateCharacterWithDoll`") [V].
- So a friend's character exists only if the owner's flags create it on
  every boot. Who owns it across a `--world` restart is not stated.

**Cost.** This decision costs a rewrite if it is wrong. Ownership kept in
boot config drifts from the world file: a renamed flag orphans a friend's
assets, or hands them to someone else.

**Fix:**
- Ownership is **world state**: a persisted `owner_user_id` on the
  character, carried in `to_snapshot`.
- `accounts.tsv` holds only `sha256 user_id role name`, plus an optional
  `start_character NAME`, which is created in the world once, on the first
  login of that account.
- `character_selection_data` filters on `owner_user_id`.
- Tests:
  - `service::tests::a_restart_keeps_each_character_on_its_account`;
  - `accounts::tests::a_start_character_is_created_once`.
- Size: this moves IP27 to L. Split it into **IP27a** (resolve and the
  userid chain: `server.rs`, `session.rs`, `handshake.rs`, `accounts.rs`)
  and **IP27b** (ownership: `service/character.rs` hunk, `model.rs`, and
  `store` via the coordinator).

### H7. The one unknown that "would sink the design" is scheduled after the design

**Evidence:** `account-spec.md` §8.2: whether `/ssoToken` survives our
launcher to `autoexec_client_core` is "[unknown], and it is the one thing
that would sink the design". The plan lists it as unknown and makes it
"IP27's first check" in IW2 [V plan §7].

**Fix:**
- Put the check on IP25's **first** human card, before IW0b: launch with
  `/ssoToken=probe-123` and read `user_sso_token` from the capture. It is a
  five-minute check.
- Design the fallback now, because H2 and C2 already provide it: **identity
  by listener**. Each friend's key gets its own `permitopen` port, and a
  listener can carry `account=<user_id>`.
- `accounts::resolve(listener, token)` takes both, so the fallback is a
  config change, not a rewrite.

### H8. Windows friends have no design, and O11 asks too late

**Evidence:**
- O11 says: "if any is on Windows... nothing in the review has one yet."
- The whole jail is Linux (netns, nft, mount namespaces).

**Cost.** If most friends are on Windows, IP14, IP15, IP22 and IP29 do not
serve the end goal. That is the most likely case for a group of 2-10
gamers [I].

**Fix:**
- Ask O11 **now**, on the first card, not when IW1 starts.
- Do not brief IP22 until it is answered.
- State the honest Windows options in O11's text:
  1. a Linux VM, with the GPU passthrough problem;
  2. Windows Firewall outbound block rules for `exefile.exe`, plus a
     `hosts` entry, plus Windows' built-in `ssh -L`. This is weaker: no
     socket or browser masks, so "no phoning home whatsoever" is not
     guaranteed;
  3. no Windows friends.

  The owner chooses. Option 2 is the only one with no installs.

---

## Medium

### M1. Packets that break the size and file rules, and "C" used as a loophole

The cap is in `content/plan.md` §6.2: no packet over 1,500 changed lines or
4 files. Plan §4.1 says a verbatim move counts its moved lines.

| packet | claimed | measured | evidence |
|---|---|---|---|
| IP38 | L (< 1,000) | ~2,180 moved | clones `character.rs:2955-4343` (1,388) plus skill training `:4345-5136` (791) [V] |
| IP37 | L | ~1,420 moved | standings `:1534-2192` plus contacts `:2194-2953` [V] |
| IP36 | L | > 1,500 [I] | `hull` alone is `destiny.rs:591-1090`; `encode` plus `warp` (`:2714-3100+`) plus `push` plus their tests |
| IP28 | C, L, "moves" | 13 files, new logic | `session_hooks` is new code, not a verbatim move |

IP28's helpers are not single-caller [V grep]:
- `item_columns` has callers in `item.rs`, `service/character.rs`,
  `service/inventory.rs` and **`eve-bot/src/verbs.rs`**;
- `type_ids_of` has callers in `market.rs`, `world.rs` and `tests.rs`;
- `max_locked_targets_of` has a caller in `targeting.rs`;
- `station_assets_rowset` has callers in `character.rs` and `tests.rs`.

So IP28 must edit `item.rs`, `targeting.rs`, `service/tests.rs` and
`eve-bot/src/verbs.rs`, none of which it owns.

IP1 and IP2 are also marked C, but they add an error path, re-wire
`mission_pool`'s RNG and add a world record. Those are seams, not verbatim
moves.

**Fix:**
- Split **IP38** into IP38a (clones) and IP38b (`skill_queue`).
- Split **IP36** into IP36a (`encode` and `hull`) and IP36b (`warp` and
  `push`).
- Relabel IP37 XL.
- Split **IP28** into:
  - **IP28a**, `session_hooks`: a normal lane, S-M, owning `service.rs`
    and `session_hooks.rs`;
  - **IP28b**, a verbatim move of only the helpers that really are
    single-caller. Leave the multi-caller ones in `service.rs`.
- Define C in §4.1 as **"verbatim moves or one-line edits; anything else is
  a seam and follows W10.5's rule: behaviour-neutral, one pinning test"**.
  IP1 is then honestly "C-seam, not neutral".

### M2. In-round dependencies inside IW0b

**Evidence.** Three IW0b packets need IP8, a sibling lane in the same round
(plan §4.3) [V]:
- IP9's `conn.rs` hunk;
- IP10's use of repeatable `--bind`;
- IP11's `bin/eve-server.rs` and `server.rs` hunks.

Lanes start from one commit (lane rules, "The shape"), so IP10 cannot run
its own check.

**Fix:**
- State the landing order in §3.2: IP8, then IP9, then IP11, then IP10.
- IP10's check runs after IP8 lands, written as a post-landing check.
- Alternatively, move IP10 to IW1, where nothing in it collides.

### M3. IW1 is too big, and it changes the landing tools while 30 lanes land through them

**Evidence:**
- IW1 is 15 IP packets plus W12's 12 build lanes and 3 research lanes:
  30 lanes in one round.
- Five of the 15 are C packets the coordinator lands serially.
- IP16 rewrites `gate.sh`, `land.sh` and the pre-push hook in that same
  round.
- At rule 11's 1-4 GB per copy, that is up to about 100 GB of `server/target`.
- V5 named the coordinator as the bottleneck.

**Fix:**
- Move IP16 to IW0a. It is small and it is the coordinator's own tool.
- Move IP23-IP26 (docs, C) to IW0a or IW0b. They touch no code.
- Cap any round at about 15 lanes.

### M4. A default decode budget would hit the bots and the probe

**Evidence:** the same `Decoder` decodes **server responses** in
`eve-proto/src/call.rs:257` and `eve-probe/src/lib.rs:71` [V], so the bots
use it too. The plan's evidence for "yes for real traffic" is a
client-to-server blob only. Server responses include `GetAgents` at 10,972
rows (CA10) and the bulk data.

**Fix:**
- The budget is opt-in, `Decoder::with_budget(..)`, applied only in
  `eve-server/src/wire.rs`'s inbound path. The default stays unbudgeted.
- New test: `value::tests::a_large_server_response_decodes_with_the_default_decoder`.

### M5. A recovered panic can save a half-written world over the good one

**Evidence:** `dispatch.rs:1615-1623` recovers a poisoned lock on purpose,
because "the mutating arms validate before they mutate" [V]. That claim is
unverified. IP19 exists to find out. Meanwhile `store.rs` saves after every
changing payload. With friends' progress at stake, one bad arm persists
corruption.

**Fix:**
- IP8: when `into_inner()` recovers, set a `Registry::tainted` flag.
- IP11: the store writes `world.dat.tainted` instead of `world.dat` while
  the flag is set, and logs it loudly. `serve.sh` prints it at exit.
- Test: `store::tests::a_tainted_world_never_overwrites_the_good_file`.
- Size: S.

### M6. The connection cap leaves no headroom, and bots can lock friends out

**Evidence and cost:**
- `MAX_CONNECTIONS` is about 64 [A], and the target is 50 bots plus 10
  humans, which is 60.
- Ghost cleanup, character switches and reconnect storms push past 64.
- One shared cap lets the bot fleet starve friends.
- Bots also open XMPP connections ("Try the server's chat after login",
  `eve-bot`), and IP8 caps nothing on chat.

**Fix:**
- A cap per listener, through C2's listener config: friends at 16 [A],
  loopback at 128 [A].
- Chat gets its own cap.
- Test: `wire_hostile::a_full_bot_listener_does_not_refuse_the_friends_listener`.
  It uses caps injected through config, never the [A] values.

### M7. IP29's check connects to 1.1.1.1, which the project removed on purpose

**Evidence:** `jail-verify.sh:28-45` explains why the third-party probes
(1.1.1.1, 8.8.8.8, Sentry) were replaced with local targets: "a probe that
leaks is a packet genuinely sent" [V]. IP29's check reintroduces
`connect(1.1.1.1:443)`.

**Fix:**
- Assert `ip route get 192.0.2.1` fails. That uses RFC 5737 TEST-NET, sends
  no packet, and proves the same thing.
- Run any `connect` probe against the LAN gateway, as `jail-verify.sh`
  does.

### M8. The content plan is not amended for the splits

**Evidence:**
- CP58b (W18) owns `src/destiny.rs`. After IP36, the hull code it retires
  lives in `src/destiny/hull.rs`.
- CP59b owns `src/service/character.rs`. After IP38, skill training lives
  in `src/skill_queue.rs`.
- No CA row covers either [V plan §3.5].

**Fix:** add **CA18**. When IP36 or IP38 lands, the coordinator re-points
CP58b's and CP59b's "owns" lists and re-runs the ownership check. CP58b
probably drops to `src/destiny/hull.rs`, `src/item.rs` and `src/npc.rs`.

### M9. IP11's backups are keyed by git sha, which is the wrong axis

**Evidence and cost:**
- Git shas are not ordered, so "newer" and "downgrade" cannot be read from
  them.
- Every dev commit is a "build change", so `BACKUPS_KEPT` churns through
  backups during a day of landings.
- `option_env!("EVE_GIT_SHA")` forces an `eve-server` recompile per commit.

**Fix:**
- Key the backup and the downgrade check on a **store schema version**: a
  `STORE_FORMAT` const plus a hash of `delta_kinds::KINDS` from IP2.
- Keep the sha in the header for humans only.
- `serve.sh` also copies `world.dat` to `var/backups/<utc>.dat` at each
  start, keeping N [A]. That is the backup friends' progress needs whether
  or not the build changed.

### M10. IP21 gates in the wrong place

**Evidence:** `Registry::slash(&mut self, actor: Option<i64>, line)` has no
session or role (`slash.rs:104-108`) [V]. Gating "in `Registry::slash`"
means changing its signature, which breaks about 10 call sites in
`service/inventory.rs`, `service/space.rs` and `service/tests.rs` [V grep].
IP21 owns none of those files.

**Fix:**
- Gate only in the `gm.rs` `slash` arm, which has the request's session.
- Add `gm::tests::public_qa_tools_stays_a_no_op_for_players`, which pins
  that `publicQaToolsServer.SlashCmd` and `MoveMeTo` still answer `None`
  (`gm.rs:116-119`).

---

## Low

- **L1. The ownership script is not committed.** It was re-created here, and
  checked independently [V, scratch `own.py`, deleted]:
  - 0 duplicate paths inside any IP wave;
  - 0 beside-wave owner clashes;
  - one registration clash, CP25's `DOMAINS` row in `service.rs` against
    IP28 in IW2. The plan acknowledges it;
  - C packets over 4 files: IP1 (5), IP17 (7), IP24 (14), IP28 (13), IP36 (5).

  The plan's PASS is reproducible. But the script misses hunks written
  without `src/` (IP12's "`server.rs`"), and it does not model C packets
  landing mid-wave. **Fix:** commit it as `tools/test_plan_owners.py`
  inside IP31, normalise bare file names, and fail on a C packet with
  non-verbatim edits (M1).
- **L2. IP16's stamp is forgeable.** Anyone can create `out/landed/<sha>`.
  Make it hold the tree hash plus the gate log's digest, and have the hook
  check `git rev-parse <sha>^{tree}`.
- **L3. IP5 edits the auto-memory**, which is outside the repo, as a repo
  packet. The coordinator should just do it. It is not a lane-owned file.
- **L4. Hosting gaps:**
  - `serve.sh` has no restart-on-crash loop. Add a bounded one: 3 restarts
    in 10 min, then stop.
  - Friends need a stable name or IP for the owner's home connection
    (dynamic DNS, unknown).
- **L5. IP17's count.** "The same 39 test names": there are 40 `#[test]`,
  one of them `#[ignore]` (`mining_bots.rs:595`) [V]. Say 39 plus 1
  ignored, so a lost ignored test is noticed.

---

## The owner's rules: where the plan bends them

| rule | where it bends | finding |
|---|---|---|
| no phoning home whatsoever | the Docker socket, the unmasked sockets, Steam IPC, `winebrowser`'s fallback chain | C3, C4 |
| client jailed | a jail with the friend's whole `$HOME`, presented as bounding the server | H1 |
| no client data in the repo | the repo goes to friends | H5 |
| Docker only | host sshd for the tunnel | H4 |
| never an [A] in a test | IP8's cap test opens `MAX_CONNECTIONS + 1` streams: say "injected by config", as IP9 does | M6 |
| lanes own files | IP28 and IP21 edit files they do not own | M1, M10 |
| std-only | holds: SHA-256, the relay and the token minting are all std | — |

## What the end goal needs that the plan lacks

These are for co-op and PvP with 2-10 friends plus about 50 bots:
- the friends' client build (C1);
- Windows friends (H8);
- character provisioning and ownership (H6);
- a friend kit (H5);
- bot tokens (C2);
- per-listener roles and caps (C2, M6);
- regular backups and a restart loop (M9, L4).

## New owner decisions proposed

- **O17:** how friends get build 3,619,879 (C1).
- **O18:** sshd in a container (recommended) or on the host (H4).
- **Move O11 to the first card, ahead of IW0b** (H8).
- **Add to O13:** the docker group is also a jail escape for the client
  (C3).

## unknown

- Whether Proton's `steam.exe` reaches a running host Steam client from
  inside the jail (C4).
- Whether `WINEDLLOVERRIDES=winebrowser.exe=d` stops the client's URL
  hand-off under Proton (C4).
- Whether `sudo -u` keeps `docker` on the direct branch. Expected yes. It
  is a one-line check inside the jail: `id -G`.
- Whether friends' Steam is still on build 3,619,879 today (C1).
