# Content: how the server gets, generates and keeps a live universe

Owner request, 2026-10-25: *"analyze the core further for additional info, as
well as the content dump to see how it plays into making a live server.
content plan: analyze -> optimized prompt for subagents to analyze further and
then collectively figure out how we should do content on the server."*

Read as: **the core** is the client's own code (`code.ccp` → `out/pyc`, plus
the native `*Loader.pyd`s that read cFSD). **The content dump** is the static
corpus (`res:/staticdata/`, 203 files, `docs/re/static-corpus.md`) and, facts
only, the EvEJS content tables (`docs/re/evejs-data.md`). `[A]` on that reading.

The process has three steps:

1. **analyze:** the coordinator's stocktake, §1.
2. **lanes C1-C7:** parallel read-only research, one brief each, §3.
3. **synthesis:** one lane reads every C-lane note and writes
   `docs/content/plan.md`, which is the decision plus implementation packets.
   A red-team lane then attacks it, and the coordinator settles what is
   disputed and files the packets in `docs/archive/plan-subsystems.md`.

## 1. Stocktake: what is already known (do not redo this)

| Area | Where | State |
|---|---|---|
| Corpus formats, 203 files | `docs/re/static-corpus.md`, `findings/corpus-inventory.tsv` | 54 decode fully. 149 cFSD decode as containers, and columns exist only for the tables that mattered. |
| cFSD per-field layouts | `docs/re/native-loaders.md`, `findings/pyd-schemas.tsv`, `tools/pyd_schema.py` | 139 of 149 have a recovered layout. |
| Dungeons, 5,516 | `docs/re/dungeon-model.md` | Fully open: 8,483 rooms, 260k objects and 30,610 NPC spawn entities. Loot is **not** in the client. |
| Missions, 2,892 | `docs/re/mission-model.md`, `docs/re/agents-missions.md` | Decoded. The agent→mission pool is **server-authored**, and EvEJS left it empty too. |
| NPC behaviour | `docs/re/npc-behaviors.md`, `spacecomponentsbytype` | Threat range, respawn interval and standings thresholds ship in the client. The missing pieces are **spawn tables (504 ids), NPC fleet types (71) and behaviour trees**. |
| The EvEJS gap list | `docs/re/evejs-data.md` §4 | Missing: market book and cost model, LP stores, wormhole statics, moon ore, insurance prices, dock geometry. |
| Universe | `--celestials`, `--belt-rocks`, `--planets` (`tools/celestials.py`, `asteroids.py`, `pi_statics.py`) | Loaded at runtime from `out/`. |
| Types, dogma, skills | `--typedata`, `--skills` (`tools/typedata.py`, `skills.py`) | Loaded at runtime. The W10 dogma lane is wiring effects. |
| Server content today | `src/npc.rs`, `dungeon.rs`, `asteroid.rs`, `mission*`, `market*`, `scan.rs` | Hand rows plus extracts. Belt rats are being built (W10). |

**What nobody has done yet:**

- Nobody has mapped the corpus to server features: which file feeds which
  subsystem, what we consume today, and what is still unread.
- The client's **live-content surface** is unmapped. That is what the client
  asks for, or expects pushed, to *show* a living universe: the Agency, the
  probe scanner's anomalies and signatures, the ESS, insurgency and
  warzones, seasonal campaigns and challenges, and the `newactivities`,
  `operations` and `objectivechains` tables.
- There is no generation model: how many anomalies of which archetype each
  system gets, at what security, how they respawn, and how belts deplete
  and regrow.
- There is no single policy for **server-authored data**: where it lives,
  in what format, and how it stays clear of the rule against committing
  extracted client data.
- There is no **liveness model**: lazy versus eager systems, persistence,
  a daily rollover, the tick budget, and how bots consume content.

## 2. Shared brief: every C-lane obeys this

- **Read-only research lane** (lane rule 7). Write only your own files:
  `docs/content/<lane>.md` and, optionally, `findings/content-<lane>*.tsv`.
  No cargo, no gate, no commits, and no edits to `server/` or `tools/`. Three
  W10 build lanes are editing the server tree right now. Ignore their
  half-written files and never touch them.
