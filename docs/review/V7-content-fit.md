# V7: how the content data fits the emulator

Review lane V7, 2026-10-25, against HEAD `3f2741d` (W10.5 and the W11 lanes
CP1-CP8, CP19, CP32a and CP33a have landed). Brief: `docs/review/README.md`
§2 and §3 V7. Inputs: `docs/content/plan.md` v2, `rulings.md`, `schema.md`,
the lane notes `docs/lanes/cp1.md` to `cp33a.md`, and the code as it is now.

Marks: **[V]** checked in this tree or measured, **[I]** inferred, with the
reasoning given, **unknown** where neither applies. `src/` means
`server/crates/eve-server/src/`.

This is the owner's "content data, comprehensive plan to address how this
fits into our emu". It does not re-plan W12-W18. It corrects the plan where
the landed code differs from it, and it re-orders the plan so that a player
sees real content sooner.

---

## 0. In one screen

- **The pipeline works.** `tools/content.py` writes the bundle, the server
  checks it, and the server boots on it. Measured on the real `out/content`
  bundle with the debug binary from 03:01: it is listening after **4.4 s**
  [V].
- **What a player sees from the bundle today** is the universe, the belts,
  types, skills and PI. That is the same as before W11. All of it still
  arrives through the five **legacy loaders**:
  - `SandboxConfig.celestials` and `.rocks`;
  - the `typedata`, `skill` and `planet` `OnceLock`s.

  None of it goes through `Content` (`bin/eve-server.rs:204-300`) [V].
- **`Content` holds 3 tables, and none of them is real content yet** [V]:
  - `reprocess::Table` and `industry::Table` return `baked()`, the committed
    `tables.rs` rows, from both `from_bundle` and `fixture`;
  - `mission_pool::Table` loads empty, because the bundle has no `agents/`
    directory;
  - and nothing calls `mission_pool` (no hits for `grep mission_pool::`
    outside its own file and `content_tables.rs`).

  The startup line `[content] 3 tables from the bundle` therefore
  overstates what is loaded.
- **Four bundle files are checked and never read:** `universe/systems.tsv`,
  `regions.tsv`, `constellations.tsv` and `dock-locators.tsv` [V, `grep`].
  Crimewatch still guesses security (`crimewatch.rs:675`). Docking still
  uses the 68 hand-copied graphics in `docking::STATIONS` (`docking.rs:299`).
- **The biggest cost today is the rocks.**
  - With the bundle, the server peaks at **541 MB** RSS. With all but 1,375
    rocks removed it peaks at **139 MB**, so the eager rock field is about
    **400 MB** (2,123,854 rocks) [V, `/usr/bin/time -v`, debug build].
  - With no bundle it peaks at 10.5 MB.
  - CP14 (lazy rocks) has every dependency landed, and it is the one packet
    that removes this cost.
- **Two seams are missing before W12 can do what the plan says:**
  1. `TableSpec` cannot see the authored directory. `from_bundle` takes only
     `&Bundle` (`content.rs:385`), and the binary parses `--authored` and
     then drops it (`bin/eve-server.rs:188-194`, `:345`).
  2. `TableSpec` cannot return an error. `mission_pool::from_bundle` panics
     on a bad header (`mission_pool.rs:374`).

  CP18, a W12 packet, is the first authored table, so both seams must land
  before W12.
- **The fastest route to real content for a player** (§5):
  1. universe facts (CP3b, CP3c);
  2. lazy rocks (CP14);
  3. **10,972 real agents offering courier missions**, by splitting
     CP26a/b so that couriers do not wait for NPC data;
  4. rats by region;
  5. the NPC market;
  6. sites, which have the longest chain.

---

## 1. The state of the W11 code

