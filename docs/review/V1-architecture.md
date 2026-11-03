# V1: architecture and module structure

Review lane V1 (`docs/review/README.md` §3), at `3f2741d`. Read-only. All
measurements were taken in an exported copy at `~/code/eve-offline-lanes/V1/`
with `EVE_MAX_JOBS=4`. The copy has been deleted.

## 0. The answer in one screen

- **The Registry is a god object, but a harmless one now.** W10.5 stopped the
  bleeding. `service.rs` had a commit in almost every wave before W10.5. Since
  then only one commit has touched it (CP5's persistence hunk). The remaining
  shared edits come from three fan-outs that have no table yet:
  - **kill side effects**;
  - **logoff side effects**;
  - **delta kind names**.

  Add those three seams. Do not break up the Registry.
- **Do not split `eve-server` into crates.** Compile time is not a cost here.
  A clean workspace build takes 22 s, and an edit to any file rebuilds
  everything in about 8 s (§3).
- **Split two files and leave the rest alone:**
  - `service/character.rs` already holds four delimited sub-files.
  - `destiny.rs` is the next lane's hot file.

  Leave `planet.rs` and `industry.rs` alone: each has a single owner and low
  churn.
- **The `pub mod` everywhere hides about 116 dead items from rustc** (§5).
  The fix is cheap, and it turns "landed ahead of its consumer" into a
  compiler-checked fact.
- A stale 4,307-line `service.rs.orig` is committed. Delete it.

## 1. The Registry

**Shape.** **[V]**

- `pub struct Registry` is at `service.rs:338`.
- It has 33 named fields, plus `ext`, `content` and `deltas`.
- It has about 100 methods across three `impl` blocks:
  - `service.rs:564`;
  - `slash.rs:93`;
  - `service/fleet.rs:94`.
- 76 files name `Registry`.
- Domain functions take `&mut Registry`. The top callers are
  `service/character.rs` (35 signatures), `drones.rs` (30) and
  `service/inventory.rs` (24).

**Does it hurt?**

- **Borrows:** no. **[V]** No `mem::take` or put-back workaround touches a
  Registry field outside the Registry's own drains (`service.rs:830,1341,
  2069,2147`). Modules collect ids, then mutate, and that works.
- **Shared edits:** yes. **[V]** `service.rs` had 147 commits since
  2026-10-01, the most of any file. `dispatch.rs` was second with 48. After
  the seam commits landed, only `41d933f` (CP5) touched `service.rs`.
- **Encapsulation:** yes, mildly. **[I]** Every module can reach every
  subsystem through `w.x_mut()`. That is the price of one world under one lock
  (V3's subject). Hiding fields behind traits would add indirection and buy
  nothing a 2-10 player server needs. See Do not do D1.

**Are W10.5's extension points enough?** I walked the "registers" column for
W11-W18 in `docs/content/plan.md` §6.2. Every packet's registration lands as
one coordinator line in a table, with five exceptions:

- the CP5, CP9, CP15, CP59b and CP60 hunks are coordinator hunks;
- of those, only CP5's is in `service.rs`, and CP59b's touches it too.

So the tables cover the content waves. Three fan-outs are not covered, and
the PvP end goal leans on all three:

### F1. Kill side effects have no seam (cost: lanes queue on `combat.rs`, the PvP hot file)

**What is there now.** **[V]**

- `combat::kill` (`combat.rs:720`) and `kill_npc` (`:858`) call each side
  effect by hand:
  - `npc::pay_bounty` (`:681`);
  - `drones::destroyed` (`:736`);
  - `mission::npc_killed` (`:903`).
- The plan already lost a slot to this. Review row 17
  (`docs/content/plan.md:65`) moved CP45 to W17 "because CP44 owns
  `combat.rs`".

**What is still to come.** **[I]** These packets want a "something died"
event, and each would otherwise edit `combat.rs` or ask CP44 for a hook:

- CP44 (insurance payout);
- CP38 and CP50 (DBS, bounty side effects);
- CP53b (FW victory points);
- CP28b (site squad cleared);
- killmails;
- kill rights and security status, for PvP.

**Change: seam S13, `src/kill_hooks.rs`.**

```rust
pub struct KillEvent {
    pub victim_character: i64,
    pub victim_ship: i64,
    pub ship_type: i64,
    pub killer: Option<i64>,
    pub system: i64,
    pub npc: Option<NpcKind>,
    pub when: BlueTime,
}

pub type Hook = fn(&mut Registry, &KillEvent);

/// Coordinator-only.
pub const PLAYER: &[Hook] = &[ /* ... */ ];
/// Coordinator-only.
pub const NPC: &[Hook] = &[ /* ... */ ];
```

- `kill` and `kill_npc` build the event once and call `fire`.
- The three existing calls become the first rows, **in today's order**, so
  the change is behaviour-neutral.

### F2. Logoff side effects are a hand-written list in `service.rs` (cost: every PvP and presence packet edits the hottest file)

**What is there now.** **[V]** `Registry::session_closed`
(`service.rs:1791`) runs, inline, when the last session drops:

1. `stamp_logoff`;
2. module stop;
3. contact presence;
4. station guest frame;
5. `crimewatch::logged_off` plus emergency warp;
6. fleet disconnect.

`session_selected_character` (`:1927`) is the mirror image.

**What is still to come.** **[I]** None of these is a listed hunk in the
plan, but each would need a line in this list:

- CP57 tackle (clear a scram on logoff);
- drones abandoned on logoff;
- chat presence;
- the bot courtesy rules in CP39.

**Change: seam S14, `src/session_hooks.rs`.**

```rust
/// (character_id)
pub const ON_LOGOFF: &[fn(&mut Registry, i64)] = &[ /* ... */ ];
pub const ON_LOGIN: &[fn(&mut Registry, i64)] = &[ /* ... */ ];
```

- Both tables are coordinator-only.
- The six steps move verbatim, in order, into named functions in their
  owning modules.
- `stamp_logoff` stays first and stays in `service.rs`.

### F3. Delta kinds are bare strings owned by 12 packets (cost: a silent collision, the "lanes reused packet names" failure in data form)

**What is there now.** **[V]**

- `Deltas::of` and `put` take any `&str`.
- Plan §4.2 lists 12 kinds: `rock`, `site`, `beltwait`, `mission`,
  `npcorder`, `npcmult`, `mktidx`, `dbs`, `lp`, `arc`, `fwvp` and `wh`.
- Each belongs to a different CP, and nothing in code lists them.
- Two packets picking the same kind would share rows with no error.

**Change: `src/delta_kinds.rs` (coordinator-only).**

- Add `pub const KINDS: &[(&str, &str)]`, each pair being
  `(kind, owning module)`. Each module exports `pub const KIND: &str` and
  uses only that.
- Add `delta_kinds::tests::kinds_are_unique_and_lowercase`.
- Add a `debug_assert!` in `Deltas::put` that the kind is registered. Loads
  still keep unknown kinds verbatim (the CP5 rule).

### What the seams do not cover, and need not

- **Persistence of the old subsystems.** **[V]** Each pre-W11 persisted
  subsystem is hand-wired in four places:
  - a `Registry` field;
  - a `Snapshot` field (`store.rs:100-170`);
  - `to_snapshot` and `apply_snapshot` (`service.rs:1109,1165`);
  - the tag dispatch in `store.rs:653-677`.

  New state goes through `deltas` (plan §4.2) or a slot, so this list stops
  growing. A `PERSISTED` table would rewrite working code that is not going
  to change. See Do not do D2.
- **`OBJECT_RETURNING`** (`service.rs:2251`). **[V]** It has 5 rows and 3
  commits this month. That is not worth a seam.

## 2. Large files: where a split pays, and where it only churns

Churn is the count of commits this month that touched each file **[V]**. A
dash means the file was not among the 30 most-touched (fewer than 8 commits).

| File | Lines (code + tests) | Churn this month | Verdict |
|---|---|---|---|
| `planet.rs` | 5,094 + 2,529 | — | **leave**: one owner (PI), pure, low churn |
| `destiny.rs` | 3,928 + 3,020 | 39 | **split** (P4): the next lane's file |
| `service/character.rs` | 1,708 code, then four sections with their own tests | 28 | **split** (P3): it is four files already |
| `industry.rs` | 2,943 + 2,235 | — | **leave**: CP33a/b rework it behind `Table` anyway |
| `service.rs` | 4,328 | 147 (1 since W10.5) | **trim** (P5): move single-caller helpers out; do not split the core |
| `service/inventory.rs` | 2,045 + 1,936 | 25 | leave |
| `service/tests.rs` | 4,480 | 10 | leave; freeze it (new tests go in module `tests`) |

**`service/character.rs`.** **[V]** It is four delimited sub-files, each with
its own banner, type and test module:

- standings: `:1534-2192`, tests `standings_tests` at `:1939`;
- contacts: `:2194-2953`, `contacts_tests` at `:2672`;
- clones: `:2955-4343`, `clones_tests` at `:3860`;
- skill training: `:4345-5136`, `skill_training_tests` at `:4738`.

The state types `Standings`, `Contacts` and `Clones` live there. That is also
why `store.rs:150-167` depends *backwards* on `crate::service::{Contacts,
Clones, ...}`.

**`destiny.rs`.** **[V]** Five concerns share one file:

- the binary ball encoder (`Ball`, `Park`, `Slim`, `:139-1340`);
- `pub mod hull` (`:591`, 525 lines): baked client rows, tracker §66, which
  CP58 replaces;
- `bootstrap_for` and `on_ballpark_bind` (`:2136`, `:2438`), which need the
  Registry;
- the pure warp math (`:2714-3013`), which `eve-bot` imports;
- the push types (`:3144-3928`).

The next direction (memory: "the Destiny stamp allocator") lands here, and
CP58b owns this file in W18.

**`service.rs`.** **[V]** Lines `:3413-4300` hold about 900 lines of free
rowset helpers. Almost every one has exactly one caller module:

- `clone_state`, `clones_rowset`, `clone_implants_rowset` and
  `ship_clones_rowset` are called only from `service/character.rs`;
- `shares_rowset` only from `corp.rs`;
- `entry_type` only from `wallet.rs`;
- `price_history_descriptor` and `empty_price_histories` only from
  `market.rs`;
- `online_status_rowset` only from `social.rs`;
- `escalating_path_rowset` only from `sites.rs`;
- `security_modified_systems` only from `world.rs`;
- `item_descriptor` only from `inventory.rs`;
- `targeting_of` only from `npc.rs`.

A lane that changes "its" rowset must edit the hot file. Move each helper to
its one caller.

## 3. Crates: measured, and the answer is no

The build image was used with `EVE_MAX_JOBS=4` unless noted. **[V]**

| Build | Wall time |
|---|---|
| clean, `--workspace --all-targets` | **22.0 s** |
| edit `service/tales.rs` → `--workspace --all-targets` | 8.2 s |
| edit `planet.rs` → same | 8.1 s |
| edit `eve-marshal/src/lib.rs` → same | 7.5 s |
| edit a leaf → `-p eve-server --lib` | 2.1 s |
| edit a leaf → `-p eve-server --lib --tests` | 4.3 s |
| edit a leaf → `--workspace --all-targets`, **16 jobs** | 8.4 s |

**Reading:**

- Four jobs and sixteen jobs give the same incremental time. The cost is
  the serial chain: the `eve-server` lib, then the `eve-server` test binary
  and `eve-bot`, then linking.
- A crate split would shorten that chain only for edits in a leaf crate.
- Lanes lose no time to compiling. They lose time to shared files, and
  crates do not fix that: `DOMAINS` and the tables would still be in one
  place.

The one crate edge worth moving is the one `eve-bot/Cargo.toml` already
apologises for:

- `crypto_hash`, `blue_marshal` and `crc_hqx` (`eve-server/src/wire.rs:75-
  126`) belong in `eve-proto`.
- That would not remove `eve-bot → eve-server`. The bot also imports
  `destiny::warp_*`, `modules::find`, `asteroid::*` and `jump::*`.
- That shared physics is the point: the bot and the server agree because
  they run the same code. So moving the crypto is housekeeping, not a win.
  See Do not do D4.

## 4. Dependency direction

**[V]** Module cycles are legal in one crate, so these cost nothing at
compile time. They cost the reader, and they create shared edits.

- **Domain → service.** 34 crate-root files import `crate::service::*`.
  Most import only `Registry`, `Request` and `ServiceError`, which is fine.
  The bad edges are:
  - `store.rs` (persistence) → `service::{Bookmarks, Contacts, Clones,
    Corporations, MarketBook}` (`store.rs:150-167`, `:653-677`). Domain state
    types live inside service-arm modules.
  - `warp_subject.rs` → `service::WARP_SUBJECT_BOOKMARK`.
  - `modules.rs` → `service::ship_attributes`.
  - `npc.rs` → `service::targeting_of`.
- **Inconsistent placement.**
  - `drones.rs` sits at the crate root but routes services itself
    (`DOMAINS` row `crate::drones::SERVICES`). The rest of the crate has
    `service/x.rs` as the arms and `x.rs` as the model.
  - Standings, contacts, clones, corporations, bookmarks and the market book
    are the opposite: model and arms both live in `service/x.rs`.
- **The rule to adopt.** `service/*.rs` holds wire arms only: parse the
  request, call the model, build the `PyValue`. State types live in
  crate-root model modules. P3 applies it to `character.rs`. Apply it to the
  others only when a lane is in the file anyway.

**Global state outside the Registry.** **[V]** Plan §1.5 lists three
`OnceLock`s. There are seven process-wide statics:

| Static | Location | In §1.5 |
|---|---|---|
| typedata | `typedata.rs:807` | yes |
| skill | `skill.rs:708` | yes |
| skill fallback items | `skill.rs:709` | no |
| planet | `planet.rs:4415` | yes |
| planet item types | `planet.rs:4416` | no |
| NPC derive cache | `npc.rs:522` | no |
| dogma sheet cache | `dogma.rs:967` | no |

The dogma cache is a process-global `Mutex<HashMap<ship_id, …>>`, taken while
the world lock is held. It is keyed by `Inputs`, so it is correct today.
**[I]** But it is shared across every Registry in a test process and across
`with_content`. The three statics missing from §1.5 should follow their
parents into `Content` (CP58a-CP60), and the dogma cache into a slot, or
CP58c will leave them behind.

## 5. The `pub` surface

**What is there now.** **[V]**

- `lib.rs` declares all 64 modules `pub mod`.
- The crate has 1,137 `pub fn`, 111 `pub(crate) fn` and 10 `pub(super) fn`.
- Outside the lib (`eve-bot`, `eve-probe`, `eve-server/tests`, `src/bin`),
  only these modules are used: `asteroid`, `authored`, `capture`,
  `celestial`, `chat`, `content`, `destiny`, `handshake`, `item`, `jump`,
  `mining`, `model`, `modules`, `notify`, `npc`, `orbit`, `planet`, `pycode`,
  `pyutil`, `server`, `service`, `skill`, `space_state` and `typedata`,
  plus the re-exports.

**Measurement.** In the copy, I made the other 36 modules `pub(crate)` and
ran `cargo check --workspace --all-targets`:

- **116 dead-code warnings** in the lib, at 119 unique sites;
- 0 errors.

The heaviest files:

| File | Warnings |
|---|---|
| `industry.rs` | 20 |
| `mission_pool.rs` | 19 |
| `reprocess.rs` | 18 |
| `seed.rs` | 16 |
| `roles.rs` | 10 |
| `fleet.rs` | 10 |
| `docking.rs` | 9 |

Many are W11 code that landed ahead of its consumer, such as `seed::Stream`
and `mission_pool`. That is planned, but at present nothing checks it.

**Change.**

- Narrow those modules to `pub(crate) mod`.
- Mark staged items with `#[expect(dead_code, reason = "CPnn consumes this")]`.
  Use `expect`, not `allow`: when the consumer lands, the `expect` becomes an
  unfulfilled-lint warning, and clippy's `-D warnings` fails the gate until
  the marker is removed.
- Delete what nothing will consume.

The gate's `clippy --all-targets -D warnings` (`tools/gate.sh:97`) then
enforces the rule on every future wave.

## 6. Small facts found on the way

- `server/crates/eve-server/src/service.rs.orig` (4,307 lines) is tracked.
  **[V]** It came in with `69aef46` (lane B, W10.5), almost certainly as
  patch residue. Nothing builds it. A reader who greps finds two `Registry`
  definitions.
- The `lib.rs` crate doc (`lib.rs:1-31`) still says the server "cannot yet
  answer what comes after" the handshake. **[V]**
- The hot-files register (`docs/lanes/README.md` rule 3) is stale. **[V]**
  - It describes `service.rs` as holding "the original ~2,200-line test
    module". That module is `service/tests.rs` now.
  - It omits the 13 coordinator-only registration files from plan §6.1.

## Proposals

1. **P1. Seam S13, `kill_hooks`** (F1). Behaviour-neutral. It unblocks CP44,
   CP45, CP38, CP50 and CP53b without anyone else owning `combat.rs`.
2. **P2. Seam S14, `session_hooks`** (F2). Behaviour-neutral. It takes logoff
   and login out of `service.rs`.
3. **P3. Split `service/character.rs` along its banners.** It becomes:
   - `service/character.rs`: the `charMgr` and related arms;
   - `standings.rs`, `contacts.rs`, `clones.rs` and `skill_queue.rs` at the
     crate root: the model plus the tests that test the model;
   - the arm glue for each section, which stays in `service/character.rs`.

   `service.rs`'s `pub use character::{Clones, Contacts}` becomes
   `pub use crate::{clones::Clones, contacts::Contacts}`, so `store.rs` is
   untouched. This also fixes `store.rs`'s backward edge.
4. **P4. Split `destiny.rs` into a `destiny/` directory**, before the stamp
   allocator lane:
   - `encode.rs`: `Ball`, `Park`, `Slim` and the binary writer;
   - `hull.rs`: the existing `pub mod hull`, which CP58b retires later;
   - `warp.rs`: pure math, which the bot imports;
   - `push.rs`: `ActionUpdate`, `BallsAdded`, `Kill` and the rest;
   - `destiny.rs`: keeps `bootstrap_for`, `on_ballpark_bind` and `tick`, and
     `pub use`s everything, so no caller changes.
5. **P5. Move the single-caller helpers out of `service.rs`** (§2). Move
   `security_modified_systems`, `ship_attributes` and `targeting_of` too.
   This takes about 900 lines out of the hot file.
6. **P6. `delta_kinds::KINDS`** (F3), with a uniqueness test and a debug
   assert on `put`.
7. **P7. Narrow the `pub` surface** (§5), with `#[expect(dead_code,
   reason)]` on staged items.
8. **P8. Housekeeping.**
   - Delete `service.rs.orig`.
   - Fix the `lib.rs` crate doc.
   - Update the hot-files register: drop the stale test-module line and add
     the 13 registration files plus the new `kill_hooks`, `session_hooks`
     and `delta_kinds`.
   - Add the four unlisted statics to plan §1.5 or the CP58-CP60 rows.
9. **P9. Adopt the layering rule** (§4): wire arms in `service/`, models at
   the crate root. Write it into `docs/lanes/README.md` beside rule 1.
   Enforce it only on new code and in files a packet already owns.

## Packets

Every packet below is behaviour-neutral. Every existing test must pass
unchanged.

| Id | Owns | Tests that pin it | Size | Notes |
|---|---|---|---|---|
| **A1** `kill_hooks` (P1) | `src/kill_hooks.rs` (new), `src/combat.rs`, `src/lib.rs` (coordinator line) | `kill_hooks::tests::hooks_fire_in_table_order_once_per_kill` (local table); `npc_and_player_tables_are_disjoint_paths`; the existing bounty, drone-destroyed and mission-kill tests unchanged | S-M | Before W16. It changes CP44's and CP45's `owns`: neither needs `combat.rs` for its payout or side effect. CP45 still needs `missile_volley`. |
| **A2** `session_hooks` (P2) | `src/session_hooks.rs` (new), `src/service.rs` (`session_closed`, `session_selected_character`), `src/lib.rs` | `session_hooks::tests::logoff_runs_each_hook_once_after_the_last_session_only` (two sessions, one closes, no hook; the second closes, all fire in order); existing logoff, crimewatch and fleet tests unchanged | S | **Coordinator-landed** (`service.rs` is hot). Land it with A5 in one serial pass. |
| **A3** split `character.rs` (P3) | `src/service/character.rs`, `src/standings.rs`, `src/contacts.rs`, `src/clones.rs`, `src/skill_queue.rs` (4 new) | Tests move with the code; `service::tests::no_service_is_claimed_by_two_modules` unchanged; `store` round-trip tests unchanged | L (about 3,400 moved lines, 0 logic lines) | 5 files breaks the 4-file packet rule. Either the coordinator does it as a verbatim move, or it is two packets: A3a standings + contacts, A3b clones + training. |
| **A4** split `destiny.rs` (P4) | `src/destiny.rs`, `src/destiny/{encode,hull,warp,push}.rs` | Pinned slim-byte and warp tests move and pass; `destiny::tests::reexports_cover_every_moved_item` (the S4 pattern); `eve-bot` builds unchanged | L (moves only) | Before the stamp allocator lane. No other lane in `destiny.rs` that round. |
| **A5** trim `service.rs` (P5) | `src/service.rs` plus each one-caller module (`service/{corp,wallet,market,social,sites,world,inventory}.rs`, `src/npc.rs`, `src/modules.rs`) | Existing tests unchanged; no new test (a pure move) | M (about 900 moved lines) | **Coordinator-only**: it touches 10 files, and each edit is one paste. Run it in a round where none of those files is owned. |
| **A6** `delta_kinds` (P6) | `src/delta_kinds.rs` (new), `src/delta.rs` | `delta_kinds::tests::kinds_are_unique_and_lowercase`; `delta::tests::put_of_an_unregistered_kind_panics_in_debug`; CP5's `an_unknown_kind_is_preserved_verbatim` unchanged | S | Before CP14, the first `put`. |
| **A7** narrow `pub` (P7) | `src/lib.rs` plus the files that carry dead items (about 20, edited by attribute only) | The gate's clippy `-D warnings` is the test | M | Coordinator. Re-run the §5 measurement first, because the list moves with every wave. |
| **A8** housekeeping (P8, P9) | `service.rs.orig` (delete), `src/lib.rs` doc, `docs/lanes/README.md`, `docs/content/plan.md` §1.5 | — | S | Coordinator. |

**Order.**

1. A8 and A6: now; they are tiny and do not collide.
2. A2 and A5: together, coordinator-serial.
3. A1: before W16.
4. A4: before the Destiny stamp lane.
5. A3: any round in which nobody owns `character.rs`.
6. A7: last, after A3-A5 have moved items.

## Do not do

- **D1. Do not break up the Registry** into per-subsystem structs or traits
  (`trait HasCombat`, `World { space, economy, ... }`).
  - There is no borrow pain to fix (§1).
  - Since W10.5, Registry edits have stopped being the bottleneck.
  - Doing it would touch every one of the 76 files and gain nothing a
    one-lock, 2-10 player server can use.
- **D2. Do not build a `PERSISTED` table** for the pre-W11 snapshot fields.
  The four-place wiring is ugly but finished. New durable state goes through
  `deltas` or slots, and rewriting `to_snapshot` and `apply_snapshot` puts
  the world file (players' assets) at risk for no new capability.
- **D3. Do not split `eve-server` into several crates.**
  - A clean build is 22 s and incremental builds are 8 s, so there is
    nothing to win (§3).
  - A split would turn `pub(crate)` into `pub` across crate boundaries:
    the opposite of P7.
  - It would add Cargo.toml edits as another shared file.
- **D4. Do not try to cut `eve-bot`'s dependency on `eve-server`.** The bot
  sharing the server's warp, module and asteroid code is what keeps the two
  agreeing. The crypto move to `eve-proto` is fine as housekeeping but buys
  nothing on its own.
- **D5. Do not split `planet.rs` or `industry.rs`.** Each has one owner and
  low churn, and CP33a/b already restructures `industry.rs`. A split only
  makes lane diffs harder to read.
- **D6. Do not make `OBJECT_RETURNING` a seam.** It has 5 rows and rarely
  changes.
- **D7. Do not move `Registry` into its own `registry.rs` "for
  cleanliness".** Both halves would still be coordinator-only, and it is
  4,000 lines of churn in the hottest file for a better path name.

## unknown

- Whether CP57 (tackle) or CP39 (bot courtesy) actually need a logoff hook.
  This is **[I]**, from what tackle and abandonment mean. The plan names no
  such hunk, so either A2 is pre-emptive or the plan has a gap.
- The exact dead-item count after W12. The §5 numbers are for `3f2741d`
  only.
- Test runtime and fixture duplication in `service/tests.rs` belong to V4. I
  did not measure them.
- Whether the dogma global cache (`dogma.rs:967`) shows contention under
  50 bots belongs to V3. It is taken while the world write lock is held, so
  it should be uncontended, but that has not been measured.
