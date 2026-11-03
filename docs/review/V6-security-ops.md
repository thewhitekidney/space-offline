# V6: security, the jail and operations

Review lane V6, 2026-10-25, at `3f2741d`. This was a read-only review. Nothing
was run that needs root, and the jail was not brought up. The only commands
run were read-only probes as the user: `ss -ltnu`, `id`, `grep` of
`/etc/nsswitch.conf`, `ls` of two sockets, `rustc -V` in the build image, and
one `unshare --user --map-current-user --net true`.

Marks: **[V]** verified (from code, or a command above), **[I]** inferred,
**unknown**.

## 0. Verdict in five lines

1. The jail's network layers are sound and well tested. There is no route,
   the default is deny, and forwarding is refused [V]. Two gaps are not about
   routing:
   - **pathname unix sockets cross the namespace**, which leaves a browser
     path and a DNS path out of the jail (F1);
   - **the unattended path verifies less, and logs nothing** (F2, F3).
2. `docs/guides/jail.md` says the server cannot run code in the client. That is
   false: `GetSettingsInfo` and the handshake `signedFunc` are code pushes,
   and we use them (F4). With `cryptoPack = Placebo` this is a plaintext
   remote-code channel. So **friends must never reach the server over a raw
   internet path.**
3. **There is no supported way for a friend to play today.** There is no
   remote path and no account layer. Every session gets the GM role mask.
   `up.sh` keeps no world file. The stack tears down when the first client
   exits (F5-F8). This is the largest cost to the end goal.
4. Supply chain is strong where it matters most: zero crates, an offline
   distro registry, and a hardened `dev.sh`. The images and the gate check
   have a few loose ends (F11, F12).
5. Context for V4: the user is in the `docker` and `libvirt` groups [V `id`].
   Both are **root-equivalent**. The sudoers helper's narrowness is real, but
   it does not bound what an agent running as `youruser` can do (F13).

## 1. Findings, ranked by cost to the end goal

Each finding gives its evidence, its cost, the change and its size.

### F5. No path for friends, and the code-push channel is plaintext. Friends cannot play.

**Evidence:**
- `eve-server` binds exactly one address, `127.0.0.1:26000` by default
  (`bin/eve-server.rs:5-10`, `server.rs:173`) [V]. `up.sh:402` binds
  `10.137.0.1` [V].
- A friend's client:
  - needs `tranquility.servers.eveonline.com` to resolve to us;
  - is otherwise un-jailed, so Sentry, LaunchDarkly and others are reached
    (`findings/outbound-endpoints.txt`);
  - speaks plaintext machoNet under Placebo, and executes the marshalled
    code objects we send (F4).

  An on-path attacker could therefore replace `signedFunc` or the
  `GetSettingsInfo` blob, and get arbitrary Python in the friend's client
  [I, which follows from `pycode.rs:173-190` and `placebo.verify`].
- There is no remote-player doc anywhere [V grep].

**Change: the friend's client runs in the same jail, and its two allowed
ports reach the owner over an SSH tunnel.**
- On the friend's host:
  `ssh -N -o GatewayPorts=yes -L 10.137.0.1:26000:10.137.0.1:26000 -L 10.137.0.1:5222:10.137.0.1:5222 eve@owner`.
- The owner adds one `authorized_keys` line per friend:
  `restrict,port-forwarding,permitopen="10.137.0.1:26000",permitopen="10.137.0.1:5222",command="/bin/false" ssh-ed25519 ...`.

What this gives:
- encryption and authentication with no new dependency and nothing
  installed (sshd is configuration, not an install; whether it already runs
  is **unknown**);
- the jail is unchanged: the friend's namespace still reaches only
  `10.137.0.1`;
- the owner's server is never on a public interface.

Chat already advertises "the address the connection arrived on"
(`--chat-advertise`). Over the tunnel that is `10.137.0.1` on both ends, so
it works unchanged [I].

**Ships as:**
- `tools/friend-tunnel.sh`: the friend side. It takes the owner's host and
  key, and checks that the jail is up first.
