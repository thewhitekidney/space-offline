# Red team R1: review of `docs/content/plan.md`

Lane R1 (`docs/content/README.md` §4), 2026-10-25. This doc attacks
`plan.md` at commit `f293520`. It does not edit the plan. Every finding
names the plan text to change or the packet to split, and the coordinator
settles them. Marks follow the brief. **[V-R1]** means R1 re-checked the
claim itself, on this tree or on build 3619879 (`tq-offline`). The
checks were `tools/dis.py` on `out/pyc`, a walk of `solarsystemcontent`
through `tools/fsd.py` + `celestials.py` inside `tools/analysis.sh`
(read-only), and `grep`/`sed` on `server/`.

Severity:

- **S1** means a rewrite, or the plan cannot land as written.
- **S2** means a wave stalls, or a packet cannot meet its own tests inside
  the files it owns.
- **S3** is cheap to fix now and costly later.
- **S4** is hygiene.

---

## 0. The top five

1. **No gate runs the extractor tests** (S1, §1.1). `tools/gate.sh` runs
   no Python. The extractor tests also need the client, which the gate does
   not have. The plan's §8 claim that "the gate re-checks it" is false.
2. **S1-S5 leave at least eight shared files in place** (S1, §1.2). These
   are `Registry` in `service.rs`, the tick list in `push.rs`,
   `destiny.rs` slims, warp resolution in `space_state.rs`, `scan.rs`,
   the `mission.rs` completion hooks, the `belts` field in `npc.rs`, and
   the bot `Role` in `eve-bot`. About fifteen packets cannot pass their
   own tests inside the files they own.
3. **The data-access model is undecided** (S1, §1.3). The tree's pattern
   is a process-wide `OnceLock` that tests never install. That
   contradicts "tests use invented fixtures", "the no-bundle server runs
   every wire test" and O3, all at once. CP32 and CP33 are each about
   three times the size the plan states.
4. **The `rock` delta is keyed by a derived id** (S1, §1.4). That breaks
   the plan's own rule in §4.2. A rock id is computed from the belt's
   ordinal in the sorted extract (`tools/asteroids.py:114`), so one
   client update moves every later belt's remainders onto other rocks.
   Site slots have the same weakness, through the order of the rows in
   `population.tsv`.
5. **The end goal is under-served** (S1, §1.5). Nothing covers
   wormholes. FW gets only what B1 already shipped. Security never reaches
   crimewatch. The server has no warp disruption at all. No bot role uses
   sites, no bot can be an opponent, and the CP39 courtesy rule makes
   bots leave when a human arrives.

---

## 1. S1 findings

### 1.1 No gate runs the extractor tests, and they need the client

**The fact [V-R1]:** `tools/gate.sh` runs `check_deps.py`, then `cargo
fmt`, `clippy` and `test`, and nothing else. `land.sh`, `dev.sh` and
`tools/hooks/` run no Python either. The tree's only `tools/test_*.py` is
K1's in-flight `test_fsd_generic.py`, and K1 already found the right
split. Its header says the tests use "synthetic bytes only. Nothing reads
the client", and the client-backed checks live in `fsd.py generic-check`.

Many of the plan's extractor tests are client-backed, though:

- CP10: `objects.tsv` has 260,242 rows;
- CP12: 53,288 rows;
- CP13: 10,972 agents, and agent 3010110 has faction 500007;
- CP11: Curse→Angel.

These cannot run in the gate, because the gate has no client. Nobody runs
them, so §8's "Each has a closure test in its extract packet (CP11, CP13,
CP24), so the gate re-checks it" does not hold. The tests also put a
dozen real client ids and counts into committed test files, which strains
the no-client-data rule.

**Fix (plan §1.1, §6.2 test columns, §8):**

