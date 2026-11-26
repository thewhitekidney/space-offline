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

- **Recommended: Docker, so nothing else is installed for this project.** The tools
  run builds, tests and extraction in containers (`tools/dev.sh`,
  `tools/analysis.sh`). With them the host needs only Docker, bash, coreutils, `python3` (standard library only,
  always run as `python3 -I`), and, for the client, Steam with Proton and
  `sudo` for the jail.
- **The client build is pinned.** `tools/builds.tsv` names it: the one row
  with status `pinned`
  (`awk -F'\t' '$3=="pinned"{print $1}' tools/builds.tsv` prints it). The
  launcher always installs the current Tranquility build. When that build is
  not `pinned`, `equivalent` or `provisional`, the server refuses it until the registry
  catches up ([tracking-tq.md](tracking-tq.md)). Server-only work (most issues) needs no
  client at all.
- **The client comes from the human.** A person installs EVE Online through
  Steam, logs in once with the official launcher, and lets it download
  every resource file. Nothing in this repository
  downloads the client. Check the build:

  ```sh
  grep '^build' "$HOME/.steam/steam/steamapps/compatdata/8500/pfx/drive_c/CCP/EVE/tq/start.ini"
  awk -F'\t' '$3=="pinned"{print "pinned: " $1}' tools/builds.tsv   # the value it should show
  ```

  Steam library elsewhere: set `EVE_ONLINE_PREFIX`.
- **Disk and RAM.** A server build takes 8-11 GB in `server/target`, and
  the client copy plus ResFiles takes tens of GB. `tools/dev.sh` waits until
  `EVE_MIN_FREE_GB` (default 12) of RAM is free.

### Supported platforms

- Linux, and WSL2 with Ubuntu 24.04, run the server, the tools and the gate.
- Python 3.11 or newer.
- glibc 2.35 or newer on the host: the server binary is built in the Docker image and
  runs on the host (`tools/serve.sh`); an older host fails at the dynamic loader.
- Docker, for `tools/dev.sh` (builds and tests) and `tools/analysis.sh`.
- Ubuntu 22.04 (Python 3.10) and macOS are not supported.
- The client does not run under WSL. It needs Linux with Steam and Proton,
  or Windows: the native client on the same PC as the server in WSL2
  ([windows.md](windows.md)), or a separate Windows machine. A server-only
  checkout needs neither.

## 2. Build the server

```sh
tools/dev.sh --image                                   # once; the only networked step
EVE_MAX_JOBS=6 tools/dev.sh cargo build --locked --release --workspace
```

Expected: the image builds, then cargo ends with `Finished release
[optimized] target(s)`. The binaries are in `server/target/release/`:
`eve-offline-server`, `eve-server`, `eve-bots`, `eve-dns`.

The container has no network, the repository mounted read-only, and one
writable directory, `server/target`. The workspace's few crates come from the image's apt registry; the build stays offline.

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

The copy lands at
`$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline`. That
is the default `EVE_CLIENT`, and `ResFiles` sits beside it.

To play on this server the copy must be prepared. Without it the server drops the
connection right after the handshake, and the client only says "The connection to the
server was closed". The patcher uses only the Python standard library:

    python3 -I tools/eve_patch.py plan   --client "$EVE_CLIENT"   # dry run
    python3 -I tools/eve_patch.py apply  --client "$EVE_CLIENT"
    python3 -I tools/eve_patch.py verify --client "$EVE_CLIENT"

It knows each build by its `start.ini` and refuses a build it has no manifest for, a
file it does not recognise, and an install named `tq` (the one you play online with).
`revert` undoes it. On Windows, run the same commands with `py -3 -I` against the copy's
folder; [windows.md](windows.md) step 4 has the commands and the prefs folder to pass.
What it changes and why: [patcher guide](patcher.md). The server-only
and content steps below need no prepared client.

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
| the build is refused | the install is not the pinned build; see [tracking-tq.md](tracking-tq.md) |
| the `wormholes` section fails | a name in `content/wormholes/combat-sites.tsv` matches no combat dungeon of this build; fix the name, see [content-bundle.md](content-bundle.md#inputs) |
| a schema guard disagreement | the client changed under the extractor; see [tracking-tq.md](tracking-tq.md) |

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
(the grant is the GM flag in the account file):

```sh
tools/account.sh login                    # pick your name and password (asks; mode 600)
sudo -v && tools/up.sh --signup           # log in: you sign up as a player; quit the client
tools/account.sh grant 'Your Name' gm     # with the server stopped
sudo -v && tools/up.sh                    # log in again: /whoami says GM
```

The world is `var/world.dat`. Services only, no client:
`sudo -v && tools/up.sh --no-client`. To host it for others, use
`tools/serve.sh`.

## 6. Run the client against it

`tools/up.sh` runs the client in the jail, a network namespace whose only reachable
address is the host end of a veth pair, `10.137.0.1`, ports 26000, 5222 and
53, and we recommend it. It does all of it ([jail.md](jail.md#use)):

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
([jail.md](jail.md)).

| Symptom | Fix |
|---|---|
| `jail-verify` fails | do not launch; read [jail.md](jail.md#the-host-firewall-has-to-let-the-jail-reach-our-services) |
| the client refuses the build | the copy is not the pinned build or is not patched (step 3) |
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
tools/dev.sh --lock cargo fmt --all
rc=0; for t in tools/test_*.py; do python3 -I "$t" || { echo "FAILED $t"; rc=1; }; done; echo "tool tests: exit $rc"
python3 -I tools/env-ref.py --check   # this page set and the code agree
tools/land.sh --no-push               # the full gate on your branch
```

Expected: every cargo test passes, clippy is quiet, each tool test prints
`OK`. `tools/land.sh` runs `tools/gate.sh` and branches on its exit code
alone. Run it bare: no `| tail`, `| tee`, `; echo` or `&&` after it, because a
pipe or a list reports its last command's status, 0 even on red. Its last
line, `land: result: green|red, ...`, is the verdict. Client scenarios (`tools/client-ci.sh`) need a client and are in
[client-ci.md](client-ci.md). How to contribute is in
[contributing.md](contributing.md).
