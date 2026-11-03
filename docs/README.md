# Documentation index

Every doc under `docs/`, by role, with its status. `tools/test_docs.py` fails if a
doc has no row here (flat lane notes are the exception: they are in
`docs/lanes/INDEX.md`, which is private). Start at `AGENTS.md`; this page is where you find the rest.

**Status:** `current` (checked against the code or newer than the code it describes),
`stale-in-part` (some statements are contradicted by the code or a later doc; the doc says
which, or the row does), `historical` (a finished piece of work, kept as history),
`superseded` (replaced by the doc named in its banner). **Role:** `HISTORY` rows carry a
`> **Demoted` banner in their first lines (the lint checks it) and live in `docs/archive/`.

## Where to start

| you want to | read |
|---|---|
| contribute, alone or beside other people's agents | `AGENTS.md` (the procedure), then `docs/guides/contributing.md` |
| share the tree with other people's agents | `docs/guides/working-together.md` |
| know what is built and proved | `docs/status/progress.tsv`, the feature index (page: `docs/status/README.md`) |
| know what comes next, and who has what | the issue tracker: `prio/*` labels, the milestone, the assignee |
| run the server, the client, bots | `docs/guides/getting-started.md` (zero to playing), `docs/reference/launch-and-env.md` (every flag and variable), `README.md`, `docs/guides/dev-mode.md`, `docs/guides/jail.md`, `docs/guides/contributing.md` |
| test what only the client can show | `docs/guides/client-ci.md` |
| debug the client against the server | `docs/guides/debugging-the-client.md`, `docs/reference/login-path.md` |
| run a debug world where nothing waits | `docs/guides/dev-mode.md` |
| use the GM commands | `docs/guides/gm-commands.md` |
| change the wire or marshal code | `docs/reference/framing.md`, `docs/reference/dbrow.md`, `docs/reference/object-wrapper.md` |
| add content tables | `docs/content/plan.md`, `docs/content/schema.md`, `content/README.md` |
| add a client fact | `docs/re/` (and mark `[V]`, `[I]` or `[A]` as the rest of the tree does) |
| find any other doc | `docs/README.md`, this index |

## Layout

```
AGENTS.md                 the procedure for contributors and their agents (CLAUDE.md imports it)
README.md                 the front page
docs/README.md            this index
docs/guides/              how to run, test and operate
docs/reference/           how the protocol and the server work now
docs/plans/               the master plan, and the parked public-release plans (private)
docs/content/, review/    the closed content and improvement plans, research, rulings
docs/status/              feature state (progress.tsv) and the page generated from it
docs/re/                  reverse-engineering facts about the client
docs/evejs/               EvEJS facts (AGPL: facts only)
docs/archive/             demoted and finished documents, the old plans among them
findings/                 generated censuses (TSV and text), cited by the docs
```

## Guides: how to run, test and operate

