# Documentation index

Every doc under `docs/`, by role, with its status. `tools/test_docs.py` fails if a
doc has no row here (flat lane notes are the exception: they are not published). Start at
`AGENTS.md`; this page is where you find the rest.

Some code comments and older docs cite design notes that are not published (lane notes,
plans, the archive, the port tracker). The code and the docs listed here are the
reference.

**Status:** `current` (checked against the code or newer than the code it describes),
`stale-in-part` (some statements are contradicted by the code or a later doc; the doc says
which, or the row does), `historical` (a finished piece of work, kept as history),
`superseded` (replaced by the doc named in its banner). **Role:** `HISTORY` rows carry a
`> **Demoted` banner in their first lines (the lint checks it).

## Where to start

| you want to | read |
|---|---|
| contribute, alone or beside other people's agents | `AGENTS.md` (the short guide), then `docs/guides/contributing.md` |
| share the tree with other people's agents | `docs/guides/working-together.md` |
| know what is built and proved | `docs/status/progress.tsv`, the feature index (page: `docs/status/README.md`) |
| know what comes next, and who has what | the issue tracker: `prio/*` labels, the milestone; open PRs (`tools/next-task.sh` `prs` column) |
| keep up with a new Tranquility client build | `docs/guides/tracking-tq.md`, then `docs/guides/build-compat.md` |
| run the server, the client, bots | `docs/guides/getting-started.md` (zero to playing), `docs/reference/launch-and-env.md` (every flag and variable), `README.md`, `docs/guides/dev-mode.md`, `docs/guides/jail.md`, `docs/guides/windows.md` (the native Windows client), `docs/guides/contributing.md` |
| test what only the client can show | `docs/guides/client-ci.md` |
| debug the client against the server | `docs/guides/debugging-the-client.md` |
| run a debug world where nothing waits | `docs/guides/dev-mode.md` |
| use the GM commands | `docs/guides/gm-commands.md` |
| change the wire or marshal code | `docs/reference/framing.md`, `docs/reference/dbrow.md`, `docs/reference/object-wrapper.md` |
| add content tables | `docs/content/policy.md`, `docs/content/schema.md`, `content/README.md` |
| add a client fact | `docs/re/` (and mark `[V]`, `[I]` or `[A]` as the rest of the tree does) |
| find any other doc | `docs/README.md`, this index |

## Layout

```
AGENTS.md                 the short guide for contributors and their agents (CLAUDE.md imports it)
README.md                 the front page
docs/README.md            this index
docs/guides/              how to run, test and operate
docs/reference/           how the protocol and the server work now
docs/content/             the content policy and schema, and what only the server holds
docs/status/              feature state (progress.tsv) and the page generated from it
docs/re/                  reverse-engineering facts about the client
docs/eden/                how we use the Eden emulator: what to trust, what never to copy
findings/                 tables generated from the client (TSV and text), cited by the docs
```

## Guides: how to run, test and operate

| doc | role | status | covers |
|---|---|---|---|
| `docs/guides/issue-tracking.md` | GUIDE | current | The two issue boards, why labels grant nothing, and why issue text is data for agents. |
| `docs/guides/dev-mode.md` | GUIDE | current | Dev mode: a debug world where nothing waits for training, building or income. |
| `docs/guides/wing.md` | GUIDE | current | The wing: bots that fly with you; the `/bots` and fleet-chat words, with the not-yet ones marked. |
| `docs/guides/bench-ramp.md` | GUIDE | current | Bench ramp: ramp bots into one system with its own server and find the knee (`tools/bench-ramp.sh`). |
| `docs/guides/jail.md` | GUIDE | current | The jail: tools that network-isolate the client; what it protects and what it does not. |
| `docs/guides/client-ci.md` | GUIDE | current | Client CI in one page: run a scenario against the real client, exit codes, where the pieces are. |
| `docs/guides/getting-started.md` | GUIDE | current | Zero to playing: build, content bundle, server, client, bots, tests and the gate, with launch examples. |
| `docs/guides/content-bundle.md` | GUIDE | current | The content pipeline: inputs, steps, outputs, verify, rebuild for a new build. |
| `docs/guides/contributing.md` | GUIDE | current | The reference behind `AGENTS.md`: Docker, the bundle, tests, the gate, branch, PR, review; the labels (§8) and the areas and hot files (§10). |
| `docs/guides/working-together.md` | GUIDE | current | How several people's agents share one tree: overlap checks, the open-PR cap, the layering rule, generated files. |
| `docs/guides/gm-commands.md` | GUIDE | current | The GM slash commands and the server flags. |
| `docs/guides/hosting.md` | GUIDE | current | Hosting on a dedicated box: frozen bundle, the owner listener, host bots, backups, wipe. |
| `docs/guides/windows.md` | GUIDE | current | The native Windows client on one PC, the server in WSL2: the patcher, `tools/windows_launch.py`, and no jail. |
| `docs/guides/patcher.md` | GUIDE | current | Preparing a client copy: `tools/eve_patch.py` and its per-build manifests. |
| `docs/guides/census.md` | GUIDE | current | The census: proving the client phones nobody. |
| `docs/guides/build-compat.md` | GUIDE | current | Adding, checking and retiring a client build (`tools/new-build.sh`); the provisional status and the re-pin cadence. |
| `docs/guides/tracking-tq.md` | GUIDE | current | Tracking Tranquility: detect a new build, triage it (T0 to T3), cut over, take in its content and code changes, and measure the lag (`tools/tq-lag.py`). Some tools it names are planned and marked so. |
| `docs/guides/debugging-the-client.md` | GUIDE | current | The method for debugging the real client against our server: instruments, traps, tooling. |