- `docs/guides/playing-with-friends.md`: the owner side, the `authorized_keys`
  line and the router port.
- [A] Friends run Linux with Proton and can run the repo's jail. Whether
  they do is **unknown**, and it is an owner question.

**Size and test:** M, tools and docs only, behaviour-neutral for the
server. The test is a human check: two hosts, one tunnel, both log in.

### F6. Every session is a GM, and the server gates no GM path. This costs PvP.

**Evidence:**
- `session.rs:69` gives every session `roles::SANDBOX_DOCKED`. That
  includes `ROLE_GML`, `ROLE_GMH` and `ROLE_ADMIN` (`roles.rs:113-125`) [V].
- `Registry::slash` says "There is no role check to perform"
  (`slash.rs:99-101`) [V]. `service/gm.rs:93` says "these commands are not
  role-gated at all" [V].
- So in PvP any friend can `/createitem`, `/giveisk`, `/setstanding` or
  `/tp`.

**Change:**
- An account carries a role: `player` maps to `ROLE_LOGIN|ROLE_PLAYER`, and
  `gm` maps to `SANDBOX_DOCKED`.
- `Registry::slash`, and the `gm.rs` dispatch arm, refuse unless the
  actor's session role has `ROLE_GML`. They return a `SlashError` string,
  not a panic.
- The default single-account dev mode keeps `gm`, so this is
  behaviour-neutral until accounts exist.

**Size and test:** S once F7 lands, M with it. The test is
`a_player_session_cannot_run_createitem`, and the same test with a `gm`
session succeeds.

### F7. There is no account layer: `/ssoToken` is parsed and ignored. This costs PvP.

**Evidence:** `handshake.rs:404` stores `user_sso_token`, and nothing reads
it [V]. `docs/lanes/account-spec.md` is written and parked
(`docs/guides/human-checks.md` §3).

**Change:**
- Build the spec.
- Add, from a security point of view:
  - an accounts file **outside the repo**, for example
    `~/.config/eve-offline/accounts.tsv` with `token user_id role`, passed
    by `--accounts FILE`;
  - store `sha256(token)`, not the token. The hash needs a std-only
    SHA-256, about 100 lines in `eve-server`. The repo already has none, so
    this is S;
  - with `--accounts` set, an unknown token is **refused** in the
    handshake, with no catch-all;
  - `capture.rs` redacts `user_sso_token` before writing. Captures are
    kept, and `captures/regression/` is committed.

**Size and test:** M. It is not behaviour-neutral. The spec §6 names the
two-connection test.

### F8. `up.sh` loses the world and is tied to one client. Friends cannot keep playing.

**Evidence:**
- `up.sh:402` starts `eve-server` without `--world`, and the default is
  "in memory only" (`bin/eve-server.rs:189`) [V]. Every `up.sh` session is
  a fresh universe. Only `soak.sh:72` passes `--world`.
- The stack is torn down "when the first client exits" (`up.sh:30`,
  `cleanup`). That is right for a debug session and wrong for hosting.
- Logs are truncated per run with `>` (`up.sh:383`, `:387`, `:404`).
- `--capture` is always on (`up.sh:402`). That is one file per connection,
  unbounded, and with 50 bots it is many files per hour. There are 275
  files today [V].

**Change:** a new `tools/serve.sh`. It is the hosting entry point; `up.sh`
stays the debug entry point. It:
- builds;
- starts the jail only if a local client is wanted;
- starts `eve-server --world var/world.dat --content out/content`, with
  `--bind` repeatable (see below);
- writes logs to `logs/<utc-stamp>-eve-server.log` and keeps the last 20;
- leaves captures off unless `--capture` is given;
- runs until Ctrl-C.

`var/` goes in `.gitignore`.

Repeatable `--bind` is a small std change in `bin/eve-server.rs` and
`server.rs`: one listener thread per address. It is how the owner's own
jailed client and the tunnel endpoint coexist.

Stop needs no new code. `store.rs:27-35` writes with tmp, `fsync` and
rename after every changing payload, so SIGTERM is safe [V for the
snapshot; I for the delta files, which share the store].