| doc | role | status | covers |
|---|---|---|---|
| `docs/guides/public-export.md` | GUIDE | current | The two-repo rule: private is canonical; a feature reaches public as one clean squashed PR, only through the export. |
| `docs/guides/issue-tracking.md` | GUIDE | current | The two issue boards: GitHub is the claim board, the triage rule (a maintainer's label), and why issue text is data for agents. |
| `docs/guides/dev-mode.md` | GUIDE | current | Dev mode: a debug world where nothing waits for training, building or income. |
| `docs/guides/wing.md` | GUIDE | current | The wing: bots that fly with you; the `/bots` and fleet-chat words, with the not-yet ones marked. |
| `docs/guides/bench-ramp.md` | GUIDE | current | Bench ramp: ramp bots into one system with its own server and find the knee (`tools/bench-ramp.sh`). |
| `docs/guides/jail.md` | GUIDE | current | The jail: the client never gets a route to CCP; what it protects and what it does not. |
| `docs/guides/client-ci.md` | GUIDE | current | Client CI in one page: run a scenario against the real client, exit codes, where the pieces are. |
| `docs/guides/playing-with-friends.md` | GUIDE | current | How a friend reaches the owner's server (ssh tunnel, kit) and how the owner runs accounts. |
| `docs/guides/getting-started.md` | GUIDE | current | Zero to playing: build, content bundle, server, client, bots, tests and the gate, with launch examples. |
| `docs/guides/content-bundle.md` | GUIDE | current | The content pipeline: inputs, steps, outputs, verify, rebuild for a new build. |
| `docs/guides/contributing.md` | GUIDE | current | The reference behind `AGENTS.md`: Docker, the bundle, tests, the gate, branch, PR, review; the labels (§8) and the areas and hot files (§10). |
| `docs/guides/working-together.md` | GUIDE | current | How several people's agents share one tree: claims, owned files, the layering rule, generated files. |
| `docs/guides/gm-commands.md` | GUIDE | current | The GM slash commands and the server flags. |
| `docs/guides/hosting.md` | GUIDE | current | Hosting on a dedicated box: frozen bundle, the owner's remote GM listener, host bots, backups, wipe. |
| `docs/guides/patcher.md` | GUIDE | current | The phone-home patcher `tools/eve_patch.py` and its per-build manifests. |
| `docs/guides/census.md` | GUIDE | current | The census: proving the patched client phones nobody. |
| `docs/guides/build-compat.md` | GUIDE | current | Adding, checking and retiring a client build (`tools/new-build.sh`). |
| `docs/guides/debugging-the-client.md` | GUIDE | current | The method for debugging the real client against our server: instruments, traps, tooling. |

## Reference: how the protocol and the server work (current truth about this client build)

| doc | role | status | covers |
|---|---|---|---|
| `docs/reference/phone-home.md` | REFERENCE | current | Every path by which the client phones home (native, Python, Wine), when it fires, what it sends, what closes it; what goes to our server by design. The pinned build in `tools/builds.tsv`. |
| `docs/reference/server.md` | REFERENCE | stale-in-part | What `eve-server` does; §§1-7 are the 2026-10-19..22 state (status note at the top), §8 lists services, flags and GM commands. |
| `docs/reference/world-state.md` | REFERENCE | current | Durable world state: STORE_FORMAT, delta kinds, the backup, the upgrade policy (the rest of the file is history). |
| `docs/reference/launch-and-env.md` | REFERENCE | current | Every server and eve-bots flag and `EVE_*` variable, with launch examples; checked by `tools/env-ref.py`. |
| `docs/reference/threads-and-locks.md` | REFERENCE | current | Threads, the world lock and the wire lock: the short picture, then the precise rules. |
| `docs/reference/unattended-root.md` | REFERENCE | stale-in-part | The threat model of the passwordless jail helper (status note at the top: the helper is built). |
| `docs/reference/framing.md` | REFERENCE | current | machoNet TCP framing, from disassembly. |
| `docs/reference/dbrow.md` | REFERENCE | current | `blue.DBRow` packed-row layout; supersedes the DBRow gap in the archived wire-format doc. |
| `docs/reference/object-wrapper.md` | REFERENCE | current | The object wrapper codes 0x17, 0x22, 0x23 and the 0x2D terminator. |
| `docs/reference/login-path.md` | REFERENCE | current | The exact login call sequence and the shape of every value, TCP connect to character select. |
| `docs/reference/push-path.md` | REFERENCE | current | The server-initiated push path (spine step D). |
| `docs/reference/push-events.md` | REFERENCE | current | The 3,177 push handlers the client already has, and how `findings/push-events.tsv` was made. |
| `docs/reference/bound-objects.md` | REFERENCE | current | Bound objects: the client's second call style, and how the server mints them. |
| `docs/reference/code-push.md` | REFERENCE | current | The two answers that hand the client executable code (CPython marshal, not our marshal). |
| `docs/reference/session-roles.md` | REFERENCE | current | The session object and the role mask: what unlocks the dev and GM tools. |
| `docs/reference/client-call-surface.md` | REFERENCE | current | The census of every machoNet call site in the client (`findings/client-calls.tsv`). |
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
| `docs/content/README.md` | DESIGN | current | The content process: analyze, lanes C1-C7, synthesis, red team. |
| `docs/content/plan.md` | PLAN | stale-in-part | The content plan v3: closed as a live plan (banner); §1-§5 and the §10 brief template stay the rules. |
| `docs/content/schema.md` | REFERENCE | current | The content schema: every authored table's header line, frozen. |
| `docs/content/rulings.md` | RECORD | current | Coordinator rulings on the content-plan review. |
| `docs/content/plan-review.md` | RECORD | current | The red-team review of the content plan. |
| `docs/content/C1-corpus-map.md` | RECORD | stale-in-part | Research lane C1: the corpus-to-feature map (cites `reprocess/tables.rs`, which was purged from the tree). |
| `docs/content/C2-live-surface.md` | RECORD | current | Research lane C2: the live-content surface of the client core. |
| `docs/content/C3-population.md` | RECORD | stale-in-part | Research lane C3: the world population generator (cites `server/content/*.tsv`, now `content/`). |
| `docs/content/C4-agents-missions.md` | RECORD | current | Research lane C4: agents, missions, arcs and the Agency. |
| `docs/content/C5-economy.md` | RECORD | stale-in-part | Research lane C5: economy content (names `tools/econ.py`, built as `tools/content_econ.py`). |
| `docs/content/C6-npc-content.md` | RECORD | stale-in-part | Research lane C6: NPCs as content (names `tools/npc_content.py`, built as `tools/content_npc.py`). |
| `docs/content/C7-liveness.md` | RECORD | current | Research lane C7: liveness and operations. |
| `docs/content/R-FW.md` | RECORD | current | Spec: factional warfare. |
| `docs/content/R-TACKLE.md` | RECORD | current | Spec: warp scramblers and disruptors. |
| `docs/content/R-WH.md` | RECORD | current | Spec: wormholes. |
| `docs/review/README.md` | DESIGN | current | The emulator review: lanes V1-V7, synthesis, red team. |
| `docs/review/rulings.md` | RECORD | current | Coordinator rulings on the review red team. |
| `docs/review/plan-review.md` | RECORD | current | The red-team review of the improvement plan. |
| `docs/review/V1-architecture.md` | RECORD | historical | Review lane V1: architecture and module structure (frozen against its tree). |
| `docs/review/V2-wire.md` | RECORD | historical | Review lane V2: wire, protocol and session robustness (frozen). |
| `docs/review/V3-runtime.md` | RECORD | historical | Review lane V3: world state, concurrency, performance (frozen). |
| `docs/review/V4-testing.md` | RECORD | historical | Review lane V4: testing, tooling and the gate (frozen). |
| `docs/review/V5-process.md` | RECORD | historical | Review lane V5: process, docs and knowledge (frozen; it asked for this index). |
| `docs/review/V6-security-ops.md` | RECORD | historical | Review lane V6: security, the jail and operations (frozen). |
| `docs/review/V7-content-fit.md` | RECORD | historical | Review lane V7: how the content data fits the emulator (frozen). |

## Lanes: how parallel work is divided, and every lane note

| doc | role | status | covers |
|---|---|---|---|

## Reverse-engineering facts about the client (measured on the pinned build in `tools/builds.tsv`, or on the build before the re-pin of 2026-10-31)

| doc | role | status | covers |
|---|---|---|---|
| `docs/re/dogma.md` | RE | current | Dogma: the attribute and effect system. |
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
| `docs/re/localization.md` | RE | current | The localisation corpus: format, lookup path, contents. |
| `docs/re/errors.md` | RE | current | The client's error vocabulary. |
| `docs/re/client-callable.md` | RE | current | The server-to-client call path and the `__exportedcalls__` catalogue. |
| `docs/re/session-and-binding.md` | RE | current | Session and binding from the client's side. |
| `docs/re/destiny-ball-crosscheck.md` | RE | current | The ball record cross-checked against a second implementation; current truth for the ball table. |
| `docs/re/evejs-crosscheck.md` | RE | current | EvEJS claims cross-checked against the client; current truth over the archived EvEJS reference. |
| `docs/re/evejs-data.md` | RE | current | What data EvEJS ships and which of it we lack (facts only). |

## EvEJS facts (AGPL, a different client build: facts only, never copy code or prose)

| doc | role | status | covers |
|---|---|---|---|
| `docs/evejs/review/README.md` | EVEJS | current | The EvEJS fork review: what it ships, its Destiny model, our four blocked features; the entry to `docs/evejs/review/`. |
| `docs/evejs/review/static-data.md` | EVEJS | current | The datamined data EvEJS ships, formats and schemas. |
| `docs/evejs/review/destiny.md` | EVEJS | current | Their space and Destiny model: constants, formulas, visibility. |
| `docs/evejs/review/features.md` | EVEJS | current | Stargates, a second station, asteroid fields, the sun (157 KB). |
| `docs/evejs/review/docs-packs.md` | EVEJS | current | Their documentation and the 339 MB of content packs. |
| `docs/evejs/evejs-run-safely.md` | EVEJS | current | Running EvEJS in a container: go/no-go, four changes, the build delta. |
| `docs/evejs/evejs-subsystem-gap.md` | EVEJS | current | A scope map of what EvEJS implements that we do not (from the file listing alone). |
| `docs/evejs/evejs-surprises.md` | EVEJS | current | Where EvEJS and the client disagree, re-verified against the client. |
| `docs/evejs/inventory/character-dogma.md` | EVEJS | current | Inventory of EvEJS: character, skills, dogma. |
| `docs/evejs/inventory/corp-sov-structures.md` | EVEJS | current | Inventory of EvEJS: corporations, sov, structures, fleets. |
| `docs/evejs/inventory/industry-mining.md` | EVEJS | current | Inventory of EvEJS: industry, mining, reprocessing, planets. |
| `docs/evejs/inventory/market-economy.md` | EVEJS | current | Inventory of EvEJS: market, wallet, economy. |
| `docs/evejs/inventory/npc-content.md` | EVEJS | current | Inventory of EvEJS: NPCs, dungeons, missions, sites. |
| `docs/evejs/inventory/social-infra.md` | EVEJS | current | Inventory of EvEJS: social, chat, session, infrastructure. |
| `docs/evejs/inventory/space-destiny.md` | EVEJS | current | Inventory of EvEJS: space, Destiny, combat. |

## Archive: demoted or finished documents, kept for history

| doc | role | status | covers |
|---|---|---|---|
| `docs/archive/plan-deep-queue.md` | HISTORY | superseded | The deep queue of 2026-10-27, with every lane's "Not done" list; superseded by the master plan and the board. |
| `docs/archive/plan-subsystems.md` | HISTORY | superseded | The subsystem plan, waves W0-W9 (finished); §2a and §2b are cited from code. |
| `docs/archive/plan-living-world.md` | HISTORY | superseded | The living-world plan: stages 0-3 landed, stage 4 is in the master plan. |
| `docs/archive/architecture-sweep.md` | HISTORY | superseded | A reading of our own code (2026-10-19); open items moved to the master plan. |
| `docs/archive/doc-inventory.md` | HISTORY | historical | The DOCS-CLEAN inventory of 2026-10-28. |
| `docs/archive/roadmap.md` | HISTORY | historical | The original roadmap; its founding vision still stands, its milestone list is long past. |
| `docs/archive/plan-bots.md` | HISTORY | historical | The bot design (milestones now built; see the lane notes). |
| `docs/archive/plan-client-ci.md` | HISTORY | historical | The client-CI plan; the harness exists, see `docs/guides/client-ci.md`. |
| `docs/archive/plan-docked-subsystems.md` | HISTORY | historical | The docked-subsystems plan, two rounds (built). |
| `docs/archive/plan-port-from-evejs.md` | HISTORY | historical | The EvEJS porting plan; §1 records the licence decision. |
| `docs/archive/plan-the-churn.md` | HISTORY | historical | The first reverse-engineering sweep plan. |
| `docs/archive/recon-v24.01.md` | HISTORY | historical | The first recon of the V24.01 client. |
| `docs/archive/session-flow.md` | HISTORY | superseded | Session establishment overview; superseded by `docs/reference/login-path.md`. |
| `docs/archive/wire-format.md` | HISTORY | superseded | The first wire-format doc; superseded by `docs/reference/framing.md` and `docs/reference/dbrow.md`. |
| `docs/archive/marshal-tags.md` | HISTORY | superseded | Marshal type codes; superseded by `docs/reference/object-wrapper.md` and `docs/reference/dbrow.md`. |
| `docs/archive/entering-the-game.md` | HISTORY | historical | From the character click to a station hangar (a 2026-10-19 snapshot). |
| `docs/archive/destiny-and-the-ballpark.md` | HISTORY | superseded | Destiny and the ballpark; §4.4 superseded by `docs/re/destiny-ball-crosscheck.md`. |
| `docs/archive/evejs-261018-reference.md` | HISTORY | superseded | The first EvEJS briefing; superseded by `docs/re/evejs-crosscheck.md`. |