| plan item | landed as | fits the plan? |
|---|---|---|
| §1.1 driver, sections by glob, atomic bundle, `extractor` hash | `tools/content.py` (808 lines); 9 files, format 1.0, phase-1 files byte-identical to the live extractors (CP1 note) | **yes** [V] |
| §1.3 `Bundle::open` refusals, startup line | `content.rs:250-320` | **yes** [V]. The row check is a count, not the sha ([A], CP3 note) |
| §1.3 `up.sh` staleness and `--check` before boot | `tools/up.sh:115-178` | **yes**. The bash `extractor_hash` equals `content.extractor_hash()` today: both give `ca624d90…` [V]. The recipe is duplicated (finding F9) |
| §1.4 `--authored DIR`, startup line per file | `authored.rs` exists, the flag is parsed, the `Dir` is dropped | **no**: no table can reach it (F1). `up.sh` does not pass it (F2) |
| §1.5 `Content` in `Registry`, fixture by default | `service.rs:565-665`; `server.rs:190-219` calls `with_content` before `apply_snapshot` | **yes**, but `TableSpec` has no error path (F1) and the fixture is not invented for 2 of 3 tables (F4) |
| §1.5 "no new `OnceLock`; three exist" | **six** process statics hold content or content-derived state: `typedata.rs:807`, `skill.rs:708-709` (2), `planet.rs:4415-4416` (2), `npc.rs:522` (`derived()`, NPC types derived from typedata). `dogma.rs:967` is a sheet cache keyed on typedata | **no**: the count is stale (F5) |
| §4.2 world record and deltas | `delta.rs`, `store.rs`; `deltas_mut` marks dirty; the first run mints | **yes** [V]. `Registry::new` has no `World` (F8) |
| §4.1 liveness pulls as the first stage | `liveness.rs:231`, `tick.rs` | **yes**. It has no consumer yet: `npc::tick` and `belts::tick_belts` keep their own lists (CP6 note) |
| CP4 `seed.rs` | `Stream`, `mix`, `Kind`, `day_of` | **yes**. The mission pool does not use it (F7), and `delta::entropy` duplicates `draw_world_seed` |
| CP19 mission pool | `mission_pool.rs` (819 lines) | **partial**: there is no consumer, it has a private RNG, and it panics on a bad header |
| CP32a / CP33a | `Table::baked()` is the only reader of `tables.rs` | **yes**, behaviour-neutral |

---

## 2. Subsystem by subsystem: what each reads today and what will feed it

"Today" is what the code reads at HEAD [V, `grep` and the file named].
"Feeds it" is the plan packet that switches it to content.