1. Split every extractor packet's tests in two:
   - `tools/test_content_<x>.py` holds **synthetic-bytes or
     synthetic-TSV unit tests only**. It is stdlib-only, so it runs on
     the host's `python3`. That is no install.
   - Every count, closure and known-id assertion moves into the section's
     `check(ctx)`. `content.py --check` runs it inside
     `tools/analysis.sh`, and the lane records the output in its lane
     note.
2. The coordinator adds one gate step (a seam, before W11):
   `python3 -I -m unittest discover -s tools -p 'test_*.py'`.
3. `up.sh` runs `content.py --check` before every boot that re-extracts,
   and refuses to start on a failure. Then the [V] counts are re-checked
   on every machine that holds a client, which is the only kind of
   machine that can check them.

### 1.2 Shared files S1-S5 do not remove

Lane rule 2 says the shared file is the finding. R1 traced what each
packet's tests actually need against the files it owns, and found these
shared files:

| shared file | why it is shared [V-R1] | packets that need an edit there without owning it |
|---|---|---|
| `service.rs` `struct Registry` (30 fields, `:316`) | every stateful subsystem is a `Registry` field (`npcs`, `missions`, `market`, …) built in `Registry::new` | CP6 (liveness), CP21 (population), CP25 (rollover state), CP28 (sites), CP31 (`mktidx`), CP35 (exchange), CP37 (LP), CP38 (DBS), CP41 (arcs), CP43 (ledger) |
| `push.rs` world ticker (`:690-720`) | the per-tick stages are a hand-written call list (`modules::tick`, `combat::tick`, `npc::tick`, …) | CP21 refill, CP25 rollover slices, CP28 in-pocket respawn and `SITE_VACANT`, CP30 belts. CP15 owns `push.rs` in W12 only |
| `space_state.rs` warp subjects (`:855-870`) | `'item'` resolves only existing balls, and `'scan'` is refused on purpose. An anomaly has **no ball** until arrival (§3.3) | CP17 ("an anomaly warps as `item`, a signature as `scan`"), CP29. CP6 owns this file in W11 only |
| `scan.rs` | probes find **ships only** (`scan.rs:13-16`, `:1110-1115`: the loop over `roster().in_system`) | CP29: signatures that cannot be probed down make every signature (data, relic, wormhole) unreachable |
| `destiny.rs` `Slim` (`:1143`, five fixed fields) | adding a field needs a new slim type in `destiny.rs`, as `GateSlim` and `WreckSlim` did | CP28 (`dunObjectID`, `entityGroupID`, `objectiveTargetGroup` on every site slim) |
| `npc.rs` `belts: dungeon::Belts` (`:679`), and the call at `:942` | the NPC module holds the belt state and calls `tick_belts` | S3 must edit `npc.rs` too, and so must CP30 if `Belts` changes shape. CP20 owns `npc.rs` in W12 only |
| `mission.rs` completion | LP credit, arc progress and the storyline counter all fire on a turn-in | CP37 ("a turn-in credits LP"), CP41, CP48. Only CP48 has a coordinator hunk |
| `server.rs:215` `spawn_world_ticker(world, writers, node)` | the ticker has no handle on the store | CP15's ticker save. `server.rs` is a hot file |
| `service.rs:303` `DOMAINS` | a new service module is one row here | S2, S5, CP18, CP25, CP43, CP44 (new `service/*.rs`). The plan says only `lib.rs` |
| `eve-bot/src/brain.rs` `Role` (`:80`), `driver.rs`, `bin/` | a new bot behaviour is a role, plus its dispatch and its flag | CP34 (W14, mission role), CP46, CP47 (`builder`, W16). CP39 owns these files in W15 only. CP34's harness test also lives in `eve-bot/tests/`, which no packet owns |

**Fix: add these seams to §6.1.** They are coordinator-only, verbatim or
mechanical, and each lands before the first wave that needs it:

- **S6, a tick-stage registry.** `push.rs` calls
  `tick::run_all(&mut w, now)` over a `const STAGES: &[(&str, fn)]` list.
  A new stage is one line the coordinator adds at landing, like
  `install_from`. CP15's `TickHealth` per-stage timings fall out of the
  same list. Before W12.