- **Client:** the build is 3619879, and the install is `tq-offline` (see
  `tools/analysis.sh`). Bytecode is in `out/pyc`. Read it with
  `tools/dis.py`, `tools/pyc27.py` and `tools/strings_of.py`, and take line
  numbers from `co_lnotab`, never from uncompyle6. The corpus readers are
  `tools/fsd.py`, `tools/fsd_dump.py`, `tools/pyd_schema.py`,
  `tools/dogma.py` and `tools/loctable.py`. Run Python with `python3 -I`,
  with scripts in your scratchpad, or through `tools/analysis.sh`. **Never
  install anything on the host**; if a tool is missing, use the Docker image.
- **EvEJS** (a local EvEJS checkout, and the archive described in
  `docs/evejs/review/README.md`): it is AGPL and built for another client build, so
  take **facts and vocabulary only, marked `[V2]`**, and never rows or code.
  Never execute anything from it.
- **No client data in the repo.** Your doc may quote schemas, column names,
  counts, distributions and at most one sample row per table. Anything bulk
  belongs in an extractor spec, which a later build lane implements to write
  `out/`.
- **Marks:**
  - **[V]** read from this build's bytecode or data, with file and line or
    file and field;
  - **[V2]** EvEJS;
  - **[I]** inferred, with the inference stated;
  - **[A]** an assumption we would ship, with what would falsify it;
  - **unknown** when you don't know. Write unknown rather than guess.
- **Deliver implementation specs, not descriptions.** Every finding ends in
  one of three forms:
  - "server does X, reading field Y of file Z, emitting call or push W with
    shape S";
  - "extractor E writes `out/F` with columns C";
  - "author table T with columns C, seeded by rule R".
- **Size:** say what a live server for 2-10 humans plus about 50 bots needs.
  Rank everything by what it unblocks for the end goal: co-op and PvP with
  friends plus player-like bots, in a universe that feels inhabited.
- **Your doc ends with:**
  - a **"Proposals"** section of numbered, concrete decisions you would have
    the synthesis adopt;
  - a **"Packets"** section of build-lane-sized work, each packet with the
    files it would own and its tests;
  - an **"unknown"** list.
- Delete any scratch copy you make before you report (lane rule 9). Report
  in 15 lines or fewer: what you found, your top 3 proposals, and where
  your doc is.

## 3. Lanes

**C1: corpus-to-feature map.**
- **Task:** for all 203 `res:/staticdata` files, and for the `.pyd`
  loaders, give the server subsystem it feeds, whether we consume it today
  (grep `tools/` and `server/`), what it is worth for a live server, and
  the single extractor that should own it.
- **Also:**
  - Propose **one unified extractor** (`tools/content.py`?) that writes a
    versioned `out/content/` bundle, plus one server flag
    (`--content DIR`) that would replace the growing flag list.
  - Name the cFSD tables whose missing per-field layout actually blocks
    content.
- **Output:** `docs/content/C1-corpus-map.md` and
  `findings/content-C1-map.tsv`.

**C2: the live-content surface of the core.**
- **Task:** map every client path that displays a dynamic universe, and
  for each one give the remote call or push and its shape:
  - the probe scanner's anomalies and signatures, and system scan results;
  - the Agency (`newactivities`, `operations`, `operationsites`,
    `objectivechains`, `objectivetypes`, `dungeonobjectives`);
  - the ESS (`essreservekeys`);
  - warzones and insurgency;
  - seasonal campaigns, login rewards, challenges, achievements and
    `goals`;
  - `agentsinspace`, `voidspaceencounters` and `epicarcs`;
  - in-space beacons and the "sites in system" overview.
- For each, say whether the client **breaks, degrades, or shows empty** if
  the server says nothing. That ranking decides what we build.
- **Output:** `docs/content/C2-live-surface.md`.