| subsystem | reads today | bundle file | switched by (wave) | what a player notices |
|---|---|---|---|---|
| celestials, gates, stations, system→region | `celestials.tsv` through the legacy path into a `Registry` field (`SystemRow` has region and constellation, `celestial.rs:134`) | `universe/celestials.tsv` | done (legacy path) | already all 8,490 systems |
| system security → crimewatch | nothing: `flags_apply` is "not declared null" (`crimewatch.rs:675`) | `universe/systems.tsv` (**in the bundle now**) | CP3b (W12) | suspect and criminal flags match the client's security |
| dock and undock points | `docking::STATIONS`, 68 graphics | `universe/dock-locators.tsv` (110 rows, **in the bundle now**) | CP3c (W13 → **move to W12**) | correct undock at stations whose graphic is not among the 68 |
| asteroid belts | all 2.1 M rocks built eagerly at boot (`asteroid.rs:408`), held in `Registry` (`service.rs:943`) | `belts/rocks.tsv` | CP14 (W12) | none, but 400 MB less RAM [V]; depletion persists |
| ships, modules, dogma | `typedata` `OnceLock`, then the hand tables (`fitting::HULLS`, `modules::TYPES`, `combat::WEAPONS`/`CHARGES`/`WRECKS`, `drones::TYPES`, `item::SPAWNABLE`/`WRECK_TYPES`/`COURIER_TYPES`, `scan::PROBE_TYPES`) | `types/typedata.tsv` | CP58a-d (W17-18, **O3**) | nothing (already from typedata) |
| skills | `skill` `OnceLock`, then `skill::SKILLS` and `STARTER` | `types/skills.tsv` | CP59a/b (W17-18, O3) | nothing |
| PI | `planet` `OnceLock` | `planets/pi-statics.tsv` | CP60 (W17) | nothing |
| reprocess | `Table::baked()` = `reprocess/tables.rs` (3,916 lines), plus `NPC_STATION_TERMS` (`reprocess.rs:459`) | `types/materials*.tsv`, `compress.tsv` (CP12) | CP32b (W15) | every reprocessable type, not only the baked list |
| industry | `Table::baked()` = `industry/tables.rs` (1,564 lines) | `industry/blueprints.tsv` (CP24) | CP33b (W15) | every blueprint |
| agents | one agent, `mission::AGENT_ID` (`mission.rs:70`), served by `agent_rowset` (`service/agents.rs:228`) | `agents/*.tsv` (CP13) | CP26a (W13) + CP26b (W14) | 10,972 agents |
| missions | `COURIER` and `BLOCKADE` specs (`mission.rs:180,198`); pocket rat `ANGEL_HIJACKER` (`mission.rs:527`) | `agents/missions.tsv` + `npc/*` | CP26a/b, CP61 | real mission variety |
| belt rats | `ANGEL_HIJACKER` via `belts::COMPOSITIONS` (`belts.rs:70`) | `npc/*` (CP11a/b) | CP20 → CP22 → CP30 (W12-W14) | the right pirate faction per region |
| sites and signatures | none | `dungeons/*` (CP10) | CP16 → CP21 → CP28a → CP29 → CP28b (W12-W15) | anomalies, signatures, combat sites |
| market | player orders only; empty history | `types/market.tsv`, `econ/prices.tsv` | CP12 → CP24 → CP31 → CP35 (W12-W15) | NPC sell and buy orders |
| FW | B1's `WARZONES` / `HQ_SYSTEMS` (now in `service/facwar.rs`) | `content/facwar/warzones.tsv` (authored) | CP18 (W12), CP53a/b | warzones from a table |
| bots | read no content (`eve-bot` has no content flag) | `npc/types.tsv` (C6 §5.2) | CP46 (W16) | smarter escorts |

---

## 3. Findings: where the plan no longer matches the code

Ranked by cost to the end goal: 2-10 friends plus about 50 bots on one
home server.

**F1. `TableSpec` cannot read authored files or report an error.**
[V `content.rs:385-395`, `bin/eve-server.rs:188-194`, `mission_pool.rs:374`]
- `from_bundle: fn(&Bundle) -> Boxed` gets no `authored::Dir`. The binary
  builds the `Dir`, prints a line about it, and throws it away.
- With no bundle, `content::load(None)` builds **fixtures only**. So a
  no-bundle server could never read `content/population.tsv` or
  `warzones.tsv`, which are our own data (class C), not client data.
- The packets that need authored tables are CP18 (W12), CP21, CP24, CP26b,
  CP35, CP37a, CP54a and CP56. Their only option today is the compiled-in
  copy, which silently ignores the owner's edits.
- A load error has to `panic!` inside `from_bundle`, so the player gets a
  backtrace instead of the binary's normal error message.
- **Cost if left alone:** every authored packet invents its own workaround
  in a file it does not own, and the fix gets harder with each table
  registered after these 3. → Proposal 1.

**F2. `up.sh` never passes `--authored`, and the default is relative to the
working directory.** [V `up.sh:394`; measured: started from the scratchpad,
the server printed `[authored] no content directory: every authored table
uses its compiled-in copy`]
- `up.sh` does not `cd` into the repo. Start it from another directory and
  the owner's edits to `content/*.tsv` are ignored without a word.
- → Proposal 2.

