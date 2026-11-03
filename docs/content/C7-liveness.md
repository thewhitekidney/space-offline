# C7: liveness and operations

Lane C7 of `docs/content/README.md` §3. Read-only research lane: this file is
its only output. It covers what is *live* in a universe of 8,089 extracted
systems with 2-10 humans and about 50 bots, what survives a restart, how a
restart reproduces the universe, the daily rollover, the tick budget, and how
an operator sees any of it.

Marks follow §2 of the brief: **[V]** read from code or client, with the
location; **[V2]** EvEJS; **[I]** inferred, with the inference stated;
**[A]** an assumption we would ship, with what would falsify it.

Tree read: HEAD `9ba25e7` plus the W10 lanes' working copies of `npc.rs` and
`dungeon.rs`, read and not touched. Line numbers are from the working tree on
2026-10-25 and will drift.

---

## 0. The answer in one screen

1. **Content is a pure function, and state is a list of differences from it.**
   `content(world_seed, day, system)` says what *should* be in a system today.
   The world stores only what players changed: a rock mined down, a site
   cleared, an NPC order bought out. A restart recomputes the function and
   reapplies the differences. That is the whole persistence model for
   generated content (§3).
2. **There are three tiers of system: cold, warm and hot.** A cold system
   holds no RAM state and is never ticked. Of 8,089 systems, a few dozen are
   ever warm. The ticker iterates only hot systems (§2).
3. **For a cold system the rollover costs nothing**, because `day` is an
   input to the function. Rollover work is proportional to the systems that
   hold differences, not to 8,089 (§4).
4. **One 64-bit world seed is drawn at world birth and persisted.**
   Everything generated derives from `mix(seed, kind, system, day, slot)`
   through one module. Today four ad hoc PRNGs do this job (§1.3).
5. **The tick budget is enforced by measurement, not by hope.** The health
   line that exists today grows per-subsystem timings and a `[live]` line.
   Hard caps (hot systems, NPCs per system) degrade gracefully when they bind
   (§5).
6. **Humans and bots share content through supply, not privilege.** Bots are
   ordinary TCP clients with characters (`eve-bot/src/lib.rs:1-7` **[V]**).
   Fairness comes from per-system supply floors, bot pacing and a courtesy
   rule in the bot brain, not from server-side favouritism (§6).

---

## 1. What the server does today (the facts this plan stands on)

### 1.1 Threads, the world, the clock

*Refreshed by IP26 against the tree after IP8, IP9 and RIDX; every
`file:line` here was re-checked then.*

- **One world:** `Arc<RwLock<Registry>>`, built once in `Server::bind`
  (`server.rs:515`). A payload turn takes `write()` exactly once
  (`dispatch.rs:1828`). **`read()` is taken too, by chat:**
  `ChatHub::reconcile` and three stanza paths on the chat connection threads
  (`chat.rs:943`, `:1257`, `:1297`, `:1445`), each reading the world and
  dropping it before taking the hub (`docs/reference/threads-and-locks.md` §3.1, F9)
  **[V]**.
- **`world-ticker`** (`push.rs` `spawn_world_ticker`) runs on a 250 ms
  deadline, `TICK` at `push.rs:395` **[V]**. Under one write lock it runs, in
  order: `targeting::sweep_out_of_range` (`push.rs:816`), then the
  `tick::STAGES` table (`tick.rs:47-79`, through `tick::run_all` at
  `push.rs:823`): `liveness::refresh` → `modules::tick` → `combat::tick` →
  `mining::tick` → `npc::tick` (which starts with `belts::tick_belts`,
  `npc.rs:804`) → `jump::tick` → `skill_training_tick`; then `take_due`
  (`push.rs:827`) and `session_changes_due` (`push.rs:853`); and every
  `SAVE_EVERY` = 30 s, a save if the world is dirty (`push.rs:854-867`, CP15).
  It then encodes and posts with no world lock held, through `Writers::post`
  (`push.rs:897`, `:910`) **[V]**.
- **`TickHealth`** (`push.rs:470-579`) prints a `[world] tick health` line
  every 40 ticks (10 s): late mean and max, over-budget count (`LATE_BUDGET` =
  100 ms), world-lock wait and held (mean and max), work max and skipped
  deadlines; and a `[world] tick stages` line with each stage's mean and max
  **[V]**. Since CP15 (`756ab56`) the wait is timed apart from the hold
  (`push.rs:805-808`, `:873`); it used to be measured from before the
  `write()`, so "lock max" was wait plus hold (V3 F3). Beside it,
  `crate::live_status` prints a `[turns]` line (payload turns, their hold, wait
  and saves, the slowest call) and a `[live]` line, and `--status FILE` writes
  the live systems as TSV **[V]**.
- **Motion is analytic.** `advance_system`/`advance_character`
  (`service.rs:1113`, `:882`) move a ball to "now" from its `Motion`, so an
  unwatched ship does not need a tick to stay correct **[V]**. This is what
  makes lazy systems cheap: a system can sleep and then be advanced on wake.