- **S7, `Registry` slots.** Pick one mechanism and write it down:
  - **(a)** the coordinator adds one field plus its `new()` line per
    packet at landing. This is the existing practice, made explicit.
  - **(b)** a std-only `ext: BTreeMap<TypeId, Box<dyn Any + Send +
    Sync>>` with `ext::<T>()` and `ext_mut::<T>()`.

  Recommend **(a)**. It keeps the "one door" design of
  `docs/archive/plan-subsystems.md` §4a, and the cost is a line per packet. State it
  in the brief so no lane edits `service.rs`.
- **S8, a warp-target resolver.** `space_state` asks a
  `fn(&Registry, subject, id) -> Option<[f64;3]>` list before it
  refuses. Sites then register `'item'` (for site ids) and `'scan'`.
  Before W12, because CP17 needs it.
- **S9, slim extras.** `Slim` gains `extra: Vec<(&'static str,
  PyValue)>`, appended by `slim_pairs`. Before W14.
- **S10, mission hooks.** `mission::on_complete(fn)`, `on_offer` and
  `on_decline` registration, inside CP26's own file (add it to CP26's
  tests). CP37, CP41 and CP48 then register their hooks and touch
  nothing else.
- **S11, bot roles.** `Role` dispatch becomes a table in `brain.rs`, and
  each role is a module (`escort.rs` is already the shape). Before W14.
- **CP29 owns `scan.rs`**, or **CP29b** does: "signatures in probe
  results", in W14.
- **CP15's line** reads "+ the `spawn_world_ticker` store argument in
  `server.rs`, by the coordinator".

### 1.3 The data-access model is undecided

**The fact [V-R1]:** every runtime table today is a process-wide
`OnceLock`, installed once from `bin/eve-server.rs`:

- `typedata.rs:807` (`install` refuses a second call);
- `planet.rs:4415`;
- `skill.rs:708`.

No test installs one. The tests pass because the fallback hand rows exist,
and `reprocess.rs` and `industry.rs` re-export `tables::*` as `&'static`
slices, for example `industry::find -> Option<&'static Blueprint>`
(`industry.rs:769`).

The plan asserts three things at once:

- "Each consumer exposes `install_from(&Bundle)`" (§1.3);
- "tests use invented fixture rows" and "the no-bundle server boots,
  docks and runs every wire test" (§2.1, §2.3);
- O3, which retires the hull, `modules::TYPES` and skill rows.

A `OnceLock` cannot hold a different fixture per test. With the hull rows
gone, every wire test that undocks a Rifter loses its hull. So O3 as
written breaks the gate, and CP32 and CP33 have no stated mechanism for
their fixtures.

The size is understated too. `industry.rs` has a 2,149-line test module
with about 40 references to `BLUEPRINTS`, `SCIENCE` and friends, plus 32
tests in `service/industry.rs`, and `service/industry.rs:2450` iterates
`crate::industry::tables::BLUEPRINTS` directly, though no packet owns
that file. `reprocess.rs` has 1,129 test lines with about 30 references.
Each of these packets is realistically more than 1,500 changed lines, not
"M".

**Fix (plan §1.3, plus a new §1.5 "how a consumer reads data"):**

1. **The rule:** each data module defines a `Table`, and pure functions
   take `&Table`. The accessor is `fn table() -> &'static Table`, which
   returns the installed `OnceLock`, else under `#[cfg(test)]` a
   module-local `fixture()` of invented rows (`get_or_init`), else an
   empty table. Tests that need other rows call the pure function with
   their own `Table`.
2. **Split CP32 into CP32a and CP32b, and CP33 into CP33a and CP33b:**
   - **a**, behaviour-neutral: introduce `Table`, route every consumer
     through it (including `service/industry.rs`), and keep `tables.rs`
     as the source;
   - **b**: delete `tables.rs` and move the tests to `fixture()`.

   Each half is under 1,000 lines.