**F3. The eager rock field is three quarters of boot memory.** [V: 541 MB
with all rocks, 139 MB with 1,375 rocks]
- Plan §1.3 already makes rocks lazy (CP14).
- CP14's "owns" column (`asteroid.rs` only) misses where the field lives
  and who builds it:
  - `model.rs:63` `SandboxConfig.rocks`;
  - `bin/eve-server.rs:229` (`asteroid::load`);
  - `server.rs` (the move into the registry);
  - the `rocks()` readers: `destiny.rs`, `mining.rs`, `targeting.rs`,
    `scan.rs` and `service.rs`, 19 call sites [V].
- If CP14 keeps the `Field` read API and materialises on a liveness edge
  (warm or hot), the readers need no edit. But the build path needs
  coordinator hunks that the plan does not list.
- **Cost:** 400 MB per server today, and a 50-bot soak memory regression of
  the §67 kind if anyone ever clones the field again. → Proposal 3.

**F4. The fixture is not invented for reprocess and industry.** [V
`reprocess.rs` and `industry.rs` `TABLE`; CP32a and CP33a notes]
- `Content::fixture()`, which is what every test and the no-bundle server
  run on, holds the full baked client rows until CP32b and CP33b (W15).
- This is the plan's own staging, and it is behaviour-neutral. Two
  consequences follow:
  - "the bundle is needed to play, never to test" is not yet true for those
    two subsystems;
  - the bundle adds nothing to them until W15.
- Accept it, but correct the startup line: Proposal 4.

**F5. Six process statics, not three.** [V, see §1]
- `npc::derived()` (`npc.rs:522`) caches NPC types derived from the
  typedata `OnceLock`. CP20 introduces `npc_data` beside it, so two
  sources of NPC types would coexist until X2.
- `planet::ITEM_TYPES` and `skill::FALLBACK_ITEMS` hang off their
  `INSTALLED`.
- `dogma::CACHE` is a per-ship sheet cache. It is not content, but its key
  assumes a single typedata table for the whole process.
- Also, CP58b-d and CP59b do not own every caller:
  - `skill::find_any` is in `slash.rs:416`, which is coordinator-only. The
    plan names the `service.rs` call, which S1 moved;
  - `drones.rs` (`drones::TYPES`) and `scan.rs` (`PROBE_TYPES`,
    `NPC_GROUPS`) are owned by no W18 packet.
- → Proposal 7.

**F6. Dependencies the plan's "after" column misses.** [I, from §3.2's
fact owners]
- **CP21 (population)** draws by security band and computes `sys_rank`
  over `universe/systems.tsv`. Both are CP3b's universe table, and CP3b is
  not in CP21's "after" column. They are in different waves (W12 and W13),
  so this only needs writing down.
- **CP26a** needs the mission security bands from CP3b. CP3b is not in its
  "after" column either.
- **CP20's `pirate(system)`** needs system→region. It can use the
  catalogue's `SystemRow.region_id` (`celestial.rs:134`), which already
  exists, so it does not have to wait for CP3b, which is in the same wave.
  Its brief must say so, or the lane will block on CP3b.
- **CP46** (bots read NPC profiles) needs a `--content` flag in
  `eve-bot/src/bin/eve-bots.rs`, owned by CP39, and `soak.sh` must pass
  it. Neither is listed. `eve-bot` already depends on `eve-server`
  (`eve-bot/Cargo.toml:25`), so it can reuse `content::Bundle`.

**F7. The mission pool's private RNG.** [V `mission_pool.rs:320,490`; CP19
note]
- Offers use a local splitmix instead of `seed::stream(seed, Kind::Offer,
  …)`.
- No `mission` delta exists yet, so switching now costs nothing. After
  CP26a persists `offer_seq` and history, the switch moves every offer.
- The same note: `delta::entropy` (`delta.rs`) and `seed::draw_world_seed`
  are two seed draws for one job.
- → Proposal 5.