- **`in_system` is an index lookup, not a scan, for both ships and rocks.**
  This bullet used to say "`Roster::in_system` is a linear scan of every
  ball", and that stopped being true twice:
  - **Ships:** `Roster` keeps `by_system: system -> BTreeSet<character>`
    (`space_state.rs:1037`), written only by `arrive` and `leave`, so
    `Roster::in_system` (`space_state.rs:1086`) costs the balls in that system
    (W11 CP6, the index §2.3 proposes for ships) **[V]**.
  - **Rocks:** RIDX (`ee8070c`) gave `asteroid::Field` the same shape,
    `by_system: system -> Vec<rock id>` ascending (`asteroid.rs:326`), built
    in `Field::new` and maintained by `Field::mine`. `Field::in_system`
    (`asteroid.rs:425`) reads it instead of filtering all 2.12M rocks of the
    content bundle. Measured by the RIDX lane: the `beyonce` bind went from a
    median 38.3 ms to 11.4 ms on release (`docs/lanes/ridx.md`) **[V]**.
    Pinned by `asteroid::tests::in_system_is_the_filter_after_every_mutation`
    (`asteroid.rs:873`).
  - **Debug builds still pay a check.** `Roster::in_system` rescans every
    ball to cross-check its answer (`space_state.rs:1092-1104`), and
    `Field::check_index` (`asteroid.rs:451`) checks the rock index on every
    call, with the full old filter only up to 50,000 rocks. So on debug a
    per-system pass is still O(all balls); on release it is O(balls in the
    system) **[V]**. The debug bind is 52.3 ms (`ridx.md`).

### 1.2 Persistence

- **Format:** `store.rs` is one line-oriented text snapshot, `SCHEMA = 2`,
  written tmp + fsync + rename (`store.rs:206-223`) **[V]**.
- **Contents:** balances, items with `nextitem`, journal with `nexttx`,
  skills, logoffs, bios, standings, jobs, blueprints, bookmarks, contacts,
  clones, queues, corps, market, planets and contracts (`store.rs:64-160`)
  **[V]**.
- **Save trigger:** `dispatch.rs:1673`, `if w.take_dirty() { store.save(...) }`,
  **inside the world lock, after a payload** **[V]**.
  - **The world ticker never saves.** A durable change the clock makes
    (mining ore into a hold through `apply_items`, a skill level, a kill)
    sets `dirty` and waits for the next payload from *any* connection
    **[V]**: `take_dirty` has one caller.
  - With 50 bots that wait is short. With nobody connected it is unbounded,
    and the only exit is a kill, which has no hook.
- **Not persisted, by explicit comment:** roster, locks, modules, combat,
  NPCs, jumps, probes, fleets and dungeon pockets
  (`service.rs:369-403`; `dungeon.rs:24-25`; `npc.rs` header) **[V]**.
  **A restart empties space.**
- **Asteroid depletion** lives as long as the process. A restart regrows
  every rock (`asteroid.rs:57-59`) **[V]**.

### 1.3 Seeding today: four PRNGs, no world seed

| Where | What | Seed |
|---|---|---|
| `asteroid.rs:450-470` `Mix` | rock layout | `belt_id ^ 0xA57E_801D` |
| `npc.rs:532` `roll` | NPC choices | `(seed, salt)` mixed |
| `scan.rs:1064` `smear` | probe result noise | splitmix from a caller seed |
| `industry.rs:1451-1460` | invention successes | `job_seed(job)` |
| `warp_in.rs:77-172` `PyRandom` | warp-in scatter | the client's own MT19937, **must match CPython** |

All are deterministic in their inputs, which is the right instinct **[V]**.
None takes a world-level seed, so two worlds are identical, and none takes a
day, so nothing varies over time. `warp_in`'s MT must stay as it is: it
reproduces a client computation, and it is not content.

### 1.4 What is already lazy

- **Belt rats are lazy, and that is the pattern to generalise.**
  `dungeon::tick_belts` (`dungeon.rs:361-399`) visits only systems that hold
  a configured character in the roster, or a belt that still has rats.
  - It fills a belt when a pilot comes within `BELT_RANGE` = 75 km.
  - It empties a belt after `VACANT_DESPAWN` = 60 s with nobody present.
  - It refills a cleared belt only after `RESPAWN_AFTER` = 5 min
    (`dungeon.rs:238-266`) **[V]**.
- **Asteroids are eager.** `Field::generate` lays out every belt in the file
  at boot (`asteroid.rs:319-337`) **[V]**.
  - `up.sh` passes a one- or two-system extract (`tools/up.sh:327-337`) **[V]**.
  - `out/belt-rocks-all.tsv` holds **2,123,854 rocks in 40,928 belts across
    3,974 systems** (counted with awk) **[V-data]**. Passing it would build
    every one of those rocks at boot.
  - Cost **[I]**: a `Rock` is around 80-100 bytes, plus a BTreeMap node per
    rock, so about 250-350 MB of RAM and seconds of rejection sampling
    before the listener opens. That is the first scaling wall a universe-wide
    server hits.