3. **O3** gains "a `#[cfg(test)]` fixture hull, module and skill set
   replaces the baked rows; the no-bundle *binary* boots with no ships".
   Then §2.3's "runs every wire test" stays true.

### 1.4 The delta keys and the digest

§4.2 says deltas are "keyed by the generated parent's coordinates, not by a
derived id, so a bundle rebuild cannot orphan them". Two keys break that:

- **`rock: rock_id → remaining`** **[V-R1]**:
  - `rock_id = 70,000,000 + ordinal·200 + n` (`asteroid.rs:69-75`);
  - `ordinal` is the belt's index in the sorted belt list of the
    extract (`tools/asteroids.py:31-33,114`);
  - a client update that adds or removes one belt shifts every later
    ordinal, so stored remainders land on other rocks, or on nothing.

  **Fix:** key `rock` by `(belt_id, n)`, with `n` the rock's ordinal
  inside its belt. Derive `rock_id` on load.
- **`site: system slot → generation state at`.** The slot index depends
  on the row order and counts in `population.tsv`. The C7 floor slots are
  "numbered after T1's" (§3.3), so an operator who edits one count
  re-maps every later slot to a different drawn dungeon, and the stored
  `Completed` state then applies to a different site. **Fix:**
  - key a slot by `(archetype, entry, k)`, not a flat index;
  - store the drawn `dungeon_id` in the `site` delta, and drop the delta
    when the re-draw disagrees.

  Tie a `gen_version` in the `world` record to the digest pin, so a
  deliberate generator change can expire every `site` delta in one place.

**DeltaKind registration** (§4.2) has no stated mechanism in std. Rust has
no static registration without a crate. Say it in CP5's line:

- `store.rs` keeps every `delta` row raw, as `(kind, fields)`;
- `Registry` holds the delta map as the **single** truth;
- a module reads `deltas(kind)` at install and writes through
  `put_delta` and `del_delta`.

No module keeps a private copy. This is cheaper to reverse than a trait
registry, and it is what makes "an unknown kind is preserved verbatim"
true.

### 1.5 The end goal: PvP and bots

The plan ranks "the PvP map" fourth (§7) and then delivers shapes only.

**FW.** B1 already landed `GetAllWarzones` and `GetHQSystemIDs`
non-empty, as Rust consts `WARZONES` and `HQ_SYSTEMS` in
`service/space.rs:737-869` [V-R1, commit `80d63e4`]. CP18 adds an
authored `warzones.tsv` (a second source of the same fact), the
occupation answer, and nothing else. C2 §5's `fw_systems` table, the VP
state and push, `GetFactionWarfareBattlefieldInstances` rows, enlistment,
and **`facWarMgr` (about 28 methods, BREAKS the stats and rank UI, and
missing from `client-calls.tsv`)** have no packet. FW complexes
(`dunArchetypesFactionalWarfare`) have no `population.tsv` band.

**Fix:**

- CP18 retires B1's consts (once S2 has moved them to
  `service/facwar.rs`) and owns `fw_systems`.
- Add **CP53, the FW loop** (W15, after CP21 and CP29): a census of the
  `facWarMgr` methods first, then enlistment, VP accrual from a
  complex-band site, `OnVictoryPointStateUpdated`, and the occupation
  flip.

**Wormholes.** These are zero mentions in plan.md. C3 §4 gives J-space
bands no rows and defers archetype 38. For 2-10 friends, wormholes are
the canonical small-group PvP and PvE space, and the corpus has
`wormholeClassID` per system [V, C1 §2.1].

**Fix:** either add **CP54, wormhole statics** (W16): an authored
`content/wormholes/statics.tsv`, a signature of archetype 38 placed by
`population`, a jump through `jump.rs`, and a lifetime and mass from type
attributes. Or name wormholes in an explicit "not planned" list with the
reason. Silence is the one wrong option.