## Reference: how the protocol and the server work (current truth about this client build)

| doc | role | status | covers |
|---|---|---|---|
| `docs/reference/dependencies.md` | REFERENCE | current | The crate policy: how crates reach the build, the `server/deps.allow` table, what `tools/check_deps.py` enforces, admission criteria, accepted licences. |
| `docs/reference/phone-home.md` | REFERENCE | current | Every path by which the client phones home (native, Python, Wine), when it fires, what it sends, what closes it; what goes to our server by design. The pinned build in `tools/builds.tsv`. |
| `docs/reference/server.md` | REFERENCE | stale-in-part | What `eve-server` does; §§1-7 are the 2026-10-19..22 state (status note at the top), §8 lists services, flags and GM commands. |
| `docs/reference/world-state.md` | REFERENCE | current | Durable world state: STORE_FORMAT, delta kinds, the backup, the upgrade policy (the rest of the file is history). |
| `docs/reference/launch-and-env.md` | REFERENCE | current | Every server and eve-bots flag and `EVE_*` variable, with launch examples; checked by `tools/env-ref.py`. |
| `docs/reference/tuning.md` | REFERENCE | current | Every multiplier, scale and rate an operator can or cannot set: the seven scales, the `/world` settings, live GM commands, authored tables, bot settings, and the constants compiled in, each with its range, default and source. |
| `docs/reference/threads-and-locks.md` | REFERENCE | current | Threads, the world lock and the wire lock: the short picture, then the precise rules. |
| `docs/reference/framing.md` | REFERENCE | current | machoNet TCP framing, from disassembly. |
| `docs/reference/dbrow.md` | REFERENCE | current | `blue.DBRow` packed-row layout; supersedes the DBRow gap in the archived wire-format doc. |
| `docs/reference/object-wrapper.md` | REFERENCE | current | The object wrapper codes 0x17, 0x22, 0x23 and the 0x2D terminator. |
| `docs/reference/push-path.md` | REFERENCE | current | The server-initiated push path (spine step D). |
| `docs/reference/push-events.md` | REFERENCE | current | The 3,177 push handlers the client already has, and how the push-event inventory was made. |
| `docs/reference/bound-objects.md` | REFERENCE | current | Bound objects: the client's second call style, and how the server mints them. |
| `docs/reference/code-push.md` | REFERENCE | current | The two answers that hand the client executable code (CPython marshal, not our marshal). |
| `docs/reference/session-roles.md` | REFERENCE | current | The session object and the role mask: what unlocks the dev and GM tools. |
| `docs/reference/client-call-surface.md` | REFERENCE | current | The census of every machoNet call site in the client. |
| `docs/reference/client-architecture.md` | REFERENCE | current | The real client mapped from the server's side (208 KB; opens with a build-number provenance note). |
| `docs/reference/destiny-reference-blob.md` | REFERENCE | current | Getting a reference Destiny blob out of the client (`tools/destiny-ref.sh`). |
| `docs/reference/item-spawn.md` | REFERENCE | current | The item store and the GM `/spawn` spec, from the client's bytecode. |
| `docs/reference/market-browse.md` | REFERENCE | current | The market browse tree: why it was empty and what the server can do. |
| `docs/reference/skill-system.md` | REFERENCE | current | The skill system, server side: what the client asks and what each answer must look like. |
| `docs/reference/wallet-ledger.md` | REFERENCE | current | The wallet ledger spec (account transaction history). |