- **Celestials:** `out/celestials-all.tsv` has 477k rows over **8,089
  systems with a star** (8,089 stars, 68,407 planets, 344,457 moons, 40,928
  belts, 13,978 gates and 1,223 stations) **[V-data]**.
  - "8,490" in the brief presumably also counts systems with no row here,
    such as wormhole, abyssal and special systems **[I]**.
  - The row is static, and the server already loads per-system subsets.

### 1.5 The client's idea of downtime **[V]**

`gametime/downtime.pyc`:

- `get_downtime_start_time` (lines 14-17) reads `monolithconfig`
  `DowntimeStarts` from `zcluster`. With no value it logs a warning and
  defaults to **`time(11, 0)`**.
- `get_last_downtime_datetime` (lines 31-37) uses **`datetime.now()`, the
  local clock, not UTC**. It takes today at 11:00 and steps back a day if
  that is still in the future.
- `get_next_downtime` adds `DAY`. `is_downtime_done_today` compares dates.
- Callers: `pvpFilaments/common/util.pyc`, `newFeatures/feature.pyc` and
  `jobboard/client/open_on_startup.pyc`.

So the client's "today" turns over at 11:00 **in the client's local zone**.
Our server's rollover should be 11:00 UTC, and the jail's `TZ` should be UTC
so that the two agree **[I]**: the client's helper agrees with the server only
if the client's zone is UTC. Falsified by a client feature that shows the
reset at the wrong hour under a non-UTC `TZ`.

---

## 2. Lazy versus eager: system tiers

### 2.1 The three tiers

| Tier | Entered when | Holds in RAM | Ticked |
|---|---|---|---|
| **cold** | default; or warm with no reason to stay for `COOL_AFTER` | nothing beyond the static catalogue | never |
| **warm** | a character is docked in it, has it as a mission or courier destination, or has an autopilot or route waypoint there; **or** it holds a persisted difference (§3) that has not expired; **or** a hot neighbour is within 1 jump [A] | the materialised content list (sites, belt compositions, NPC order book for its stations) | no |
| **hot** | a ball that is a character (human or bot) is in space in it; **or** live NPCs remain (the despawn timers still run) | everything warm has, plus rocks as balls, NPCs, wrecks and pockets | every tick |

- **Rule: content is materialised on the cold-to-warm edge and dropped on
  the warm-to-cold edge.** The edges are the only place generation runs. A
  cold system answers any question (a probe scan, the agency, `/live`) by
  calling the pure generator *without* materialising.