**Security never reaches crimewatch.** `crimewatch.rs:665-676`'s
`flags_apply` is an [A] guess, and its doc says "the fix is the system's
class from `systems.static`, which the server does not load" [V-R1]. The
plan extracts `security` into `universe/systems.tsv` (CP1), but no packet
loads it into the server (see §2.4). **Fix:** CP3b (§2.4) also replaces
`flags_apply`'s guess.

**Tackle does not exist.** `grep -i scram` over the server is empty
[V-R1]. CP49 adds NPC scrams in W17, through a "`space_state`
warp-refusal hunk, by the coordinator". The same refusal is the first PvP
mechanic players need. **Fix:** make the warp-refusal hook a seam (S12)
landing in W12. CP49 then uses it, and so does a dogma-lane packet for
player scramblers and disruptors.

**Bots never use the content and never oppose anyone:**

- There is no ratter or explorer role. C3 §9 proposed one, and the plan
  dropped it. CP34 is courier-only.
- No role reads `dungeonInstanceCacheMgr` or warps to a site.
- CP39's courtesy rule has a ratting or mining bot *leave* when a human is
  within 100 km. That is right for co-op and wrong for PvP.
- No bot role can be hostile.

**Fix:**

- add **CP55, the bot site runner** (W14 after CP29, on the S11 role
  seam): Agency counts → warp to an anomaly → clear it → loot;
- add **CP56, the hostile bot** (W16): an opt-in role with a
  `--hostile` flag that roams the humans' 2-jump home ring, engages
  non-fleet humans by a policy table, and respects crimewatch;
- scope CP39's courtesy rule to co-op roles in its line.

---

## 2. S2 findings

### 2.1 The plan is stale on B1, and the seams can land now

§6.1 says B1 is "running now". It landed as `80d63e4`, and it also
touched `service.rs` (+4), which is outside its declared ownership.
The seams are "during W11 and after B1". Since B1 is in, they should land
**before** W11, so that every W11 lane starts from the post-seam commit
(lane rule: one starting commit per round). Verbatim moves that land
mid-wave break the paths the W11 lanes already wrote against.

**Fix:** in §6.1, change "they land during W11" to "they land before W11
starts". Add S6-S12 from this review to the same serial run.

### 2.2 Producer/consumer pairs inside one wave

The W12 pairs are CP10↔CP16 (dungeons), CP11↔CP20 (npc) and CP13↔CP19
(agents). Each pair runs in parallel against headers scattered across C1,
C3, C4 and C6. They already disagree:

- C1 §2.6's `npc/factions.tsv` has six columns, and the plan adds
  `home_pirate`;
- C1 §2.7's `agents.tsv` differs from C4 §3.1's, which the plan says
  wins.

**Fix:** before W12, the coordinator writes the exact header line of
every `out/content/` file into one place: a new §1.2 column, or
`docs/content/schema.md`. Both members of a pair pin that string. The
loaders already reject a bad header (CP20's test). Do the same for
`content/*.tsv`.

### 2.3 Hidden edits that the "owns" lines miss

These are beyond the shared-file table in §1.2:

- **CP17, CP29:** warping to an anomaly needs S8 (§1.2).
- **CP25 ↔ CP21, both W13:** CP25's test "a site under a pilot survives"
  is CP21's logic, and CP21's "ttl expiry at rollover" is CP25's
  driver. **Fix:** CP21 tests `population::on_rollover(day)` directly,
  CP25 tests with a fake registered hook, and the coordinator wires them
  at landing.
- **CP45 vs CP44, both W16:** NPC missile damage needs a volley path in
  `combat.rs`, which CP44 owns that wave. **Fix:** move CP45 to W17, or
  name the `combat.rs` API it may call without editing it.
- **CP6:** "fed by `Registry::touch_system`". That function does not
  exist [V-R1]; `advance_system` is at `service.rs:995`. The call sites
  (login, undock, `jump::take_arrival`, dock) decide "a jump warms the
  destination before arrival". **Fix:** list them, and give CP6 the
  coordinator hunk.
- **CP12:** widening `typedata.tsv` to categories 17 and 25 turns every
  ore and commodity type into a grantable `ItemType` through
  `typedata.rs:600-620`. That replaces hand rows' volumes in `item` and
  `mining` with extract rows. **Fix:** CP12 also owns a `typedata.rs`
  test asserting the asteroid and commodity rows install as stackable
  items, or the plan names the hand rows this retires.
- **CP14:** "lazy rocks" from a 249,314-row `belt-rocks` file needs a
  per-system index or seek contract from CP1. Today `asteroid::load`
  reads it whole. **Fix:** say CP1 writes `belts/rocks.tsv` sorted by
  system, and that CP14 builds an offset index at startup.

### 2.4 The universe loader has no owner

§3.2 says "`celestial.rs` loads `universe/systems.tsv` (CP3 installs
it)", but CP3 owns `content.rs`, `bin/eve-server.rs` and `up.sh`, not
`celestial.rs`. The consumers are:

- security and bands (C3), `pirate(system)` in production (CP20, which
  uses fixtures only), C4's security bands and LP formula, and
  crimewatch, for `systems.tsv`, `regions.tsv` and
  `constellations.tsv`;
- `universe/dock-locators.tsv` (C1 §2.1, written by CP1), which has no
  reader at all, so `docking::STATIONS`' 68 hand rows stay.

**Fix:** add **CP3b, the universe loader** (W12, after CP1 and CP3). It
owns `celestial.rs` and `crimewatch.rs` (`flags_apply`). Its tests: a
fixture `systems.tsv` gives security and region, and a 0.0 system has no
suspect flags. Dock locators can be a later packet, but list it.

### 2.5 Packets too large or too wide

| packet | why | split |
|---|---|---|
| CP32, CP33 | §1.3: each is over 1,500 lines with its tests, and CP33 misses `service/industry.rs` | a: a behaviour-neutral `Table` that keeps `tables.rs`; b: delete it and move the tests to fixtures |
| CP26 (L) | rewrites `mission.rs` (1,504 lines) for N×N, adds the delta and rewards, plus the service arms. Two old packets (C4 K3 + K4) were merged into one | CP26a `mission.rs` state, delta and S10 hooks (W13); CP26b `service/agents.rs` arms and `rewards.tsv` (W14) |
| CP28 (L) | its owned file is one, but it needs `destiny.rs` (S9), `space_state.rs` (arrival) and `npc.rs` (slim fields on NPCs) | CP28a: room-0 balls, rocks and the slim extras; CP28b: squads, triggers, respawn, `SITE_VACANT` |
| CP37 | two languages and five files (it omits `tools/test_content_lp.py`, which §1.1 requires) | CP37a `content_lp.py` (W14, after CP24); CP37b loyalty and the wallet (W15) |
| CP11 (L) | nine output files and the R1-R3 and R7 derivations, the long pole of W12 | CP11a: factions, corporations, types, components and bounty; CP11b: R3, regions, `home_pirate` and the overrides |

---

## 3. S3 findings

### 3.1 [A] values pinned in tests (the owner's rule)

- **CP39:** "at most 2 payloads a second over 10 s" and "within 100 km"
  are both C7 §6.3 **[A]**.
- **CP40:** "ticks every other tick under load" is [A] policy.
- **CP35:** "refill after 6 h is a quarter" pins the trickle's [A] rate
  ("the formula against itself" is the right idea, but the sentence pins
  numbers).
- **CP21:** any "3 day ttl" assertion would also be an [A] pin.

**Fix:** rewrite each as a relation to the named const, for example
"≤ `BOT_PAYLOADS_PER_S`" or "after `TRICKLE_PERIOD`·k, refill =
f(k)". The house-rules list in §6.2 should add: *"a test names the const,
never its value"*.

### 3.2 CP2's test figure is wrong by one station

