# The content plan, v3: how the server gets, generates and keeps a live universe

> **Status 2026-11-01:** closed as a live plan; `docs/plans/master-plan.md` holds the work ahead. This file stays here because code comments cite its sections and `tools/test_plan_owners.py` parses its packet tables. §1-§5 (pipeline, authored-data policy, generators, liveness) and the §10 brief template are still the rules. The `state` column of `docs/packets.tsv` lags the tree: confirm a packet in the code before you act on it.

**v3** (2026-10-25, against HEAD `6cea759`) applies the emulator review's
content amendments CA1-CA19 (`docs/review/plan.md` §3.5, from
`docs/review/V7-content-fit.md` and `docs/review/plan-review.md`) and
`rulings.md` errata 10-14. What changed is in §0.0. Everything else is v2.

Revision lane P2, 2026-10-25. **v2 replaces v1** (synthesis lane S1, commit
`f293520`). Inputs: v1, the red-team review `plan-review.md` (R1), the
coordinator's binding `rulings.md`, C1-C7 (`docs/content/C*.md`),
`docs/lanes/README.md`, and the tree at HEAD `0954132`. HEAD includes B1
(`80d63e4`), K1 (`0ed5edf`) and the gate's Python test step. This document
**decides**. Where it does not decide, it says **OWNER DECISION** and gives
a recommendation (§9). A build lane is briefed from §6 plus §10.

Marks: **[V]** this build or this tree, **[V2]** EvEJS (facts only),
**[I]** inferred (inference stated), **[A]** an assumption we ship (with its
falsifier). **No test asserts an [A] value.** A test asserts a rule, a [V]
value, or code against itself (determinism), and names a tuning const by
its name, never by its number.

The owner's constraints, restated once:

1. The server is std-only Rust with zero crates (`server/deps.allow` stays
   empty).
2. No extracted client data in the repo, ever. Extracts go to gitignored
   `out/` and load at runtime by flag. Tests use invented rows and ids.
3. Never install on the host. Tools run in Docker (`tools/analysis.sh`,
   `tools/dev.sh`). The host's stock `python3` runs stdlib-only unit tests,
   which is what the gate already does.
4. [A] values ship, and they never appear in a test.
5. The target is co-op **and PvP** for 2-10 humans plus about 50 bots.
6. Lanes own files, not subsystems.

---

## 0. Changelog

### 0.0 v3 changes (v2 → v3)

**What landed since v2** (HEAD `6cea759`):

| commit | what | effect on this plan |
|---|---|---|
| `0bf9762` `1f8ff69` `eec5834` `9c11144` `41d933f` `fa9a924` `e12066f` `75c320b` `42549c4` `8cb1fb8` | **W11**: CP1-CP8, CP19, CP32a, CP33a and their coordinator hunks | W11 is done except CP9, which waits on O6 (§6.2) |
| `ee8070c` | **RIDX**, the per-system rock index under `Field::in_system` | CP14' keeps its contract and its test (CA3) |
| `2ef2e6d` | the review's content seam: `TableSpec { name, load, fixture }` with `Sources` and an error path; one source line per table; the mission pool on `seed::stream` | CA1, CA4 and CA5's first half are **done** |
| `8ee703d` | a world record in every `Registry`; `delta_kinds::KINDS`; `Registry::tainted` (set by nobody yet) | CA5's second half, CA6 and CA16's delta-kind half are **done** |

**The amendments** (`docs/review/plan.md` §3.5; V7 = `V7-content-fit.md`,
RR1 = `plan-review.md`). "Pending" means it waits on a review packet that
has not landed; the plan text already says what changes when it does.

| CA | status | what v3 changed | where |
|---|---|---|---|
| CA1 | **done** `2ef2e6d` | `TableSpec::load(&Sources) -> Result`, called with or without a bundle | §1.3, §1.4, §1.5, §6.2 W11 note |
| CA2 | pending (the `up.sh` and `soak.sh` packets) | `--authored "$REPO/content"`, absolute; the hash from `content.extractor_hash()` | §1.3, §1.4 |
| CA3 | applied | CP14 → **CP14'** (M-L): coordinator hunks in `bin/eve-server.rs`, `model.rs`, `server.rs`; the `Field` API stays; RIDX's contract and test kept | §6.2 W12, §6.4 |
| CA4 | **done** `2ef2e6d` | one `[content] table <name>: <origin>` line per table | §1.3 |
| CA5 | **done** `2ef2e6d` (offers) and `8ee703d` (world seed) | one RNG | §3.1, §4.3, CP26a |
| CA6 | **done** `8ee703d` | `reg.world()` is `&World`; no `None` branch | §4.2, §10 |
| CA7 | applied | seven process statics, each with an owner (CP20, CP58c, CP59a, CP60) | §1.5, CP20, CP58c, CP59a, CP60 |
| CA8 | applied | new **CP58e** (`drones.rs`, `scan.rs`, O3); CP59b's hunk is `slash.rs:416` | §2.2, §6.2 W18, §9 O3 |
| CA9 | applied | CP21 and CP26a after CP3b; CP26a drops CP20 (kill templates wait for `npc_data`); CP20 reads `SystemRow.region_id`; CP46's `--content` hunks | §6.2 W12-W16 |
| CA10 | applied | CP3c to W12; CP26b → CP26b-1 (W13) + CP26b-2 (W14, after CP22) | §2.5, §3.2, §6.2, §6.4, §7 |
| CA11 | applied | brief: a slot's `make` never reads deltas | §10 |
| CA12 | applied | brief: `$SCRATCH/<lane>`, series ids, `existing:`/`new:`, rule-16 header; no session path left in this file | §6.2 key, §10 |
| CA13 | applied | CP15 (M-L): wait vs hold, stage timings, `[turns]`, a `dispatch.rs` hunk | §4.6, §6.2 W12 |
| CA14 | applied (the review's bot-test move lands before W15) | `eve-bot/tests/bots/<name>.rs` plus a `mod` line | §6.1, CP34, CP55, CP56 |
| CA15 | applied (seam S13 lands before W15) | kill side effects are `kill_hooks` rows | §6.1, CP38, CP44, CP50, CP53b |
| CA16 | delta kinds **done** `8ee703d`; logoff steps applied (seam S14) | `KINDS` rows; `session_hooks::ON_LOGOFF` | §4.2, §6.1, CP39, CP57, §10 |
| CA17 | applied | O1's measured trigger | §4.5, §9 |
| CA18 | applied as a **standing rule** | a split re-points the "owns" lists and re-runs the check; CP58b and CP59b carry their pending re-points | §6.3, §6.2 W18 |
| CA19 | applied | bots never send their name as a token; `BotConfig` carries the token | §6.2 key, W15-W16, §10 |

**Coordinator errata 10-14** (`rulings.md`):

- **10** (the pinning test is `no_service_is_claimed_by_two_modules`): v2
  already used the right name, and v3 found no use of the old one.
- **11** (CP26b after CP22): CP26b-2's "after" column.
- **12** (`objectiveTargetGroup` has no source): §3.3 and CP28a; the lane
  drops or derives it, and says which.
- **13** (`fixed_lp` is the Omega value, [A]): CP13 and CP37b.
- **14** (families key on `chain_key`, else `name_id`; CP13 canonicalises
  `name_id` per title): CP13; CP19 already landed it (`42549c4`).

**The ownership check** was re-created and re-run (§6.3): 80 packets
(77 build, 3 research), 0 duplicate paths within a wave, 0 packets over
four files.

### 0.1 v1 → v2: what landed since v1, and what it changes

| commit | what | effect on this plan |
|---|---|---|
| `80d63e4` B1 | breaker sweep: `service/live_misc.rs` (new), epic-arc arms in `service/character.rs`, FW warzones, HQs and battlefields in `service/space.rs` (`WARZONES` `:827`, `HQ_SYSTEMS` `:839`), and a `DOMAINS` row in `service.rs` | Wave 0 is done, so the seams land **before** W11 (§6.1). S2 moves B1's FW consts to `service/facwar.rs`. CP18 retires them in favour of `warzones.tsv`. CP27 owns the file B1 created |
| `0ed5edf` K1 | `fsd.rows(client, stem)` decodes 140 of 149 cFSD tables with CCP's field names. **`types.factionID` is at record +72, not +76**, fixed in `fsd.py` and `dogma.py` (`dogma.py:307`). 1,383 types carry it [V, K1 note] | C6 §2.1's "`types.factionID` is not usable" read the wrong offset and is **superseded**. CP11a uses `types.factionID` as R1's first step where it is present. The 9 tables K1 cannot decode (`manufacturableitems`, the challenge tables, the skin tables, `audiometadata`, `seasonselection`) are inputs to no packet here. Under `python3 -I`, a tool that imports `dogma` must put its own directory on `sys.path` itself (the K1 note). This is a brief rule (§10) |
| `0954132` | `tools/gate.sh` runs every `tools/test_*.py` as `python3 -I "$t" -q` | Each extractor packet's unit test file must be a runnable stdlib `unittest` script that fixes its own `sys.path` the way `tools/test_fsd_generic.py` does. Client-backed counts go in `content.py --check` (ruling 1) |

### 0.2 v1 → v2: every review finding, and what v2 did with it

"Applied" means v2's text does what the finding asks. "Ruling N" names the
coordinator ruling that decided it.

| # | finding (R1 section) | outcome | where in v2 |
|---|---|---|---|
| 1 | No gate runs the extractor tests, and they need the client (§1.1) | **applied** (ruling 1). Unit tests use synthetic bytes or synthetic TSV. Counts, closures and known ids move into each section's `check(ctx)`. `up.sh` runs `content.py --check` before boot and refuses to start on a failure. v1's "the gate re-checks it" is struck. The gate step landed in its per-file form, not `unittest discover` | §1.6, every extractor packet, §8 |
| 2 | S1-S5 leave eight shared files (§1.2) | **applied** (ruling 2), as seams S6-S12 in W10.5. These deviate from R1's text: S7 is a registration table (ruling 2's "one registration line"), not R1's recommended per-field option (a). `scan.rs` gets a source table in S8 instead of CP29 owning it. The ticker's store argument is a coordinator hunk at CP15. New `DOMAINS` rows are a brief rule. Bot harness tests are files the packet owns | §6.1 |
| 3 | The data-access model is undecided (§1.3) | **applied as ruling 3**, which overrules R1's mechanism (a module `OnceLock` plus a `#[cfg(test)]` fixture) in favour of a `Content` value held by the world and reached through the `Registry`. R1's CP32 and CP33 splits are applied, and CP33 includes `service/industry.rs`. The three existing `OnceLock`s (`typedata`, `skill`, `planet`) move into `Content` in CP58-CP60 | §1.5, CP32a/b, CP33a/b, CP58-CP60 |
| 4 | The rock delta is keyed by a derived id (§1.4) | **applied** (ruling 4): the key is `(belt itemID, the rock's ordinal in that belt's own client rows)`, and type and position are checked on load | §4.2, CP14 |
| 5 | Site slots depend on row order (§1.4) | **applied** (ruling 4): the key is `(system, archetype, population.tsv row key, k)`, the drawn `dungeon_id` is stored and checked, and `gen_version` is tied to the digest pin | §4.2, CP21 |
| 6 | `DeltaKind` registration has no std mechanism (§1.4) | **applied**: the `Registry` holds one delta map (`delta.rs`, S7) as the single truth. `store.rs` keeps rows raw, and modules read and write through it | §4.2, S7, CP5 |
| 7 | FW gets only B1's consts; no `facWarMgr`, VP or enlistment (§1.5) | **applied** (ruling 5): CP18 retires B1's consts, and CP53a/b add `facWarMgr`, enlistment, VP and the occupation flip after research lane R-FW | CP18, R-FW, CP53a/b |
| 8 | `fw_systems` owned by CP18 (§1.5) | **deferred** to CP53b. How the client lists warzone systems is unknown (C2 §5), and R-FW must find the source first | CP53b |
| 9 | Wormholes are absent (§1.5) | **applied** (ruling 5): authored statics, K162, mass and decay, as CP54a/b after research lane R-WH | CP54a/b |
| 10 | Security never reaches crimewatch (§1.5) | **applied** (ruling 5): CP3b replaces `flags_apply`'s guess with `systems.tsv` security | CP3b |
| 11 | No tackle exists (§1.5) | **applied** (ruling 5): S12 is a movement-guard seam. CP57 adds player scramblers and disruptors (after R-TACKLE), and CP49 adds NPC tackle | S12, CP57, CP49 |
| 12 | Bots never run sites or oppose anyone; courtesy makes them leave (§1.5) | **applied** (ruling 5): CP55 is the site runner, CP56 is the opt-in hostile bot, and CP39's courtesy rule applies only to roles marked civilian | CP55, CP56, CP39, §4.7 |
| 13 | v1 is stale on B1; the seams should land before W11 (§2.1) | **applied** | §0.1, §6.1 |
| 14 | Producer and consumer headers disagree; freeze them (§2.2) | **applied**: coordinator step X1 writes `docs/content/schema.md` in W10.5, and both sides of a pair pin its header line | §6.1 X1 |
| 15 | CP17 and CP29 need warp-to-site (§2.3) | **applied** through S8 | S8 |
| 16 | CP25 and CP21 test each other's logic (§2.3) | **applied**: CP21 tests `population::on_rollover(day)` directly, and CP25 tests with a fake step | CP21, CP25 |
| 17 | CP45 needs `combat.rs` while CP44 owns it (§2.3) | **applied**: CP45 moves to W17 | W17 |
| 18 | `Registry::touch_system` does not exist (§2.3) | **applied, by a different route**: liveness *pulls* from the roster, sessions and jumps in flight, as a tick stage, so it needs no call-site hunks at all | CP6, §4.1 |
| 19 | CP12 silently widens grantable types (§2.3) | **applied**: CP12 owns a `typedata.rs` test | CP12 |
| 20 | Lazy rocks need a per-system index (§2.3) | **applied**: CP1 writes `belts/rocks.tsv` sorted by system, and CP14 builds an offset index | CP1, CP14 |
| 21 | The universe loader has no owner (§2.4) | **applied**: CP3b (systems, regions, constellations) and CP3c (dock locators, retiring `docking::STATIONS`) | CP3b, CP3c |
| 22 | CP32, CP33, CP26, CP28, CP37 and CP11 are too large (§2.5) | **applied**: each is split into a and b. CP11's split also moves `home_pirate` into its own file, so every file keeps one writer | §6 |
| 23 | [A] values pinned in tests: CP39, CP40, CP35, CP21 (§3.1) | **applied** (ruling 8). Every test in §6 was re-audited, and CP14, CP22, CP38, CP46, CP47 and CP48 were reworded too | §6 |
| 24 | CP2 is short one station, under the star (§3.2) | **applied** (ruling 6): the extractor takes star stations, and the target stays 5,210, in `--check` | CP2 |
| 25 | The rollover hour depends on each friend's clock (§3.3) | **applied** (ruling 7): a known limitation plus a human check | §4.4, O4 |
| 26 | `up.sh` serves a stale bundle after an extractor change (§3.4) | **applied**: the manifest carries an `extractor` hash, `up.sh` compares it, and `Bundle::open` logs it | CP1, CP3 |
| 27 | `rewards.tsv` copies EvEJS rows; add O8 (§3.5) | **overruled** by ruling 9: we author our own values from the client's data, and O8 is dropped | §2.5, conflict 12, CP26b |
| 28 | Site ids above 2³¹ are [A]; compute ids in one function; drop the "with a star" restriction (§3.6, §4) | **applied**: one function per id kind, never persisted, and the system index is the rank of the system id | §3.4 |
| 29 | Conflict 7's evidence: materialised counts must equal the static counts (§4) | **applied** to CP14's tests | CP14 |
| 30 | `agent_rowset` cited by line (§4) | **applied**: cited by name | S5 |
| 31 | Two sources for each authored file (§4) | **applied**: CP8 logs which source was used | CP8 |
| 32 | New `service/*.rs` modules need a `DOMAINS` row (§4) | **applied**: a coordinator registration line, in the brief | §6.1, §10 |
| 33 | The bot marker must be bot-side (§4) | **applied**: a bot-side name list, never slim or overview data | CP39, CP40 |
| 34 | CP1's byte-identity pin versus CP12's change (§4) | **applied**: CP1's `check` compares against the live `typedata.py` at run time, so CP12 has no baseline to re-pin | CP1, CP12 |
| 35 | Claims table: B1 stale, "the gate re-checks", `touch_system`, the `tables::` consumers (§5) | **applied** | §8 |