**Size and test:** M. The server change is behaviour-neutral. The test is
`two_bind_addresses_both_accept`.

### F1. Pathname unix sockets are an egress path out of the jail. This risks phone-home.

**Evidence:**
- `docs/guides/jail.md:38-40` says unix sockets are "completely unaffected by
  network namespacing". That is true, and it is the hole.
- The client runs with the user's `XDG_RUNTIME_DIR` (`jail-client.sh:29`),
  so the session D-Bus at `$XDG_RUNTIME_DIR/bus` is reachable [V], and so
  are `/run/systemd/resolve/io.systemd.Resolve` (mode 666) and
  `/run/avahi-daemon/socket` (mode 666) [V `ls`].

Three concrete routes:

1. **The browser.** `blue.os.ShellExecute(url)` and `webbrowser.open_new`
   go through Wine to `winebrowser` and then `xdg-open`. An already-running
   host browser takes the URL over D-Bus or X11 remoting, and loads it from
   the host's network namespace.
   - `docs/lanes/plex-vault.md:165-170` already names this [I].
   - The DNS log never sees it, because no resolution happens in the jail.
   - Reachable from: three `buy_plex_online` call sites, and any Help
     Center link (`chat-petitions.md:124`).
2. **DNS through systemd-resolved.** Today
   `hosts: files mdns4_minimal [NOTFOUND=return] dns` has no `resolve`
   [V]. So glibc honours the jail's `resolv.conf`. If a distro update adds
   `resolve`, which is Fedora's default, jail DNS goes over varlink to the
   host's upstream, and **query names leak** even though no packet routes.
   `jail-verify.sh:79-88` would catch it. The helper's `verify` would not
   (F2).
3. **mDNS.** `mdns4_minimal` goes through the avahi socket to LAN
   multicast. Only `.local` names go this way. Low cost.

**Change:** give the client a **private mount namespace with the egress
sockets masked**.

`ip netns exec` already unshares the mount namespace. So in the helper's
`v_client`, as root and before `setpriv` (code in the root-owned file, so
rule 1 holds), add:
- `mount -t tmpfs none` over `/run/systemd/resolve`, `/run/avahi-daemon`
  and `/run/dbus`;
- `mount --bind /dev/null` over `/run/user/$SUDO_UID/bus`;
- `mount --bind /bin/false` over `/usr/bin/xdg-open`.

Leave `/tmp/.X11-unix`, `$XDG_RUNTIME_DIR/wayland-*` and `pipewire-*`
alone. Each is a display or audio server with no network verb [I].

Mirror this in `run-client.sh`'s non-helper branch with
`sudo ip netns exec NS sh -c '<same mounts>; exec sudo -u ...'`. The
helper then asserts `[[ ! -S /run/systemd/resolve/io.systemd.Resolve ]]`
inside the new namespace before exec, and refuses to launch on failure.

Fix `docs/guides/jail.md:38-40` to say what is shared and what is masked.

**Size and test:** S-M, tools only, behaviour-neutral for gameplay. The
test is the in-helper assertion plus a human check: in game, click "Buy
PLEX"; no browser opens.

### F2. The helper's `verify` is weaker than `jail-verify.sh`, and it is the one unattended runs use. This risks phone-home.

**Evidence:** `eve-offline-jail.in:194-220` [V]:
- It probes **hard-coded** `192.0.2.1:80`, `172.17.0.1:53` and
  `192.0.2.151:22`. On any other LAN, including a friend's machine, two
  of the three probes hit addresses that do not exist, and pass trivially.
- It has no UDP port-allowlist probe. `jail-verify.sh:69` has one.
- It has no DNS-must-not-resolve checks. `jail-verify.sh:79-88` has them.
  Those are the checks that catch F1 route 2.
- It does not assert that the nft tables exist with `policy drop`.
- Neither script detects a non-default broad route such as `0.0.0.0/1`.
  `jail-verify.sh:91` and helper `:208` look only for `default`.

**Change:**
- Port `jail-verify.sh`'s probe set into `v_verify`, deriving the gateway
  and self address at run time as `jail-verify.sh:47-48` does.