**[V-R1]:** walking `solarsystemcontent` gives **1,223** planet stations
and **3,986** moon stations, which is **5,209**. The 5,210th unique
station sits under **`star.npcStations`** (one row). As written, CP2's
test ("under planets **and moons** … 5,210 expected") either fails or
ships an extract that silently drops a station.

**Fix:** CP2 walks the star, the planets and the moons. Keep the
"computed, not typed in" rule. The check belongs in `content.py --check`
(§1.1), not in a unit test.

### 3.3 The rollover hour against friends' clocks

**[V-R1]** `gametime/downtime.pyc`:

- `get_downtime_start_time:14-17` reads
  `monolithconfig['DowntimeStarts']`, else `time(11,0)`;
- `get_last_downtime_datetime:31-36` combines it with **`datetime.now()`**,
  which is *local* time.

The jail's `TZ=UTC` fixes only the owner's client. A friend in CET
computes "downtime" at 10:00 UTC.

**Fix (O4 text):** add "client-side daily timers are off by each
friend's UTC offset [I]; harmless unless a client timer gates an action;
falsified by a daily UI that disagrees with `/live`".

### 3.4 `up.sh` serves a stale bundle after an extractor change

§1.3 re-extracts only when `MANIFEST.tsv` is absent or the source md5s
differ. A landed change to `content_npc.py` leaves the old bundle in use,
with no message. **Fix:** `up.sh` also compares the manifest's
`extractor` hash, taken over `tools/content*.py` and the readers it
imports. `Bundle::open` logs `format` and `extractor` on its startup
line.

### 3.5 `rewards.tsv` copies EvEJS rows

C4 §4's ten-row reward table is "[V2] values, as EvEJS's table records
them", and CP26 commits it as `content/agents/rewards.tsv`. The house rule
for EvEJS is "facts and vocabulary only, **never rows**".

**Fix:** a new **OWNER DECISION O8**, with two options:

- accept these as public Tranquility figures, with provenance in the
  `why` column;
- or author our own curve (ISK = a·bᴸ, fitted by hand [A]).

Recommend the curve. It is two consts, and the rule stays clean.

### 3.6 Decisions that cost a rewrite if wrong, and how to make each cheap to reverse

| decision | why it is expensive | how to make it cheap |
|---|---|---|
| the data-access model (§1.3) | every consumer's signature | the `Table`, `table()` and `fixture()` rule, written down before W12 |
| the delta keys (§1.4) | a stored world misapplies after a client update | key by coordinates only, guard on `dungeon_id`, carry `gen_version` |
| deltas as one generic record | a format change rewrites every module's save and load | the `Registry`-held map as the single truth (§1.4) |
| the site id band 8.1e9-8.64e9 | ids above 2³¹ are an **[A]** in `npc.rs:49-51` (falsified by a client path mishandling them) that the §3.4 table presents as [V] | compute every site and instance id in one function each, never persist one, and mark the >2³¹ assumption [A] in §3.4. The band also holds all 8,490 systems (8.1e9 + 8,490·65,536 = 8.656e9 < 8.7e9), so drop the "8,089 with a star" restriction rather than tie the index to stars |
| the TSV bundle | — | already reversible behind `install_from` |
| deleting `tables.rs` (CP32, CP33) | — | reversible by `git revert`, since O2 says no history rewrite. Keep O2 = no |
| the 11:00 UTC rollover | — | already a flag (`--downtime`) |
| the off-lock saver (O1) | — | staged behind CP15's measurement; fine |

---

## 4. S4 findings

- **§3.4's table labels.** The bands' *locations* are [V], but the
  safety of ids above 2³¹ is [A] (`npc.rs:49-51`). Say so.
- **Conflict 7 is right, and the evidence is stronger than cited**
  [V-R1]:
  - `asteroid_scanning_service.get_depletion_status_for_belt:222-226`
    compares the client's **static** `asteroidcountbytypeidperbelt`
    against every asteroid slim in the ballpark (a `defaultdict(int)`,
    `:216`);
  - so a materialised belt must reproduce the static per-type counts
    **exactly** to read "full";
  - site ore rocks inside the same ballpark inflate the ratio.

  Add to CP14: "the materialised count by type equals the `belts/rocks.tsv`
  counts".