Nothing in R1 was left unaddressed. One finding (#8) is deferred, and one
(#27) is overruled.

---

### 0.3 The decisions in one screen (v3)

1. **One pipeline.** `tools/content.py` is a thin driver that discovers
   `tools/content_<section>.py` by glob and writes one versioned, atomic
   bundle `out/content/` with a `MANIFEST.tsv`. The server loads it with
   `--content DIR`. `content.py --check` runs the client-backed counts, and
   `up.sh` refuses to boot on a failure (§1).
2. **Code reads loaded data through the world.** A `Content` value is held
   by the `Registry`. Tests build `Content::fixture()` from invented rows. A
   server without a bundle runs on the fixture and says so loudly. The
   bundle is needed to *play*, never to *test* (§1.5).
3. **The repo holds only what we invent** (`content/*.tsv`, Rust consts). Both
   `tables.rs` files are deleted (§2).
4. **Content is a pure function plus deltas**, keyed by ids the client keeps
   stable, never by row position (§3, §4).
5. **Liveness is C7's model:** cold, warm and hot systems; a rollover at
   11:00 UTC with no disconnect; a measured tick budget (§4).
6. **The end goal is first-class.** FW, wormholes, security in crimewatch,
   tackle, a bot that runs sites and a bot that fights, all as packets with
   a spec or a research lane (§4.7, §6).
7. **Seams first, then 8 build waves.** There are twelve coordinator seams
   (W10.5), 77 build packets in waves W11-W18 (v3: CP26b split, CP58e
   added), and three research lanes. No file is owned twice in one wave,
   which a script checks, and re-checks after every split (§6.3).

---

## 1. The content pipeline

### 1.1 Extract: `tools/content.py` plus section modules

- **`tools/content.py`** is the driver. It owns:
  - argument parsing (`--client`, `--resfiles`, `--out`, `--sections`,
    `--systems`, `--check`);
  - the atomic write: everything goes to `out/content.tmp-<pid>/`,
    `MANIFEST.tsv` is written last, and the directory is renamed over
    `out/content/`;
  - the manifest, including the **`extractor` hash**: a sha256 over
    `tools/content*.py` and every reader they import (`fsd`, `dogma`,
    `pyd_schema`, `loctable`, `celestials`, `asteroids`, `pi_statics`,
    `typedata`, `skills`);
  - the four phase-1 sections (`universe`, `belts`, `types` for
    `typedata.tsv` and `skills.tsv`, `planets`). These import the existing
    extractors and stay byte-identical to them;
  - `belts/rocks.tsv` **sorted by system, then belt, then the client's own
    row order inside the belt**. CP14 relies on both orders.
- **Section modules** are `tools/content_<name>.py`. Each one exports
  `SECTION`, `DEPENDS = (...)`, `def section(ctx) -> [(relpath, header_cols,
  rows_iter)]` and `def check(ctx) -> [str]`. The driver finds them by glob,
  so **adding a section edits no shared file**.
- **`sys.path`:** the driver and every module append their own directory
  **last**, because `tools/dis.py` shadows the stdlib, and `-I` drops the
  script directory (K1 note). Then `python3 -I` works.
- **Readers are imported, never copied.** These are `fsd` (with K1's
  `fsd.rows`), `dogma`, `pyd_schema` and `loctable`.
- **The schema guard** (C1 §3): each section checks its layouts against
  `pyd_schema.schema_for` and refuses to run on a disagreement.
- **Every header is the one in `docs/content/schema.md`** (X1, §6.1).
- It runs in Docker:
  `tools/analysis.sh python3 -I /work/tools/content.py --client /client --out /work/out/content`.

### 1.2 The bundle: `out/content/`

C1 §5.2's manifest: `format`, `client_build`, `extractor`, `systems`, then
one `file` row per file (rows, sha256, sources) and one `source` row per
corpus file (md5 from `resfileindex.txt`). The format is C1 §5.3: TSV, `#`
comments, one record kind per file, lists `a,b,c`, maps `k=v;k=v`, floats by
`repr`, and names last.

**Each file has exactly one writer:**

| dir | file | writer | columns from |
|---|---|---|---|
| `universe/` | `systems.tsv`, `regions.tsv`, `constellations.tsv`, `celestials.tsv`, `dock-locators.tsv` | `content.py` (CP1); stations under star, planets and moons via `celestials.py` (CP2) | C1 §2.1, §5.3 |
| `belts/` | `rocks.tsv` (sorted, §1.1) | CP1 | existing format |
| `types/` | `typedata.tsv`, `skills.tsv` | CP1 | existing formats |
| `types/` | `market.tsv`, `marketgroups.tsv`, `typelists.tsv`, `materials.tsv`, `materials-random.tsv`, `compress.tsv` | `content_types.py` (CP12) | C1 §2.3 |
| `planets/` | `pi-statics.tsv` | CP1 | existing format |
| `dungeons/` | `dungeons.tsv`, `rooms.tsv`, `objects.tsv`, `gates.tsv`, `spawns.tsv`, `archetypes.tsv`, `resources.tsv` (+ `ore_band`), `room-rocks.tsv`, `agents-in-space.tsv` | `content_dungeons.py` (CP10) | C1 §2.5; C3 §7.2 |
| `npc/` | `factions.tsv`, `corporations.tsv`, `types.tsv`, `components.tsv`, `reactive-spawn.tsv`, `bounty.tsv` | `content_npc.py` (CP11a) | C6 R1, R2, §4; C1 §2.6 |
| `npc/` | `regions.tsv`, `home-pirate.tsv`, `trades.tsv`, `loot.tsv` | `content_npc_regions.py` (CP11b) | C6 R3, R7; C5 §2.3; C4 §2.4 |
| `agents/` | `agents.tsv`, `npc-characters.tsv`, `missions.tsv`, `mission-messages.tsv`, `agent-types.tsv`, `divisions.tsv`, `epic-arcs.tsv`, `schools.tsv` | `content_agents.py` (CP13) | C4 §3.1 |
| `agents/` | `lp-offers.tsv` | `content_lp.py` (CP37a) | C4 §5.2, C5 §5.6 |
| `industry/` | `blueprints.tsv` | `content_econ.py` (CP24) | C1 §2.8 |
| `stations/` | `stations.tsv`, `operations.tsv` | CP24 | C5 §3.3, C1 §2.8 |
| `econ/` | `prices.tsv` | CP24 | C5 §3.2-§3.3 |

`home_pirate` was a column of `factions.tsv` in v1. It is now its own file,
`npc/home-pirate.tsv` (`faction_id pirate_faction_id`), so that CP11a and
CP11b never write the same file.

**Dropped as duplicates** (unchanged from v1): C3's `sites.tsv` and
`site-objects.tsv`, C6's `dungeon_spawns.tsv`, C3's
`rat-factions-seed.tsv`, C1's `belts/ore-by-system.tsv`, and C4's
`jumps.tsv`.

### 1.3 Load: `--content DIR`

C1 §5.4, as CP3:

- **`content::Bundle::open(dir)` refuses** a wrong `format` major, a
  `client_build` that differs from the handshake's, and a listed file that
  is missing or has the wrong row count. Its startup line logs `format`,
  `client_build` and `extractor`.
- **`content::load(dir, &authored::Dir)`** builds every registered table
  (§1.5), with or without a bundle (CA1, landed `2ef2e6d`). An absent
  optional file gives an empty table, never a panic. A file that is there
  but wrong is a startup error naming the table (`[content] table <name>:
  …`), never a panic.
- **One startup line per table, naming its source** (CA4, landed
  `2ef2e6d`): `[content] table <name>: <origin>`, where the origin is
  `bundle`, `baked` (reprocess and industry, until CP32b and CP33b),
  `authored:disk <path>`, `authored:compiled <file>` or `fixture`
  (`content::Origin`, `content.rs:409-431`). The old `[content] N tables
  from the bundle` line overstated what was loaded (V7 F4).
- **The overrides stay working.** `--celestials`, `--belt-rocks`,
  `--planets`, `--typedata` and `--skills` override one file each. They
  are deprecated in the usage text from W13.
- **`tools/up.sh`** re-extracts when `MANIFEST.tsv` is absent, when a
  `source` md5 differs, or when the `extractor` hash differs from a fresh
  hash of `tools/content*.py` and its readers. It then runs
  `content.py --check` and **refuses to boot on any failure**. It passes
  `--content out/content`.
- **CA2 (pending, the review's `up.sh` and `soak.sh` packets):** `up.sh` and
  `soak.sh` pass `--authored "$REPO/content"`, and the startup line prints
  the absolute path. `up.sh` gets the extractor hash from
  `content.extractor_hash()` (`python3 -I`), and its bash copy of the recipe
  (`up.sh:119-130`, V7 F9) is deleted, so a W12 section that imports a new
  reader cannot leave a stale bundle booting.
- **Loading is eager, except for rocks.** Catalogue files load at startup.
  Rocks are lazy per system, through CP14's offset index into the sorted
  `rocks.tsv`.

### 1.4 Authored data: `--authored DIR`

`src/authored.rs` (CP8) parses the repo's own tables:

- each file is read from `--authored DIR`, which defaults to `content/` and
  is passed by `up.sh` as an absolute path (CA2: pending; until it lands, a
  server started from another directory silently uses the compiled-in
  copies, V7 F2);
- tables reach the directory through `Sources::authored(rel, compiled,
  header)` (CA1, landed `2ef2e6d`), which notes the source for the
  per-table startup line;
- each consumer also compiles its file in with `include_str!`, so the binary
  always has the shipped defaults;
- **the startup line names the source used for each file** (disk or
  compiled-in).

Tests may parse a shipped file for well-formedness. They may not assert any
value in it.

### 1.5 How code reads loaded data (ruling 3)

- **`Content`** (`src/content.rs`) is a typed map: `TypeId → Box<dyn Any +
  Send + Sync>`, with `get::<T>() -> &T`. It is built once and held by the
  `Registry` as `Arc<Content>`, reached through `reg.content()`.
- **Each data module defines its own table type.** The module (for example
  `dungeon_data::Table`) exposes `fn load(&Sources) -> Result<Table,
  String>` and `fn fixture() -> Table`, made of **invented** rows. Pure
  functions take `&Table`.
- **Registration** is one line per module in `src/content_tables.rs`
  (`TABLES: &[TableSpec]`). The file is coordinator-only, the same pattern
  as `DOMAINS` (S7). **CA1 (landed `2ef2e6d`):**
  `TableSpec { name, load: fn(&Sources) -> Result<Boxed, String>, fixture }`
  (`content.rs:498-510`), where `Sources { bundle: Option<&Bundle>,
  authored: &authored::Dir }` (`content.rs:443`).
  - `content::load` calls `load` for **every** table, **even with no
    bundle**. A class-B table (§2.1) with no bundle returns its fixture; a
    class-C table reads `authored` through `Sources::authored`.
  - A wrong file is an `Err`, which `content::load` reports as a startup
    error naming the table. No `load` panics.
  - `Content::fixture()` (tests) still calls `fixture` only.
  - **Every "registers `TABLES` +=" line in §6.2 means this signature.**
- **`Registry::new(config)` uses `Content::fixture()`**, which is every
  registered table's `fixture()`. So every existing test, and every wire
  test, runs on the fixture with no setup. A test that needs other rows
  calls the pure function with its own table, or uses
  `Registry::with_content(Arc<Content>)`.
- **The server without `--content`** runs on the fixture and prints a loud
  `[content] NO BUNDLE: running on invented fixture rows; use tools/up.sh
  to play` line at startup, and again on every login. **The bundle is needed
  to play, never to test.**
- **No new `OnceLock`.** **CA7: there are seven process statics, not
  three** (V1 §4, V7 F5). Each has an owner:

  | static | holds | retired by |
  |---|---|---|
  | `typedata.rs:807` | the typedata table | CP58a, then X2 |
  | `skill.rs:708` | the skill table | CP59a, then X2 |
  | `skill.rs:709` | the fallback items | CP59a |
  | `planet.rs:4415` | the PI statics | CP60 |
  | `planet.rs:4416` | the PI item types | CP60 |
  | `npc.rs:522` (`derived()`) | NPC types derived from typedata | CP20 (W12, owns `npc.rs`), which replaces it with `npc_data` |
  | `dogma.rs:967` | the per-ship sheet cache, keyed on one process-wide typedata | CP58c (owns `dogma.rs`), as a world slot |

  Until its owner lands, each keeps working as it is.

### 1.6 Two kinds of extractor check (ruling 1)

| kind | where | runs | may contain |
|---|---|---|---|
| **unit** | `tools/test_content_<x>.py`, stdlib `unittest`, runnable as `python3 -I tools/test_content_<x>.py -q` | in the gate, on every push; host `python3`, no client | synthetic bytes or synthetic TSV only. No client id, count or name |
| **check** | the section's `check(ctx)` | `content.py --check`, by `up.sh` before every boot, and by the lane in `analysis.sh`; output pasted into the lane note | every [V] count, closure and known id (for example 260,242 objects, 5,210 stations, agent faction closures) |

---

## 2. The authored-data policy

### 2.1 Three classes of data

| class | what it is | where it lives | example |
|---|---|---|---|
| **A: protocol vocabulary** | ids and constants the wire and the logic need (service names, enum values, branch group ids, dialog keys, refTypes, archetype ids, attribute ids) | Rust source, cited [V] | `groupCosmicAnomaly = 885`, `dunArchetypeCombatSites = 24` |
| **B: client content rows** | per-type or per-row numbers and lists | **`out/content/` only** | hull numbers, `typematerials`, blueprints |
| **C: what we invent** | policy tables and tuning | `content/*.tsv` (tables) or Rust consts (scalars) | `population.tsv`, `B(sec)` anchors, reward multipliers |

Class B never enters the repo. A test that needs class-B-shaped data uses a
`fixture()` table of invented rows.

### 2.2 What moves out

- **`reprocess/tables.rs`** (3,916 lines) and **`industry/tables.rs`** (1,564
  lines) are deleted:
  - CP32a and CP33a first route every consumer through a `Table` with no
    change in behaviour. The consumers are `reprocess.rs` and
    `service/reprocess.rs`; `industry.rs` and `service/industry.rs`
    (`:2450` iterates `tables::BLUEPRINTS`).
  - CP32b and CP33b then switch `load` (CA1) to the bundle, move the tests
    to `fixture()`, and delete the files.
  - With no bundle, both services answer from the fixture tables.
    Reprocessing a type that is not in the fixture answers the `UserError`
    the client already shows, naming `--content`.
- **The hand rows**, each retired by the packet that switches its last
  production consumer:
  - `npc::ANGEL_HIJACKER` and `ANGEL_CARTEL`: the production consumers are
    `mission.rs:527` (CP26a) and the belt spawn (CP30). CP61 then makes them
    an invented `#[cfg(test)]` fixture;
  - `dungeon::COMPOSITIONS`: CP30;
  - `mission.rs` `COURIER`, `BLOCKADE` and `AGENT_ID`: CP26a;
  - `docking::STATIONS` (68 rows): CP3c;
  - hulls, `modules::TYPES`, skills and wrecks: CP58-CP60, gated on
    **OWNER DECISION O3**;
  - `drones::TYPES`, `scan::PROBE_TYPES` and `scan::NPC_GROUPS`: CP58e
    (CA8), also gated on O3.
- **History rewrite** is OWNER DECISION O2.

### 2.3 The no-bundle server

The no-bundle server boots, docks and runs every wire test, on
`Content::fixture()` (§1.5). The fixture has one invented system's worth of
content (one site, one rat type, one agent, one market row), so every code
path runs. It is not a world anyone plays in. Before CP58-CP60, ships,
modules and skills still come from today's built-in rows.

### 2.4 The format of `content/`

- TSV, `#` comments, a header row, one record kind per file: the same house
  format as `out/`.
- **Every [A] row carries a `why` column** with its falsifier.
- **Scalars are Rust consts** with an `[A]` doc comment and its falsifier. A
  value keyed by band, level, faction or region is a table row; a single
  number is a const.
- **Controller-tuned values are world deltas, not files.** These are
  `m_ask`, `m_bid`, `DBS_GLOBAL` and the trading index.

### 2.5 The authored files, each owned by the packet that first writes it

| file | owner | contents | seeded from |
|---|---|---|---|
| `content/README.md` | CP8 | this section, short | §2 |
| `content/population.tsv` | CP21 (W13); rows added by CP54a (W16) and CP53b (W17) | `key band archetype entry count difficulties respawn_s ttl_s why`. `key` is a stable author-chosen name and the slot key (§4.2) | C3 §4.1 T1 |
| `content/npc/region-pirates.tsv` | CP11b | sparse overrides of R3: `region_id faction_id weight why` | C6 R3 [A] rows |
| `content/facwar/warzones.tsv` | CP18 | `warzone_id faction_a faction_b hq_a hq_b why` | C2 §5 |
| `content/agents/rewards.tsv` | CP26b-2 | **our own multipliers** (ruling 9): `scale level k_isk k_bonus k_lp bonus_minutes agent_gain corp_gain why`. The ISK base comes from the client's own data at load: per level, the median bounty sum of that level's kill-mission spawns (`npc/bounty.tsv`, `dungeons/spawns.tsv`, `agents/missions.tsv`), as C5 §5.4. No EvEJS row | C5 §5.4 (formula); C4 §4 (columns only) |
| `content/econ/price-overrides.tsv`, `content/econ/meta-mult.tsv` | CP24 | `type_id price why`; `meta_group_id mult why` | C5 §3.5, §3.2 step 4 |
| `content/agents/lp-overrides.tsv` | CP37a | `table_id type_id action lp_cost isk_cost why` | C4 §5.2 |
| `content/econ/hubs.tsv` | CP35 | `station_id why` | C5 §4.3 |
| `content/wormholes/statics.tsv` | CP54a | `class static_type_id dest_class why`, authored because the client ships no statics | R-WH |
| `content/bots/hostile.tsv` | CP56 | `ship_class min_sec max_sec engage why`, the hostile bot's engagement policy [A] | §4.7 |

**Region-pirate is derived, not hand-written** (C6 R3, conflict 4). The repo
holds only the sparse override file.

---

## 3. The generator architecture

### 3.1 The shape

```
seed.rs ── mix(seed, Kind, parts) ─────────────────────────────────────────┐
out/content ─► Bundle ─► Content (tables, §1.5) ─► pure generators ─► materialise
                          dungeon_data, npc_data,   population (sites)    site.rs, site_spawn.rs
                          universe (celestial),     npc_gen (squads,loot) belts.rs / npc.rs
                          mission_pool tables,      mission_pool (offers) mission.rs
                          econ tables               econ_exchange         market readers
content/*.tsv ─► authored.rs                                               │
Registry delta map (delta.rs) ─────────────────────────────────────────────┘
liveness.rs decides WHEN materialise runs (cold → warm → hot); rollover.rs advances the day.
```

**One RNG (CA5, landed).** Every draw goes through `seed.rs`. The mission
pool draws through `seed::stream(seed, Kind::Offer, [character, agent,
offer_seq])` (`mission_pool.rs:331`, landed `2ef2e6d`), and
`delta::World::mint` calls `seed::draw_world_seed` (`delta.rs:71`, landed
`8ee703d`), so `delta::entropy`'s second seed draw is gone. Both landed
before CP26a persists any offer, so no stored offer moved.

**One rule for every generator** (C7 §4.1): its output is a pure function
of `(world_seed, system, slot key, generation)` plus loaded tables. It never
reads the wall clock, `HashMap` order or touch order. A digest pin over a
fixture world (CP4, CP21) catches any change that moves the universe. The
commit that moves it bumps `gen_version` (§4.2) and says why.

### 3.2 One owner per fact

| fact | single owner | consumers |
|---|---|---|
| system security, band, region, constellation, wormhole class | `celestial.rs` universe table (**CP3b**) | population bands, `pirate(system)`, mission security bands, LP and DBS, **crimewatch** |
| dock points | `docking.rs` from `dock-locators.tsv` (**CP3c**) | undock, warp-to-station |
| the dungeon catalogue | `dungeon_data.rs` (CP16) | population draw, sites, mission pockets, squads, tracker fields |
| `pirate(system)`, faction prefixes, wrecks, tags | `npc_data.rs` (CP20) | population, belts, mission pool, loot |
| `home_pirate(empire)` | `npc_data.rs` (CP20), from `npc/home-pirate.tsv` | mission pool, storyline |
| NPC types and capabilities | `npc_data.rs` (CP20) | `npc_gen.rs`, `npc.rs`, bots |
| live sites `(system, slot key, generation, state)` | `population.rs` (CP21) | `site.rs`, `signal_tracker.rs`, Agency, `/live`, bot site runner |
| instantiated pockets | `site.rs` (CP28a), `site_spawn.rs` (CP28b) | `dungeon_tracking.rs`, combat |
| belt rocks and depletion | `asteroid.rs` (CP14) | mining, site ore, rollover |
| agent rows and the mission pool | `mission_pool.rs` (CP19) | `mission.rs`, storyline, arcs |
| mission rewards | `mission_rewards.rs` (CP26b-2) | `mission.rs` turn-in |
| `P_ref` and the trading index | `econ_prices.rs` (CP31) | prices, industry, reprocess, insurance |
| NPC orders | `econ_exchange.rs` (CP35) | `service/market.rs` readers |
| FW warzones, occupation and VP | `facwar.rs` (CP18, CP53b) | `service/facwar.rs`, `facwar_mgr.rs` |
| wormhole connections | `wormhole.rs` (CP54a) | population (archetype 38 signatures), `wormhole_jump.rs` |
| warp and jump refusal | `movement_guard.rs` (S12, coordinator) with guards from `tackle.rs` (CP57) and `npc_ewar.rs` (CP49) | warp, gate jump |
| the world seed, `day` and the delta map | `seed.rs` (CP4), `store.rs` + `delta.rs` (CP5) | everything above |

### 3.3 The generators and how they compose

v1 §3.3 stands, with these changes:

- **Sites (C3 §4).**
  - `population::slots(system)` iterates the rows of `population.tsv`
    for the system's band, each with its `key`, then `k in 0..count`.
  - The C7 floor (`1 + ceil(pilots/3)` [A]) adds `k` values **after** the
    row's own count on the same `key`, so an edit to one row never moves
    another row's slots.
  - The draw is C3 §4.2: the [V] filters, then
    `mix(seed, Site, system, key, k, generation)`.
- **Instantiation** is split:
  - **`site.rs` (CP28a):** one ball per room-0 object in one `AddBalls2`, ore
    rocks through `asteroid::Field`, and the `dunObjectID` and
    `entityGroupID` slim extras through S9. **Erratum 12:**
    `objectiveTargetGroup` has no data source (set on 0 objects and 0
    entities), so CP28a drops it or derives it from the objective tables;
    the lane decides and its note says which.
  - **`site_spawn.rs` (CP28b):** squads from `npc_gen::dungeon_squads`
    (C6 R5), triggers, in-pocket respawn and `SITE_VACANT`.
- **Belts**, **agents** and **the NPC market**: as v1. Kill pockets are
  `site.rs` instances of the mission's `dungeonID`, never population slots.
- **Wormholes (CP54a):** a wormhole is a population signature of archetype
  38. Its far end is drawn from `content/wormholes/statics.tsv` and
  `wormholeClassID` [V, C1 §2.1]. The K162 exit appears on first jump.
- **Bots** use the same wire APIs and the same extract (C6 §5.2, C7 §6.1).
  No generator is bot-specific.

### 3.4 Ids: one allocation table

**Each id kind is computed by exactly one function, and no id below is ever
persisted.** Deltas are keyed by client ids (§4.2).

| band | use | owner |
|---|---|---|
| 70,000,000-80,000,000 | belt rocks (`asteroid.rs:64-67`) [V]. Recomputed from `(belt, n)` on load | asteroid |
| 5e9 upward | player market orders (`market.rs:408`) [V] | market |
| 7e9-8e9 | NPC balls (`npc.rs:55,58`) [V] | npc |
| 8.0e9-8.1e9 | customs offices, `8e9 + planetID` (`planet.rs:4134`) [V] | planet |
| **8.1e9-8.7e9** | **sites**: `8_100_000_000 + ((sys_rank·64 + slot)·1024 + generation % 1024)`, where `sys_rank` is the **rank of the system id among `universe/systems.tsv`'s ids sorted ascending**, a function of the client's id set and not of row order, below 9,155 (8,490 systems [V]), and `slot < 64` is the slot's index in `slots(system)` | population (CP21) |
| 8.7e9-9.0e9 | instance objects: `8_700_000_000 + (seq % 300,000)·1,000 + n`, n < 1,000 [V: the largest dungeon has 813 objects] | site (CP28a) |
| ≥ 9e9 | escrow (`item.rs:1644`, `is_escrow` is `>=`) [V] | item |
| 1e13 + … | NPC orders (C5 §4.1) | econ_exchange (CP35) |

**The band locations are [V]. Three things are [A]:**

- **Ids above 2³¹ are safe on the client** (`npc.rs:49-51`). Falsified by a
  client path that truncates one.
- **A generation wraps after 1,024 respawns**, about 14 days for a
  20-minute slot. Falsified by a client that caches a site object longer.
- **A client update that inserts a lower system id renames the live sites
  once.** Harmless, because no site id is persisted. Falsified by a client
  that keeps a site id across a server restart.

---

## 4. The liveness model

### 4.1 Tiers

C7 §2:

- **cold:** no RAM state, never ticked;
- **warm:** docked pilots, mission destinations, unexpired deltas, or a hot
  neighbour within one jump [A];
- **hot:** a character in space, or live NPCs.

**Liveness pulls; nothing pushes to it.** v1's `Registry::touch_system`
does not exist. CP6 instead runs `liveness::refresh` as a tick stage (S6),
reading:

- the roster (in space),
- the sessions (docked),
- `jumps` (in flight, which warms the destination before arrival),
- and the npcs.

So no other file calls into liveness. Demotion happens after `COOL_AFTER`
[A]. A cold system answers queries through the pure generator, without
materialising anything.

### 4.2 Persistence: `world = generate(seed) + deltas`

CP5 adds two records to `store.rs`:

```
world  <seed_hex> <birth_filetime> <day> <last_rollover_filetime> <gen_version>
delta  <kind> <tab-separated key and value fields>
```

- **The delta map is the single truth.** `src/delta.rs` (S7) is a `Deltas`
  map, `kind → (key fields → value fields)`, held by the `Registry`.
  `store.rs` saves and loads every row **raw**, so an unknown kind is
  preserved verbatim. A module reads `reg.deltas().of(kind)` and writes
  through `put` and `del`. **No module keeps a private copy.**
- **Keys are client ids that stay stable, never row positions** (ruling 4):

| kind | key → value | owner | expires |
|---|---|---|---|
| `rock` | `belt_item_id n → type_id x y z remaining`. `n` is the rock's ordinal among **that belt's own client rows**. On load, if the bundle's rock `(belt, n)` has a different `type_id` or position, **the delta is dropped** | asteroid (CP14) | at regrowth, deleted when full |
| `site` | `system_id archetype key k → generation state at dungeon_id`. On load, if the re-draw gives another `dungeon_id`, **the delta is dropped** | population (CP21) | at respawn or ttl expiry |
| `beltwait` | `belt_item_id → respawn_at` | belts (CP30) | when it passes |
| `mission` | `char agent → mission_id state offer_seq accepted_at expires_at history` | mission (CP26a) | at C4 §3.7 expiry |
| `npcorder` | `station_id type_id side → remaining touched_at` | econ_exchange (CP35) | pruned when full |
| `npcmult` | `station_id type_id side → m` | econ_exchange (CP35) | never |
| `mktidx` | `type_id → avg day` | econ_prices (CP31) | never |
| `dbs` | `system_id → d touched_at` | econ_dbs (CP38) | at 1.0 |
| `lp` | `char corp → balance` | loyalty (CP37b) | never |
| `arc` | `char arc_id → state completed_at` | epic_arc (CP41) | never |
| `fwvp` | `system_id → score occupier taken_at credited`. `credited` is the completion time of the newest FW complex already counted (CP53b) | facwar (CP53b) | never |
| `wh` | `system_id key k → dest_system expires_at mass_left generation`. `generation` is the slot's hole the row debits (CP54a) | wormhole (CP54a/b) | at expiry |

- **Every kind above is registered** (CA16's second half, landed
  `8ee703d`): `src/delta_kinds.rs` `KINDS` has one `(kind, owner)` row per
  line of the table, in this order, and `Deltas::put` of an unregistered
  kind panics in debug. The owning module exports `pub const KIND: &str`
  when it lands. A new kind is a coordinator `KINDS` row, and a change to
  the list is a world-format change.
- **Every Registry has a world record** (CA6, landed `8ee703d`):
  `Registry::new` carries `World::FIXTURE`, and `reg.world()` returns
  `&World`, never `None` (`service.rs:717`). `server.rs` replaces it on
  load or mint. **Generator briefs have no `None` branch.**
  `Registry::tainted` exists and is set by nobody yet.
- **`gen_version`** is a const in `population.rs`. Bumping it on purpose
  (when the digest pin moves) drops every `site` and `wh` delta in one
  place.
- **Not persisted** (C7 §3.1): NPC balls, wrecks, locks and pocket balls.
- **The allocator leap on load** (C7 §3.4) ships in CP5.

### 4.3 Seeding and the day

v1 §4.3 stands: one `world_seed` drawn at birth (by
`seed::draw_world_seed`, CA5); `day_of(t)` counts days since the first
11:00 UTC after birth; sites do not take `day`; agent offers do;
`warp_in::PyRandom` is untouched.

### 4.4 The rollover (ruling 7)

C7 §4.2 is adopted:

- **The trigger** is the first tick after **11:00 UTC** where
  `day_of(now) > world.day`.
- **No disconnect.** The work runs in slices of 64 systems per tick [A], a
  catch-up runs once with `elapsed_days`, and `/rollover now` forces it.
- **The steps are a list** in `src/rollover_steps.rs`
  (`STEPS: &[(&str, fn(&mut Registry, day))]`, coordinator-only after CP25
  lands). In order:
  1. advance `day`;
  2. `asteroid::on_rollover` (CP14);
  3. `population::on_rollover` (CP21);
  4. `econ_exchange::on_rollover` (CP35);
  5. `mission::on_rollover` (CP26a);
  6. `belts::on_rollover` (CP30);
  7. `econ_ledger::on_rollover` (CP43);
  8. the `[rollover]` log line.
- **The jail sets `TZ=UTC`** for our own clients.
- **Known limitation [I]:** the client computes downtime from local time
  (`gametime/downtime.pyc:31-36`, `datetime.now()`). A friend's client in
  another time zone shows a reset countdown off by their UTC offset. This
  is cosmetic unless a client timer gates an action. **Human check H-ROLL**
  (`docs/guides/human-checks.md`): a friend in another zone opens a daily UI and
  confirms that only the countdown differs.

### 4.5 Saves: OWNER DECISION O1

As v1: CP15 is rule-neutral (the ticker also saves under the lock, and adds
the `[save]` measurement), and CP52 (the off-lock saver) is gated on O1.
**CA17:** O1 now has a measured trigger (§9).

### 4.6 Tick budget, sharing and observability

As v1 (C7 §5-§7). Per-stage timings come from S6's `tick::run_all`, which
returns `(name, Duration)` per stage, and CP15 writes them into
`TickHealth`. Degradation uses CP40's bot marker. **CA13:** CP15 also
splits lock wait from lock hold and adds a `[turns]` line (§6.2 W12).

### 4.7 Bots: co-op, opposition and courtesy (ruling 5)

- **Every role in `eve-bot/src/roles.rs` (S11) carries `civilian: bool`.**
  `mine`, `courier` and `builder` are civilian. `escort`, `sites` and
  `hostile` are not.
- **Courtesy (CP39) applies only to civilian roles.** A civilian yields when
  a non-fleet human is within `COURTESY_RANGE` [A]. A non-civilian role
  never yields.
- **The site runner (CP55)** reads Agency counts
  (`dungeonInstanceCacheMgr`, C2 §3), warps to an anomaly through the S8
  resolver, clears it and loots. It competes with humans only through the
  supply floor (C7 §6.2).
- **The hostile bot (CP56)** is **opt-in per bot** (`--hostile`). It roams
  within `HOSTILE_RANGE_JUMPS` [A] of the humans' home and engages a
  non-fleet human by an authored policy table, `content/bots/hostile.tsv`
  [A] (`ship_class min_sec max_sec engage why`). It respects crimewatch: in
  security above 0.0 it engages only where the flag rules in CP3b allow,
  so it is never CONCORDed for an action a player could not take either.
- **The bot marker stays bot-side**: a name list the bots read, never slim
  or overview data a human client renders.

---

## 5. Conflicts, decided

v1's conflicts 1-11 and 13-20 stand as written in v1, with renumbered packet
names where §6.4 says so. Changes and additions:

| # | conflict | decision | why |
|---|---|---|---|
| 4 (amended) | where `home_pirate` lives | its own file, `npc/home-pirate.tsv`, written by CP11b and read by `npc_data` only | one writer per file (CP11a writes `factions.tsv`) |
| 12 (**changed**) | mission rewards: C4's EvEJS table vs C5's `k_L × median bounty` | **C5's formula with our own multipliers** in `rewards.tsv`; the bounty base comes from the client's data at load | ruling 9: no EvEJS rows. Missions and ratting then pay comparably by construction |
| 14 (amended) | committed tables | delete, via a and b halves; the no-bundle server runs on the fixture tables | rulings 3, R1 §1.3 |
| 21 | how code reads data: per-module `OnceLock` (tree, R1) vs a world-held value | **`Content` held by the `Registry`** | ruling 3: per-test fixtures, and a no-bundle server that still runs |
| 22 | delta keys: v1 derived ids and row indices vs client ids | **client ids** plus type and position guards | ruling 4 |
| 23 | liveness feeding: v1 `touch_system` call sites vs a pull | **pull, as a tick stage** | no shared call sites; jumps in flight are already in `Registry` |
| 24 | bot courtesy vs PvP bots | **courtesy is for civilian roles only**; hostile is opt-in | ruling 5 |
| 25 | `types.factionID` (C6 §2.1 says unusable) vs K1 | **K1**: it is at +72 and present on 1,383 types [V]. R1 takes it first where present | the C6 reading used the wrong offset |

---

## 6. The packets

### 6.1 Wave 0 (landed) and W10.5, the coordinator seams

**Wave 0 has landed:** B1 (`80d63e4`) and K1 (`0ed5edf`). **So have W10.5
and W11** (§6.2), and the review's content seams: `TableSpec` with
`Sources` and an error path, one source line per table, and the mission
pool on `seed::stream` (`2ef2e6d`, CA1, CA4, CA5); a world record in every
`Registry`, `delta_kinds::KINDS` and `Registry::tainted` (`8ee703d`, CA5,
CA6, CA16's delta-kind half); and RIDX, the per-system rock index
(`ee8070c`), which CP14' keeps (CA3).

**W10.5 runs before W11.** The coordinator lands it alone, serially, one
commit per seam, with the gate green after each. Every W11 lane starts
from the post-W10.5 commit. **No seam changes behaviour:**

- every existing test passes unchanged;
- each seam adds the one test named below, which pins the new extension
  point;
- moves are verbatim, and tests move with the code they test.

**The registration files.** After its seam, each of these is
**coordinator-only**. A packet exports the item, and the coordinator adds
the one line at landing:

- `src/service.rs` `DOMAINS`
- `src/lib.rs` `mod`
- `src/tick.rs` `STAGES`
- `src/world_ext.rs` `SLOTS`
- `src/content_tables.rs` `TABLES`
- `src/warp_subject.rs` `RESOLVERS`
- `src/scan_sources.rs` `SOURCES`
- `src/slim_extra.rs` `SOURCES`
- `src/mission_hooks.rs` `HOOKS`
- `src/movement_guard.rs` `WARP_GUARDS` and `JUMP_GUARDS`
- `src/slash.rs` `COMMANDS`
- `src/rollover_steps.rs` `STEPS` (created by CP25)
- `eve-bot/src/roles.rs` `ROLES`
- `src/delta_kinds.rs` `KINDS` (landed `8ee703d`; CA16)
- `src/kill_hooks.rs` `PLAYER` and `NPC`, once the review's seam S13
  lands (CA15)
- `src/session_hooks.rs` `ON_LOGOFF` and `ON_LOGIN`, once the review's
  seam S14 lands (CA16)
- `eve-bot/tests/bots/main.rs` `mod` lines, once the review's
  one-bot-test-binary move lands (CA14)

`src/` means `server/crates/eve-server/src/`, and `eve-bot/` means
`server/crates/eve-bot/`.

| seam | files | the exact refactor and the extension point | the test that pins it |
|---|---|---|---|
| **S1** slash registry | `src/service.rs`, `src/slash.rs` (new), `src/lib.rs` | Move `Registry::slash` (`service.rs:2362`) and its arms (`giveisk`, `tp`, the rest) to `slash.rs`. Add `pub struct Command { name: &'static str, usage: &'static str, run: fn(&mut Registry, Option<i64>, &[&str]) -> Result<PyValue, ServiceError> }` and `pub const COMMANDS: &[Command]`. `Registry::slash` becomes a lookup in `COMMANDS`, and an unknown name answers exactly as today | `slash::tests::command_names_are_unique_and_lowercase`; the existing `/giveisk` tests in `service/tests.rs` pass unchanged |
| **S2** split `service/space.rs` | `src/service/space.rs`, `src/service/sites.rs`, `src/service/facwar.rs`, `src/service/bounty.rs`, `src/service/tales.rs` (all new except `space.rs`), `src/service.rs` (`DOMAINS` +4) | Verbatim moves of `SERVICES` entries and their `dispatch` arms. **`sites.rs`:** `scanMgr` (not the `scanMgr::systemScanMgr` object, which stays), `dungeonInstanceCacheMgr`, `dungeonExplorationMgr`. **`facwar.rs`:** `fwWarzoneSolarsystem`, `fwCharacterEnlistmentMgr`, `fwVictoryPointMgr`, plus B1's `WARZONES` and `HQ_SYSTEMS`. **`bounty.rs`:** `dynamicBountyMgr`, `bountyProxy`. **`tales.rs`:** `taleMgr` | the existing `no_service_is_claimed_by_two_modules` test, plus `service::tests::every_moved_service_still_routes` (each moved name resolves to its new module's `dispatch`) |
| **S3** belts out of `dungeon.rs` | `src/dungeon.rs`, `src/belts.rs` (new), `src/npc.rs`, `src/lib.rs` | Move `tick_belts`, `COMPOSITIONS`, `Belts` and the belt consts (`BELT_RANGE`, `VACANT_DESPAWN`, `RESPAWN_AFTER`) to `belts.rs`. `npc.rs:679`'s field becomes `belts: crate::belts::Belts`, and `:942` calls `crate::belts::tick_belts` | the belt tests move with the code and pass; `belts::tests::consts_are_reexported_nowhere_else` (`dungeon.rs` keeps no belt const) |
| **S4** split `npc.rs` | `src/npc.rs`, `src/npc/brain.rs`, `src/npc/loot.rs`, `src/npc/bounty.rs` (new) | `npc.rs` keeps types, `Faction`, `derive`, `Npcs` and `tick`. `decide` and `apply` go to `brain.rs`, `loot` to `loot.rs`, `pay_bounty` to `bounty.rs`, each with its tests. `npc.rs` re-exports with `pub use` so no caller changes | the moved tests pass; `npc::tests::reexports_cover_every_moved_item` (a compile-time use list) |
| **S5** agents out of `character.rs` | `src/service/character.rs`, `src/service/agents.rs` (new), `src/service.rs` | Move the `agentMgr` and `epicArcStatus` `SERVICES` entries and arms (`character.rs:42-43`, `:1447-1451…`), their tests (the `agentMgr` calls at `:5508-5678` and the `epicArcStatus` calls at `:6039-6048`) and `fn agent_rowset` (`service.rs`, by name) to `agents.rs`. Add a `DOMAINS` row | the moved tests pass; `no_service_is_claimed_by_two_modules` |
| **S6** tick stages | `src/push.rs`, `src/tick.rs` (new), `src/lib.rs` | Replace the hand-written calls at `push.rs:701-722` with `crate::tick::run_all(&mut w, now, ft)`. `tick.rs` defines `pub struct Stage { name: &'static str, run: fn(&mut Registry, Instant, BlueTime) }` and `pub const STAGES: &[Stage]` = modules, combat, mining, npc, jump, skill_training, **in today's order**. `run_all` returns `Vec<(&'static str, Duration)>`, which `push.rs` ignores until CP15. `targeting::sweep_out_of_range` stays explicit, before `run_all`, because it returns a count | `tick::tests::stage_order_is_the_dependency_order` asserts the rule: modules before combat (combat consumes cycles), combat before npc (a ship killed this tick is out of the roster), all before `take_due` |
| **S7** Registry doors | `src/service.rs`, `src/world_ext.rs`, `src/content.rs`, `src/content_tables.rs`, `src/delta.rs` (all new except `service.rs`), `src/lib.rs` | **(a) Slots:** `world_ext::Slots` (`BTreeMap<TypeId, Box<dyn Any + Send + Sync>>`), `pub struct SlotSpec { name, make: fn(&Content) -> Box<dyn Any + Send + Sync> }` and `pub const SLOTS: &[SlotSpec]`, empty. `Registry` gains `ext: Slots`, built from `SLOTS` in `new`, with `ext::<T>()` and `ext_mut::<T>()`. **(b) Content:** `content::Content` (§1.5) and `content_tables::TABLES` (empty). `Registry` gains `content: Arc<Content>`, `content()` and `with_content()`, and `new` uses `Content::fixture()`. **(c) Deltas:** `delta::Deltas` with `of`, `put` and `del`. `Registry` gains `deltas` and `deltas_mut`, not yet persisted | `world_ext::tests::a_registered_slot_is_built_once_and_typed` (a local spec list); `content::tests::fixture_builds_every_registered_table`; `delta::tests::put_get_del_round_trip` |
| **S8** site addressing: warp resolvers and scan sources | `src/service/space.rs`, `src/warp_subject.rs` (new), `src/scan.rs`, `src/scan_sources.rs` (new), `src/lib.rs` | **Warp:** move the `match subject` in `service/space.rs::warp` (`:1055-1069`) into `warp_subject::RESOLVERS: &[(&[u8], Resolver)]`, where `Resolver = fn(&Registry, actor, id: i64, raw: Option<&PyValue>) -> Option<Target>`. Rows: item, char, bookmark, scan, mission, in today's order. `resolve(table, …)` returns the **first `Some`** among the rows whose subject matches, so a later packet can add a second `item` row for ids with no ball. **Scan:** `scan_sources::SOURCES: &[fn(&mut Registry, i64, &[Probe]) -> Vec<ProbeResult>]` = `[scan::scan_ships]`, and the probe-scan caller concatenates over `SOURCES` | `warp_subject::tests::first_match_wins_and_unknown_subjects_refuse` (a local table); `scan_sources::tests::results_concatenate_in_table_order`; `a_warp_schedules_its_own_arrival` and the scan tests unchanged |
| **S9** slim extras | `src/destiny.rs`, `src/slim_extra.rs` (new), `src/lib.rs` | `Bootstrap` and the `AddBalls2` builder gain `extras: Vec<(i64, Vec<(&'static str, PyValue)>)>` with `with_extras`. `slim_pairs` appends the entry whose item id matches, the same lookup `modules` uses (`destiny.rs:1249`). `slim_extra::SOURCES: &[fn(&Registry, &[i64]) -> Vec<(i64, Vec<(&'static str, PyValue)>)>]` is empty, and the bootstrap and `AddBalls2` builders call it. `Slim` stays `Copy` | `destiny::tests::an_extra_lands_on_its_own_slim_only`; every pinned slim byte test unchanged |
| **S10** mission hooks | `src/mission.rs`, `src/mission_hooks.rs` (new), `src/lib.rs` | `pub struct MissionEvent { character_id, agent_id, mission_id, change: Change }`. `pub const HOOKS: &[(Change, fn(&mut Registry, &MissionEvent))]` is empty. `mission::converse` calls `mission_hooks::fire(HOOKS, w, &ev)` after `missions_mut().set` (`mission.rs:539`) whenever `turn.change` is `Some` | `mission_hooks::tests::only_matching_hooks_fire_in_table_order` (a local table); existing mission tests unchanged |
| **S11** bot roles | `eve-bot/src/brain.rs`, `eve-bot/src/roles.rs` (new), `eve-bot/src/driver.rs`, `eve-bot/src/bin/eve-bots.rs`, `eve-bot/src/escort.rs`, `eve-bot/src/lib.rs` | Replace `enum Role` (`brain.rs:80`) with `&'static RoleSpec`. `RoleSpec { name, greeting, civilian: bool, docked: fn, in_space: fn, banner: fn }`, and `ROLES = [MINE, ESCORT]`, with greetings, the dispatch (`brain.rs:747-763`), the driver banners (`driver.rs:512-516`, `:1434-1443`) and `--role` parsing (`eve-bots.rs:104-105`) all read from the table. `civilian` is `true` for mine and `false` for escort, and nothing reads it yet | `roles::tests::names_are_unique_and_parse_round_trips`; `mining_bots` and `escort_bots` harness tests unchanged |
| **S12** movement guards | `src/movement_guard.rs` (new), `src/service/space.rs`, `src/jump.rs`, `src/lib.rs` | `pub type Guard = fn(&Registry, i64) -> Option<Refusal>`, where `Refusal { user_error: &'static str, args }`. `WARP_GUARDS: &[Guard]` and `JUMP_GUARDS: &[Guard]` are empty. `service/space.rs::warp` (`:1024`, which the `CmdWarpToStuffAutopilot` arm at `:318` also calls) calls `movement_guard::check(WARP_GUARDS, …)` after `advance_character` and **before any state change**. `jump.rs`'s `CmdStargateJump` entry (`:146`) does the same with `JUMP_GUARDS`. A refusal answers as the caller's existing refusal path does | `movement_guard::tests::a_refusing_guard_stops_the_warp_before_any_change` (a local table, and the roster is unchanged); existing warp and jump tests unchanged |

**X1 (coordinator doc, in W10.5): `docs/content/schema.md`.** The exact
header line of every file in §1.2 and §2.5. C4 §3.1 wins over C1 §2.7.
`npc/home-pirate.tsv` is new. Producer and consumer packets each pin the
string. Every loader rejects a header that differs. X1 lands before W11,
because CP19 consumes `agents.tsv` in W11.

### 6.2 The house rules and the column key

The brief (§10) restates the rules. Columns:

- **size:** S < 100 changed lines, M < 400, L < 1,000, XL ≤ 1,500. No packet
  exceeds 1,500 changed lines or 4 owned files.
- **owns:** the only files the lane writes, besides its lane note
  `docs/lanes/<cp>.md`.
- **after:** packets or seams that must have landed.
- **registers:** lines the coordinator adds at landing, which the lane names
  in its report and writes into its coordinator diff.
- **spec:** where the lane implements from.
- **tests:** every test asserts a rule or a [V] value. Client counts are in
  `check`, and are named here as **check:**. Rows written or changed in v3
  cite `new: name` or `existing: path::name` (CA12, lanes rule 14); the
  older rows are all `new:` unless they say "existing" or "unchanged".

**Owned files that split** follow §6.3's standing rule (CA18).

**Bot tokens (CA19, ruling 2).** No bot sends its name as a token. Every
bot packet (CP34, CP39, CP47, CP55, CP56) builds `BotConfig` with the token
field the review's bot-token packet adds (`eve-bots --tokens FILE`, outside
the repo), and CP39, which owns `eve-bot/src/driver.rs` and
`eve-bot/src/bin/eve-bots.rs` in W15, keeps that `--tokens` path working.