- Add three assertions to both scripts:
  1. `ip -n NS -4 route` is exactly the connected `/30`;
  2. `nft list chain inet eve_offline_out output` contains `policy drop`;
  3. the host table `inet eve_offline_jail` exists.
- Add a gate check, `tools/check_jail_sync.py` (stdlib), which extracts
  the `table inet ... { }` heredocs from `jail-setup.sh` and
  `eve-offline-jail.in` and fails on any difference. That turns the
  hand-kept snapshot (`install-jail-helper.sh:16-19`) into a checked one.
- In `up.sh`, warn when `generate | cmp -s - /usr/local/sbin/eve-offline-jail`
  differs. The installed file is world-readable, so no root is needed.

**Size and test:** S, tools only. The gate check is the test, and it is
falsified by editing one rule.

### F3. In helper mode the resolver log, which is the phone-home audit, is empty. This makes phone-home invisible.

**Evidence:**
- `jail-drops.sh:20-24`: "`logs/eve-dns.log` is a complete list of every
  service the client wanted to talk to".
- But `v_dns` sends the resolver's output to `/dev/null`
  (`eve-offline-jail.in:297-300`), deliberately, so that root does not open
  a user-replaceable path.
- `up.sh:383` captures only the helper's one "resolver started" line [V].
  So every unattended or helper run has no phone-home record, and
  `jail-drops.sh` reports nothing.

**Change:** add `eve-dns --log FILE`. **eve-dns opens the file itself,
after `setpriv` has dropped to the user**, so root never touches the path.
The helper passes `--log $REPO/logs/eve-dns.log`, which is a fixed string
in the root-owned file.

**Size and test:** S, behaviour-neutral. The test is an `eve-dns` unit
test: `--log` appends one line per query in the format `jail-drops.sh`
parses (`query NAME. TYPE -> IP` / `NXDOMAIN`).

### F4. `docs/guides/jail.md` gets the server-to-client boundary wrong.

**Evidence:**
- `docs/guides/jail.md:409-415` says "There is no code execution", citing
  `debug.Eval` and `debug.Exec`.
- But `charMgr.GetSettingsInfo` returns a marshalled code object that the
  client installs as `SettingsInfo.__code__` and calls
  (`pycode.rs:173-190`, `service/character.rs:630`) [V].
- Handshake H8 `signedFunc` is the same mechanism (`handshake.rs:418-420`)
  [V].
- `--scenario` uses it to `execfile` arbitrary scripts (`pycode.rs:77-82`)
  [V].

**Change:** rewrite `docs/guides/jail.md`'s last section:
- the server can run arbitrary Python in the client, under the user's uid
  and inside the jail;
- the jail is therefore what bounds a compromised or MITM'd server, not
  the exported-call set;
- this is why F5's tunnel is mandatory for remote play.

**Size:** S, docs only.

### F9. Exposed-server robustness: connection caps, and accept errors kill the server.

**Evidence:**
- `server.rs:285-301` spawns one thread per accepted connection with no
  cap.
- `self.listener.accept()?` returns on the first error, so EMFILE ends
  `run()` and with it the server [V for the code path; I for the effect].
- Each connection can buffer up to `MAX_FRAME + READ_CHUNK` ≈ 10 MB
  (`conn.rs:67`, `frame.rs:36`).
- Before the handshake completes, the idle timeout is 600 s (`conn.rs:36`).

So about 100 silent connections pin about 1 GB for 10 minutes [I]. This
matters even behind F5's tunnel, because a buggy bot loop does the same.

**Change:**
- Add `--max-connections N`, default 64, as an atomic counter in `run()`.
  Over the cap, accept and close immediately.
- On an accept error, log it, sleep 100 ms and continue.
- Add a 30 s handshake deadline, separate from the idle timeout.

This overlaps V2's wire findings: land it in V2's packet if V2 claims
`conn.rs`.

**Size and test:** S, behaviour-neutral under the cap. The test is
`connections_over_the_cap_are_closed_not_queued`.

### F10. A world upgrade is guarded, but there is no backup and a downgrade silently loses data.

