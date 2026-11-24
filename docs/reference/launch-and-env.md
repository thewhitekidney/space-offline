# Launch flags and environment variables

Every command-line flag of `eve-server` / `eve-offline-server` and
`eve-bots`, and every `EVE_*` name the code under `tools/`, `deploy/` and
`server/crates/` reads or mentions. `python3 -I tools/env-ref.py --check`
fails when this page and the code disagree: a name in the code that has no
row here, or a row whose name the code no longer has. A row is a table line
that starts with a backticked name.

The authoritative text of each flag is the program's own `--help`. This page
groups them and gives one example each. Paths are relative to the repository
root. `$HOME` is your home directory.

For the order you run things in, see
[getting-started.md](../guides/getting-started.md).

## Launch examples

Copy-paste forms, each with the line that says it is ready. The binaries are
in `server/target/release/` after `tools/dev.sh cargo build --locked
--release --workspace`. The bundle is `out/content`
([content-bundle.md](../guides/content-bundle.md)). The addresses `192.0.2.x`
are documentation placeholders: use your server host's address.

### Server

**A minimal local dev server**, no accounts and no jail. It answers any
login on loopback as GM:

```sh
server/target/release/eve-offline-server --dev --content out/content
# ready: eve-server listening on 127.0.0.1:26000 in Gps mode (...)
```

**A dev world with accounts, chat and a GM.** The first login signs up.
Then stop the server and grant GM:

```sh
server/target/release/eve-offline-server --dev --world var/dev/world.dat \
    --content out/content --world-accounts --chat \
    --bind 127.0.0.1:26000,signup=on --bind 10.137.0.1:26000,ceiling=gm,signup=on
# ready: eve-server listening on 127.0.0.1:26000 ..., then [chat] listening on 127.0.0.1:5222 (cap ...)
touch var/dev/world.dat.stop                       # clean stop: the world is written, exit 0
server/target/release/eve-offline-server --dev --world var/dev/world.dat --grant 'Your Name' gm
```

