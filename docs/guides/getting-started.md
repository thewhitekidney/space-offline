# Getting started

One ordered path from a fresh clone to playing: build the server, build the
content bundle, run the server, run the client in its jail, add bots, and
pass the gate. Each step gives the command, what you should see, and what to
do when you do not. The other guides hold the detail; this one links to
them and does not restate them.

Paths are relative to the repository root. `$HOME` is your home directory.
Every flag and `EVE_*` variable is in
[launch-and-env.md](../reference/launch-and-env.md).

## 1. Prerequisites

- **Docker, and nothing else installed for this project.** Builds, tests and
  extraction all run in containers (`tools/dev.sh`, `tools/analysis.sh`). The
  host needs only Docker, bash, coreutils, `python3` (standard library only,
  always run as `python3 -I`), and, for the client, Steam with Proton and
  `sudo` for the jail.
- **The client build is pinned.** `tools/builds.tsv` names it: the one row
  with status `pinned` (today `3662168`). The server refuses other builds
  unless the registry marks them `equivalent`
  ([build-compat.md](build-compat.md)).
- **The client comes from the human.** A person installs EVE Online through
  Steam, logs in once with the official launcher, and lets it download
  every resource file at the pinned build. Nothing in this repository
  downloads the client. Check the build:

  ```sh
  grep '^build' "$HOME/.steam/steam/steamapps/compatdata/8500/pfx/drive_c/CCP/EVE/tq/start.ini"
  # build = 3662168
  ```

  Steam library elsewhere: set `EVE_ONLINE_PREFIX`.
- **Disk and RAM.** A server build takes 8-11 GB in `server/target`, and
  the client copy plus ResFiles takes tens of GB. `tools/dev.sh` waits until
  `EVE_MIN_FREE_GB` (default 12) of RAM is free.

## 2. Build the server

```sh
tools/dev.sh --image                                   # once; the only networked step
EVE_MAX_JOBS=6 tools/dev.sh cargo build --locked --release --workspace
```

Expected: the image builds, then cargo ends with `Finished release
[optimized] target(s)`. The binaries are in `server/target/release/`:
`eve-offline-server`, `eve-server`, `eve-bots`, `eve-dns`.

The container has no network, the repository mounted read-only, and one
writable directory, `server/target`. The workspace has no dependencies.

| Symptom | Fix |
|---|---|
| `image 'eve-offline-build' missing` | run `tools/dev.sh --image` |
| it prints that it waits for free memory | close something, or lower `EVE_MIN_FREE_GB` |
| killed after an hour | raise `EVE_DEV_TIMEOUT` |
| out of memory in the container | lower `EVE_MAX_JOBS`, raise `EVE_DEV_MEMORY` |

`tools/up.sh` and `tools/serve.sh` run this build themselves; `--no-build`
skips it.

## 3. Make the offline client copy