- **Hysteresis:**
  - hot to warm: when the last pilot leaves space and the NPC despawn
    finishes (today's `VACANT_DESPAWN`, 60 s **[V]**);
  - warm to cold: `COOL_AFTER` = 10 min **[A]** after the last warm reason
    ends.

  The 10 min stops a pilot who bounces between two gates from regenerating a
  system every jump. Falsified by an operator seeing a regenerate storm in
  `[live]`.
- **The one-jump neighbour rule** **[A]** exists so that a gate jump lands
  in a warm system and no generation happens inside the jump turn. Falsified
  if materialising is cheap enough (measured under §5) that it can run
  inline on arrival. Then drop the rule.

### 2.2 Expected scale **[I]**

These are estimates, not measurements.

- **Hot systems:** 10 humans plus 50 bots in space, clustered by design
  (§6.3), gives **about 10-30 hot systems** and about 50-120 warm ones.
  Everything else is cold.
- **Steady-state RAM for content:** a hot system materialises at most
  ~2,000 rocks (its belts) and a few hundred NPCs. That is well under 50 MB
  for the whole hot set.
- **Comparison:** about 300 MB for eager rocks.

### 2.3 Server shape: one index, consulted by every tick

The server keeps one `Liveness` index, `BTreeMap<system_id, SystemLive>`,
owned by `Registry` and kept current by:

- `session_closed`, undock, dock, jump arrival and warp-in, through a single
  method, `Registry::touch_system(system, reason)`;
- `liveness::tick(w, now)`, run first in the world ticker. It demotes
  systems whose reasons expired and returns the hot list.

Every per-system tick takes that hot list instead of deriving its own:

- `tick_belts` derives its system list from `config().characters`
  (`dungeon.rs:362-375`) **[V]**;
- `npc::tick` derives its list from NPCs (`npc.rs:957-961`) **[V]**.

`Roster` gains a per-system index (`BTreeMap<system, BTreeSet<id>>`), kept in
step by `insert`/`remove`/system change, so `in_system` is O(balls in system)
**[I]**: needed once the hot set has hundreds of NPCs.

---

## 3. Persistence of generated content

### 3.1 The model: `world = generate(seed, day) + deltas`

**Generated content is never stored.** Only its *differences* are, and each
difference expires at a known time. Every stored difference names its
generated parent by a deterministic id, so it reattaches after a restart
without a lookup table.

| Generated thing | Its id | The difference stored | Expires |
|---|---|---|---|
| a rock | `MIN_ROCK_ID + ordinal*200 + n` (`asteroid.rs:62-75`) **[V]**, already deterministic | `rock <id> <remaining_qty>`; a gone rock is `0` | at rollover every rock is reset in full (the daily full reset, BELTS #188; supersedes R1's regrowth, §4.2); the record is deleted |
| a site (anomaly or signature) | `site_id(seed, day, system, slot)` (§3.3) | `site <id> <state> <cleared_at>`, with state `warped`, `cleared` or `looted` | at the site's own despawn time, or at rollover, whichever is first |
| an NPC sell or buy order | `order_id(seed, day, station, type)` | `npcorder <id> <remaining>` | at rollover (refilled to the generated volume) |
| an agent's offer | `(agent_id, day, slot)` | `offer <char> <agent> <mission> <state> <expires>` | 7 days for an accepted mission **[A]**; offers rotate at rollover |
| a belt's respawn wait | `belt_id` | `beltwait <belt_id> <respawn_at>` | when it passes |

**Not persisted, and why (unchanged from today):**

- NPC balls, wrecks, locks and modules. A restart that empties space
  respawns the rats in a site that is not `cleared`, which is
  indistinguishable from "they warped back in".
- A cleared site **stays cleared**, because that is the one fact a player
  would notice losing.
- Wreck contents are a gap: loot left in a wreck at a restart is lost
  **[A]**. Falsified by a player losing a drop they cared about. Then
  persist wrecks that hold items as items with `locationID = wreck` and a
  `wreck` record.

### 3.2 Record format

The format is new snapshot record types, not a schema bump. `store.rs`
already tolerates unknown records for exactly this (`store.rs:94-99`, the
`logoff` comment) **[V]**.

```
world   <seed_hex>  <birth_filetime>  <day>  <last_rollover_filetime>
rock    <rock_id>   <remaining>
site    <site_id>   <state>  <until_filetime>
npcorder <order_id> <remaining>
offer   <char_id>   <agent_id>  <mission_id>  <state>  <expires_filetime>
beltwait <belt_id>  <respawn_at_filetime>
```

- **The `world` line is written first after the magic line.** A file without
  one is a pre-C7 world. The loader then draws a seed, sets `day =
  day_of(now)`, and marks dirty: a lazy migration that loses nothing,
  because no generated content existed before.
- **Size bound [I]:** differences are deleted at rollover once regenerated.
  At 60 pilots that is hundreds to low thousands of lines a day, small next
  to the journal.

### 3.3 Ids for generated things

- **Sites:** `site_id = 8_000_000_000 + (day % 1000) * 600_000 + sys_index *
  64 + slot` **[A]**.
  - `sys_index` is the system's ordinal in the sorted celestial catalogue,
    below 8,192, so ×64 < 524,288 < 600,000.
  - The range sits just past `END_NPC_ID` (8e9, `npc.rs`) **[V]** and inside
    the same "above every client id class" argument the NPC range makes.
  - Falsified as the NPC range is: a client path that mishandles ids above
    2³¹.
  - Ids differ per day, so a client that cached yesterday's site is never
    handed today's under the same id. That is the hazard
    `docs/reference/world-state.md` §2 item 4 names for item ids.
- **Signature target ids** (the "ABC-123" a scanner shows): three letters and
  three digits drawn from `mix(seed, SIG, site_id)`. Unique within a system
  by rejection.
- **NPC order ids:** a block below the player order allocator, derived as
  `order_base + hash(...) % span`. The span and the player allocator's floor
  are **unknown** until `service/market.rs`'s allocator is read by the C5
  owner. **This must be settled with C5.**

### 3.4 Allocators across a crash: leap on load

`nextitem`, `nexttx` and job ids are persisted **[V]**, but the save is
coalesced (§5.4) and can trail RAM. A crash then reissues ids the client may
have cached.

**Rule:** on load, each allocator becomes `max(stored, persisted) +
LEAP` with `LEAP` = 1,000,000 **[A]**. That costs nothing against a 64-bit
range, and it removes the stale-cached-object class that
`docs/reference/world-state.md` §2.4 calls out. Falsified by nothing realistic; it can
only waste ids.

---

## 4. Deterministic seeding and the daily rollover

### 4.1 One seed module

**`src/seed.rs`** **[A]** for the name. The server does:

- `pub fn mix(seed: u64, parts: &[u64]) -> u64`: splitmix64 folded over the
  parts. It is the same constants as `asteroid::Mix` (`asteroid.rs:456-463`)
  **[V]**, so asteroid layouts can migrate without changing.
- `pub struct Stream(u64)` with `next`, `unit` and `range`, replacing
  `asteroid::Mix` and `scan::smear`'s inline mixer. `npc::roll` becomes
  `mix(world.seed, &[NPC, id, salt])`.
- `pub enum Kind { Site, Sig, Belt, NpcOrder, Offer, Spawn, Loot, ... }`:
  `as u64` constants, so two generators never draw the same stream.
- `pub fn day_of(filetime) -> u32`: days since world birth's first 11:00
  UTC.

**The rule:** every generator takes `(seed, day, system, slot)` and *no
other randomness*. In particular it never reads wall-clock time, never reads
`HashMap` iteration order, and never depends on the order systems were
touched. A test pins a digest: generate days 0..3 for 20 fixed systems from
seed `0x5EED` and compare one u64 against a constant. A change to any
generator then fails the test, and the commit has to say why the universe
moved.

**Keep `warp_in::PyRandom` as it is.** It reproduces the client's MT19937 and
is not content.

### 4.2 The rollover

**When:** the first world tick after `11:00:00 UTC` where `day_of(now) >
world.day` **[A]**. The client default is 11:00 local
(`gametime/downtime.pyc:14-17`) **[V]**. Falsified by an operator who wants
another hour: a `--downtime HH:MM` flag overrides it, and `tools/up.sh` must
then set the jail's `TZ` to match.

**There is no downtime.** Nobody is kicked; the world keeps running. The
client's helpers treat 11:00 as a boundary, and nothing in the client is
known to require a disconnect (**unknown**: C2 should confirm that no client
path assumes a session never spans downtime).

**What it does,** under the world lock, in **slices of at most
`ROLLOVER_SLICE` = 64 systems per tick** **[A]**. That is 64 systems × 250 ms,
so 1,000 touched systems take 4 s. Cold systems are not in the list.

1. `world.day += 1`, and `last_rollover = now`. Mark dirty.
2. **Rocks (R1, superseded by BELTS #188):** every `rock` difference is
   reset in full: the daily full reset (offline agents are settled first, step
   `econ_agents_close`). The old `REGROW` = 0.5 a day is gone, and the record
   is deleted.
   - In a hot system, regrown rocks get their balls back through the same
     path `mining` uses to remove them (an `AddBalls` and slim to watchers).
   - A belt mined flat therefore comes back at the next rollover.
3. **Sites:** every `site` difference with `until <= now` is deleted. Each
   *warm* system re-materialises its site list for the new day. Sites a
   pilot is standing in (hot, with a character within 100 km) are **kept
   until the pilot leaves**: rollover never despawns content under a
   player. Their ids belong to the old day, which is why the id embeds the
   day.
4. **NPC market:** every `npcorder` difference is deleted. The book refills
   to the generated volume on next materialisation. Volumes and prices are
   C5's.
5. **Agent offers:** unaccepted offers are rotated. Accepted missions keep
   their own expiry. The pool is C4's.
6. **Belt waits:** past ones are deleted.
7. One log line, `[rollover] day N: S systems, D deltas pruned, R rocks
   regrown, T ms over K ticks`.

**Catch-up after downtime of the process:** on load, if `day_of(now) >
world.day + 1`, rollover runs **once** with `elapsed_days` as a parameter.
Rocks regrow by `elapsed × REGROW`; everything else is idempotent. It does
not run N times: a server off for a month must start in seconds.

**Rollover is the only scheduled mutation of persisted content.** Respawn
timers inside a day (belt rats 5 min, sites' own despawn) stay as they are
today, on the tick.

---

## 5. The tick budget

### 5.1 Numbers

| Budget | Value | Status |
|---|---|---|
| tick period | 250 ms | **[V]** `push.rs:395` |
| late budget | 100 ms | **[V]** `push.rs:404` |
| **world-lock hold per tick, target** | **≤ 25 ms max** (10% of a period) | **[A]**; falsified by `over_budget > 0` with lock max under 25 ms |
| world-lock hold per payload turn, target | ≤ 5 ms excluding fsync | **[A]** |
| hot systems, soft cap | 64 | **[A]**; above it, §5.3 |
| NPCs per system | 60 balls | **[A]**; belt plus site spawns are capped together |
| NPCs, global | 1,200 | **[A]** |
| rollover slice | 64 systems per tick | **[A]** |
| materialise one system | ≤ 5 ms | **[A]**; falsified by `[live] gen max` |
| bot payload rate | ≤ 2 per second per bot | **[A]**; that is 100 turns a second from 50 bots, about 10-50% of the lock at 1-5 ms each. Falsified by human latency in `[live]` |

### 5.2 Measure per subsystem

`TickHealth` gains one `Duration` max per stage (`sweep`, `modules`,
`combat`, `mining`, `npc`, `belts`, `jump`, `skills`, `liveness`,
`rollover`, `take_due`). Each stage is timed with `Instant::now()` around the
existing calls. The health line grows a second line:

```
[world] tick stages max ms: sweep 0.1 modules 0.3 combat 0.4 mining 0.1 npc 2.0 belts 0.2 jump 0.0 skills 0.0 live 0.1 roll 0.0 due 0.2
```

This is what turns "the server is slow" into a name. The EvEJS lesson is
already recorded at `push.rs:440-450`: measure lateness *and* the inside of
the tick.

### 5.3 Graceful degradation, in order

When `lock_max` runs over 25 ms for three reports in a row:

1. Hot systems with no **human** in them tick every second tick
   (`tick_parity`). Their NPCs still fire, at half the decision rate, and
   `npc::MAX_SHOTS_PER_TICK` = 8 (`npc.rs`) **[V]** already absorbs the
   catch-up.
2. New belt and site spawns in bot-only systems are deferred.
3. A `[live] DEGRADED` line names the stage that exceeded.

Humans are never degraded. This needs one bit per character, `is_bot`. The
bot config already exists as a separate binary (`bin/eve-bots.rs` per
`eve-bot/src/lib.rs:37-40`) **[V]**. Mark the bot characters in their
`--character` spec or by an account flag **[A]**.

### 5.4 Saves: coalesce them, and save from the clock too

Today, with 50 bots mining, almost every payload is dirty, so the server
rewrites the whole snapshot with an fsync **inside the world lock**, possibly
dozens of times a second **[I]** (from `dispatch.rs:1673` plus `apply_items`
marking dirty). That is the single largest threat to the tick budget at bot
scale.

**Proposal:**

- Replace "save after every dirty payload" with a **dirty generation plus a
  saver**:
  - the world ticker, every `SAVE_EVERY` = 5 s **[A]**, renders the snapshot
    to a `String` *under the lock*, which is CPU only;
  - it hands the string to a `saver` thread over a `std::sync::mpsc`
    channel, and that thread writes, fsyncs and renames *with no lock*.
- This deliberately revises `docs/reference/world-state.md` §4 rule 3 (RAM and disk
  change together). RAM may lead disk by up to 5 s, and a crash loses at
  most 5 s.
  - The allocator leap (§3.4) removes the one hazard that makes that
    unsafe.
  - **This is an owner decision:** say so in the commit.
- The world ticker also saves, which closes the "nobody connected, clock
  mutated, never saved" gap from §1.2.
- Log `[save] gen G, B bytes, render R ms, write W ms` at most once every 10
  s.

---

## 6. Bots and humans sharing content

### 6.1 Principle

Bots are players. The server does not hand them content by privilege, and it
does not hide content from them. Starvation is prevented by **supply**,
**pacing** and **courtesy**.

### 6.2 Supply floors (server)

- **Sites:** the generator guarantees, per hot or warm system, at least
  `max(base(sec), 1 + ceil(pilots_in_system / 3))` combat anomalies of the
  system's archetype **[A]**. `base(sec)` comes from C3.
  - A cleared site in a populated system is replaced the moment a slot
    frees, after a `SITE_RESPAWN` = 10 min delay **[A]**, by a new slot
    draw.
  - Draws are deterministic: `slot` increments, so the day's sequence is
    reproducible.
- **Belts:** the existing 5 min `RESPAWN_AFTER` stands **[V]**. A system's
  belts are its supply. A system where bots have mined every rock regrows
  only at rollover. That is intended: it pushes bots outward (§6.3).

### 6.3 Pacing and placement (bot side, `eve-bot`)

- **Rate:** the driver sleeps so a bot issues at most 2 payloads a second.
- **Home systems:** each bot is assigned a home from a short list,
  `--bot-systems`, by default the humans' home system plus its neighbours
  within 2 jumps **[A]**. This keeps the hot set small (§2.2) and the
  universe *visibly* inhabited where humans are.
  - Falsified by humans feeling crowded. Then widen to 4 jumps.
- **Courtesy rule (brain):** on landing at a site or belt where a
  non-fleet-member **human** is already within 100 km, a bot in a
  ratting or mining role leaves for the next one.
  - The bot can see this from its own ballpark: a ship whose owner is a
    character id with no bot marker. That needs the marker in the
    slim/overview data, or a bot-side name list **[A]**.
  - Escort bots (`eve-bot/src/escort.rs`, uncommitted W10 work) are exempt.
  - Falsified by "the bots never let me find anything" or "the bots act
    scripted". The first means the floor is too low; the second means the
    courtesy range is too large.

### 6.4 The world lock and fairness

`std::sync::RwLock` on Linux makes no FIFO promise among writers **[I]**, so
50 bot threads can in principle delay a human's turn. Pacing (§6.3) and the
5 ms turn budget bound it.

**The measure:** `[live]` reports the max wait for the world lock on
human-owned connection threads, `lock_wait_human_max`, taken as
`Instant::now()` before and after `world_write`. If that exceeds 50 ms
**[A]**, the next step is a ticket lock (a std `Mutex<u64>` plus a
`Condvar`) in front of the world, not a dependency.

---

## 7. Observability: how an operator sees what is live

There is no web UI and no new dependency. Everything is greppable text.

1. **`[live]` line**, every 10 s beside the health line:
   ```
   [live] day 12 next-rollover 7h41m | systems hot 14 warm 63 cold 8012 | pilots 7h 48b (in space 5h 41b) | npcs 412 sites 188 rocks 9,340 | deltas rock 1,203 site 77 npcorder 410 | gen max 3.1 ms | lock-wait human max 4 ms | save gen 8812 2.4 MB 38 ms
   ```
2. **`--status FILE`** (default `out/live/status.tsv` when `--world` is
   set). The file is rewritten atomically (tmp + rename, no fsync; it is
   disposable) every 10 s by the saver thread from a snapshot the ticker
   renders:
   ```
   #system  name?  tier  humans  bots  npcs  sites_live  sites_cleared  rocks  rocks_depleted_pct  last_touch_utc  reasons
   ```
   - Names are unknown server-side unless the celestial extract carries
     them, so write ids only and let the operator's `tools/` script join
     names from `out/`.
   - `watch column -t out/live/status.tsv` is the operator console.
3. **`/live` and `/live <systemID>` slash commands** (GM role, as
   `/giveisk`). They print the `[live]` line, or one system's row plus its
   site list (id, archetype, state, until), into the chat reply. Today's
   slash set is `/createitem /giveisk /giveskill /givestarterskills
   /removeskill /tp /whoami /pushtest` **[V]** (grep of `service/*.rs`).
4. **`/rollover now`** (GM): runs §4.2 immediately and advances `day`. It is
   the only way to test a rollover without waiting a day, and the morning
   human check needs it.
5. **Log lines with fixed prefixes**, so `tools/client-ci.sh` and an
   operator can grep them: `[world]`, `[live]`, `[rollover]`, `[save]`,
   `[gen] system S materialised in T ms (sites N, rocks R)` (once per
   materialisation), and `[gen] system S cooled`.

---

## 8. Restart, step by step (what the operator should expect)

1. `bind`: load the snapshot.
   - No `world` line: draw a seed from `SystemTime` nanos ^ pid through
     `mix` (it does not need to be unpredictable, the same argument as
     `docs/reference/server.md` §3), set `day`, and mark dirty.
   - Apply the allocator leap (§3.4).
2. Run catch-up rollover if a day or more was missed (§4.2).
3. Every system starts **cold**, and space is empty, as today **[V]**.
4. Logins promote systems:
   - docked: warm (the NPC book and agent offers materialise);
   - undocked: hot (rocks, sites and rats materialise from `generate(seed,
     day)` with differences applied). A cleared site stays cleared. A
     half-mined rock is half-mined. A mined-out rock is gone, until
     rollover.
5. Pilots who logged off in space appear docked, because the W9-L7 hold is
   not persisted **[V]** (commit `e1b21a7` message). Persisting it is out of
   scope here and noted in unknowns.

---

## Proposals

1. **Adopt `world = generate(seed, day, system) + expiring deltas`** as the
   persistence model for all generated content. Store no generated row, only
   differences keyed by deterministic ids (§3.1).
2. **One `seed.rs`.** A world seed persisted in a new `world` snapshot
   record. Every generator is a pure function of `(seed, day, system,
   slot)`, with a pinned digest test (§4.1).
3. **Three system tiers (cold, warm, hot)** in one `Liveness` index that
   every per-system tick consumes. Materialise on cold-to-warm, drop on
   warm-to-cold after 10 min (§2).
4. **Asteroid rocks become lazy per system.** `Counts` is indexed by system,
   and the `Field` is filled on warm. This unblocks loading
   `belt-rocks-all.tsv` (2.12M rocks) without a boot wall (§1.4).
5. **The daily rollover runs at 11:00 UTC** (the client default is 11:00,
   `gametime/downtime.pyc:14-17`). It never disconnects anyone, runs in
   slices of 64 systems per tick, catches up once after an outage, and has
   a `/rollover now` GM command (§4.2). Set the jail `TZ=UTC`.
6. **Coalesced saves:** render under the lock every 5 s from the world
   ticker, and write and fsync on a saver thread with no lock. Leap the
   allocators by 1,000,000 on load. This needs the owner's sign-off because
   it revises `docs/reference/world-state.md` §4 rule 3 (§5.4).
7. **Tick budget:** 25 ms of world lock per tick, with per-stage timings in
   `TickHealth`, caps on hot systems and NPCs, and degradation that slows
   bot-only systems first and never humans (§5).
8. **Bots share by supply, pacing and courtesy:** per-system site floors
   scaled by pilots, 2 payloads a second per bot, bot homes near the humans,
   and a brain rule to yield to a human at a site (§6).
9. **Operator observability:** the `[live]` line, `--status FILE` TSV,
   `/live` and `/live <sys>`, and fixed log prefixes (§7).
10. **Site ids embed the day** (`8e9 + day%1000 × 600k + sys_index × 64 +
    slot`), so a client never sees two days' sites under one id (§3.3).

## Packets

Each packet owns its files (lane rule 1). Edits to `push.rs` (the ticker's
call list) and `dispatch.rs` (the save site) are one-line hooks. The
coordinator applies them, or they go first as a seam commit. `dispatch.rs`
is a hot file.

| # | Packet | Owns | Depends on | Tests |
|---|---|---|---|---|
| **L1** | Seed module | new `src/seed.rs`; migrate `asteroid::Mix`, `scan::smear`'s mixer and `npc::roll` to it (those three call sites only) | none | `mix` vectors pinned; `asteroid` layouts unchanged (an existing test, or a pinned rock position); `day_of` boundaries at 10:59:59 and 11:00:00 UTC |
| **L2** | World record and deltas in the store | `store.rs` (new records `world`, `rock`, `site`, `npcorder`, `offer`, `beltwait`; `Snapshot` fields); `Registry::to_snapshot`/`apply_snapshot` hunks | L1 | round-trip of each record; a file without `world` loads, gets a seed and marks dirty; unknown-record tolerance kept; allocator leap applied on load |
| **L3** | Liveness index | new `src/liveness.rs`; `Registry::touch_system`; the per-system index in `space_state::Roster` | L1 | promote and demote with hysteresis by simulated time; a jump makes the destination warm before arrival; a cold system has no RAM entry; `in_system` index matches the scan |
| **L4** | Lazy rocks | `asteroid.rs` (`Counts` by system, `Field::materialise(system)`, `drop(system)`, `rock` delta apply and capture) | L2, L3 | materialise, mine, drop, rematerialise keeps the remainder; a restart keeps a depleted rock gone; memory of a cold universe is about zero rocks |
| **L5** | Rollover | new `src/rollover.rs`; `/rollover` slash in its own service module | L2, L3, L4 | regrowth by `REGROW`; slices bounded per tick; catch-up runs once for an N-day gap; a site under a pilot survives rollover; idempotent when run twice for the same day |
| **L6** | Tick stages and `[live]` | `push.rs` `TickHealth` only (stages, `[live]` line), plus the `--status` writer in `liveness.rs` | L3 | the health line names every stage and unit (extends `the_health_line_names_every_number_and_its_unit`); the status TSV header is pinned |
| **L7** | Coalesced saver | `store.rs` (`Saver` thread and channel), the `dispatch.rs:1673` hunk, and the ticker hook. **Owner sign-off first.** | L2 | a dropped server reloads state at most `SAVE_EVERY` old; a clock-only mutation is saved with no payload; the existing `the_world_survives_the_server_being_dropped` still passes |
| **L8** | Degradation and bot marker | `liveness.rs` (parity), `Character::is_bot` plus `--character` spec parsing in `bin/eve-server.rs` | L3, L6 | a bot-only hot system ticks every other tick under the flag; a human system never does |
| **L9** | Bot pacing and courtesy | `eve-bot/src/driver.rs` (rate), `eve-bot/src/brain.rs` (yield rule), `bin/eve-bots.rs` (`--bot-systems`). **After the W10 bot lane lands**: those files are dirty now. | none server-side | the brain yields when a human is within 100 km; the driver never exceeds 2 payloads a second over a 10 s window |
| **L10** | `/live` GM command | one new service module | L3, L6 | `/live` output contains the `[live]` fields; `/live <bad id>` answers usefully |

Sites, NPC orders and agent offers *as content* are C3, C5 and C4. C7 gives
them the frame: an id scheme, a delta record and a rollover hook. Their
packets call `liveness::materialise_hook` and `rollover::register`.

## unknown

- Whether any client path assumes the session never spans 11:00. The client
  helpers only compute times; their three callers were not read
  (`pvpFilaments/common/util`, `newFeatures/feature`,
  `jobboard/client/open_on_startup`). This is for C2.
- The real cost of materialising one system, and of a tick with 30 hot
  systems: there are no measurements, and this lane may not run cargo. All
  §5.1 numbers are **[A]** until L6 reports.
- Whether `std::sync::RwLock` writer starvation is observable at 50 bots on
  this kernel.
- Snapshot size at a week of bot play (journal and market history are
  append-forever). This decides when the SQLite question of
  `docs/reference/world-state.md` §3 comes back.
- The NPC order id span and the player order allocator floor
  (`service/market.rs`, for C5).
- The bot marker on the wire: whether anything in slim or overview data can
  carry "bot" without lying to the real client, or whether the brain must
  use a name list.
- Why `celestials-all.tsv` has only 1,223 stations against EVE's ~5,000 NPC
  stations. A filter in `tools/celestials.py` or `solarsystemcontent` is
  the suspect. Station count bounds the NPC market's warm set.
- The 401 systems in "8,490" that have no celestial row, and whether any of
  them (wormholes) should ever be hot.
- Persisting the W9-L7 log-off-in-space hold across a restart: not
  specified here.
- Wreck loot across a restart (§3.1 [A]).