**C3: the world population generator.**
- **Task:** specify the generator that decides what exists in space,
  covering:
  - combat, ore, gas and ice anomalies, and signatures, per system, by
    security and region;
  - the dungeon archetypes (`archetypes.fsdbinary`, `dungeons` fields,
    `oretypesbydungeonid`, `gastypesbydungeonid`, `icetypesbydungeonid`,
    `asteroidcountbytypeid*`);
  - belt contents and depletion and regrowth (`asteroidtypesbysolarsystemid`);
  - NPC belt spawns (`spacecomponentsbytype` `reactiveSpawnComponent`);
  - despawn on completion, and respawn timers.
- Deliver numbers, or say **unknown**, with the rule we would author for
  each.
- **Output:** `docs/content/C3-population.md`.

**C4: agents, missions, arcs and the Agency's PvE.**
- **Task:**
  - Design the server-authored agent→mission pool: by agent level,
    division, corp and faction, from `npccharacters`, `npccorporations`,
    `npccorporationdivisions`, `missions` (`contentTemplate`, `dungeonID`)
    and `agenttypes`.
  - Cover epic arcs (`epicarcs`), `conversations` and `conversationagents`,
    storyline triggers, LP rewards, and the LP store (absent: say how we
    would author it).
  - Read `docs/re/mission-model.md` and `agents-missions.md` first, and
    extend them; do not repeat them.
- **Output:** `docs/content/C4-agents-missions.md`.

**C5: economy content.**
- **Task:** specify the market seeding and economy faucets and sinks:
  - NPC sell and buy orders: which types, which stations, how prices are
    set (`basePrice`, `marketgroups`), and how the book refills;
  - blueprints and industry (`blueprints.static`, the `industry_*` tables);
  - reprocessing (`typematerials`) and `compressibletypes`;
  - bounties (typedogma), insurance, LP and taxes;
  - a stable economy with few humans plus bots.
- Read the server's current market and industry code to see what exists.
- **Output:** `docs/content/C5-economy.md`.

**C6: NPCs as content (the authored gap).**
- **Task:** spawn tables (504 `spawnTableID`s), NPC fleet types (71),
  loadouts, loot tables (`crates` references them), and behaviour profiles.
  Specify for each:
  - its authoring format;
  - a **derivation rule from the corpus**, so we generate rather than
    hand-write: for example, rat composition from the dungeon spawn
    entities and the faction, and loot from the NPC's faction, meta level
    and bounty;
  - how it plugs into `src/npc.rs` and the bot brain (`eve-bot`).
- Read `docs/re/npc-behaviors.md`, `dungeon-model.md` §5-6 and
  `evejs-data.md` §4.1 first.
- **Output:** `docs/content/C6-npc-content.md`.

**C7: liveness and operations.**
- **Task:** read the server's world ticker, `Registry`, the persistence
  code and `docs/reference/world-state.md`, then specify:
  - lazy versus eager systems, given 8,490 systems and a few dozen pilots;
  - the persistence of generated content across restarts;
  - deterministic seeding, so a restart reproduces the universe;
  - a daily "downtime" rollover for respawns, market refills and agent
    offers;
  - a tick budget, and how bots and humans share the content without
    starving each other;
  - observability: how an operator sees what is live.
- **Output:** `docs/content/C7-liveness.md`.

## 4. Synthesis

**S1, after C1-C7 report.**
- **Input:** every `docs/content/C*.md`.
- **Output:** `docs/content/plan.md`, which settles:
  - the content pipeline (extract → `out/content` → load);
  - the authored-data policy (what goes in the repo, and in what format);
  - the generator architecture;
  - a liveness model;
  - a ranked packet list for build waves W11+, with ownership that does not
    collide (lane rule 1).
- Where C-lanes disagree, pick one and say why.

**R1, red team.** Attack `plan.md`: name the decisions that would cost us
a rewrite if wrong, the claims not marked `[V]`, and the collisions
between packets. Write `docs/content/plan-review.md`.

The coordinator resolves the review, then files the packets.