- **`agent_rowset`** has moved to `service.rs:4442` (the plan says
  `:4438`). Cite it by name in S5.
- **`include_str!` plus `--authored`** gives two sources for each
  authored file. Add a startup log line naming which one was used.
- **New `service/*.rs` modules need a `DOMAINS` row** (`service.rs:303`).
  Add "and `DOMAINS`" to "the coordinator registers every new module in
  `lib.rs`".
- **The bot marker.** C7 §6.3's courtesy rule needs a bot marker the
  *bot* can see. Do not put it in slim or overview data that human clients
  render. Use the bot-side name list.
- **CP1 versus CP12.** CP1 pins `typedata.tsv` byte-identical to
  `typedata.py`, and CP12 (the next wave) changes `typedata.py`. CP12's
  line should say it re-baselines CP1's test.

---

## 5. Claims re-checked by R1

| claim | result |
|---|---|
| `GetWarzoneIdForFaction` `random.choice` on `{}` (`fwWarzoneSvc:203-207`) | **holds** [V-R1] |
| 11:00 downtime default (`downtime:16-17`) | **holds**, but it is *local* time (`:32 datetime.now`); see §3.3 |
| depletion graded by count, 0.9/0.3/0 (`mining_util:104-108`) | **holds**; the inputs are the static per-belt counts (§4) |
| 1,223 planet stations; moon stations missing (`celestials.py:151`) | **holds**; the total is 5,210 *including one star station* (§3.2) |
| 8,089 systems with a star | **holds** (`celestials-all.tsv`) |
| `take_dirty` has one non-test caller, `dispatch.rs:1656` | **holds** |
| the customs band `8e9 + planetID` (`planet.rs:4134`); NPC 7e9-8e9 (`npc.rs:55,58`); escrow ≥ 9e9 (`item.rs:1644`); orders 5e9 (`market.rs:408`) | **holds** |
| rock ids 70M-80M (`asteroid.rs:64-67`) | **holds**; the ordinal derivation breaks §4.2 (§1.4) |
| `VACANT_DESPAWN` 60 s, `RESPAWN_AFTER` 5 min, `BELT_RANGE` 75 km | **holds** |
| `GetAverageMarketPrices` answers `{}` (`base.rs:53`) | **holds** |
| B1 "running now"; owns `space.rs`, `character.rs`, `live_misc.rs` | **stale**: landed in `80d63e4`, and also edited `service.rs` |
| "the gate re-checks it" (§8) | **false** (§1.1) |
| `Registry::touch_system` | **does not exist** (§2.3) |
| the generated tables have consumers only in `reprocess.rs` and `industry.rs` | **false**: `service/industry.rs:2450` uses `tables::BLUEPRINTS` directly |

Inherited and not re-checked: C6's 882 reactive spawners, C4's 10,972 and
2,892, C5's `corporationTrades`, and the 260,242 and 30,610 dungeon rows.
Per §1.1, these belong in `content.py --check`, not in a unit test.

## 6. Changes to apply to plan.md, in order

1. Add a gate step plus the `--check` split (§1.1), and seams S6-S12
   (§1.2, §1.5), all landing **before W11** (§2.1).
2. Write the data-access rule (§1.3) and the delta-keying rule (§1.4)
   into §1.3 and §4.2.
3. Freeze the headers (§2.2).
4. Add CP3b (§2.4) and CP53-CP56 (§1.5). Split CP11, CP26, CP28, CP32,
   CP33 and CP37 (§2.5). Move CP45 to W17, or give it an API (§2.3).
5. Fix the test lines in §3.1 and §3.2, and the O4 text (§3.3). Add O8
   (§3.5).
6. Refresh §6.1 for B1, and §8 for the false and stale rows (§5).