**Evidence:**
- An unknown schema is refused (`store.rs:489-495`) [V], which is good.
- But unknown *records* are skipped (`store.rs:681-685`) [V]. An older
  build reading a newer world drops those records **on its next save**.
- Nothing backs up the world before a new build first writes it.

**Change:**
- Add a `build <git-sha>` header record. `tools/dev.sh` passes
  `-e EVE_GIT_SHA`, and the server reads it with
  `option_env!("EVE_GIT_SHA")`, std only.
- On boot with `--world`, if the file's build differs from ours, copy it
  to `world.dat.<old-sha>.bak` before the first save, and keep the last 5.
- When the file contains records this build skipped, refuse to save over
  it unless `--accept-downgrade` is given.

**Size and test:** S. The tests are `a_newer_world_is_backed_up_before_first_save`
and `skipped_records_block_the_save`.

### F11. `check_deps.py` prints what it should assert.

**Evidence:**
- `check_deps.py:63-65` *prints* the external packages in `Cargo.lock`,
  but never fails on them [V].
- It does not check `[patch]` or `[replace]` in `server/Cargo.toml`.
- It does not check that `server/.cargo/config.toml` keeps
  `offline = true` and `replace-with = "distro"`. Those two lines are what
  make the zero-crate rule hold for builds: the registry is the image's
  apt directory, and there is no network.
- There is no `build.rs` today [V `find`], and nothing keeps it that way.

**Change:**
- Fail when `Cargo.lock` has any package with a `source` that is not
  covered by `deps.allow`. Today that means "fail on any".
- Fail on `[patch]` and `[replace]`.
- Fail on a missing `offline = true` or `replace-with = "distro"`.
- Fail on any `crates/*/build.rs` that is not allowlisted.

**Size and test:** S. Add `tools/test_check_deps.py` on synthetic trees,
following the `test_*.py` pattern the gate already runs.

### F12. Image pinning is uneven.

**Evidence:**
- `build.Dockerfile:14` is pinned by digest [V]. But
  `apt-get install rustc cargo` is unpinned (`:17-21`), so a rebuild can
  move the toolchain. Today it is rustc 1.93.1 on 26.04.1 [V]. The image
  records crates (`:24`) but not `rustc -V`.
- `display.Dockerfile:24` uses `ubuntu:26.04`, a tag.
- `analysis.Dockerfile:8` uses `python:3.11-slim`, a tag. Its pip pins
  (`:19-26`) have no `--require-hashes`. This is the only PyPI fetch in
  the project.

All three images run `--network none` [V: `dev.sh:51`,
`client-headless.sh:151`, `analysis.sh:86`]. That bounds the damage to
what a poisoned tool writes into `out/` or `server/target/`. Of those,
`server/target/eve-dns` and `eve-server` are executed on the host.

**Change:**
- Pin `display` and `analysis` by digest.
- Use `apt-get update --snapshot <date>` in all three images. This is
  supported by apt on 26.04 [I].
- Use `pip install --require-hashes -r containers/analysis.requirements.txt`.
- Run `rustc -V > /etc/eve-offline-toolchain.txt` in the image, and have
  the gate print it.

**Size:** S, behaviour-neutral.

### F13. The privilege picture is wider than the helper suggests.

**Evidence:**
- `id` shows `docker` and `libvirt` [V]. Both are root-equivalent: for
  example, `docker run -v /:/h`.
- So `docs/reference/unattended-root.md` §5 ("what this still grants") understates
  the existing posture. The helper adds little, because the user already
  has root by another door.
- Separately, the non-helper path runs the **user-built** `eve-dns` as full
  root with `sudo "$DNS" --bind ...` (`up.sh:387`) [V]. That breaks the
  helper's rule 1, "root never executes anything the invoking user can
  write".

**Change:**
- Use
  `sudo setpriv --reuid=$(id -u) --regid=$(id -g) --clear-groups --inh-caps=+net_bind_service --ambient-caps=+net_bind_service "$DNS" ...`
  in `up.sh:387`.