#### W11: the foundation (12 lanes; after W10.5 and X1) — **landed**

Every W11 packet has landed except CP9, which waits on **O6** and has no
commit: CP1 `0bf9762`, CP2 `1f8ff69`, CP3 `eec5834`, CP4 `9c11144`, CP5
`41d933f`, CP6 `fa9a924`, CP7 `e12066f`, CP8 `75c320b`, CP19 + CP32a +
CP33a `42549c4`, and the coordinator hunks `8cb1fb8`. The rows below are the
record of what they were briefed to do. Since landing, the review's seam
changed three of them: CP3's `Content::from_bundle` is now `content::load`
over `TableSpec::load` (CA1, `2ef2e6d`); CP19 draws through `seed::stream`
and returns an error on a bad header (CA5, `2ef2e6d`); and CP5's world
record is in every `Registry` (CA6, `8ee703d`). CP19's family draw keys on
`chain_key`, else `name_id` (erratum 14, landed in `42549c4`).

| CP | size | owns | after | registers | spec | tests |
|---|---|---|---|---|---|---|
| **CP1** content driver, phase 1 | L | `tools/content.py`, `tools/test_content.py` | K1 | — | C1 §5.1-§5.3, K2; §1.1, §1.6 | **unit:** a fixture `content_zz.py` written to a temp dir is discovered with no driver edit; `DEPENDS` orders sections; an exception mid-section leaves no `out/content/` and no stale tmp dir; manifest rows equal rows written; the list, map and `repr` float encodings round-trip; the rocks writer sorts by (system, belt) and keeps in-belt order; the `extractor` hash changes when any listed reader's bytes change. **check:** the four phase-1 files are byte-identical to the live extractors' output; `systems.tsv` has one row per `systems.static` row, and `security` equals `solarsystemcontent` |
| **CP2** star, planet and moon stations | S | `tools/celestials.py`, `tools/test_celestials.py` | — | — | C4 §1, §11.5; ruling 6 | **unit:** a synthetic system with one station under the star, one under a planet and one under a moon emits three rows with the right parents; planet-only output is unchanged byte for byte. **check:** 5,210 unique stations [V] |
| **CP3** `--content` | M | `src/content.rs`, `src/bin/eve-server.rs`, `tools/up.sh` | S7 | — | C1 §5.4, K3; §1.3, §1.5 | on a synthetic temp bundle: a wrong `format` errs; a wrong build errs; a missing listed file errs naming it; a row-count mismatch errs; `--typedata` overrides the bundle's file; an absent optional file gives an empty table; `Content::from_bundle` calls each `TABLES` entry once; no bundle means fixture content and the warning line |
| **CP4** seed | S | `src/seed.rs` | — | — | C7 §4.1, L1 | `mix` is stable (rule: a golden vector the function itself produced at landing, so any change to it is visible); `mix` equals `asteroid::Mix` on the same input; `Stream` stays in range; `day_of` at 10:59:59 and 11:00:00 UTC [V: the client's 11:00 default] |
| **CP5** world record and delta persistence | M | `src/store.rs`, `src/delta.rs` | S7 | coordinator hunk: `to_snapshot` and `apply_snapshot` in `service.rs` carry `deltas` | C7 §3, L2; §4.2 | `world` round-trips with `gen_version`; a pre-C7 file gets a seed and is marked dirty; every `delta` row round-trips raw; an unknown kind is preserved verbatim; after load every allocator is above every id on disk (the rule, not the leap size) |
| **CP6** liveness index | M | `src/liveness.rs`, `src/space_state.rs` | S6, S7 | `STAGES` += `liveness::refresh`; `SLOTS` += `liveness::SLOT` | C7 §2, L3; §4.1 | promote and demote with hysteresis over simulated time, measured in `COOL_AFTER` units; a jump in flight warms its destination; a cold system has no entry; the per-system roster index equals `in_system`'s scan on random rosters |
| **CP7** `new_style` helper | S | `src/pyutil.rs`, `src/skill.rs` | — | — | C2 §1.2, K2 | `CharacterSkillEntry` encodes byte-identically to today; a `CosmicAnomalyInfo` fixture encodes as `0x23` with plain keys [V] |
| **CP8** authored loader | S | `src/authored.rs`, `content/README.md` | — | — | §1.4, §2 | header and `#` parsing; a malformed row is skipped and logged; the `--authored` file wins over `include_str!`; a missing file falls back to the compiled one; the source line names which was used |
| **CP9** market taxes | S | `src/service/market.rs` | **O6 = yes** | coordinator hunk: `+54` in `service/wallet.rs` `GetEntryTypes` | C5 §5.1, E4 | sales tax at Accounting 0 and 5 equals the client's formula [V]; the broker fee floor is the client's minimum [V]; ref 54 is journaled to the seller only |
| **CP19** mission pool | M | `src/mission_pool.rs` | S7, X1 | `TABLES` += `mission_pool::TABLE` | C4 §3, K2 | the same inputs give the same offer; over a fixture pool no family exceeds its uniform share by more than the binomial bound; an agent of faction F never gets enemy F; an Ammatar fixture agent draws Amarr-locked rows (the [V] rule); an L5 fixture agent draws L4; part 2 follows a chain start; `CanUseAgent` thresholds L2 1.0 … L5 7.0 [V] |
| **CP32a** reprocess behind a `Table` | M | `src/reprocess.rs`, `src/service/reprocess.rs` | S7 | `TABLES` += `reprocess::TABLE` (both `from_bundle` and `fixture` still return `tables.rs`'s rows) | R1 §1.3; ruling 3 | every existing reprocess test passes through `&Table`; `grep` finds no `tables::` outside `Table::baked()` |
| **CP33a** industry behind a `Table` | L | `src/industry.rs`, `src/service/industry.rs` | S7 | `TABLES` += `industry::TABLE` | R1 §1.3; ruling 3 | every existing industry test passes through `&Table`; `service/industry.rs:2450` reads the table; no `tables::` outside `Table::baked()` |

#### W12: extracts, loaders and the first live surfaces (13 build lanes + 3 research lanes)

v3: CP3c moved here from W13 (CA10); CP14 is CP14' (CA3); CP15 absorbs V3
R3 (CA13). CP20's brief adds CA7 and CA9's region note.

| CP | size | owns | after | registers | spec | tests |
|---|---|---|---|---|---|---|
| **CP3b** universe loader and security in crimewatch | M | `src/celestial.rs`, `src/crimewatch.rs` | CP1, CP3 | `TABLES` += `celestial::UNIVERSE` | C1 §2.1, §5.3; C3 §3; `crimewatch.rs:665-676`; `docs/re/damage-death.md` §7.2.1 | a fixture `systems.tsv` gives security, region and wormhole class; `flags_apply` is false where the client's rounded security is ≤ 0.0 and true above [V rule]; a system missing from the table falls back to today's rule |
| **CP3c** dock locators (moved from W13, CA10) | S | `src/docking.rs` | CP1, CP3 | `TABLES` += `docking::LOCATORS` | C1 §2.1 | a fixture locator gives the dock point and undock position; a station with no locator falls back to its centre plus the standoff; `STATIONS` is gone |
| **CP10** dungeons section | M | `tools/content_dungeons.py`, `tools/test_content_dungeons.py` | K1, CP1, X1 | — | C1 §2.5, K4; C3 §7.2 | **unit:** on synthetic rows, room-rocks equals the room objects; a bad `entry_object_id` is reported; `ore_band` follows C3 R4 on a synthetic resource row. **check:** 260,242 objects and 30,610 spawns [V]; resource types close into `types` |
| **CP11a** NPC section: factions and types | L | `tools/content_npc.py`, `tools/test_content_npc.py` | K1, CP1, X1 | — | C6 R1, R2, §4, N1; C1 §2.6; K1 note (`types.factionID`) | **unit:** on synthetic types, a present `factionID` wins over `sofFactionName`; the alias collapse keeps the lowest typeID; the bounty row comes from attribute 481 [V]. **check:** at least 95% `sofFactionName` agreement for the five human pirates [V]; every `npccorporations.factionID` closes into `factions`; the `factionID`/`sofFactionName` disagreements are listed |
| **CP11b** NPC section: regions, home pirate, trades, loot | M | `tools/content_npc_regions.py`, `tools/test_content_npc_regions.py`, `content/npc/region-pirates.tsv` | K1, CP1, X1 | — | C6 R3, R7; C5 §2.3; C4 §2.4 | **unit:** on a synthetic universe, R3 steps 0-4 in order (a pirate HQ region, a pirate `regions.factionID`, station majority, the empire vote, gate-BFS to the nearest pirate station); an override row replaces the derived weight; the secondary weight is applied as `SECONDARY_WEIGHT` by name. **check:** Curse→Angel, Venal→Guristas, Stain→Sansha [V]; `home_pirate` for the four empires equals the mission vote [V] |
| **CP12** types phase 2 | M | `tools/content_types.py`, `tools/typedata.py`, `tools/test_content_types.py`, `src/typedata.rs` | K1, CP1 | — | C1 §2.3; C6 R7; C5 §2.2 | **unit:** synthetic materials close into synthetic types; the typelists expansion of include and exclude sets on a synthetic list; in `typedata.rs`, a fixture category-25 row and a category-17 row install as stackable, grantable items. **check:** `market.tsv` has 53,288 rows [V]; `typelists` closes every `allowed_ships_list` |
| **CP13** agents section | M | `tools/content_agents.py`, `tools/test_content_agents.py` | K1, CP1, CP2, X1 | — | C4 §3.1, K1 | **unit:** on synthetic missions, kind, level-through-dungeon and the chain `(k of n)` parse; **the writer canonicalises `name_id` per title** (erratum 14): every variant with the same title text gets the lowest of its `name_id`s, so CP19's family draw (keyed on `chain_key`, else `name_id`) sees one family per title, with no schema change; `fixed_lp` is the client's Omega value, marked [A] (erratum 13: the client keeps separate Alpha and Omega values). **check:** 10,972 agents and 2,892 missions [V]; every agent's station exists in `universe/` (needs CP2); every title that had more than one `name_id` is listed |
| **CP14'** lazy rocks and regrowth (CA3) | M-L | `src/asteroid.rs` | CP1, CP4, CP5, CP6, RIDX (`ee8070c`) | `SLOTS` += `asteroid::SLOT`; later `STEPS` += `asteroid::on_rollover` (at CP25); `KINDS` already has `rock`; **coordinator hunks:** `bin/eve-server.rs` loads the offset index instead of `asteroid::load`; `model.rs` `SandboxConfig.rocks` becomes the index; `server.rs` moves it into the registry | C7 §1.4, L4; C3 §7.3; ruling 4; V7 F3; V3 R1 | **Design rule: the `Field` read API stays.** Rocks materialise on the liveness warm/hot edge, inside a stage, so the 19 `rocks()` reader call sites do not change, and CP14' keeps RIDX's landed `in_system` contract. new: the offset index finds every system's rows in a fixture file; materialise, mine, drop and re-materialise keeps the remainder; a restart keeps a gone rock gone; **the delta key is `(belt, n)` and a delta whose type or position disagrees is dropped**; regrowth restores rocks by ordinal at their original id and position, `REGROW_FRACTION` by name, never above the static count; a partly mined rock keeps its radius; the materialised count by type equals the file's counts [V rule: `asteroid_scanning_service:216-226`]; `a_cold_system_holds_no_rock`. existing: `asteroid::tests::in_system_is_the_filter_after_every_mutation` stays green. The lane note has a soak RSS line before and after (V7 measured about 400 MB of eager rocks in debug) |
| **CP15** tick health, `[live]`, status, ticker save, `[turns]` (CA13) | M-L | `src/push.rs`, `src/live_status.rs` | CP6, S6 | coordinator hunks: the `spawn_world_ticker` store argument in `server.rs:242` (was `:215`); **the per-turn timing in `dispatch.rs:1621-1665`** | C7 §5.2, §5.4 (rule-neutral half), §7, L6 + L7a; V3 F3, R3, R5 | `TickHealth` splits `lock_wait` from `lock_held`, with held measured after acquisition (`push.rs:665`, `:688`), and keeps `run_all`'s stage timings (max and mean). A `[turns]` line per window: turns/s, hold mean and max, wait mean and max, saves/s, save write max, and the slowest `service.method`, from per-thread counters or recorded under the lock already held, **never a new global mutex**. The clock save runs every `SAVE_EVERY` [A] (V3 R5). new: the health line names every `tick::STAGES` entry and its unit; the status TSV header equals `schema.md`'s; a clock-only mutation is saved with no payload; wait and hold are reported separately; the `[turns]` line has its named fields. existing: `loopback::the_world_survives_the_server_being_dropped` passes |
| **CP16** dungeon catalogue | M | `src/dungeon_data.rs` | CP3, X1 | `TABLES` += `dungeon_data::TABLE` | C1 §2.5; C2 §2.4; C3 §4.2 | loads a synthetic bundle; the C3 §4.2 filters as pure functions over `fixture()`; `allowed_types` resolves through typelists; absent files give an empty catalogue; a bad header is rejected |
| **CP17** signal tracker | M | `src/signal_tracker.rs`, `src/service/sites.rs` | CP7, S2, S8 | `RESOLVERS` += `(b"item", signal_tracker::anomaly_target)` | C2 §2, K3 | full state has arity 4 with dict values [V]; an add then a remove gives one `AnomalyUpdate`; deviation equals the client's `CalculateDeviationForSignal` at s = 1, 11 and 30 [V]; `anomaly_target` resolves a fixture anomaly id with no ball, and `warp_subject::resolve` over a local table reaches it after `ball_target` misses |
| **CP18** warzones and occupation | M | `src/facwar.rs`, `src/service/facwar.rs`, `content/facwar/warzones.tsv` | CP7, CP8, S2 | `TABLES` += `facwar::WARZONES` | C2 §5, K6 | from a fixture table, `GetAllWarzones` is non-empty (a client crash rule [V]) and every warzone faction maps to a zone; `GetLocalOccupationState[0]` is the session system; OccupationState keys are underscored [V]; B1's consts are gone |
| **CP20** NPC data loader | M | `src/npc_data.rs`, `src/npc.rs` | S4, CP3, X1 | `TABLES` += `npc_data::TABLE` | C6 R1-R3, §5.1, N2; CA7, CA9 | **Brief (CA9):** `pirate(system)` reads the region from the catalogue's `SystemRow.region_id` (`celestial.rs:134`), which exists now; do not wait for CP3b. **CA7:** `npc::derived()` (`npc.rs:522`) is replaced by `npc_data`, so there is one source of NPC types; whether the two can coexist for a step is the lane's call. new: derived fields from a fixture row; `needs ∋ SCRAM` for effect 563 [V]; `missile.is_some()` when attribute 507 > 0 [V]; `pirate(system)` follows the R3 steps on fixture regions; `home_pirate` reads `home-pirate.tsv`; a bad header is rejected. `ANGEL_HIJACKER` stays, because it still has production consumers |
| **R-FW** research: `facWarMgr` and FW sites | — | `docs/content/R-FW.md` | — | — | C2 §5 | delivers an implementation spec: the ~28 `facWarMgr` methods with arity and shape [V], the source of the warzone system list, the VP push rules, enlistment, and `dunArchetypesFactionalWarfare` site rows |
| **R-WH** research: wormholes | — | `docs/content/R-WH.md` | — | — | C1 §2.1, C3 §4 | delivers: the wormhole type attributes (mass, lifetime, jump mass) [V], the jump call and pushes, the K162 behaviour, the signature fields, and a proposed `statics.tsv` [A] |
| **R-TACKLE** research: scram and disrupt | — | `docs/content/R-TACKLE.md` | — | — | C6 N6; `modules.rs` | delivers: the effect and attribute ids (`warpScrambleStatus`, strength), the stacking rule, the client's warp-refusal `UserError` and its args, and which effects block MWD or MJD [V] |

#### W13: the generators (9 lanes)

v3: CP3c moved to W12, and CP26b-1 joins (CA10). CP21 and CP26a are after
CP3b (CA9).

| CP | size | owns | after | registers | spec | tests |
|---|---|---|---|---|---|---|
| **CP21** population | L | `src/population.rs`, `content/population.tsv` | CP3b (CA9), CP4, CP5, CP6, CP8, CP16, CP20 | `TABLES` += `population::AUTHORED`; `SLOTS` += `population::SLOT`; `STEPS` += `population::on_rollover` (at CP25) | C3 §4, §6, P-C3b/c; §3.3, §3.4, §4.2; ruling 4 | deterministic; the count per row key equals the fixture table's; every draw satisfies the filters; ids are unique and inside §3.4's band; refill after the row's `respawn_s` with `generation + 1`; **`on_rollover(day)` expires an unvisited site after the row's `ttl_s` and keeps one with a pilot inside**; editing one row's count moves no other row's slots; a `site` delta whose stored `dungeon_id` disagrees is dropped; a `gen_version` change drops every `site` delta; an empty candidate set gives an empty slot; `sys_rank` is injective over a fixture system set; a digest pin over a fixture world |
| **CP22** NPC generators | M | `src/npc_gen.rs`, `src/npc/loot.rs` | CP20 | — | C6 R4, R5, R7, N3 | a squad's bounty sum is at most `budget(sec)` (the rule against the function); no type outside `SUPPORTED`; the same seed gives the same squad; every member is of the requested faction; every loot type is grantable; loot is stable per NPC id |
| **CP23** NPC brain, turret only | M | `src/npc/brain.rs` | CP20, S4 | — | C6 R8, N4 | no `Shoot` before the type's own delay attribute; retarget at the type's timer; leash returns the NPC past `chase_max`; self-repair heals per the type's period |
| **CP24** econ and industry extract | M | `tools/content_econ.py`, `tools/test_content_econ.py`, `content/econ/price-overrides.tsv`, `content/econ/meta-mult.tsv` | CP12 | — | C5 §3.2-§3.5, E1 + E10; C1 §2.8 | **unit:** on a synthetic recipe graph, a manufactured type's price is the recursion's and a leaf's is its `basePrice`; every priced type is at least its floor; the trade-key decode on synthetic bytes; an override replaces the derived price. **check:** Tritanium equals its `basePrice` [V]; the Rifter's source is `recipe`; one `stations.tsv` row per station; the blueprints round-trip `blueprints.static` |
| **CP25** rollover and GM views | M | `src/rollover.rs`, `src/service/gm_live.rs`, `src/rollover_steps.rs` (created; coordinator-only afterwards) | CP5, CP6, CP15, S1, S6 | `STAGES` += `rollover::tick`; `COMMANDS` += `/live`, `/rollover`; `DOMAINS` if `gm_live` routes a service | C7 §4.2, §7.3-§7.4, L5 + L10; §4.4 | with a fake registered step: the trigger fires on the first tick past 11:00 UTC with `day_of(now) > day`; slices touch at most `SLICE` systems per tick, by name; catch-up runs once for an N-day gap; running twice for one day is idempotent; `/live` holds the `[live]` fields; a bad system id answers usefully |
| **CP26a** mission state N×N | L | `src/mission.rs` | CP5, CP13, CP19, CP3b, S10 (CA9: CP20 dropped, CP3b added) | `SLOTS` += `mission::SLOT`; `STEPS` += `mission::on_rollover` (at CP25) | C4 §2.7, §3.7-§3.9, K3; CA9 | **CA9:** courier offers need no NPC data. Kill-template offers are filtered out while `content().try_get::<npc_data::Table>()` is `None`, and `BLOCKADE` stays the only kill mission until CP20 lands. Offers draw through `seed::stream` (CA5, landed). new: the existing tests pass with invented fixture missions; two agents' missions coexist; a missing `offered` slot produces the `MissionOfferN` label tuple [V]; the standing gate refuses below and accepts at the [V] threshold; the `mission` delta round-trips; once `npc_data` is loaded, pocket rats come from it by the mission's enemy faction (no `ANGEL_HIJACKER`), and with no `npc_data` no kill template is offered; `COURIER` and `AGENT_ID` are gone, and `BLOCKADE` goes when CP20 has landed |
| **CP26b-1** agents from the mission pool (CA10) | S | `src/service/agents.rs` | CP26a, CP19, S5 | — | V7 §5 Milestone B; C4 §3.1 | `agent_rowset` (`service/agents.rs:228`) serves `mission_pool::Table`'s agents; the fixture gives one invented agent. new: every rowset row has 10 elements [V]; the fixture's invented agent appears at its station; `GetAgents` on a fixture world lists every fixture agent. Until CP26b-2 lands, couriers pay the reward `mission.rs` pays today, marked [A]. The lane measures `GetAgents` with 10,972 rows against the client's 30 s frame cliff and puts the number in its note |
| **CP27** interference and dynamic resources | S | `src/service/live_misc.rs` | B1, CP7 | — | C2 §6, K7 | the interference state keys satisfy the class invariants [V]; the settings dict has the [V] key set |
| **CP51** world-event tales | S | `src/tales.rs`, `src/service/tales.rs` | CP7, S2 | — | C2 §7, K8 | a tale without `objectiveChainID` is never sent; the values carry `taleID` and `managerSolarSystemID` [V] |

#### W14: content reaches space (7 lanes)

v3: CP26b is now CP26b-2, after CP22 (CA10, erratum 11).

| CP | size | owns | after | registers | spec | tests |
|---|---|---|---|---|---|---|
| **CP26b-2** mission rewards (CA10) | M | `src/service/agents.rs`, `src/mission_rewards.rs`, `content/agents/rewards.tsv` | CP26b-1, CP22 (erratum 11), CP11a, CP10 | `TABLES` += `mission_rewards::TABLE` | C4 §4, K4; C5 §5.4; ruling 9; erratum 11 | **Erratum 11:** dungeon entity ids are not type ids (5,777 of 30,586 coincide only by chance), so the bounty base needs CP22's squad fill. new: the rowset shape is unchanged and every row has 10 elements [V]; the ISK base is the median of the fixture level's spawn bounty sums, computed from the fixture and not typed in; reward = `k_isk × base` for a fixture row; the bonus applies only inside `bonus_minutes`; refs 33 and 34 [V] |
| **CP28a** site instantiation: balls | L | `src/site.rs` | CP14, CP16, CP21, CP22, S8, S9 | `SLOTS` += `site::SLOT`; `slim_extra::SOURCES` += `site::slim_extras`; `RESOLVERS` += `(b"item", site::object_target)` | C3 §4.5, P-C3d; C2 §4 | every room-0 object becomes a ball at `anchor + position` in one `AddBalls2`; `every_slim_has_a_ball`; every site slim carries `dunObjectID` and `entityGroupID` [V names]; `objectiveTargetGroup` is dropped or derived from the objective tables, and the lane note says which (erratum 12); ids are inside the instance band; ore rocks go through `asteroid::Field` |
| **CP29** sites on the wire | M | `src/signal_tracker.rs`, `src/service/sites.rs` | CP17, CP21, S8 | `scan_sources::SOURCES` += `signal_tracker::signatures`; `RESOLVERS` += `(b"scan", signal_tracker::signature_target)` | C2 §2.4, §3, K4 | population add and remove produce tracker deltas; Agency rows for the own system, ints for others, per archetype [V shapes]; a fixture signature appears in probe results through its source and is warpable once its certainty reaches the client's warpable threshold [V]; every `dungeonID` exists in the catalogue |
| **CP30** belts v2 | M | `src/belts.rs` | CP6, CP20, CP22, S3 | `STEPS` += `belts::on_rollover` | C3 §8, P-C3e; C6 R4 | a system's rats are of `pirate(system)`; the decision is not re-taken inside `DECISION_CADENCE`; it consumes the liveness hot list; `COMPOSITIONS` and the belt use of `ANGEL_HIJACKER` are gone; the fixture content still spawns a squad |
| **CP31** price oracle | M | `src/econ_prices.rs`, `src/service/base.rs` | CP3, CP24 | `TABLES` += `econ_prices::TABLE` | C5 §3.1, §3.4, E2 | a dict of `KeyVal` with `adjustedPrice` and `averagePrice` [V names]; the EWMA stays inside its clamp (relation to `CLAMP`); empty with an empty table; the `mktidx` delta round-trips |
| **CP37a** LP offers extract | M | `tools/content_lp.py`, `tools/test_content_lp.py`, `content/agents/lp-overrides.tsv` | CP24 | — | C4 §5.2; C5 §5.6; conflict 13 | **unit:** on a synthetic offer, `LP·R + ISK + Σreq` equals `P_ref` within rounding, with `R` and the ISK share named; an override row wins. **check:** every offer type closes into `types` |
| **CP40** degradation and the bot marker | S | `src/liveness.rs`, `src/bin/eve-server.rs` | CP6, CP15 | — | C7 §5.3, L8 | under a budget overrun, a bot-only hot system is ticked strictly less often than a human system; a system with a human is never skipped; the `--character` bot spec sets `is_bot` |

#### W15: economy, missions and sites in depth (10 lanes)

v3: bot tests live in the one bot test binary (CA14); bots send a token from
`--tokens FILE`, never their name (CA19); a logoff step is a
`session_hooks::ON_LOGOFF` row (CA16); a kill side effect is a `kill_hooks`
row (CA15). The review lands all four seams before W15.

| CP | size | owns | after | registers | spec | tests |
|---|---|---|---|---|---|---|
| **CP28b** site squads and lifecycle | L | `src/site_spawn.rs` | CP28a, CP22, CP23 | `STAGES` += `site_spawn::tick` | C3 §4.5, §6; C6 R5; conflict 6 | ungated squads on arrival, gated ones by the R5 rule; `entitySpawnOnce` never respawns [V]; in-pocket respawn inside the object's [min, max] [V]; completion then `SITE_VACANT`, by name, gives one `RemoveBalls`; completion marks the population slot |
| **CP32b** reprocess off `tables.rs` | L | `src/reprocess.rs`, `src/reprocess/tables.rs` (deleted), `src/service/reprocess.rs` | CP32a, CP12, CP31 | — | §66; C1 Proposal 7; C5 §5.2 | fixture materials give the client's yield formula [V]; the gas refine is charged; a type outside the fixture is refused with a message naming `--content`; no generated table remains |
| **CP33b** industry off `tables.rs` | L | `src/industry.rs`, `src/industry/tables.rs` (deleted), `src/service/industry.rs` | CP33a, CP24, CP31 | — | §66; C5 §5.2 | the EIV of a fixture blueprint equals the client formula over `p_ref` [V]; the service tests run on `fixture()`; no generated table remains |
| **CP34** bot courier role | S | `eve-bot/src/verbs.rs`, `eve-bot/src/courier.rs`, `eve-bot/tests/bots/missions.rs` (CA14; was `eve-bot/tests/missions.rs`) | CP26b-1, S11 | `ROLES` += `courier::ROLE` (civilian); `mod missions;` in `eve-bot/tests/bots/main.rs` (CA14) | C4 K8; CA19 | `BotConfig` carries the token field, never the name as a token (CA19). new: a bot accepts, carries and completes a courier mission drawn from the fixture against a live server in the harness |
| **CP35** NPC exchange | L | `src/econ_exchange.rs`, `src/service/market.rs`, `content/econ/hubs.tsv` | CP5, CP25, CP31 | `STEPS` += `econ_exchange::on_rollover` | C5 §4, E3 | an NPC ask mints with no escrow; an NPC bid destroys; after `TRICKLE_PERIOD·k` the refill equals `trickle(k)`, capped at full; ids are disjoint from the 5e9 and 7e9 bands; the C5 §6 arbitrage invariants as property tests over a fixture catalogue; `GetRegionBest` includes NPC asks |
| **CP36** dungeon tracking | S | `src/dungeon_tracking.rs` | CP28a | — | C2 §4, K5 | `OnEnteringDungeonRoom` has arity 5 [V]; completion fires once; exit on warp-out |
| **CP37b** LP wallet and store | M | `src/loyalty.rs`, `src/service/wallet.rs` | CP37a, CP26a, S10 | `HOOKS` += `(Completed, loyalty::on_complete)`; `SLOTS` += `loyalty::SLOT` | C4 §4-§5, K5; C5 §5.6 | balances are `(corpID, balance)` pairs [V]; an offer is a `KeyVal` with the 8 [V] keys; `TakeOfferForCharacter` debits and creates; too little LP answers falsy; a completion event credits the mission's `fixed_lp` (the Omega value, [A], erratum 13; the test asserts the credit equals the fixture row's `fixed_lp`, never a number) and pushes a 4-argument `OnLPChange` [V] |
| **CP38** DBS | M | `src/econ_dbs.rs`, `src/service/bounty.rs`, `src/npc/bounty.rs` | CP4, CP25, S2, S4, S13 | `kill_hooks::NPC` += its bounty step, if it changes one (CA15: never an edit to `combat.rs`) | C5 §5.3, E5 | the pool drains and recovers lazily; the push fires on a step of at least `DBS_PUSH_STEP`, by name; the answer is a `(float, bool)` tuple [V] |
| **CP39** bot pacing, homes and courtesy | M | `eve-bot/src/driver.rs`, `eve-bot/src/brain.rs`, `eve-bot/src/bin/eve-bots.rs` | CP40, S11, the review's bot-token packet | `session_hooks::ON_LOGOFF` += a row, if courtesy needs a logoff step (CA16) | C7 §6.3, L9; ruling 5; ruling 2 (CA19) | **CA19:** keeps the `--tokens FILE` path and the `BotConfig` token field working; no token, key or role is derived from a visible name. The bot-token packet's `driver::tests::the_token_is_never_the_name` stays green. new: the payload rate never exceeds `BOT_PAYLOADS_PER_S` over any `PACING_WINDOW`; a civilian role yields when a non-fleet human is within `COURTESY_RANGE`; a non-civilian role never yields; a home is within `HOME_RANGE_JUMPS` of the humans' home |
| **CP61** retire the hand rat | S | `src/npc.rs` | CP26a, CP30 | — | §2.2 | `ANGEL_HIJACKER` and `ANGEL_CARTEL` are `#[cfg(test)]` with invented numbers; no non-test code names them; the `combat.rs` and `drones.rs` tests that use them by name still pass |

#### W16: depth and the PvP loop (12 lanes)

v3: kill side effects register in `kill_hooks` (CA15, seam S13), bot tests
live in the one bot test binary (CA14), and every bot sends a token, never
its name (CA19). CP46 gains CA9's `--content` hunks.

| CP | size | owns | after | registers | spec | tests |
|---|---|---|---|---|---|---|
| **CP41** epic arcs | M | `src/epic_arc.rs`, `src/service/agents.rs` | CP26b-2, S10 | `HOOKS` += `(Completed, epic_arc::on_complete)` | C4 §7, K6 | `{}` for a new character [V]; one entry after a fixture start is accepted; a fixture branch leads to one of its `nextMissions`; an end mission sets the replay; arcs hidden by `IsValidArc` are never offered [V rule]. **check (CP13):** the message map's 278 missions |
| **CP42** market history and pruning | M | `src/service/market.rs` | CP25, CP35 | — | C5 §8, E7 | the aggregate equals the trades; pruning keeps `HISTORY_DAYS`, by name; a world with no history loads |
| **CP43** econ ledger and `/econ` | M | `src/econ_ledger.rs`, `src/service/gm_econ.rs` | CP25, CP40, S1 | `COMMANDS` += `/econ`; `STEPS` += `econ_ledger::on_rollover` | C5 §7.3-§7.4, E8 | the ref classification is total over the refs in use; the bot burn caps at `BOT_WALLET_CAP` and journals it; the C5 §5.4 reward sanity line |
| **CP44** insurance | M | `src/service/insurance.rs`, `src/service/inventory.rs`, `src/combat.rs` | CP31, S13 | `DOMAINS` += `insurance`; `kill_hooks::PLAYER` += `insurance::on_kill` (CA15: the payout is a hook row; CP44 owns `combat.rs` only for `missile_volley`) | C5 §5.5, E6 | the quote equals the client's premium formula [V]; payout on an NPC kill; none on a self or fleet kill; contracts expire. Exposes `combat::missile_volley(…)` for CP45 |
| **CP46** bots read NPC profiles | S | `eve-bot/src/escort.rs`, `eve-bot/src/profile.rs` | CP11a | coordinator hunks (CA9): a `--content DIR` flag in `eve-bot/src/bin/eve-bots.rs` (reusing `eve_server::content::Bundle`; `eve-bot` already depends on `eve-server`), and `tools/soak.sh` passes it | C6 §5.2, N7 | a tackler is chosen over a nearer DPS ship; retreat above `RETREAT_AT`, by name |
| **CP47** bot market roles | M | `eve-bot/src/verbs.rs`, `eve-bot/src/builder.rs` | CP34, CP35, S11 | `ROLES` += `builder::ROLE` (civilian) | C5 §7.2, E9; CA19 | `BotConfig` carries the token field (CA19). new: the miner sells and its wallet rises; the builder posts under the ceiling; spend over a simulated day stays within `BOT_DEMAND_BUDGET` |
| **CP48** storyline and fixed lists | M | `src/storyline.rs` | CP26a, S10 | `HOOKS` += `(Completed, storyline::on_complete)`; `SLOTS` += `storyline::SLOT` | C4 §6, §8, K7 | the `STORYLINE_EVERY`-th completion for (F, L) creates an offer at the nearest fixture agent of type 6 or 7 of faction F; the counter is keyed (character, F, L); a career agent offers its fixed list first |
| **CP53a** `facWarMgr` and enlistment | L | `src/service/facwar_mgr.rs` | R-FW, CP18, S2 | `DOMAINS` += `facwar_mgr`; `SLOTS` += `facwar_mgr::SLOT` | **R-FW** (needs the research lane first) | every method R-FW lists answers its [V] shape; join and leave militia change `GetMyEnlistment` and push `OnEnlistmentStateUpdated` [V arity]; the rank UI methods answer non-`None` |
| **CP54a** wormhole statics and signatures | L | `src/wormhole.rs`, `content/wormholes/statics.tsv`, `content/population.tsv` | R-WH, CP21, CP29, CP3b | `TABLES` += `wormhole::TABLE`; `SLOTS` += `wormhole::SLOT` | **R-WH** (needs the research lane first); C3 §4 archetype 38 | a fixture class-N system's static draws a destination of the row's class; the `wh` delta round-trips and expires; a wormhole signature appears through population as archetype 38 [V]; the K162 exists after the first jump |
| **CP55** bot site runner | M | `eve-bot/src/site_runner.rs`, `eve-bot/tests/bots/site_runner.rs` (CA14) | CP28a, CP28b, CP29, S11 | `ROLES` += `site_runner::ROLE` (not civilian); `mod site_runner;` in `eve-bot/tests/bots/main.rs` | C3 §9; C2 §3; C7 §6.2; §4.7; CA19 | `BotConfig` carries the token field (CA19). new: in the harness, a bot reads Agency counts, warps to a fixture anomaly, clears it and loots; two runners in one system never pick the same site twice in a row |
| **CP56** hostile bot | M | `eve-bot/src/hostile.rs`, `eve-bot/tests/bots/hostile.rs` (CA14), `content/bots/hostile.tsv` | CP39, CP3b, S11 | `ROLES` += `hostile::ROLE` (not civilian, opt-in); `mod hostile;` in `eve-bot/tests/bots/main.rs` | §4.7 (authored policy, no research needed); C7 §6.3; CA19 | `BotConfig` carries the token field (CA19). new: without `--hostile` the role cannot be chosen; it engages only targets the policy table allows (rule over a fixture table); it never fires where CP3b's flag rule would make the act criminal; it never engages a fleet member |
| **CP57** player scram and disrupt | M | `src/tackle.rs` | R-TACKLE, S12, S14 | `WARP_GUARDS` += `tackle::scrammed`; `JUMP_GUARDS` += `tackle::scrammed`; `SLOTS` += `tackle::SLOT`; `session_hooks::ON_LOGOFF` += `tackle::on_logoff`, if a scram must clear on logoff (CA16) | **R-TACKLE** (needs the research lane first) | a fixture scrambler cycle at strength ≥ the target's stability refuses warp and gate jump through the guards [V rule]; a lower strength does not; the refusal is R-TACKLE's [V] `UserError` |

#### W17: later (8 lanes)

v3: CP50 and CP53b register kill hooks (CA15); CP59a and CP60 own CA7's
fallback-item and item-type statics.

| CP | size | owns | after | registers | spec | tests |
|---|---|---|---|---|---|---|
| **CP45** NPC missiles | M | `src/npc_missile.rs`, `src/npc/brain.rs` | CP23, CP44 | — | C6 N5 | the volley lands after flight time; damage is the missile type's × attribute 212 [V]; it goes through CP44's `combat::missile_volley` |
| **CP50** bounty side effects | S | `src/npc/bounty.rs` | CP38, S13 | `kill_hooks::NPC` += `npc::bounty::on_kill` (CA15) | C6 N8 | a kill raises security status by the type's bonus, capped by attribute 563 [V] |
| **CP52** off-lock saver | M | `src/store.rs`, `src/dispatch.rs` (hot; the coordinator lands it) | CP15, **O1 = yes** | — | C7 §5.4, L7 | a dropped server reloads at most `SAVE_EVERY` old; a clock-only mutation is saved; the world-survival test passes |
| **CP53b** VP, occupation and FW sites | L | `src/facwar.rs`, `src/service/facwar.rs`, `content/population.tsv` | CP53a, CP28b, R-FW, S13 | `STAGES` += `facwar::tick` (it pulls completed FW-archetype slots from `population`, the liveness pattern); `SLOTS` += `facwar::SLOT`; `kill_hooks::PLAYER` += a row, if a kill counts for VP (CA15); `KINDS` already has `fwvp` | **R-FW**; C2 §5 | completing a fixture FW complex raises the system's VP; `OnVictoryPointStateUpdated(ssid, VPState)` [V arity] narrowcasts to the system; crossing the threshold flips `occupierID` and pushes `OnWarzoneOccupationStateUpdated` [V]; the `fwvp` delta round-trips |
| **CP54b** wormhole jumps, mass and decay | M | `src/wormhole_jump.rs` | CP54a, R-WH, S12 | `JUMP_GUARDS` += `wormhole_jump::too_heavy`; `STAGES` += `wormhole_jump::tick` | **R-WH** | a jump moves the ship to the far end and debits mass by the hull's mass [V attribute]; a ship heavier than the hole's jump mass is refused through the guard; a hole past its lifetime or out of mass collapses and is removed |
| **CP58a** typedata into `Content` | S | `src/typedata.rs` | S7, CP12 | `TABLES` += `typedata::TABLE` | ruling 3; §1.5 | the `Content`-taking accessors (`typedata::item_in(&Content, id)`, …) return what the `OnceLock` versions return for an installed fixture table; the old free functions still work |
| **CP59a** skills into `Content` | S | `src/skill.rs` | S7 | `TABLES` += `skill::TABLE` | ruling 3; CA7 | as CP58a, for `skill::find_in(&Content, id)`; the fallback items (`skill.rs:709`) come from the same table |
| **CP60** planet statics into `Content` | M | `src/planet.rs` | S7 | `TABLES` += `planet::STATICS`; coordinator hunk: drop `planet::install` from `bin/eve-server.rs` | ruling 3; CA7 | every planet test runs on `fixture()`; both `OnceLock`s (`planet.rs:4415`, `:4416`, the item types) are gone |

#### W18: the `OnceLock` callers and NPC tackle (6 lanes; CP58b-e and CP59b gated on **O3**)

v3: CP58e is new (CA8). CP58b and CP59b carry their split re-points in
parentheses (CA18, §6.3). CP58c makes the dogma sheet cache a world slot
(CA7).

| CP | size | owns | after | registers | spec | tests |
|---|---|---|---|---|---|---|
| **CP49** NPC tackle and ewar | M | `src/npc_ewar.rs`, `src/npc/brain.rs` | CP45, CP57, S12 | `WARP_GUARDS` += `npc_ewar::scrammed`; `JUMP_GUARDS` += `npc_ewar::scrammed` | C6 N6; R-TACKLE | an NPC with `needs ∋ SCRAM` at strength ≥ stability refuses warp through the guard [V rule]; a web applies the speed factor from the type's attributes [V] |
| **CP58b** typedata callers, 1 | M | `src/item.rs`, `src/npc.rs`, `src/destiny.rs` (CA18: becomes `src/destiny/hull.rs` when the review's hull split lands) | CP58a, **O3** | — | ruling 3 | every call goes through `reg.content()`; the existing tests pass on `fixture()`, which has an invented hull, module and skill set |
| **CP58c** typedata callers, 2 | M | `src/dogma.rs`, `src/damage.rs`, `src/fitting.rs` | CP58a, **O3** | `SLOTS` += `dogma::SHEETS` (CA7) | ruling 3; CA7 | as CP58b; the sheet cache (`dogma.rs:967`) becomes a world slot, so its key no longer assumes one process-wide typedata |
| **CP58d** typedata callers, 3 | M | `src/combat.rs`, `src/modules.rs` | CP58a, **O3** | — | ruling 3 | as CP58b |
| **CP58e** drone and scan types (CA8) | S | `src/drones.rs`, `src/scan.rs` | CP58a, **O3** | — | ruling 3; V7 F5 | `drones::TYPES`, `scan::PROBE_TYPES` and `scan::NPC_GROUPS` read `reg.content()`; as CP58b |
| **CP59b** skill callers | S | `src/service/character.rs`, `src/service/industry.rs`, `src/reprocess.rs` (CA18: plus or instead `src/skill_queue.rs`, whichever holds its `skill::` callers, when the review's skill-queue split lands) | CP59a, **O3** | coordinator hunks: the `skill::find_any` call in `slash.rs:416` (CA8; S1 moved it out of `service.rs`); drop `skill::install` and `typedata::install` from `bin/eve-server.rs` | ruling 3 | as CP58b |

After W18, the coordinator deletes the two remaining `OnceLock`s and the
old free functions as one mechanical commit (**X2**).

**Research lanes R-FW, R-WH and R-TACKLE** write no code (lane rule 7).
Each must deliver an implementation spec, not a description: the call or
attribute table with [V] citations, plus the test list for its packets.

### 6.3 The ownership check

**Standing rule (CA18).** The ownership check is re-run, and its output
below replaced, **every time** one of these happens:

1. **A split lands** (a commit that moves code out of a file some CP row
   owns, such as the review's `destiny.rs` and `service/character.rs`
   splits). In the same landing, the coordinator re-points every CP "owns"
   list that named the old file to the file that now holds that packet's
   callers, deletes the parenthesised note, and re-runs the check.
2. **A CP row's "owns" column or wave changes** for any other reason.

Until a split lands, a row names the file as it is in the tree and puts
the future path in parentheses, which the script ignores. Pending:

- **CP58b** (W18): `src/destiny.rs` → `src/destiny/hull.rs` when the hull
  split lands; it keeps `src/item.rs` and `src/npc.rs`;
- **CP59b** (W18): `src/skill_queue.rs` in addition to, or instead of,
  `src/service/character.rs`, whichever holds its `skill::` callers, when
  the skill-queue split lands.

Both splits land before W18, so the W18 lanes are briefed with the
re-pointed rows.

**The script** (`$SCRATCH/<lane>/owners.py`, stdlib only, not committed;
re-create it from this description). It reads every table under the
`#### W11` to `#### W18` headings, takes the third column ("owns") of each
`| **id** |` row, deletes any text in parentheses, and takes every
backticked path left. It reports:

- packets per wave, and any packet id listed twice;
- any path owned twice inside one wave (must be 0);
- any packet owning more than four files (must be 0);
- the paths owned in more than one wave, which is allowed (`-v` lists them).

W10.5 is serial and coordinator-only, so it is not part of the check.
The re-created script gives v2's numbers on v2's text (`git show
6cea759:docs/content/plan.md`: W11=12 W12=15 W13=9 W14=7 W15=10 W16=12
W17=8 W18=5, total 78, 0, 0, 21), so it is the same check.

**Result on v3: 0 duplicate paths within any wave; 0 packets over four
files; no id listed twice.** The packet count includes the 3 research
lanes, so there are 77 build packets. The same run with CA18's two
re-points applied in place of their current paths gives the same result.

**Files owned in more than one wave**, which is allowed, in wave order:

- `content/population.tsv`: CP21 (W13), CP54a (W16), CP53b (W17);
- `eve-bot/src/verbs.rs`: CP34 (W15), CP47 (W16);
- `src/bin/eve-server.rs`: CP3 (W11), CP40 (W14);
- `src/combat.rs`: CP44 (W16), CP58d (W18);
- `src/facwar.rs` and `src/service/facwar.rs`: CP18 (W12), CP53b (W17);
- `src/industry.rs`: CP33a (W11), CP33b (W15);
- `src/liveness.rs`: CP6 (W11), CP40 (W14);
- `src/npc.rs`: CP20 (W12), CP61 (W15), CP58b (W18);
- `src/npc/bounty.rs`: CP38 (W15), CP50 (W17);
- `src/npc/brain.rs`: CP23 (W13), CP45 (W17), CP49 (W18);
- `src/reprocess.rs`: CP32a (W11), CP32b (W15), CP59b (W18);
- `src/service/agents.rs`: CP26b-1 (W13), CP26b-2 (W14), CP41 (W16);
- `src/service/industry.rs`: CP33a (W11), CP33b (W15), CP59b (W18);
- `src/service/market.rs`: CP9 (W11), CP35 (W15), CP42 (W16);
- `src/service/reprocess.rs`: CP32a (W11), CP32b (W15);
- `src/service/sites.rs` and `src/signal_tracker.rs`: CP17 (W12), CP29
  (W14);
- `src/skill.rs`: CP7 (W11), CP59a (W17);
- `src/store.rs`: CP5 (W11), CP52 (W17);
- `src/typedata.rs`: CP12 (W12), CP58a (W17);
- outside the check: `src/service/character.rs`, S5 (W10.5) and CP59b
  (W18).

#### 6.3.1 The script's output (v3, HEAD `6cea759` plus this edit)

```
$ python3 -I $SCRATCH/<lane>/owners.py docs/content/plan.md -v
packets per wave: W11=12 W12=16 W13=9 W14=7 W15=10 W16=12 W17=8 W18=6 total=80
packet ids listed twice: 0
duplicate paths within a wave: 0
packets owning more than 4 files: 0
paths owned in more than one wave (allowed): 21
  content/population.tsv: CP21 (W13), CP54a (W16), CP53b (W17)
  eve-bot/src/verbs.rs: CP34 (W15), CP47 (W16)
  src/bin/eve-server.rs: CP3 (W11), CP40 (W14)
  src/combat.rs: CP44 (W16), CP58d (W18)
  src/facwar.rs: CP18 (W12), CP53b (W17)
  src/industry.rs: CP33a (W11), CP33b (W15)
  src/liveness.rs: CP6 (W11), CP40 (W14)
  src/npc.rs: CP20 (W12), CP61 (W15), CP58b (W18)
  src/npc/bounty.rs: CP38 (W15), CP50 (W17)
  src/npc/brain.rs: CP23 (W13), CP45 (W17), CP49 (W18)
  src/reprocess.rs: CP32a (W11), CP32b (W15), CP59b (W18)
  src/service/agents.rs: CP26b-1 (W13), CP26b-2 (W14), CP41 (W16)
  src/service/facwar.rs: CP18 (W12), CP53b (W17)
  src/service/industry.rs: CP33a (W11), CP33b (W15), CP59b (W18)
  src/service/market.rs: CP9 (W11), CP35 (W15), CP42 (W16)
  src/service/reprocess.rs: CP32a (W11), CP32b (W15)
  src/service/sites.rs: CP17 (W12), CP29 (W14)
  src/signal_tracker.rs: CP17 (W12), CP29 (W14)
  src/skill.rs: CP7 (W11), CP59a (W17)
  src/store.rs: CP5 (W11), CP52 (W17)
  src/typedata.rs: CP12 (W12), CP58a (W17)
```

**The critical path to "the universe has content":**

1. W10.5;
2. K1 → CP10 + CP11a/b + CP16 + CP20 (W12);
3. CP21 + CP22 (W13);
4. CP28a + CP29 (W14);
5. CP28b (W15).

**The critical path to "PvP that matters":**

1. CP3b (W12);
2. R-TACKLE and R-FW (W12);
3. CP57 + CP53a + CP56 (W16);
4. CP53b (W17).

### 6.4 Renumbering: old packet → CP

v1 §6.3's table stands. Additions and splits in v2:

| v1 or R1 | v2 |
|---|---|
| CP11 | CP11a (factions and types), CP11b (regions, home pirate, trades, loot) |
| CP26 | CP26a (`mission.rs`, W13), CP26b (service arms and rewards, W14) |
| CP28 | CP28a (`site.rs`, W14), CP28b (`site_spawn.rs`, W15) |
| CP32, CP33 | CP32a/CP33a (a `Table`, W11), CP32b/CP33b (delete, W15) |
| CP37 | CP37a (extract, W14), CP37b (wallet, W15) |
| R1 CP3b | CP3b (universe and crimewatch), CP3c (dock locators) |
| R1 CP53 | CP53a (`facWarMgr`, W16), CP53b (VP and occupation, W17) |
| R1 CP54 | CP54a (statics, W16), CP54b (jumps, W17) |
| R1 CP55, CP56 | CP55, CP56 (W16) |
| R1 "dogma-lane scram packet" | CP57 (W16) |
| ruling 3 | CP58a-d, CP59a/b, CP60 |
| §2.2 hand rat | CP61 |
| v2 CP14 (v3, CA3) | CP14' (W12): CP14 plus its coordinator hunks; size M-L. Elsewhere in this plan, "CP14" means CP14' |
| v2 CP26b (v3, CA10) | CP26b-1 (`service/agents.rs`, W13), CP26b-2 (rewards, W14) |
| v2 CP3c (v3, CA10) | CP3c, moved from W13 to W12 |
| V7 F5 (v3, CA8) | CP58e (`drones.rs`, `scan.rs`, W18) |
| v1 seams S1-S5 | S1-S5 (W10.5, unchanged scope; S2 adds `tales.rs` and B1's consts) |
| R1 S6-S12 | S6-S12 (W10.5), with S8 also covering `scan.rs` |

### 6.5 The end-goal packets from ruling 5, at a glance

| gap | packets (wave) | spec source |
|---|---|---|
| system security fed into crimewatch | CP3b (W12) | C1 §2.1, §5.3; `crimewatch.rs:665-676`; `docs/re/damage-death.md` §7.2.1 |
| full `facWarMgr`, enlistment, VP, FW sites | CP53a (W16), CP53b (W17) | **needs a research lane first:** R-FW (W12); C2 §5 for the pushes |
| wormholes: authored statics, K162, mass and decay | CP54a (W16), CP54b (W17) | **needs a research lane first:** R-WH (W12); C3 §4 archetype 38, C1 §2.1 `wormholeClassID` |
| tackle: warp disruption and scrambling | S12 (W10.5), CP57 players (W16), CP49 NPCs (W18) | CP57: **needs a research lane first**, R-TACKLE (W12). CP49: C6 N6 plus R-TACKLE |
| a bot role that runs sites | CP55 (W16) | C3 §9, C2 §3, C7 §6.2, plan §4.7 |
| a bot role that fights humans, opt-in per bot | CP56 (W16) | plan §4.7 (an authored policy, no research needed), C7 §6.3 |
| courtesy only for civilian bots | S11 (`civilian` field), CP39 (W15) | ruling 5; C7 §6.3 |

---

## 7. Ranked by what each unblocks for the end goal

1. **W10.5, K1 and CP1-CP3.** Without the seams, the decoder and the bundle,
   nothing reaches the server.
2. **CP3b + CP57 + CP56.** Security in crimewatch, tackle, and an opponent:
   the minimum for PvP with friends and bots.
3. **CP2.** Every agent and market packet names a station.
4. **CP5, CP6, CP15.** Deltas, the hot list, and the measurement.
5. **CP17 + CP18 + CP27, then CP53a/b.** Live breakers and the FW loop, the
   built-in "two sides fight" game.
6. **CP21 → CP28a/b → CP29, then CP55.** Sites exist, are scanned and
   fought, and bots use them too.
7. **CP54a/b.** Wormholes: small-group PvP and PvE space.
8. **CP20, CP22, CP30.** Six pirate factions everywhere.
9. **CP26a, CP26b-1, then CP26b-2.** 10,972 real agents with courier work
   from W13 (CA10), then rewards: co-op PvE.
10. **CP31, CP35.** A market that clears with 2 humans.
11. Everything after that is depth.

---

## 8. Spot-checks

v1's twelve checks stand, with these corrections:

| # | claim | status at HEAD `0954132` |
|---|---|---|
| 2 | `fsd.py` decodes only the `CFSD_COLUMNS` tables | **superseded**: K1 landed, `fsd.rows` decodes 140 of 149 |
| 12 | where the agent and live-content arms live | still true, and B1 added FW consts at `service/space.rs:827,839`. `agent_rowset` is cited by name. S2 and S5 move them |
| — | "the gate re-checks it" (v1 §8) | **struck**. The gate runs synthetic unit tests only; client counts run in `content.py --check` before boot (ruling 1) |
| — | `Registry::touch_system` | **does not exist**; liveness pulls instead (§4.1) |
| — | the generated tables' consumers | `reprocess.rs`, `service/reprocess.rs`, `industry.rs`, `service/industry.rs:2450`. `item.rs:1125` and `destiny.rs:5834` mention them in comments only [V, `grep`] |
| — | warp subjects resolve in `space_state.rs` (R1 §1.2) | **corrected**: the `match` is `service/space.rs:1055-1069`, and `space_state.rs` holds only the subject consts. S8 moves the `match` |
| — | `types.factionID` unusable (C6 §2.1) | **superseded** by K1: record +72, present on 1,383 types |
| — | the system id space | 8,089 systems with a star (5,431 below 30,100,000; 54 in 301000xx; 2,604 in 31xxxxxx) [V, `out/celestials-all.tsv`]; ids are sparse, hence the rank in §3.4 |

---

## 9. Owner decisions

| # | question | recommendation |
|---|---|---|
| **O1** | **Save off the world lock?** Render the snapshot under the lock every 5 s, and write it on a saver thread. RAM may lead disk by 5 s. This changes `docs/reference/world-state.md` §4 rule 3 | **Yes, staged** (**CA17**, measured by V3 F4): saves cost 1-2.5% of the lock and about 10.5 ms of fsync each, 1-4 per second, at 50-100 bots. **CP52 lands when** the world file passes 1 MB, **or** save write max passes 25 ms on the owner's disk (CP15's `[turns]` line shows both, CA13), **or** bots start writing the journal or the market. Until then rule 3 stands |
| **O2** | **Rewrite git history** to purge the two `tables.rs` files? | **No.** Delete them going forward (CP32b, CP33b). Revisit only if the repo goes public |
| **O3** | **Retire the baked hulls, `modules::TYPES`, skills and wrecks** into the bundle, with an invented fixture for tests? This makes `--content` required to *play*, never to *test* (ruling 3) | **Yes.** It gates CP58b-e and CP59b (W18). Ids and protocol constants stay in code |
| **O4** | **The daily rollover at 11:00 UTC**, jail `TZ=UTC`, no disconnect. A friend's client in another zone shows a different countdown, which is cosmetic and recorded as a known limitation with human check H-ROLL | **Yes.** It is the client's own default [V]; `--downtime HH:MM` exists |
| **O5** | **Bots as economy fuel:** a wallet cap burned at rollover, and a daily budget for bot purchases | **Yes, after the `[econ]` ledger** (CP43) shows a week of drift |
| **O6** | **Market taxes** and a **1 M ISK starting wallet** | **Yes to both.** It gates CP9; the wallet is one line in `model.rs:256`, by the coordinator |
| **O7** | **Where do the humans live?** It anchors bot homes, Tier B hubs, the hostile bot's range and the co-op arc | **Tanoo, station 60012526** [V, C4 §7.1]: one row in `hubs.tsv` and one bot flag |

O8 (the EvEJS reward rows) is **dropped** by ruling 9.

---

## 10. Brief template

The coordinator pastes this block into every build lane's prompt, and fills
in the `<…>` fields from the packet's row in §6. **v3 (CA11, CA12, CA19):**
the template writes `$SCRATCH/<lane>`, never a session path (lanes rule 15);
the coordinator defines `SCRATCH` in the preamble. `<CP>` is always written
with its series (`CP21`, never `21`; rule 12). Test citations read
`existing: path::name` or `new: name` (rule 14). The lane note opens with
the rule-16 header. Two house rules are new: slots never read deltas, and
nothing is derived from a visible name.

```
SCRATCH=<the session scratchpad, set by the coordinator>

You are build lane <CP> on eve-offline ($REPO),
a std-only Rust private server for EVE Online build 3619879. Starting
commit: <sha>. Spec: <spec column>. Plan row: docs/content/plan.md §6, <CP>.

GOAL: <one line from the packet name>. Done means every `new:` test in the
row exists and passes, every `existing:` test still passes, and the size stays under <size> (no packet exceeds
1,500 changed lines).

YOU OWN (rule 1: files, not subsystems; write nothing else):
  <owns column>
  docs/lanes/<cp>.md       (your lane note, rule 5; it opens with the
                           rule-16 header: id with series, starting sha,
                           owned files, `new:` tests, Read first, Not done,
                           and a Human check block if a player sees it)
Read first: <spec sections>, docs/content/schema.md, the files above.

COORDINATOR DIFF. An edit you need in a file you do not own (a
registration line from the row's "registers" column, a DOMAINS row, a
`mod` in lib.rs, a named hunk) is NOT applied. Write it as a unified diff
against <sha> to
  $SCRATCH/<lane>/coordinator-<lane>.diff
and list each hunk in your report. Expected: <registers column>.

FOREIGN ERRORS (rule 4). Other lanes are editing this tree now. Ignore any
compile error or test failure outside your files, and never fix one.

TESTING (rules 8, 9, 10). Never run tools/gate.sh, never commit or push,
never git stash or touch files you do not own. Test in ONE scratch copy:
  S=$SCRATCH/<lane>
  T=$LANES/<lane>   (on disk: /tmp is RAM, lanes rule 11)
  mkdir -p $T && git -C $REPO archive <sha> | tar -x -C $T
  then copy your owned files (and apply your coordinator diff) into $T.
  Rust:   EVE_MAX_JOBS=4 $T/tools/dev.sh cargo test --locked -p <eve-server|eve-bot> <filter>
          EVE_MAX_JOBS=4 $T/tools/dev.sh cargo clippy --locked -p <crate> --all-targets -- -D warnings
          $T/tools/dev.sh cargo fmt --all --check
  Python: python3 -I tools/test_content_<x>.py -q   (stdlib unittest; the gate
          runs exactly this. Append the script's own dir to sys.path LAST,
          as tools/test_fsd_generic.py does.)
  Client: tools/analysis.sh python3 -I /work/tools/content.py --sections <x> --check
          and paste the output into your lane note. Counts live ONLY here.
Before reporting: rm -rf $T (rule 9). Keep only the coordinator diff
in $S. Delete nothing outside $S (rule 10). Say in your report that the
copy is gone. If you are told to stop, or stop early for any reason,
rm -rf $T/server/target before anything else.

HOUSE RULES:
- No dependencies, ever (server/deps.allow stays empty). Nothing installed
  on the host; Docker only.
- NO CLIENT DATA IN THE REPO. No extracted row, count, name or real id in
  any committed file, tests included. Tests use invented ids and rows
  (fixture()). Real values go to out/ or to check().
- NO [A] VALUE IN A TEST. A test asserts a rule, a [V] value, or code
  against itself. Name a tuning const (`COOL_AFTER`), never its number.
  Mark every shipped assumption [A] with its falsifier in a doc comment
  or a `why` column.
- Tests live in the module you own. Doc comments compile as doctests.
- Read data through reg.content() and your module's Table (plan §1.5);
  no new OnceLock. Persist only through reg.deltas() with client-id keys
  (plan §4.2).
- A world slot's `make` must not read deltas. Derive delta-backed state
  lazily from reg.deltas(), on a liveness edge or on first use (CA11:
  slots are built before the snapshot is applied). reg.world() is never
  None; write no None branch (CA6).
- No token, key or role is derived from a visible name (ruling 2, CA19).
  A bot's BotConfig carries the token from `--tokens FILE`; the name is
  never sent as a token.
- A new delta kind is a coordinator row in src/delta_kinds.rs KINDS, and
  the module exports `pub const KIND`. A kill side effect is a kill_hooks
  row, and a logoff step a session_hooks::ON_LOGOFF row; never an edit to
  combat.rs or service.rs.
- Write every packet id with its series (CP21, S7, W12). Cite tests as
  `existing: path::name` or `new: name`.
- A wrong answer is evidence; silence is not. Cite [V] file:line.

REPORT (15 lines or fewer): what landed, the test names, the coordinator
hunks, the lane-note path, anything you deliberately did not do, and
"scratch copy deleted".
```