**F8. Test registries have no world record.** [V, CP5 note: "generators must
handle `deltas().world() == None`"]
- Every generator packet (CP14, CP21, CP26a, CP35, …) would grow its own
  `None` branch, or its own test set-up.
- → Proposal 6.

**F9. The extractor-hash recipe exists twice.** [V `up.sh:119-130` vs
`content.extractor_hash()`, equal today]
- The bash copy names the nine readers by hand.
- The first W12 section that imports a new reader makes `up.sh` miss
  changes to it. The hash does not change, so the bundle is stale and still
  boots.
- The host `python3 -I` can import `content` today [V: it printed the hash],
  so `up.sh` can just ask it. → Proposal 8.

**F10. Slots are rebuilt by `with_content` and never see deltas.** [V
`service.rs:656-657`; `server.rs:196` before `:212`]
- The order is correct today. But a slot whose `make` derives state from
  deltas would be built before `apply_snapshot`, and would silently start
  empty.
- The rule the plan already has ("no module keeps a private copy") covers
  this. It needs to be written into the brief, because CP14, CP21 and CP26a
  are the first slots with persistent state. → Proposal 9 (a brief line, no
  code).

**F11. Housekeeping found on the way:** `src/service.rs.orig` (4,307 lines)
is tracked in git since `69aef46` [V `git ls-files`]. Delete it. (V1 and V5
territory, and trivial.)

---

## 4. Measurements

All runs used `server/target/debug/eve-server` (built 03:01, HEAD `3f2741d`),
`--bind 127.0.0.1:0`, with no world file, killed after 20 s.

| run | peak RSS | notes |
|---|---|---|
| no `--content` | 10.5 MB | fixture tables |
| `--content out/content` | 541 MB | 481,069 celestials, 2,123,854 rocks; listening after 4.4 s |
| `--content` + `--belt-rocks` (first 100 rows: 1,375 rocks) | 139 MB | ⇒ rocks ≈ 400 MB |

These are debug-build numbers. Release would be smaller per rock, but not
by a different order of magnitude [I: the cost is data, not code].
`content.py --check` time per boot: **unknown** (CP1 reports extract plus
check at about 25 s).

---

## 5. The order that gets real content to a player soonest

The plan's waves stand. The changes below move player-visible content
earlier without breaking the "one owner per file per wave" check.

**Before W12, as coordinator seam X3:** Proposals 1, 2, 5, 6 and 8. All are
small, and all must precede the first authored table (CP18) and the first
generator with persistent state (CP14).

**Milestone A, W12: "the universe is right".** These need nothing that is
not landed already.
- CP3b: security into crimewatch.
- **CP3c moved up from W13**: its dependencies CP1 and CP3 have landed, and
  it owns only `docking.rs`.
- CP14: lazy rocks, with Proposal 3's hunks.
- In parallel, the extractors CP10, CP11a/b, CP12 and CP13.

**Milestone B, W13: "real agents".** Split CP26a/b so that couriers stop
waiting for NPC data:
- **CP26a** (mission state, N×N): drop its CP20 dependency for courier
  templates. Kill-template offers are filtered out while
  `content().try_get::<npc_data::Table>()` is `None`, and the existing
  `BLOCKADE` stays as the only kill mission until CP20 lands. **After:**
  CP5, CP13, CP19, CP3b and S10.
- **CP26b-1** (new, S, W13): `service/agents.rs` `agent_rowset` serves
  `mission_pool::Table`'s agents. The fixture gives one invented agent.
- **CP26b-2** (the rest of CP26b, W14): rewards, after CP22 (ruling 11).
  Until then, couriers pay the reward `mission.rs` pays today, marked [A].

A friend then docks anywhere in the universe and finds real agents with
courier work, which is co-op PvE on day one of W13 rather than at W14.

**Milestone C, W13-W14: "the right rats"**: CP20 → CP22 → CP30, then CP61.
This is unchanged.

**Milestone D, W14-W15: "a market that clears"**: CP24 → CP31 → CP35.
This is unchanged.

**Milestone E, W14-W15: "sites"**: CP16 → CP21 → CP28a + CP29 → CP28b.
This is unchanged. It is the longest chain, which is why it should not
hold the others back.

**PvP** (CP57, CP56, CP53) is unchanged. CP3b in Milestone A is its only
content prerequisite.

---

## Proposals

1. **Seam X3: `TableSpec` gets the authored directory and an error path.**
   - Change `from_bundle: fn(&Bundle) -> Boxed` to
     `load: fn(&Sources) -> Result<Boxed, String>`, where
     `Sources { bundle: Option<&Bundle>, authored: &authored::Dir }`.
   - `content::load(dir, authored)` calls `load` for every table **even with
     no bundle**. A class-B table with no bundle returns its `fixture()`. A
     class-C table reads `authored`.
   - `Content::fixture()` (tests) keeps calling `fixture` only.
   - The binary passes the `Dir` it already builds.
   - Migrate the 3 registered tables. `mission_pool` returns `Err` instead
     of panicking.
2. **`up.sh` and `soak.sh` pass `--authored "$REPO/content"`** (in the
   container, `/repo/content`), and the startup line names the absolute
   path.
3. **CP14's row gains its coordinator hunks:**
   - `bin/eve-server.rs`: load the offset index instead of
     `asteroid::load`;
   - `model.rs`: `SandboxConfig.rocks` becomes the index;
   - `server.rs`: the move into the registry.

   Its design rule: **the `Field` read API stays**, and materialisation
   happens on the liveness warm/hot edge, inside a stage. The 19 reader
   call sites therefore do not change.
4. **An honest startup line.** Each `TableSpec` reports its source:
   `bundle`, `baked` (until CP32b and CP33b), `authored:disk`,
   `authored:compiled` or `fixture`. `[content] N tables` becomes one line
   per table.
5. **One RNG, while that is still free.** `mission_pool` draws through
   `seed::stream(seed, Kind::Offer, [character, agent, offer_seq])`, and
   `delta::World::mint` calls `seed::draw_world_seed`. Both land before
   CP26a persists any offer.
6. **`Registry::new` carries `World::FIXTURE`** (a fixed test seed), and
   `server.rs` replaces it on load or mint, as it does today. Generators
   then read `reg.world()`, which returns `&World` and is never `None`.
7. **Correct the plan's §1.5 and §2.2:**
   - six statics, not three;
   - `npc::derived()` goes to CP20, which owns `npc.rs` in W12 and
     replaces it with `npc_data`;
   - `drones.rs` and `scan.rs` join a W18 packet (CP58d, or a new CP58e);
   - `slash.rs:416` is a coordinator hunk for CP59b.
8. **`up.sh` asks `content.py` for its hash.** Use
   `python3 -I -c 'import sys; sys.path.append("tools"); import content;
   print(content.extractor_hash())'` and delete the bash copy.
9. **Brief rule (plan §10):** "A world slot's `make` must not read deltas.
   Derive delta-backed state lazily from `reg.deltas()`, on a liveness edge
   or on first use."
10. **Re-wave** as §5 describes:
    - CP3c to W12;
    - CP26a's "after" drops CP20 and adds CP3b;
    - CP26b splits into 26b-1 (W13) and 26b-2 (W14);
    - CP21 adds CP3b to its "after" column;
    - CP46 gains the `eve-bots.rs` `--content` hunk and the `soak.sh`
      flag.
11. **Delete `src/service.rs.orig`.**

## Packets

| id | what | files | tests | size | behaviour-neutral |
|---|---|---|---|---|---|
| **X3** (coordinator, before W12) | Proposals 1 + 4 | `src/content.rs`, `src/bin/eve-server.rs`, `src/reprocess.rs`, `src/industry.rs`, `src/mission_pool.rs` (each a 3-line `TableSpec` change) | `content::tests::load_hands_every_table_the_authored_dir`; `content::tests::a_table_error_is_a_startup_error_naming_the_table`; `content::tests::no_bundle_still_reads_authored_tables` (a local spec list, an invented TSV in a temp dir); `mission_pool::tests::a_bad_header_is_an_error_not_a_panic` | M | yes, except that a bad header becomes an error instead of a panic |
| **X4** (coordinator, before W12) | Proposals 2 + 8 | `tools/up.sh`, `tools/soak.sh` | the existing synthetic-tree staleness exercise (CP3 note) re-run; a check that the server's startup line shows `[authored] tables from /…/content` when started from another directory | S | yes |
| **X5** (coordinator, before W12) | Proposals 5 + 6 | `src/mission_pool.rs`, `src/delta.rs`, `src/service.rs` (`Registry::new`, `world()`) | `mission_pool::tests::the_same_inputs_give_the_same_offer` (unchanged, still passes); `seed::tests::mix_is_stable` unchanged; `service::tests::a_new_registry_has_a_world` | S | yes. Offers move, but none is persisted |
| **X6** (coordinator) | Proposals 7 + 10 + 9 | `docs/content/plan.md` | `scratchpad/P2/owners.py` re-created and re-run: 0 duplicates per wave | S | doc only |
| **CP14'** | CP14 plus Proposal 3's hunks | `src/asteroid.rs`; coordinator hunks in `model.rs`, `bin/eve-server.rs`, `server.rs` | the plan's CP14 list, plus `asteroid::tests::a_cold_system_holds_no_rock` and a soak RSS line in the lane note (before and after) | M-L | no (lazy); every existing mining test unchanged |
| **CP26b-1** | `agent_rowset` from `mission_pool::Table` | `src/service/agents.rs` | the rowset has 10 columns [V]; the fixture's invented agent appears at its station; `GetAgents` on a fixture world lists every fixture agent | S | no |
| **H1** | Proposal 11 | `src/service.rs.orig` (deleted) | the gate | S | yes |

## Do not do

- **Do not move the celestial catalogue or the rocks into `Content` for
  their own sake.** The legacy path is already a single copy (§67), and
  moving 481k rows changes no answer. Only CP14's laziness pays.
  Re-evaluate at O3.
- **Do not check the manifest sha at boot.** Hashing 60 MB costs boot time
  for a corruption that the row count and `up.sh`'s re-extract already
  catch in practice (CP3's [A]).
- **Do not hot-reload content.** `Arc<Content>` plus a restart is enough
  for a home server, and reloading under the world lock invites the
  lock-hold problems V3 is measuring.
- **Do not let bots read content the client does not have.** C6 §5.2
  allows only client-equivalent data. Keep the bot marker bot-side (plan
  §4.7).
- **Do not pull CP32b and CP33b earlier.** They need CP12, CP24 and CP31's
  prices. Running them before those land would put the baked rows back in
  under another name.
- **Do not add a per-module `OnceLock` "just for now"** to get around F1.
  That is exactly what ruling 3 forbids, and X3 is small.

## Unknown

- **Release-build RSS** with the bundle. Measured in debug only.
- **`content.py --check` time per `up.sh` boot**, and whether it re-reads
  the client every time. It runs in Docker, and was not measured here.
- **Whether the client's `GetAgents` copes with a 10,972-row rowset**
  without a frame-time problem. This is the 30 s cliff in the V2 brief.
  CP26b-1's lane should measure it with a bot.
- **The memory of the W12-W14 tables when loaded eagerly**
  (`dungeons/objects.tsv` is 260,242 rows). An estimate of tens of MB [I];
  CP16 should print it.
- **Whether `npc::derived()` and `npc_data` can coexist** in CP20 without
  the two disagreeing on a type. CP20's lane decides.