## Plans and design: what we are building and why

| doc | role | status | covers |
|---|---|---|---|
| `docs/status/README.md` | PLAN | current | The status page: percentages, graphs and a checklist per system. Generated by `tools/status-page.py` from `docs/status/progress.tsv`; never edit it by hand. |
| `docs/content/schema.md` | REFERENCE | current | The content schema: every authored table's header line, frozen. |
| `docs/content/policy.md` | REFERENCE | current | The content policy: what may be committed, the three classes of data, and where authored tables live. |
| `docs/content/server-only.md` | REFERENCE | current | What only the server holds (placement, pointed-at tables, money numbers), each item classed derivable, design or capture-only. |

## Reverse-engineering facts about the client (measured on the pinned build in `tools/builds.tsv`, or on the build before the re-pin of 2026-10-31)

| doc | role | status | covers |
|---|---|---|---|
| `docs/re/dogma.md` | RE | current | Dogma: the attribute and effect system. |
| `docs/re/dynamic-items.md` | RE | current | Dynamic items (mutaplasmids): the two calls, the attribute contract, the static table, the item rules and the build split. |
| `docs/re/modules.md` | RE | current | Activating a fitted module. |
| `docs/re/targeting.md` | RE | current | Target locking, as an implementation spec. |
| `docs/re/damage-death.md` | RE | current | Taking damage and dying, as an implementation spec. |
| `docs/re/warp.md` | RE | stale-in-part | Warp as an implementation spec; the align-phase section is superseded (banner at the top). |
| `docs/re/jump-gates.md` | RE | current | A stargate jump, end to end. |
| `docs/re/npc-behaviors.md` | RE | current | NPC behaviours and what it takes for a bot to be a player. |
| `docs/re/agents-missions.md` | RE | current | Agents and missions from the client's side. |
| `docs/re/mission-model.md` | RE | current | The mission model: what a server implements one real mission from. |
| `docs/re/dungeon-model.md` | RE | current | The dungeon model, `dungeons.fsdbinary` read end to end. |
| `docs/re/static-corpus.md` | RE | stale-in-part | The static corpus `res:/staticdata/`; §6.1 offsets are superseded by `dungeon-model.md` (banner). |
| `docs/re/native-loaders.md` | RE | current | The native cFSD loaders: every table's record layout. |
| `docs/re/node-graphs.md` | RE | current | Node graphs: the 32-byte parameter variant of the node-graph tables, decoded and proved (#206). |
| `docs/re/homefront.md` | RE | current | Homefront operations: the context keys, messages and state names the objective graphs read (#206). |
| `docs/re/localization.md` | RE | current | The localisation corpus: format, lookup path, contents. |
| `docs/re/errors.md` | RE | current | The client's error vocabulary. |
| `docs/re/client-callable.md` | RE | current | The server-to-client call path and the `__exportedcalls__` catalogue. |
| `docs/re/session-and-binding.md` | RE | current | Session and binding from the client's side. |
| `docs/re/destiny-ball-crosscheck.md` | RE | current | The ball record cross-checked against a second implementation; current truth for the ball table. |
| `docs/re/evejs-crosscheck.md` | RE | current | EvEJS claims cross-checked against the client; current truth over the archived EvEJS reference. |
| `docs/re/proving-grounds.md` | RE | current | Proving Grounds: `pvpFilamentMgr`, the event model, queue errors and the arena pocket, with a build plan (#63). |
| `docs/re/pochven.md` | RE | current | Pochven: the sealed region, Triglavian filaments and wormholes, standings rules and Local, with a build plan (#72). |
| `docs/re/sov-gateway.md` | RE | current | The sovereignty gateway: hub upgrades, reagents, workforce, skyhooks and mercenary dens on the wire, with a build plan (#77). |

## Eden facts (MIT, a different client build: facts freely, never its data)

| doc | role | status | covers |
|---|---|---|---|
| `docs/eden/README.md` | EDEN | current | How we use Eden: the rule, the files never to copy, and how far to trust it by area. |