- Add a paragraph to `docs/reference/unattended-root.md` §5 on docker and libvirt group
  membership. Whether to remove it is the owner's call.

**Feasibility note for V4, verified:** unprivileged
`unshare --user --map-current-user --net` works on this host [V]. It is
already used by `client-headless.sh:316`.

A **fully rootless jail** is therefore available:
- user, network and mount namespaces, with only `lo` inside;
- `eve-dns` on `127.0.0.1:53` inside, where namespace-root can bind 53;
- `resolv.conf` and F1's socket masks bind-mounted in;
- the server reached through a unix-socket relay.

`std::os::unix::net::UnixListener` keeps the server std-only and free of
`unsafe`.

With only `lo`, egress is impossible by construction, which is strictly
stronger than a veth. That is V4's call to size. It removes `sudo` from
the client path entirely.

### Re-verified and sound (no change)

- **Network layers.** Four things hold [V, from `jail-setup.sh:63-165` and
  `eve-offline-jail.in:89-174`]:
  - no default route is ever added;
  - the namespace output chain is `inet` with `policy drop`, which covers
    IPv6 as well (link-local v6 is dropped);
  - host forwarding is dropped at priority −300 on subnet and interface;
  - `nft -c` runs before apply, so a half-built ruleset cannot fail open.
- **The 2026-10-19 result.** It was recorded in `docs/guides/jail.md:427-450`. It was
  not re-run here, because that needs root.
- **The launch env allow-list.** It is data and not `source`
  (`jail-client.sh:27-46`), and `setpriv` drops root before any repo file
  runs (`eve-offline-jail.in:312-318`) [V].
- **The helper's trust chain.** `assert_trusted` walks the path to `/`
  (`:64-76`) [V].
- **Crash upload.** `/noCrashReportUpload` is passed (`run-client.sh`, the
  `args`) [V]. Crashpad still tries, and fails at routing.
- **Port 8443.** It is allowed by the jail (`jail-setup.sh:32`), and
  nothing of ours listens on it [V grep]. Nothing on the host listens on
  `*:8443`, `*:5222` or `*:53` today [V `ss`].
  - A future wildcard listener on one of those ports would be reachable
    from the jail.
  - A wildcard DNS forwarder there would leak query names upstream.
  - Fix: drop 8443 until something serves it, as part of P1.
- **`dev.sh`.** It runs with no network, no capabilities, a read-only root,
  a read-only repo, a CPU, memory and time cap, and runs as the user
  (`dev.sh:50-70`) [V].
- **Secrets.** None are tracked [V `git ls-files`]. The git host tokens live
  in `the agent secrets folder`, outside the repo.

## Proposals

1. Harden the jail's verification and audit: F2, F3, the `up.sh:387`
   privilege drop, and dropping 8443 from the allowed ports.
2. Mask the unix-socket egress paths in the client's mount namespace (F1).
3. Correct `docs/guides/jail.md` on code push and unix sockets (F1, F4).
4. Remote play by SSH tunnel into the friend's own jail, with no public
   bind (F5).
5. Accounts from `/ssoToken`, with per-account role and server-side GM
   gating (F6, F7).
6. Add `tools/serve.sh` as the hosting entry point: persistent world,
   rotated logs, captures off, repeatable `--bind` (F8).
7. World upgrade safety: a build stamp, backup before the first save, and
   refusal to save after a downgrade (F10).
8. Connection cap, handshake deadline, and accept that survives errors
   (F9).
9. Supply-chain tightening: `check_deps` asserts, digests, apt snapshot,
   pip hashes, toolchain record (F11, F12).
10. Owner-facing: document the docker and libvirt root-equivalence; V4
    sizes the rootless jail (F13).

## Packets