The same through the wrapper, with jail, resolver and client:
`sudo -v && tools/up.sh --signup` ([getting-started.md](../guides/getting-started.md#5-run-the-server-locally)).

**The server with bots.** Start the server with a control link, then dial it
from `eve-bots`:

```sh
server/target/release/eve-offline-server --dev --world var/dev/world.dat \
    --content out/content --world-accounts --chat --bot-control var/dev \
    --bind 127.0.0.1:26000,signup=on,cap=2000
server/target/release/eve-bots --connect 127.0.0.1:26000 --content out/content \
    --control var/dev --chat-host 127.0.0.1:5222
# ready: the server prints the bot-control link; eve-bots idles until a GM spawns
```

Then, in game as a GM (`/bots help` lists them all):

```text
/bots 10 allied miners
/bots 5 hostile roamers
/bots 4 allied escorts here
/bots spawn gang1 RED mine 3 role Hulk 30000042
/bots list
/bots come gang1
/bots despawn all
```

Or all at once: `tools/dev-world.sh world50` (a roster of 50 bots), or
`tools/bots.sh --no-client 5`.

**The container form.** Run the two release binaries and `content/` in any image you
build; the server flags are under "Server" above. Mount the content bundle from outside,
because client data never goes into an image you might push.
`docker stop eve-offline` writes the world first.

**With the common environment variables set**, for the container:

```sh
docker run -d --name eve-offline -v "$HOME/eve-data:/data" -v "$PWD/out/content:/content:ro" \
    -p 26000:26000 -p 5222:5222 -p 8443:8443 -p 127.0.0.1:8090:8090 \
    -e EVE_GM_ACCOUNT='Your Name' \
    -e EVE_CHAT_ADVERTISE=192.0.2.10 \
    -e EVE_ADMIN_ADDR=0.0.0.0:8090 \
    -e EVE_SERVER_ARGS='--time-scale 2 --world-start-isk 5000000' \
    eve-offline:latest server
```

For the host tools:

```sh
EVE_MAX_JOBS=6 EVE_TIME_SCALE=10 EVE_SERVER_BIN=eve-offline-server sudo -E tools/up.sh --no-client
```

### Client

`tools/up.sh` and `tools/run-client.sh` launch the client in the jail ([jail.md](../guides/jail.md)), which we recommend.

**The normal launch against a local server:**

```sh
sudo -v && tools/up.sh                 # jail, resolver, server, then the client
# or, with the server already up through tools/up.sh --keep / --no-client:
tools/run-client.sh
# ready: "== game server accepting after ~Ns", then the client window; it logs in
# with the name and password in $HOME/.config/eve-offline/login
```

**A second client, or a second character.** Make the second prefix once,
then keep the stack up and launch both:

```sh
tools/client2-setup.sh                 # once: prefix 8501, install tq-offline-2
sudo -v && tools/up.sh --keep          # first client; --keep leaves the services up
tools/run-client.sh --second           # second client, its own prefix and login
```

A second character on one account is picked on the character screen. For a
no-accounts run, `tools/up.sh --catch-all -- --character Alice --character Bob`
puts two characters there.

**A client against a server on another machine.** Here the server runs on
192.0.2.10 with ports 26000, 5222 and 8443 published, and with
`--chat-advertise 10.137.0.1` (the container's `EVE_CHAT_ADVERTISE`
default). The jail is unchanged: `tools/jail-forward.py` ends each connection
on this machine and opens a fresh one to the server host, so the jail's
forward chain never opens:

```sh
sudo tools/jail-setup.sh && sudo tools/jail-verify.sh
sudo /usr/local/sbin/eve-offline-jail dns       # the stub resolver (helper from tools/install-jail-helper.sh)
JAIL_FORWARD_TARGET=192.0.2.10 python3 -I tools/jail-forward.py 26000 5222 8443 &
tools/run-client.sh
# ready: the client reaches the login screen; stop the forwarder by its pid, then
sudo tools/jail-teardown.sh
```

**One client-CI scenario** ([client-ci.md](../guides/client-ci.md)):

```sh
tools/client-ci.sh list                # the scenarios
sudo -v && tools/client-ci.sh run undock_distance
# ready/verdict: the run prints the scenario's PASS or FAIL lines and exits 0 on PASS;
# artefacts in out/client-ci/
```

## eve-server and eve-offline-server

`eve-offline-server` is `eve-server` plus the world modules
(`server/crates/eve-offline-server`); the tools start it by default
(`EVE_SERVER_BIN` picks the plain core binary). Flags are parsed in
`server/crates/eve-server/src/cli.rs`; module flags come from
`server/crates/eve-mod-*/src`.

### Listening and accounts

| Flag | Default | Meaning | Example |
|---|---|---|---|
| `--bind LISTENER` | `127.0.0.1:26000` | a machoNet listener, repeatable: `ADDR[,ceiling=gm\|player][,cap=N][,account=UID][,chat=ADDR][,advertise=IP][,signup=on\|off][,capture=on\|off][,botnames=on\|off][,gm-public=on\|off]`; in an accounts mode a listener off loopback with the default `ceiling=gm` is refused unless it says `gm-public=on` (a GM password crosses plaintext) | `--bind 10.137.0.1:26000,ceiling=gm` |
| `--client-build N[,M]` | every build `tools/builds.tsv` marks `pinned`, `equivalent` or `provisional` on the reference line (`eve_marshal::build::accepted`) | the client builds this server answers | `--client-build <build>` |
| `--accounts FILE` | off | accounts mode from a token file (`sha256(token) user_id role name [start_character]`) | `--accounts $HOME/.config/eve-offline/accounts.tsv` |
| `--world-accounts` | off | accounts live in the `--world` file; the client logs in with `/ssoToken=USER:PASSWORD` | `--world var/world.dat --world-accounts` |
| `--signup` | off | `signup=on` for every listener (`--world-accounts` only) | `tools/up.sh --signup` |
| `--grant USERNAME ROLE` | | set an account's role (`player` or `gm`) in the `--world` file and exit; server must be stopped; must come last | `--world var/world.dat --grant Alice gm` |
| `--mark-bot USERNAME [off]` | | mark an account as a bot (or clear it) and exit; must come last | `--world var/world.dat --mark-bot hulka3` |
| `--new-token NAME ROLE [START_CHARACTER]` | | print a new account line and token, write nothing, exit | `--new-token Alice gm` |
| `--user-id N` | | the account id in session_init (no-accounts mode) | `--user-id 1` |
| `--proxy-node N` | | proxy_nodeid handed to the client | `--proxy-node 1` |
| `--character NAME[,bot][,school=N]` | one default character | add a character to the select screen; repeatable; refused with `--accounts` | `--character Alice --character Bob,school=2` |
| `--mode MODE` | `gps` | `gps` runs the handshake, `capture` answers nothing | `--mode capture` |
| `--once` | off | serve one connection, then exit | `--once` |
| `--reference-build` | | print the reference client build (`eve_marshal::build::REFERENCE`) and exit 0; `deploy/entrypoint.sh` uses it to pick `/content/<build>/` | `eve-server --reference-build` |
| `--probe ADDR` | | health check, not a server: exit 0 if ADDR greets like us; must be first | `eve-server --probe 127.0.0.1:26000` |

### World and content

What each scale and world setting changes, its range, and the numbers that have no flag yet are in [tuning.md](tuning.md).

| Flag | Default | Meaning | Example |
|---|---|---|---|
| `--world FILE` | off (fresh universe each run) | keep the world in FILE between runs | `--world var/world.dat` |
| `--accept-downgrade` | off | save over a world file holding records this build does not know (backed up first) | `--accept-downgrade` |
| `--content DIR` | none (invented fixture rows) | the extracted content bundle | `--content out/content` |
| `--authored DIR` | `content` | our authored tables; a missing file uses the compiled-in copy | `--authored content` |
| `--belt-rocks FILE` | bundle's | asteroid belt rocks (`tools/asteroids.py`), needs `--celestials` | `--belt-rocks out/rocks.tsv` |
| `--celestials FILE` | bundle's | celestial catalogue (`tools/celestials.py`) | `--celestials out/celestials.tsv` |
| `--planets FILE` | bundle's | PI static table (`tools/pi_statics.py`) | `--planets out/pi.tsv` |
| `--skills FILE` | bundle's | skill table (`tools/skills.py`) | `--skills out/skills.tsv` |
| `--typedata FILE` | bundle's | type and dogma table (`tools/typedata.py`) | `--typedata out/typedata.tsv` |
| `--dev` | off | DEV-MODE world ([dev-mode.md](../guides/dev-mode.md)); keeps `out/dev/world.dat` unless `--world` | `--dev` |
| `--time-scale N` | 1, else `EVE_TIME_SCALE` | slow game timers N times faster (1..=60) | `--time-scale 10` |
| `--training-scale N` | 1, else `EVE_TRAINING_SCALE` | skill training N times faster still (1..=10000) | `--training-scale 100` |
| `--bounty-scale X` | 1, else `EVE_BOUNTY_SCALE` | scales NPC bounties, live and offline-settled (0..=1000); the system's bounty pool drains by the unscaled amount; the client shows the unscaled bounty attributes | `--bounty-scale 2.5` |
| `--mission-scale X` | 1, else `EVE_MISSION_SCALE` | scales agent mission ISK rewards (0..=1000); the mission offer window shows the unscaled ISK | `--mission-scale 2` |
| `--npc-buy-scale X` | 1, else `EVE_NPC_BUY_SCALE` | scales what NPC buy orders pay, never their stock (0..=1000); a raised bid stops at the larger of its 1.0 price and the type's retail ask, and NPC asks keep their refine guard over the raised bids; Tier A bids only (Tier B loot bids, where LP-store items sell, stay unscaled) | `--npc-buy-scale 1.5` |
| `--insurance-scale X` | 1, else `EVE_INSURANCE_SCALE` | scales insurance payouts, not premiums (0..=1000); a raised payout stops at what a full policy pays; the quote window shows the unscaled price | `--insurance-scale 2` |
| `--isk-scale X` | 1, else `EVE_ISK_SCALE` | master: multiplies the four above (0..=1000); ISK moving between players is never scaled | `--isk-scale 3` |
| `--concord` | on | CONCORD answers high-sec crime | `--concord` |
| `--no-concord` | | a CONCORD-free world | `--no-concord` |
| `--no-undock-invuln` | | no 30 s undock/gate invulnerability | `--no-undock-invuln` |
| `--world-training` | 1 | skill training multiplier for a new world (1..=10000; changes live with `/world training`) | `--world-training 2` |
| `--world-start-isk` | 1000000 | ISK a new character starts with, in a new world (0 or more; fixed once the world is made) | `--world-start-isk 5000000` |
| `--world-start-sp` | 0 | unallocated SP a new character starts with, in a new world (0..=1000000000; fixed once the world is made) | `--world-start-sp 500000` |
| `--world-anchor-time` | 3600 | seconds an Upwell structure takes to anchor, new world (60..=604800; changes live with `/world anchortime`) | `--world-anchor-time 600` |
| `--world-delete-time` | 3600 | seconds before a character can be deleted, new world (0..=2592000; changes live with `/world deletetime`) | `--world-delete-time 60` |

### Services beside the game

| Flag | Default | Meaning | Example |
|---|---|---|---|
| `--chat` | off | in-game chat (XMPP) beside every listener, port 5222 | `--chat` |
| `--chat-advertise IP` | the listener's | the chat address clients are told; implies `--chat` | `--chat-advertise 10.137.0.1` |
| `--gateway` | off | answer the client's public-gateway (gRPC) requests on port 8443 | `--gateway` |
| `--gateway-addr IP` | beside the listeners | run the gateway on IP; implies `--gateway` | `--gateway-addr 0.0.0.0` |
| `--gateway-port N` | 8443 | the gateway's port | `--gateway-port 8444` |
| `--bot-control DIR` | the world file's directory | where the eve-bots control link writes `bot-control.addr` and `.token` | `--bot-control var` |
| `--admin ADDR` | off | the admin page and JSON API (pilots, systems (every live system, with `in_space` for pilots and `in_space_all` for all balls in space), map, `ticks` per-minute p50/p95/max (a minute is the minute a report window ended), `methods` slowest calls, `metrics` Prometheus text; the token as `X-Admin-Token` or, on GET, `Authorization: Bearer`; `tools/metrics_sample.py` polls it into a TSV) plus token-guarded actions `POST /api/action/` `bots`, `broadcast`, `save`, `kick`, each at most one a second and logged to `admin-actions.log`; token in `admin.token` | `--admin 127.0.0.1:8090` |
| `--signups-per-hour N` | `10` | with `--world-accounts`: at most N human sign-ups an hour, server-wide (and 3 an hour per IPv4 address or IPv6 /64); `0` is unlimited; listeners with `botnames=on` are exempt | `--signups-per-hour 30` |
| `--max-accounts N` | `500` | with `--world-accounts`: a human sign-up is refused once the world holds N human accounts (bot accounts do not count); `0` is unlimited | `--max-accounts 1000` |
| `--image-server ADDR` | off | a portrait image server on ADDR (plain HTTP, GET only, at most 16 connections at once), advertised to every client at login as its `imageserverurl` (`machoNet.OnGlobalConfigUpdated`), so the select screen, NEOCOM and chat show character portraits | `--image-server 127.0.0.1:26080` |
| `--image-url URL` | `http://ADDR/` | the URL clients are told for the image server, when it is reached through another address than the one it listens on | `--image-url http://my.host:26080/` |
| `--portraits DIR` | `portraits/` beside the world file | the image server's pictures, `<charID>_<size>.jpg`; a missing size is served from the largest one there is | `--portraits /path/to/Pictures/Characters` |
| `--status FILE` | `out/live/status.tsv` with `--world` | per-system TSV, rewritten every 10 s | `--status out/live/status.tsv` |
| `--stop-file PATH` | `FILE.stop` with `--world` | stop cleanly when PATH appears | `--stop-file var/up-server.stop` |
| `--capture DIR` | off | one `.evecap` per connection | `--capture captures` |

### Tuning and debugging

| Flag | Default | Meaning | Example |
|---|---|---|---|
| `--writers N` | a quarter of the cores, 1..8 | socket-writer threads | `--writers 4` |
| `--coalesce-destiny` | on | merge same-stamp DoDestinyUpdate pushes | `--coalesce-destiny` |
| `--no-coalesce-destiny` | | one frame per push | `--no-coalesce-destiny` |
| `--conn-stack-mib N` | 16 | stack of each connection thread, MiB | `--conn-stack-mib 32` |
| `--tick-threads N` | half the hardware threads | threads for the tick's parallel stages; 1 is serial | `--tick-threads 1` |
| `--turn-sched` | off | schedstat samples in `[turn-slow]` lines | `--turn-sched` |
| `--log-sync` | off | write log lines on the calling thread | `--log-sync` |
| `--scenario NAME` | off | dev only: run client-CI scenario NAME in the client | `--scenario fci_smoke` |
| `--push-probe N` | off | dev only: N inert probe notifications per connection | `--push-probe 5` |
| `--help` | | the usage text (`-h` too) | `eve-server --help` |

## eve-bots

`server/crates/eve-bot/src/bin/eve-bots.rs`. The usage text
(`eve-bots --help`) is the full reference; [dev-mode.md](../guides/dev-mode.md)
covers rosters.

| Flag | Default | Meaning | Example |
|---|---|---|---|
| `--connect IP:PORT` | required | the server, as an IP literal | `--connect 127.0.0.1:26000` |
| `--name NAME` | | one bot by character name; repeatable | `--name Miner1` |
| `--prefix P --count N` | | N bots named P1..PN | `--prefix hulka --count 10` |
| `--loop ROLE` | the default role | the role of the bots named after it (sticky) | `--loop miner` |
| `--pace human\|fast` | `human` | human reaction delays, or none (tests) | `--pace fast` |
| `--gm-setup on\|off` | `on` | a bot gives itself missing kit with GM commands | `--gm-setup off` |
| `--accept-fleet WHO` | `any` | `any`, `from:NAME`, `allied` or `none` | `--accept-fleet allied` |
| `--chat on\|off` | `on` | talk in fleet chat or Local | `--chat off` |
| `--chat-host IP:PORT` | the advertised address | dial chat here instead | `--chat-host 127.0.0.1:5222` |
| `--run SECONDS` | 0 (until killed) | stop after this long | `--run 600` |
| `--tokens FILE` | none (`dev`) | `NAME TOKEN` per line, chmod 600 | `--tokens $HOME/.config/eve-offline/bots.tsv` |
| `--agent ID` | | the agent a courier takes missions from | `--agent 3008416` |
| `--hostile` | off | allow `--loop hostile`, per `content/bots/hostile.tsv` | `--hostile` |
| `--allies FILE` | | humans the bots treat as allies | `--allies allies.tsv` |
| `--roster FILE` | | DEV-MODE roster rows (`content/bots/dev/`) | `--roster content/bots/dev/pvp.tsv` |
| `--roster-home SYSTEM` | | seat for roster rows without a system | `--roster-home Jita` |
| `--dev-status VALUE` | off | with `--roster`: each hunter-role seat sends `/secstatus VALUE` (-10..10) once at its dev setup, then re-reads its status (#29) | `--dev-status -3` |
| `--difficulty LEVEL` | `normal` | `easy`, `normal`, `hard`, `brutal` or 0..4 | `--difficulty hard` |
| `--closed` | off | the closed economy: no GM setup | `--closed` |
| `--bootstrap` | off | with `--closed`: GM setup once per connection | `--closed --bootstrap` |
| `--home SYSTEM` | | the humans' home system | `--home Jita` |
| `--bot-systems ID,..` | every system near `--home` | the systems bots live in | `--bot-systems 30000142` |
| `--content DIR` | | the content bundle (gate graph, NPC profiles, minerals) | `--content out/content` |
| `--real-fits FILE` | the bundle's sibling `bots/fits.tsv` | player fits that seed the doctrines | `--real-fits out/bots/fits.tsv` |
| `--celestials FILE` | | the celestial catalogue alone | `--celestials out/celestials.tsv` |
| `--security-floor F` | see `--help` | the least security a route passes | `--security-floor 0.5` |
| `--society` | off | every bot gets a mind and a career | `--society --state var/society` |
| `--state DIR` | | with `--society`: one state file per bot | `--state var/society` |
| `--society-seed N` | see `--help` | trait seed | `--society-seed 7` |
| `--sessions WHEN` | `daily` | `daily` or `always` | `--sessions always` |
| `--day-secs N` | see `--help` | a day lasts N seconds | `--day-secs 600` |
| `--society-pvp` | off | allow `--loop hunter` and the pirate career | `--society-pvp` |
| `--society-wars` | off | bot corps declare wars | `--society-wars` |
| `--control DIR` | | dial the server's control link (GM `/bots`) | `--control var` |
| `--bubble` | off | keep bots near humans, shed under load; needs `--control` | `--bubble --control var` |
| `--brawl` | off | fleet-fight bench behaviour | `--brawl` |
| `--login-rate R` | `2` | process login rate, logins a second (0: old 3-10 s cumulative stagger); first login = 0-10 s name jitter through the rate; also gates window starts, bubble wakes and reconnects, but not bots a GM spawns with `/bots`; 0 or at least `0.001`; ignored with `--pace fast` or `--ramp` | `--login-rate 5` |
| `--max-online N` | unlimited | at most N (>= 1) bots logged on at once (whole process; GM `/bots` spawns are not counted); a bot that finds it full waits offline; priority: bubble-pulled, fleet members, longest waiting; a dropping limit ends the longest-online sessions the ordinary way | `--max-online 300` |
| `--online-profile FILE` | none | requires `--max-online`: 24 rows `hour<TAB>fraction` (UTC hour, or the `--day-secs` day spread over 24 hours), limit = floor(N x fraction); format and `[A]` defaults in `content/bots/online-budget.tsv` | `--online-profile content/bots/online-budget.tsv` |
| `--ramp N0,K,T[,GAP_MS]` | | bench login ramp (`tools/bench-ramp.sh`) | `--ramp 10,5,30` |
| `--quiet` | off | no act lines or per-bot summary | `--quiet` |
| `--stats SECS` | off | a `[stats]` line every SECS | `--stats 60` |
| `--help` | | the usage text | `eve-bots --help` |

## Environment variables

"Read by" names one file; the scan (`python3 -I tools/env-ref.py`) prints
every file. Defaults come from the code.

### Server and its tests (`server/crates`)

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_FAST_FORWARD` | `eve-server/src/cli.rs` | unset | **refused**: if set (even empty) `eve-server` and `eve-offline-server` exit 2 with a message. Fast-forward runs only under `eve-bots --host` (#147), which is not built yet; the clock refuses every mode but wall until then | `EVE_FAST_FORWARD=20` (exits 2) |
| `EVE_TIME_SCALE` | `eve-server/src/game_clock.rs` | 1 | fallback for `--time-scale` | `EVE_TIME_SCALE=10` |
| `EVE_TRAINING_SCALE` | `eve-server/src/game_clock.rs` | 1 | fallback for `--training-scale` | `EVE_TRAINING_SCALE=100` |
| `EVE_BOUNTY_SCALE` | `eve-server/src/econ_scale.rs` | 1 | fallback for `--bounty-scale` | `EVE_BOUNTY_SCALE=2` |
| `EVE_MISSION_SCALE` | `eve-server/src/econ_scale.rs` | 1 | fallback for `--mission-scale` | `EVE_MISSION_SCALE=2` |
| `EVE_NPC_BUY_SCALE` | `eve-server/src/econ_scale.rs` | 1 | fallback for `--npc-buy-scale` | `EVE_NPC_BUY_SCALE=2` |
| `EVE_INSURANCE_SCALE` | `eve-server/src/econ_scale.rs` | 1 | fallback for `--insurance-scale` | `EVE_INSURANCE_SCALE=2` |
| `EVE_ISK_SCALE` | `eve-server/src/econ_scale.rs` | 1 | fallback for `--isk-scale` | `EVE_ISK_SCALE=2` |
| `EVE_GIT_SHA` | `eve-server/src/store.rs` | a fixed build name | names this build in the world store; the host tools set it | `EVE_GIT_SHA=$(git rev-parse HEAD)` |
| `EVE_JAIL_HOST_IP` | `eve-server/src/server.rs`, tools | `10.137.0.1` | the host end of the jail's veth pair | `EVE_JAIL_HOST_IP=10.137.0.1` |
| `EVE_PUBLIC_GATEWAY_ADDRESS` | client launch env (`tools/jail-client.sh`) | `<host>:8443` | where the client dials the public gateway; `--gateway` answers it | `EVE_PUBLIC_GATEWAY_ADDRESS=10.137.0.1:8443` |
| `EVE_CLOCK_GATE` | `eve-server/src/push.rs` | on | `0` turns the clock gate off | `EVE_CLOCK_GATE=0` |
| `EVE_CLOCK_SLICES` | `eve-server/src/push.rs` | on | `0` turns clock slicing off | `EVE_CLOCK_SLICES=0` |
| `EVE_READ_TURNS` | `eve-server/src/dispatch.rs` | on | `0` forces the write-lock path (the oracle) | `EVE_READ_TURNS=0` |
| `EVE_TURRET_APPLICATION` | `eve-server/src/combat.rs` | on | `0` turns turret application off | `EVE_TURRET_APPLICATION=0` |
| `EVE_ECON_TSV` | `eve-server/src/econ_report.rs` | off | file the economy report appends rows to (`tools/econ-report.py`); `tools/soak.sh` sets it to `econ.tsv` in its output dir | `EVE_ECON_TSV=out/econ.tsv` |
| `EVE_ECON_WATCH` | `eve-server/src/econ_report/tape.rs` | off | economy tape watch list | `EVE_ECON_WATCH=34,35` |
| `EVE_TEST_BUNDLE` | `eve-server/src/reactions.rs` and others | unset (test skipped) | real-bundle tests read this bundle | `tools/dev.sh env EVE_TEST_BUNDLE=/repo/out/content cargo test ...` |
| `EVE_BRAIN_TYPEDATA` | `eve-server/src/brain.rs` | unset (ignored test) | typedata.tsv for an ignored test | `EVE_BRAIN_TYPEDATA=/repo/out/content/types/typedata.tsv` |
| `EVE_FUZZ_SEED` | `*/tests/fuzz.rs`, `q_fuzz.rs` | fixed | fuzz seed, long mode | `EVE_FUZZ_SEED=7` |
| `EVE_FUZZ_CASES` | `*/tests/fuzz.rs`, `q_fuzz.rs` | short run | fuzz case count, long mode | `EVE_FUZZ_CASES=5000000` |
| `EVE_BOTS_KIT_DRILL` | `eve-bot/src/pvp_kit.rs` | off | CI only: the PvP kit drill | set by `tools/client-ci.sh` |
| `EVE_DNS_BIND` | `eve-dns/src/main.rs` | see `eve-dns --help` | resolver listen address | `EVE_DNS_BIND=10.137.0.1:53` |
| `EVE_DNS_TARGET` | `eve-dns/src/main.rs` | see `--help` | the address names resolve to | `EVE_DNS_TARGET=10.137.0.1` |
| `EVE_DNS_SUFFIX` | `eve-dns/src/main.rs` | `servers.eveonline.com` | suffixes answered (comma-separated) | `EVE_DNS_SUFFIX=servers.eveonline.com` |
| `EVE_DNS_TTL` | `eve-dns/src/main.rs` | `DEFAULT_TTL` | answer TTL | `EVE_DNS_TTL=60` |
| `EVE_DNS_NO_TCP` | `eve-dns/src/main.rs` | unset | set: no TCP listener | `EVE_DNS_NO_TCP=1` |
| `EVE_DNS_LOG` | `eve-dns/src/main.rs` | unset | log file for queries | `EVE_DNS_LOG=logs/dns.log` |

Not an environment variable: `EVE_SERVERS_SUFFIX` is a Rust constant in
`eve-dns/src/policy.rs`.

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_SERVERS_SUFFIX` | `eve-dns/src/policy.rs` | | constant: the suffix the resolver answers | n/a |

### Build, test and gate (`tools/dev.sh`, `tools/gate.sh`, `tools/land.sh`)

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_MAX_JOBS` | `tools/dev.sh` | all cores | cargo jobs and container CPUs | `EVE_MAX_JOBS=6 tools/dev.sh cargo test` |
| `EVE_PARITY_DIR` | `tools/export-parity.sh` | `out/parity` | scratch directory (on disk, never /tmp) for the export of HEAD that the export-parity stage runs in | `EVE_PARITY_DIR=$HOME/scratch tools/export-parity.sh` |
| `EVE_MIN_FREE_GB` | `tools/dev.sh` | 12 | wait until this much RAM is free; 0 = no wait | `EVE_MIN_FREE_GB=0` |
| `EVE_MEMINFO` | `tools/dev.sh`, `tools/host-check.sh` | `/proc/meminfo` | file read for MemTotal and MemAvailable; tests point it at a fake | `EVE_MEMINFO=/path/to/fake` |
| `EVE_DEV_MEMORY` | `tools/dev.sh`, `tools/arena.sh` | `10g` (arena `8g`) | container memory limit | `EVE_DEV_MEMORY=16g` |
| `EVE_DEV_TIMEOUT` | `tools/dev.sh` | 3600 | container kill timeout, seconds | `EVE_DEV_TIMEOUT=7200` |
| `EVE_DEBUG_INFO` | `tools/dev.sh` | `line-tables-only` | cargo debug info level | `EVE_DEBUG_INFO=full` |
| `EVE_TARGET_DIR` | `tools/dev.sh` | `server/target` | cargo target dir | `EVE_TARGET_DIR=$HOME/eve-target` |
| `EVE_ARENA_TIMEOUT` | `tools/arena.sh` | 7200 | arena container timeout | `EVE_ARENA_TIMEOUT=600` |
| `EVE_BOT_TEST_THREADS` | `tools/gate.sh` | unset | `--test-threads` for the bot socket tests | `EVE_BOT_TEST_THREADS=2` |
| `EVE_GATE_SERIAL_TESTS` | `tools/gate.sh` | unset | `1` forces serial test jobs | `EVE_GATE_SERIAL_TESTS=1` |
| `EVE_GATE_NO_RERUN` | `tools/gate.sh` | unset | set: a failed test run is never rerun, even for a test on `tools/flaky.tsv` | `EVE_GATE_NO_RERUN=1` |
| `EVE_LAND_DIR` | `tools/land.sh` | `$HOME/code/eve-offline-lanes/land` | the landing build copy (a name for a scratch copy; any directory works) | `EVE_LAND_DIR=$HOME/land` |
| `EVE_NO_POST_LAND` | `tools/land.sh` | unset | set: skip the maintainer's after-push hook for this landing (a contributor has none) | `EVE_NO_POST_LAND=1` |
| `EVE_LANES_DIR` | `tools/lanes-gc.sh` | `$HOME/code/eve-offline-lanes` | build copies ("lanes" is only a name for them) | `EVE_LANES_DIR=$HOME/lanes` |
| `EVE_LANES_IDLE_MIN` | `tools/lanes-gc.sh` | 60 | idle minutes before a lane's target/ is removed | `EVE_LANES_IDLE_MIN=120` |
| `EVE_TEST_DISPLAY_UP` | `tools/test_client_ci_parallel.py` | unset | test stub: display is up | test only |
| `EVE_TEST_FAIL` | `tools/test_client_ci_parallel.py` | unset | test stub: rows that fail | test only |
| `EVE_TEST_LIVE` | `tools/test_client_ci_parallel.py` | unset | test stub: live-process list | test only |
| `EVE_TEST_LOG` | `tools/test_client_ci_parallel.py` | unset | test stub: call log | test only |
| `EVE_TEST_SLEEP` | `tools/test_client_ci_parallel.py` | 0.5 | test stub: run time | test only |
| `EVE_PROC_DIR` | `tools/prefix-lib.sh` | `/proc` | proc root (tests give a fake) | test only |
| `EVE_REPIN_REPO` | `tools/re-pin.sh` | this checkout | another checkout (tests) | test only |
| `EVE_BUILDS_TSV` | `tools/content-lib.sh` | `tools/builds.tsv` | another build registry (tests) | test only |
| `EVE_ANALYSIS` | `tools/new-build.sh` | `tools/analysis.sh` | the analysis runner (tests) | test only |

### Client install and content (`tools/analysis.sh`, `tools/content-lib.sh`)

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_CLIENT` | `tools/analysis.sh`, `tools/run-client.sh`, `tools/content.py` | `$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/$EVE_CLIENT_DIR` | the client install (holds `start.ini`) | `EVE_CLIENT=/path/to/tq-offline` |
| `EVE_CLIENT_DIR` | `tools/analysis.sh` | `tq-offline` | install dir name inside the offline prefix | `EVE_CLIENT_DIR=tq-3619879` |
| `EVE_PATCH_REAL_CLIENT` | `tools/test_eve_patch.py` | | runs one extra test: `eve_patch.py plan` against this real install (writes nothing) | `EVE_PATCH_REAL_CLIENT=/path/to/tq-offline` |
| `EVE_RESFILES` | `tools/fsd.py`, `tools/check-resources.sh` | `ResFiles` beside the install | the ResFiles store | `EVE_RESFILES=/path/to/ResFiles` |
| `EVE_CONTENT` | `tools/content-lib.sh`, `tools/bots.sh` | `out/content` | which bundle the tools use (under `out/`) | `EVE_CONTENT=out/content-3619879` |
| `EVE_CBUILD` | `tools/content-lib.sh` (sets) | | the client's `start.ini` build, for callers | set by the lib |
| `EVE_CSTATUS` | `tools/content-lib.sh` (sets) | `legacy` | its `tools/builds.tsv` status | set by the lib |
| `EVE_CONTENT_TOOL_ARGS` | `tools/content-lib.sh` (sets) | empty | `--build B` for content.py, unverified build | set by the lib |
| `EVE_SERVER_BUILD_ARGS` | `tools/content-lib.sh` (sets) | empty | `--client-build B` for the server | set by the lib |
| `EVE_IMPORT_SRC_DIR` | `tools/import-build.sh` | `tq` | install dir in the online prefix | `EVE_IMPORT_SRC_DIR=tq` |
| `EVE_IMPORT_OUT` | `tools/import-build.sh` | `out/new-build` | provenance and pin output | `EVE_IMPORT_OUT=out/new-build` |
| `EVE_REPORT_OLD_BUNDLE` | `tools/new-build.sh` | `out/content` | `--report`: old bundle | `EVE_REPORT_OLD_BUNDLE=out/content` |
| `EVE_REPORT_NEW_BUNDLE` | `tools/new-build.sh` | `out/content-<build>` | `--report`: new bundle | `EVE_REPORT_NEW_BUNDLE=out/content-N` |
| `EVE_REPORT_OLD_PYC` | `tools/new-build.sh` | `out/pyc` | `--report`: old unpacked code | `EVE_REPORT_OLD_PYC=out/pyc` |
| `EVE_REPORT_NEW_PYC` | `tools/new-build.sh` | `out/new-build/<build>/pyc` | `--report`: new unpacked code | |
| `EVE_SURFACE_OLD_CLIENT` | `tools/port-surface.sh` | unset | old install for `--old-client` | `EVE_SURFACE_OLD_CLIENT=/path/old` |
| `EVE_SURFACE_NEW_CLIENT` | `tools/port-surface.sh` | unset | new install for `--new-client` | `EVE_SURFACE_NEW_CLIENT=/path/new` |

### Wine prefixes and the client (`tools/run-client.sh`, `tools/prefix-lib.sh`)

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_PREFIX` | `tools/run-client.sh`, `tools/client-ci.sh` | the offline prefix | the Wine prefix the client runs in | `EVE_PREFIX=$HOME/.local/share/eve-offline/prefix` |
| `EVE_PREFIX_ID` | `tools/run-client.sh` | `offline` | prefix id (`slot-N` for CI slots) | `EVE_PREFIX_ID=offline` |
| `EVE_OFFLINE_PREFIX` | `tools/offline-prefix.sh`, `tools/client-setup.sh` | `$HOME/.local/share/eve-offline/prefix` | where the offline prefix is made | `EVE_OFFLINE_PREFIX=/data/eve-prefix` |
| `EVE_OFFLINE_PREFIX_SET` | `tools/client-setup.sh` | | internal: the user set `EVE_OFFLINE_PREFIX` | internal |
| `EVE_ONLINE_PREFIX` | `tools/offline-prefix.sh`, `tools/import-build.sh` | Steam `compatdata/$EVE_ONLINE_PREFIX_ID` | the Steam (online) prefix | `EVE_ONLINE_PREFIX=$HOME/.steam/steam/steamapps/compatdata/8500` |
| `EVE_ONLINE_PREFIX_ID` | `tools/prefix-lib.sh` | `8500` | Steam app id of the online prefix | `EVE_ONLINE_PREFIX_ID=8500` |
| `EVE_REL` | `tools/client-setup.sh` | `pfx/drive_c/CCP/EVE` | internal: EVE dir inside a prefix | internal |
| `EVE_CLIENT_ARGS` | `tools/run-client.sh` | | extra client arguments | `EVE_CLIENT_ARGS="/noconsole"` |
| `EVE_SSO_TOKEN` | `tools/run-client.sh` | from `$HOME/.config/eve-offline/login` | the `/ssoToken=` the client logs in with | `EVE_SSO_TOKEN=Alice:secret` |
| `EVE_CLIENT_FILE_LOG` | `tools/run-client.sh` | `on` | `off` drops the client file log | `EVE_CLIENT_FILE_LOG=off` |
| `EVE_CHAT_VERBOSE` | `tools/run-client.sh` | 0 | `1` sets xmppChatVerbose in prefs | `EVE_CHAT_VERBOSE=1` |
| `EVE_LOG_HOST` | `tools/run-client.sh` | `$EVE_PREFIX/pfx/drive_c/eve-logs` | internal: host side of the client log dir | internal |
| `EVE_LOG_WINDOWS` | `tools/run-client.sh` | `C:\eve-logs` | internal: client side of the log dir | internal |
| `EVE_OFFLINE_ALLOW_REAL_CLIENT` | `tools/run-client.sh` | 0 | `1` allows launching the real `tq` install (do not) | |
| `EVE_OFFLINE_STOCK_CRYPTO` | `tools/run-client.sh` | 0 | `1`: stock crypto pack, diagnostic only (`up.sh --stock-crypto`) | |
| `EVE_CLIENT_SENTRY_DSN` | `tools/jail-client.sh` | `http://0@127.0.0.1:9/0` | client launch env: crash reports go nowhere | |
| `EVE_SECOND_SRC_DIR` | `tools/client2-setup.sh` | `tq-offline` | source install for the second client | `EVE_SECOND_SRC_DIR=tq-offline` |
| `EVE_SECOND_CLIENT_DIR` | `tools/client2-setup.sh`, `tools/client-ci.sh` | `tq-offline-2` | the second client's install dir | |
| `EVE_SECOND_PREFIX_ID` | `tools/client-ci.sh` | `8501` | the second client's prefix id | |
| `EVE_SECOND_DELAY` | `tools/client-ci.sh` | 5 | seconds before the second client starts | `EVE_SECOND_DELAY=10` |
| `EVE_LOGIN_FILE` | `tools/login-lib.sh` | `$HOME/.config/eve-offline/login` | the login file | `EVE_LOGIN_FILE=$HOME/.config/eve-offline/login2` |
| `EVE_LOGIN_NAME_RE` | `tools/login-lib.sh` | | constant: valid account names | internal |
| `EVE_LOGIN_TOOL_TOKENS` | `tools/login-lib.sh` | | constant: tool tokens refused as passwords | internal |

### The jail (`tools/jail-*.sh`, `tools/up.sh`)

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_JAIL_NS` | `tools/jail-setup.sh` | `eveoffline` | network namespace name | |
| `EVE_JAIL_NS_IP` | `tools/jail-setup.sh` | `10.137.0.2` | namespace end of the veth pair | |
| `EVE_JAIL_HOST_IF` | `tools/jail-teardown.sh` | `eve-host` | host end interface name | |
| `EVE_JAIL_PORTS` | `tools/jail-setup.sh` | `26000,5222,8443` | ports the jail may reach | `sudo EVE_JAIL_PORTS=26000,26001,26002 tools/jail-setup.sh` |
| `EVE_JAIL_PROBE_GW` | `tools/jail-verify.sh` | the default gateway | LAN gateway the egress probe aims at | |
| `EVE_JAIL_PROBE_SELF` | `tools/jail-verify.sh` | this host's LAN address | address the egress probe aims at | |
| `EVE_JAIL_HELPER` | `tools/install-jail-helper.sh` | the installed path | the root helper (tests give a fake) | |
| `EVE_BOOT_WAIT` | `tools/up.sh`, `tools/jail-rootless.sh` | 120 | seconds to wait for the server to accept | `EVE_BOOT_WAIT=300` |
| `EVE_STOP_TIMEOUT` | `tools/stop-server.sh` | 30 | seconds before SIGTERM after a stop file | `EVE_STOP_TIMEOUT=60` |
| `EVE_CAPTURE_DIR` | `tools/up.sh`, `tools/evidence.sh` | `captures` | where `--capture` writes | |
| `EVE_SERVER_BIN` | `tools/up.sh`, `tools/account.sh` | `eve-offline-server` | server binary name | `EVE_SERVER_BIN=eve-server` |
| `EVE_RL_SLOT` | `tools/jail-rootless.sh` | unset | rootless jail: CI slot | set by `tools/client-ci.sh` |
| `EVE_RL_APPARMOR_SYSCTL` | `tools/jail-rootless.sh` | the AppArmor userns sysctl | rootless jail: sysctl path (tests) | test only |
| `EVE_RL_CENSUS` | `tools/jail-rootless.sh` | 0 | internal: census mode | internal |
| `EVE_RL_CMD_LOG` | `tools/jail-rootless.sh` | `eve-server.log` | internal: log name of the jailed command | internal |
| `EVE_RL_DNS` | `tools/jail-rootless.sh` | | internal: `relay` DNS mode | internal |
| `EVE_RL_GW` | `tools/jail-rootless.sh` | | internal: gateway to probe | internal |
| `EVE_RL_LAN` | `tools/jail-rootless.sh` | | internal: LAN address to probe | internal |
| `EVE_RL_PRUNE` | `tools/jail-rootless.sh` | | internal: socket scan prune list | internal |
| `EVE_RL_ROOTS` | `tools/jail-rootless.sh` | | internal: socket scan roots | internal |
| `EVE_RL_UID` | `tools/jail-rootless.sh` | | internal: expected uid in the view | internal |
| `EVE_RL_WANT` | `tools/jail-rootless.sh` | | internal: socket allowlist | internal |

### Dev world, bots and hosting

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_DEV_PORT` | `tools/dev-world.sh` | 26000 | dev world port on 127.0.0.1 | `EVE_DEV_PORT=26010` |
| `EVE_DEV_VAR` | `tools/dev-world.sh` | `var/dev` | dev world dir | `EVE_DEV_VAR=var/dev2` |
| `EVE_DEV_BOT_CAP` | `tools/dev-world.sh` | 2000 | listener cap for bots | |
| `EVE_DEV_BOT_TOKENS` | `tools/dev-world.sh` | `$HOME/.config/eve-offline/dev-bots.tsv` | dev bot passwords | |
| `EVE_BOT_TOKENS` | `tools/bots.sh`, `tools/account.sh` | `$HOME/.config/eve-offline/bots.tsv` | bot passwords | |
| `EVE_BOTS_NICE` | `tools/host-bots.sh` | 10 | nice level of the bots | |
| `EVE_BIN_DIR` | `tools/host-bots.sh` | `server/target/release` | binaries | |
| `EVE_VAR_DIR` | `tools/host-bots.sh`, `tools/host-backup.sh` | `var` | world dir | |
| `EVE_LOGS_DIR` | `tools/host-bots.sh` | `logs` | log dir | |
| `EVE_BACKUPS_KEPT` | `tools/host-backup.sh` | 48 | backups kept | |
| `EVE_BACKUP_RSYNC` | `tools/host-backup.sh` | unset | off-disk backup target, `user@host:dir/` | `EVE_BACKUP_RSYNC=backup@your-backup-host:eve/` |
| `EVE_REMOTE_ADVERTISE` | `tools/serve.sh` | the jail host address | address remote clients are told | `EVE_REMOTE_ADVERTISE=127.0.0.1` |
| `EVE_WEBHOOK_FILE` | `tools/serve.sh` | `$HOME/.config/eve-offline/webhook` | optional file holding a notification URL; absent: no notifications | |
| `EVE_TUNNEL_DIR` | `tools/tunnel.sh` | see the script | the sshd container's state | |
| `EVE_TUNNEL_KEY` | `tools/remote-tunnel.sh` | `$HOME/.ssh/eve-offline` | a remote player's ssh key | |
| `EVE_OWNER_SSH_USER` | `tools/remote-tunnel.sh` | your user name | the account on your server host | |
| `EVE_BENCH_OUT` | `tools/bench-ramp.sh` | `out` | bench output root | |
| `EVE_BENCH_VAR` | `tools/bench-ramp.sh` | `var/bench` | bench world dir | |
| `EVE_BENCH_TOKENS` | `tools/bench-ramp.sh` | `$HOME/.config/eve-offline/bench-bots.tsv` | bench bot passwords | |
| `EVE_BENCH_SERVER_ARGS` | `tools/bench_ramp.py` | empty | extra server flags | `EVE_BENCH_SERVER_ARGS="--tick-threads 1"` |
| `EVE_BENCH_BOT_ARGS` | `tools/bench_ramp.py` | empty | extra bot flags | `EVE_BENCH_BOT_ARGS=--brawl` |

### Soak roster (`tools/soak.sh`)

`tools/soak.sh --help` is the authoritative text. These are the PvP options
(BP-SOAK, #12); a row here starts with the script name, so `tools/env-ref.py`
does not read it as an `eve-bots` flag.

| Flag | Default | Meaning | Example |
|---|---|---|---|
| `tools/soak.sh --hunters N` | 0 | N bots named `HunterN` play `--loop hunter` (tackle, counters, probes, fields, bubble holds); implies `--society-pvp`; needs `out/content` | `tools/soak.sh --hunters 10 --miners 5 --escorts 0 --secs 600` |
| `tools/soak.sh --society-pvp` | off | passes `--society-pvp` to `eve-bots` (with `--society` the pirate career plays the hunts); after the verdict prints `tools/pvp-report.py`'s PASS/FAIL line per PvP behaviour, never judged | `tools/soak.sh --society --society-pvp --secs 600` |
| `tools/soak.sh --dictors N` / `--hics N` / `--foes N` | 0 / 0 / 4 | the dev roster (#136): DictorN in Sabres, HicN in Devoters, FoeN in Rifters; any one selects it; needs `--roster-home` and `out/content` | `tools/soak.sh --dictors 2 --hics 2 --hunters 4 --foes 4 --roster-home T22-QI --secs 1200` |
| `tools/soak.sh --roster-home SYSTEM` | none | the roster's home; refused unless null-sec or wormhole (security <= 0 in `out/content/universe/systems.tsv`, by name or id; unchecked with a note when the bundle is missing) | `tools/soak.sh --dictors 2 --roster-home T22-QI` |
| `tools/soak.sh --kit-drill smartbomb\|mjd` | off | sets `EVE_BOTS_KIT_DRILL` inside the container; `mjd` is refused with `--dictors`/`--hics` | `tools/soak.sh --hunters 4 --foes 4 --roster-home T22-QI --kit-drill mjd` |
| `tools/soak.sh --hunter-status VALUE` | off | passes `--dev-status VALUE` to `eve-bots`: each hunter sends `/secstatus VALUE` once at its dev setup (#29) | `tools/soak.sh --hunters 4 --foes 4 --roster-home T22-QI --hunter-status -3` |

### Container deploy (`deploy/entrypoint.sh`, `deploy/stack.yml`)

`deploy/` is not in the public export, so `tools/env-ref.py` treats these
rows as optional.

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_GM_ACCOUNT` | `deploy/entrypoint.sh` | unset | account to make GM before the world opens | `EVE_GM_ACCOUNT=Alice` |
| `EVE_CHAT_ADVERTISE` | `deploy/entrypoint.sh` | `10.137.0.1` | chat address told to clients | |
| `EVE_ADMIN_ADDR` | `deploy/entrypoint.sh` | off | admin page (`--admin`) | `EVE_ADMIN_ADDR=0.0.0.0:8090` |
| `EVE_ADMIN_WEB_LIVE` | `tools/test_admin_web.py` | unset (test skipped) | `1` builds the server and runs the admin web check against a dev server in Docker | `EVE_ADMIN_WEB_LIVE=1 python3 -I tools/test_admin_web.py` |
| `EVE_SERVER_ARGS` | `deploy/entrypoint.sh` | empty | extra server flags | `EVE_SERVER_ARGS="--time-scale 2"` |
| `EVE_BOT_ARGS` | `deploy/entrypoint.sh` | `--brawl` | extra bot flags | |
| `EVE_BOT_CHAT` | `deploy/entrypoint.sh` | `on` | whether bots talk | `EVE_BOT_CHAT=off` |
| `EVE_BOT_CHAT_HOST` | `deploy/entrypoint.sh` | `127.0.0.1:5222` | where bots dial chat | |
| `EVE_CONTENT_ROOT` | `deploy/entrypoint.sh` | `/content` | test only: the content root the entrypoint resolves `<root>/<reference build>/` under (the deploy entrypoint test, maintainer side only) | |
| `EVE_RETIRE_REPO` | `tools/retire-build.sh` | this checkout | test only: the checkout whose `tools/builds.tsv` and `out/` the retire tool reads (`tools/test_retire_build.py`) | |
| `EVE_BOT_NICE` | `deploy/entrypoint.sh` | `10` | niceness of the bots process | `EVE_BOT_NICE=15` |
| `EVE_BOT_CPUS` | `deploy/stack.yml` | `6` | CPU limit of the bots container (`cpus`) | `EVE_BOT_CPUS=4` |
| `EVE_BOT_MAX_ONLINE` | `deploy/entrypoint.sh` | unset (unlimited) | `--max-online`: bots logged on at once | `EVE_BOT_MAX_ONLINE=500` |
| `EVE_BOT_LOGIN_RATE` | `deploy/entrypoint.sh` | unset (eve-bots' 2 a second) | `--login-rate`: logins a second | `EVE_BOT_LOGIN_RATE=4` |

### Client CI (`tools/client-ci*.sh`, `tools/client-headless.sh`)

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_CI_SLOT` | `tools/client-ci.sh` | 0 | parallel slot number | `EVE_CI_SLOT=2 tools/client-ci.sh ...` |
| `EVE_CI_JOBS` | `tools/client-ci-regress.sh` | 3 | clients at once | `EVE_CI_JOBS=2` |
| `EVE_CI_SCENARIOS` | `tools/client-ci.sh` | `tools/client-ci/scenarios` | scenario dir | |
| `EVE_CI_CONTENT` | `tools/client-ci-regress.sh` | `out/content` | bundle for scenario lookups | |
| `EVE_CI_FWSTATION` | `tools/client-ci-regress.sh` | computed | station for `fci_fw_enlist` | |
| `EVE_CI_ACCOUNT_PORT` | `tools/client-ci.sh` | 26093 | accounts listener port (+slot) | |
| `EVE_CI_PILOT_TOKENS` | `tools/client-ci.sh` | see the script | `--with-bots` pilot logins | |
| `EVE_CI_RUNNER_LOCK` | `tools/client-ci/slot-lib.sh` | `out/client-ci/slots/runner.lock` | runner lock file | |
| `EVE_CI_RUNNER_LOCK_HELD` | `tools/client-ci/slot-prefix.sh` | unset | internal: lock already held | internal |
| `EVE_CI_CLIENT_CI` | `tools/client-ci-regress.sh` | `tools/client-ci.sh` | runner (tests) | test only |
| `EVE_CI_HEADLESS` | `tools/client-ci-regress.sh` | `tools/client-headless.sh` | display tool (tests) | test only |
| `EVE_CI_QUIET` | `tools/client-ci-regress.sh` | `tools/client-ci/quiet-settings.py` | quiet-settings tool (tests) | test only |
| `EVE_CI_SLOT_PREFIX` | `tools/client-ci-regress.sh` | `tools/client-ci/slot-prefix.sh` | slot prefix tool (tests) | test only |
| `EVE_SLOT_HOME` | `tools/client-ci/slot-prefix.sh` | `$HOME` | home for slot prefixes | |
| `EVE_SLOT_SRC_PREFIX` | `tools/client-ci/slot-prefix.sh` | the offline prefix | source prefix slots copy | |
| `EVE_CHARID_BASE` | `tools/client-ci.sh` | 90000001 | first character id | |
| `EVE_WAIT_MS` | `tools/client-ci/scenarios/fci_prog_pve.py` | 15000 | constant in a scenario: wait budget | n/a |
| `EVE_CENSUS` | `tools/client-ci.sh`, census scenarios | unset | `1` in a census run's launch env | set by `--census` |
| `EVE_CENSUS_CONTROL` | `tools/client-ci.sh` | unset | `1`: census control copy | set by `--census-control` |
| `EVE_CENSUS_NOENV` | `tools/client-ci.sh` | unset | `1`: census run in the no-extra-env role | set by the launcher |
| `EVE_HEADLESS_DISPLAY` | `tools/client-headless.sh` | 99 | X display number | |
| `EVE_HEADLESS_SCREEN` | `tools/client-headless.sh` | `1920x1080x24` | Xvfb screen | |
| `EVE_HEADLESS_DESKTOP` | `tools/client-headless.sh` | `1600x900` | Wine desktop size | |
| `EVE_DISPLAY_IMAGE` | `tools/client-headless.sh` | `eve-offline/display:latest` | display container image | |
| `EVE_DISPLAY_NAME` | `tools/client-headless.sh` | `eve-headless-display` | display container name | |
| `EVE_LAVAPIPE_ICD` | `tools/client-headless.sh` | `/usr/share/vulkan/icd.d/lvp_icd.json` | software Vulkan ICD | |
| `EVE_RADV_ICD` | `tools/client-ci.sh` | `/usr/share/vulkan/icd.d/radeon_icd.json` | GPU Vulkan ICD | |
| `EVE_SMOKE_CLIENT_DIR` | `tools/client-headless.sh` | `tq-offline-2` | smoke test install | |
| `EVE_SMOKE_PREFIX_ID` | `tools/client-headless.sh` | `8501` | smoke test prefix | |
| `EVE_SMOKE_SECONDS` | `tools/client-headless.sh` | 75 | smoke test length | |

### Issue tracker and status page (`tools/tracker.py`, `tools/pr_overlap.py`, `tools/next-task.sh`, `tools/status-page.py`)

| Name | Read by | Default | Meaning | Example |
|---|---|---|---|---|
| `EVE_TRACKER` | `tools/tracker.py`, `tools/pr_overlap.py` | from the `upstream` remote, else `origin` | tracker kind: `github` or `gitea` | `EVE_TRACKER=github` |
| `EVE_TRACKER_API` | `tools/tracker.py`, `tools/pr_overlap.py` | from the same remote's host | tracker REST base | `https://api.github.com` |
| `EVE_TRACKER_REPO` | `tools/tracker.py`, `tools/pr_overlap.py` | from the same remote's path; a GitHub fork resolves to its source repo (cached in the git common dir as `eve-tracker`) | `owner/name` of the board | `EVE_TRACKER_REPO=someone/space-offline` |
| `EVE_TRACKER_TOKEN` | `tools/tracker.py`, `tools/pr_overlap.py` | unset (read-only on a public repo) | API token, set by the human in the environment; never pasted to an agent | |
| `EVE_TRACKER_TOKEN_FILE` | `tools/tracker.py`, `tools/pr_overlap.py` | unset | file holding that token, instead of the variable | `$HOME/.config/tracker-token` |
| `GH_TOKEN` | `tools/tracker.py`, `tools/pr_overlap.py` | unset | GitHub token, read after `EVE_TRACKER_TOKEN` and its file, and only when the API is `https://api.github.com`; set by the human, e.g. `export GH_TOKEN=$(gh auth token)`; never pasted to an agent | |
| `GITHUB_TOKEN` | `tools/tracker.py`, `tools/pr_overlap.py` | unset | the same, read after `GH_TOKEN` | |
| `EVE_ISSUES_URL` | `tools/status-page.py` | unset: no issue column | issue URL prefix; set, the status page links issue numbers | `https://github.com/someone/space-offline/issues/` |