The server and the content tools read a **copy** of the install in a Wine
prefix of its own, never the install you play on
([jail.md](jail.md#we-run-a-copy-never-the-install-you-play-on)):

```sh
# with EVE closed:
cp -a .../compatdata/8500/pfx/drive_c/CCP/EVE/tq .../compatdata/8500/pfx/drive_c/CCP/EVE/tq-offline
tools/offline-prefix.sh --link-resfiles   # the offline prefix, ResFiles hard-linked
tools/offline-prefix.sh --check           # what is there; changes nothing
```

A friend uses `tools/friend-setup.sh copy`, which does both
([playing-with-friends.md](playing-with-friends.md)).

The copy lands at
`$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline`. That
is the default `EVE_CLIENT`, and `ResFiles` sits beside it. Then patch the
copy and record its pin:

```sh
python3 -I tools/eve_patch.py apply --client "$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline"
python3 -I tools/eve_patch.py verify --client "$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline"
tools/pin-client.sh            # verify the copy against tools/client-pin.sha256
```

The patcher's tiers and exit codes are in [patcher.md](patcher.md). The pin
is in [jail.md](jail.md#the-build-is-pinned-and-now-it-is-checked).

## 4. Build the content bundle

The server's universe, types, NPCs and so on come from the client's own
static data, extracted into `out/content/`. `out/` is gitignored: client
data never enters the repository. The full input list and the steps are in
[content-bundle.md](content-bundle.md).

```sh
tools/analysis.sh --build      # once: the analysis image
tools/analysis.sh python3 -I /work/tools/content.py \
    --client /client --resfiles /client-root/ResFiles --out /work/out/content
tools/analysis.sh python3 -I /work/tools/content.py \
    --client /client --resfiles /client-root/ResFiles --out /work/out/content --check
```

`tools/analysis.sh` mounts the repository at `/work`, `EVE_CLIENT` read-only
at `/client` and its parent at `/client-root`, with no network.

Expected: one line per section, then `out/content/MANIFEST.tsv`, whose first
line is the banner `# eve-offline content bundle. CCP's data: never commit
this directory.` and whose `client_build` row names the pinned build. With
`--check`, the exit code is 0.

You rarely run this by hand: `tools/up.sh` and `tools/serve.sh` re-extract
when the bundle is missing or stale, then run `--check` and refuse to boot
on a failure (`logs/content-check.log`).

| Symptom | Fix |
|---|---|
| no `resfileindex.txt` | `EVE_CLIENT` is not an install; set it or `EVE_CLIENT_DIR` |
| no ResFiles | set `EVE_RESFILES`, or make the copy with `--link-resfiles` |
| the build is refused | the install is not the pinned build; see [build-compat.md](build-compat.md) |
| the `wormholes` section fails | a name in `content/wormholes/combat-sites.tsv` matches no combat dungeon of this build; fix the name, see [content-bundle.md](content-bundle.md#inputs) |
| a schema guard disagreement | the client changed under the extractor; see [build-compat.md](build-compat.md) |

## 5. Run the server locally

**Fast path: a dev world**, with no client and no jail
([dev-mode.md](dev-mode.md)):

```sh
tools/dev-world.sh none        # dev server on 127.0.0.1:26000, no bots; Ctrl-C stops
```

Expected: a `# DEV MODE` banner, then
`eve-server listening on 127.0.0.1:26000 in Gps mode (...)`. The world is
`var/dev/world.dat`. Wipe it with `rm -r var/dev`.

**The normal world, accounts, and your GM grant**
([playing-with-friends.md](playing-with-friends.md#the-owner)):

```sh
tools/account.sh login                    # pick your name and password (asks; mode 600)
sudo -v && tools/up.sh --signup           # log in: you sign up as a player; quit the client
tools/account.sh grant 'Your Name' gm     # with the server stopped
sudo -v && tools/up.sh                    # log in again: /whoami says GM
```

The world is `var/world.dat`. Services only, no client:
`sudo -v && tools/up.sh --no-client`. To host it for others, use
`tools/serve.sh` ([hosting.md](hosting.md)).

## 6. Run the client against it

The client always runs in the jail: a network namespace whose only reachable
address is the host end of a veth pair, `10.137.0.1`, ports 26000, 5222 and
53. `tools/up.sh` does all of it ([jail.md](jail.md#use)):

```sh
sudo -v && tools/up.sh         # build, jail, resolver, server, client
```

The pieces, if you would rather drive them yourself:

```sh
sudo tools/jail-setup.sh      # once per boot
sudo tools/jail-verify.sh     # assert no egress; non-zero exit if leaking
tools/run-client.sh           # launches only if verify passes
sudo tools/jail-teardown.sh   # remove
```

`run-client.sh` refuses to launch on a failed verify, and it refuses the
real `tq` install. It passes `/ssoToken=` from your login file to skip the
launcher. A second client: `tools/client2-setup.sh`, then
`tools/up.sh --keep` and `tools/run-client.sh --second`.

More copy-paste launches, each with its ready line, are in
[launch-and-env.md](../reference/launch-and-env.md#launch-examples):

- a minimal dev server;
- accounts with chat and a GM;
- the server with bots and the `/bots` commands;
- the container form;
- a second client;
- a client against a server on another machine (through
  `tools/jail-forward.py`, so the jail stays closed);
- one client-CI scenario.

Without root, `tools/jail-rootless.sh` gives the same view
([jail.md](jail.md)). The phone-home patcher and its DNS census are in
[patcher.md](patcher.md) and [census.md](census.md). Client logs and
debugging are in [debugging-the-client.md](debugging-the-client.md).

| Symptom | Fix |
|---|---|
| `jail-verify` fails | do not launch; read [jail.md](jail.md#the-host-firewall-has-to-let-the-jail-reach-our-services) |
| the client refuses the build | patch and pin the copy (step 3) |
| login refused | sign up first (`--signup`), or check `tools/account.sh login` |
| slow fitting window or menus | run the server with `--gateway` |

## 7. Bots

```sh
sudo -v && tools/bots.sh 3                  # you + Miner1..3, client launches
tools/bots.sh --no-client 5                 # headless: server + 5 bots, ctrl-c stops
tools/dev-world.sh pvp --home "A LowSec System"   # a dev roster ([dev-mode.md](dev-mode.md))
tail -f logs/eve-bots.log
```

Every `eve-bots` flag is in
[launch-and-env.md](../reference/launch-and-env.md#eve-bots). In game, a GM
on a dev server spawns bots with `/bots spawn`.

## 8. Tests and the gate

```sh
EVE_MAX_JOBS=6 tools/dev.sh cargo test --locked --workspace
EVE_MAX_JOBS=6 tools/dev.sh cargo clippy --locked --workspace --all-targets -- -D warnings
docker run --rm --network none -u $(id -u):$(id -g) -v $PWD/server:/s:rw -w /s eve-offline-build cargo fmt --all
for t in tools/test_*.py; do python3 -I "$t" || echo "FAILED $t"; done
python3 -I tools/env-ref.py --check   # this page set and the code agree
tools/land.sh --no-push               # the full gate on your branch
```

Expected: every cargo test passes, clippy is quiet, each tool test prints
`OK`. `tools/land.sh` runs `tools/gate.sh` and branches on its exit code
alone. Never pipe a test command into `&&`, because the pipe hides the
exit code. Client scenarios (`tools/client-ci.sh`) need a client and are in
[client-ci.md](client-ci.md). How to contribute is in
[contributing.md](contributing.md).