| # | Files (owner) | Tests / checks | Size | Neutral |
|---|---|---|---|---|
| **K1** jail verify + audit | `tools/eve-offline-jail.in`, `tools/jail-verify.sh`, `tools/jail-setup.sh`, `tools/up.sh`, new `tools/check_jail_sync.py` + `tools/test_check_jail_sync.py`; `server/crates/eve-dns` (`--log`) | `check_jail_sync` in the gate; eve-dns `--log` unit test; human: `sudo -n eve-offline-jail verify` on this host | S | yes |
| **K2** socket masks | `tools/eve-offline-jail.in` (`v_client`), `tools/run-client.sh` (direct branch), `docs/guides/jail.md` | in-helper assertion refuses the launch; human: "Buy PLEX" opens no browser | S-M | yes |
| **K3** jail doc | `docs/guides/jail.md`, `docs/reference/unattended-root.md` §5 | none (docs) | S | yes |
| **K4** remote play | new `tools/friend-tunnel.sh`, new `docs/guides/playing-with-friends.md` | human: two hosts, one tunnel | M | yes |
| **K5** accounts + roles | `handshake.rs`, `server.rs`, `session.rs`, `slash.rs`, `service/gm.rs`, `capture.rs`, `bin/eve-server.rs` (per `account-spec.md` §7) | spec §6 two-connection test; `a_player_session_cannot_run_createitem`; capture-redaction test | M-L | **no** |
| **K6** serve mode | new `tools/serve.sh`, `.gitignore` (`var/`), `bin/eve-server.rs` + `server.rs` (repeatable `--bind`) | `two_bind_addresses_both_accept` | M | yes |
| **K7** world safety | `store.rs`, `bin/eve-server.rs`, `tools/dev.sh` (`EVE_GIT_SHA`) | the two F10 tests | S | yes |
| **K8** connection limits | `server.rs`, `conn.rs` (coordinate with V2) | `connections_over_the_cap_are_closed_not_queued` | S | yes, under the cap |
| **K9** supply chain | `tools/check_deps.py` + `tools/test_check_deps.py`, `containers/*.Dockerfile`, new `containers/analysis.requirements.txt`, `tools/gate.sh` (print toolchain) | `test_check_deps.py` | S | yes |

**Order.** K1, K3 and K9 can go now. They are small, touch only tools and
docs, and do not collide with content waves. K2 follows K1, because both
touch the same file. K5 waits for the owner's answer in
`docs/guides/human-checks.md` §3. K6, K7 and K4 come next, in that order. Together
they are "friends can play", and K4 assumes K5 and K6.

## Do not do

- **TLS inside `eve-server`.** It needs a crate or hand-rolled crypto,
  which breaks std-only or is worse than a crate. The client speaks
  Placebo anyway. SSH (K4) covers the transport.
- **Password hashing or a login form.** `user_password` is the client
  literal `fakePW` (`account-spec.md` §2). There is nothing to check. The
  token is the credential.
- **Patching `ShellExecute`, Sentry or Crashpad in `blue.dll`.** K2's
  mount masks cover every browser path with no client change. The jail
  already stops Sentry. A third binary patch is fragile across builds.
- **Per-RPC rate limiting.** Friends are trusted, and bots are ours.
  Connection caps (K8) bound the real failure, a runaway loop.
- **A systemd unit or host service for the server.** That is host
  configuration outside the repo, and the owner runs it by hand. Keep
  `serve.sh`.
- **Binding `0.0.0.0` "just for the LAN".** It means an unauthenticated
  GM shell for anyone on the network, and a plaintext code-push channel
  into every client. Use K4 even on a LAN.
- **Defeating the `manifest.dat` signature.** It is not needed.
  `client-patch-manifest.sh` already does the minimum.

## Unknown

- Whether Proton's `winebrowser`/`xdg-open` path actually reaches a
  running host browser (F1 route 1). It is the wrong thing to confirm
  empirically, which is why K2 masks it regardless.
- Whether the client opens any URL **unprompted**, for example on login
  or on an error. Only player actions have been traced.
- What platform the friends use, and whether they can run the jail
  (Linux, Proton, sudo). This decides whether K4 is enough.
- Whether `apt-get update --snapshot` is available to the pinned
  build-image digest.
- Delta-file write atomicity under SIGTERM. `store.rs` covers the
  snapshot, and the deltas were not traced.
- The jail with a client generating traffic inside it. `docs/guides/jail.md:461-462`
  still lists this as not exercised.
