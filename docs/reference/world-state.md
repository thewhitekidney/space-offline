# Durable world state

## Upgrade policy: STORE_FORMAT, delta kinds and the backup (Q-FORMAT)

This section is current; the rest of the file is history. Code:
`server/crates/eve-server/src/store.rs` (header comment "Upgrade safety"),
`delta.rs`, `delta_kinds.rs`. Every claim below is pinned by a test in
`store.rs` (`q-format` tests, `a_newer_format_*`, `skipped_records_*`).

### What the header says

`eve-offline-world 2 format=<STORE_FORMAT>.<kinds hash> build=<sha>`. The
kinds hash is FNV-1a over the sorted set of names in `delta_kinds::KINDS`, so
**any** added, renamed or removed kind changes it. The sha is for names only.

### What happens when builds differ

| world file | loaded by | result |
|---|---|---|
| older build, same `STORE_FORMAT`, fewer kinds | newer build | loads with every row; **backed up** before the first save; first save rewrites it under the new header. Nothing diverted. |
| newer build, same `STORE_FORMAT`, more kinds | older build | loads; a kind the build does not list is kept **verbatim** (`Deltas::put_verbatim`) and written back byte for byte; **backed up** first. Nothing lost, nothing refused. |
| newer `STORE_FORMAT` | older build | loads, but the file is **never saved over**: the session goes to `<file>.downgrade` and the log says `DOWNGRADE REFUSED`, unless the server runs with `--accept-downgrade` (backup first). |
| an unknown *record type* (not a `delta` row) | any build | skipped on load, counted; same refusal as above, because a save would drop it. |
| schema number differs, or not a world file | any build | load error; the server does not start on an empty world. |

A different kinds hash alone is therefore harmless to data. It costs one
backup per change of the hash, nothing else.

### When STORE_FORMAT bumps

Only when an **existing** record or row changes meaning in a way an older build
would read past and then write back wrong: a field appended to the `character`
record, a delta row's key or value shape changed, a field repurposed. The bump
is what makes the older build divert its saves instead of corrupting the file.
Document the bump in the `STORE_FORMAT` doc comment: what changed, what an
old-shaped record reads as.

Adding a kind does not bump it. A new kind has no old rows, and an older build
keeps the new kind's rows verbatim. Adding a new `delta` row *column* to an
existing kind is a shape change: append it as a trailing optional field the
reader defaults when absent (then it needs no bump only if an older build
rewriting the row without the field is harmless; if not, bump).

### How a lane adds a kind safely

1. Ask the coordinator for a row in `delta_kinds::KINDS`; the module exports
   `pub const KIND: &str` and writes only through it. Do not touch
   `STORE_FORMAT`.
2. Read rows leniently: a missing trailing field has a default, an extra one is
   ignored. A row you cannot parse is skipped with the row left in place; never
   delete rows you cannot read.
3. Test with an invented row of your kind round-tripping through
   `Registry::apply_snapshot` and `to_snapshot`. (The Q-FORMAT test
   `every_listed_kind_and_an_unlisted_one_survive_the_registry` already covers
   one row of every listed kind.)
4. State in the lane note whether the change is "kind added" (nothing else to
   do) or "meaning changed" (needs the coordinator to bump).

### The backup, and rolling back

Name: `<file>.f<old format>-<old build sha>.bak`, beside the world file, for
example `world.dat.f4.9dae2000-1a2b3c4.bak`: the *old* file's format id and
build. Made once, before the first save after a load whose format id (or
skipped count) differs, byte for byte, atomically; if the copy fails the save
fails and the good file stays. The newest `BACKUPS_KEPT` (5) are kept, **and always the oldest backup of each
`STORE_FORMAT`** (`backups_to_delete`): the hash changes with every kind
landed, so a night of landings makes many backups, but the pre-upgrade
original of that format is never pruned. Delete it by hand when it is no
longer wanted. `serve.sh` also
copies the world at each start (IP10); keep one of those before a run of
upgrades.

To roll back: stop the server, copy the chosen `.bak` over the world file,
start the build named in the backup's sha (or any build with the same
`STORE_FORMAT`; an older one keeps unknown kinds verbatim). Work done since the
backup is lost; a `.downgrade` file from a refused session holds the newer
session's world if it is wanted.

> **Demoted 2026-10-24** — "the server persists nothing" is no longer true; it does. Current truth: [store.rs](../../server/crates/eve-server/src/store.rs), [docs/guides/gm-commands.md](../guides/gm-commands.md). Kept for history; do not brief a lane from this file.

Planning doc. Nothing here is implemented yet; it exists so the first person
to start does not have to re-derive the ordering. Marks follow the house
convention: **[V]** read from the code, **[I]** inferred.

## Delta kinds and records added by the 2026-10-27 lanes (Q-DOCS)

Each is in `delta_kinds::KINDS` (the list is the truth; this table says what
the newer rows hold and which lane note has the detail). A kind that changes
the list changes the world-format hash, which the policy above already covers.

| kind | owner module | holds | lane |
|---|---|---|---|
| `killindex` | `killmail` | per character, the kill ids it appears in, with a `ready` marker. Built lazily by the first `put` after an upgrade, so `involving` stops parsing the whole book | `sec-fix` |
| `jumpfat` | `jump_fatigue` | per character `activation_until fatigue_until last_updated`, wall FILETIMEs; dropped once both are over | `cap-f` |
| `agent` | `econ_agents` | the parked state of a logged-off bot career, so a restart keeps it (lazy settlement) | `e1-settle` |
| `moonrock` | `moon_belt` | what is left of a moon belt's rocks; a moon belt writes no `rock` row | `mn-b` |
| `corpoff` | `corp_office` | a corporation's office rental: office item id, station, paid-until | `corp-o` |
| `shipcfg` | `ship_config` | one short row per ship whose fleet-hangar or maintenance-bay switches are off the default | `cap-h` |
| `cnode` | `command_nodes` | a live command node of a sov campaign | `sov-d3b` |

One world-file record is not a delta kind: **`cloneimplant`** in
`Clones::RECORDS` (`clones.rs`), one line per implant per jump clone,
`cloneimplant <clone> <type>`, read by the same strict parser, so an old file
reads back identical (lane `implants`).

Not persisted on purpose: the CONCORD responses and checkpoints (`concord-gate`,
like every NPC), cynosural fields and open portals (`cap-d`, `cap-g`), the
`/dock-capitals` switch (`cap-k`) and the fighter squadrons in space
(`ft-b`).

The save itself changed shape without changing bytes: the snapshot is cloned
under the lock and rendered and written by the saver thread, so the lock hold
is a few milliseconds at 50 characters (lane `perf-save`; `STORE_FORMAT` stays 4).

## 0. Where we actually are

**Today the server persists nothing, and every connection gets its own private
universe.** `server.rs` builds each connection's dispatcher with

```rust
Registry::new(config.sandbox.clone())
```

**[V]** — so two clients connected at the same time are two separate worlds
that happen to agree because they were cloned from the same `Default` impl.
Nothing one does can ever be visible to the other. That is not a persistence
problem and it is not fixed by adding a database; it is one `Arc` away, and it
is the first step below.

The world itself is `SandboxConfig::default()` — one character, one station,
rebuilt from constants at every boot.

Two facts about the server that shape everything else, both **[V]**:

- It is **thread-per-connection with blocking I/O**, `std::net` only, argued
  for in `lib.rs`. A shared world therefore needs an `RwLock`, not an async
  runtime. The no-dependency property survives.
- `conn::serve` only writes **in response to a read**. There is no way to push
  a frame to an idle client, which is exactly what the roadmap puts first.
  That needs `TcpStream::try_clone()` and a writer handle in a session table —
  no runtime, no new dependency.

And one comment that this work makes false, in `server.rs`: *"there is no
shared state for a panicking thread to corrupt."* It must change in the same
commit that introduces sharing, or the next reader will trust it.

## 1. Two stores, not one

The recon settled that static data is entirely client-side (`docs/archive/recon-v24.01.md`),
and the station-id bug recorded in `service.rs` is what happens when we
disagree with it: a station lives in a different system than the one the code assumed, and
*the client holds its own copy of that table*.

But we cannot avoid needing a read-only slice of it, because our own wire
shapes carry static fields — `item_columns()` has `groupID` and `categoryID`,
which is why `capsule_item_row` hardcodes them today. The moment
`/createitem` can grant an arbitrary type, we need `typeID → groupID →
categoryID` server-side.

So the boundary is not "no static data on the server". It is **two files with
two different lifetimes**:

| | contents | policy |
|---|---|---|
| `static.db` | a projection of the client's FSD: types→group, groups→category, later the system→constellation→region chain that `service.rs` still flags **[I]** | **regenerable. Safe to delete.** |
| `world.db` | everything in §2 | **irreplaceable.** Nothing in it may be derivable from `static.db`. |

Reading `*.fsdbinary` server-side is a known open question
(`docs/archive/recon-v24.01.md`). **Do not let it block the store.** The stopgap is what
the code already does — a curated table of the dozen types a sandbox grants —
and the only rule that matters is that the curated table lives on the
`static.db` side.

## 2. What persists

In the order each one is forced. Every entry traces to an arm that currently
returns a constant or an empty container.

1. **Account** — `user_id`, `user_name`, `character_slots`, plus the
   `creationDate` and `subscriptionEndTime` that `character_selection_data`
   fakes today. Becomes a table the day a second person logs in.
2. **Characters** — all of `Character` minus the four station-identity fields
   (static) and minus `constellation_id`/`region_id` (derivable once the map
   chain exists). Two fields already argue for persistence in their own
   comments: `create_date_time` (*"a birth date that changes every time the
   server restarts is a thing you would eventually chase"*) and `logoff_date`,
   which is **the sort key for the character-select screen** — meaning
   **disconnect is a write**, and `serve_one` has no hook for that today.
3. **Items.** The biggest gap and the one that unblocks the most. The schema is
   already fixed and confirmed from CCP's own code: `itemID, typeID, ownerID,
   locationID, flagID, quantity, groupID, categoryID, stacksize, singleton`.
   Persist all but the two derivable ones. One table retires the **[I]** in
   `invbroker::inventory` about not being able to tell the hangar from any
   other container.
4. **Id sequences**, which are world state in their own right. `ship_id` must
   be `>= 100000000` or the client routes the lookup to its own sqlite before
   it ever asks us; characters must sit in `[90000000, 100000000)`; fittings
   need `fittingID > 0`; notifications need a monotonic id because the client
   reads `notificationID > fromID`. **Reusing an id across a restart hands a
   live client a stale cached object**, and the client caches aggressively.
5. **Fittings** — blocked on items, not merely on storage; see §5.
6. **Wallet** — balance, plus an append-forever journal. `GetEntryTypes`
   already ships `refGMCashTransfer` for exactly this, and that call *latches*.
7. **Skills** — and the first **time-dependent** state. Store
   `trainingStart`/`End` and compute SP on read; a restart that loses the start
   time loses the training.
8. **Standings**, 9. **Market** (orders, transactions, per-type history),
   10. **Mail / notifications / calendar / contacts / bookmarks** — note
   `accessGroupBookmarkMgr.AddFolder` is *already a mutator we answer with
   `None`*, i.e. already a silent lie.
11. **Character settings** — a pure blob the client subscripts and `del`s
   from. The cheapest possible first real round-trip.
12. **Corporations**, 13. **World metadata** — schema version, world birth
   time, last clean shutdown, so "the world outlived the process" is
   assertable rather than hoped.

### Not persisted

`Session` and its derived attribute map (`locationid` and `corprole` are
*derived by the client* and must not be stored either), bound objects and
`next_object_id` — `handle_bound_call` already names the bug that persisting
those would manufacture — captures, diagnostics, and the client-side latches
we merely feed.

### A third category the obvious two miss: shared but ephemeral

`station.GetGuests`, `multiLoginBlocker.Login` and `onlineStatus.*` are
questions about **who is online right now**. They need a live session table
next to the world, not a store — and they are the cheapest "two players can
see each other" feature in the tree.

## 3. Storage: SQLite, behind a seam, but not first

**Recommendation: a hand-written snapshot file first, behind a `Store` trait;
SQLite through a distro `librust-rusqlite-dev` once the schema stops being one
character.**

What decides it is not today's world — which would fit in a struct — but
*"runs for years"*. The wallet journal, skill history, market orders and market
history are all **append-forever, read-by-range**. A snapshot holds the whole
world in RAM and rewrites all of it on every save; after a year of market
history that is the wrong shape, and it forces a migration exactly when the
world is most valuable.

The supporting arguments, in order of weight:

1. **Durability is the one place a bug means "the world is gone"** rather than
   "one call returned the wrong shape". We write our own codecs because the
   codecs *are the thing being reverse-engineered*; crash-safe storage is the
   opposite — a solved problem with no EVE content in it, where fsync
   ordering and torn writes are easy to get subtly wrong and hard to test
   honestly in a container.
2. **Inspectability.** `sqlite3 world.db 'select * from items'` answers "where
   did my item go" in one command. We already know what a bespoke format
   costs: `.evecap` needed its own reader binary.
3. **Migrations are the steady state here**, not an event. `PRAGMA
   user_version` is a day-one answer.
4. **The image was designed for it.** `build.Dockerfile`'s own comment names
   *"a database driver for persistence"* as an expected future dep.

### What it costs, stated plainly

- The **zero-dependency property**, which is currently load-bearing for this
  project's identity. `deps.allow` stops being empty and `Cargo.lock` grows by
  roughly 10–15 transitive packages **[I]**.
- **Pure Rust.** `libsqlite3-sys` is C and uses `unsafe`. The workspace's
  `unsafe_code = "forbid"` applies only to crates that opt in, so this is
  legal **[V]** — but a C library inside a project that forbids unsafe
  deserves saying out loud rather than discovering.
- A **workflow wrinkle**: `tools/dev.sh` builds with `--network none`, so the
  image must be rebuilt before any code importing `rusqlite` will compile.
  Someone will hit that exactly once and lose an hour.
- **One unverified fact.** That Ubuntu 26.04 ships a usable
  `librust-rusqlite-dev` is **[I]**. Verify it with a scratch image build and
  `/etc/eve-offline-crates.txt` — which the Dockerfile already generates for
  this purpose — *before any code is written*.

Rejected: an embedded KV store (almost certainly not in the archive, and you
hand-roll both the value encoding and every secondary index); a hand-rolled
binary world file as the destination rather than a stepping stone.

## 4. Shape and locking

```rust
pub struct World {
    state:    RwLock<WorldState>,   // the read path
    store:    Box<dyn Store>,       // the durability path
    sessions: RwLock<SessionTable>, // shared but ephemeral: who is online
    statics:  Statics,              // immutable after load
}
```

`SandboxConfig` splits: `node_id`/`bind`/`capture_dir` stay configuration;
everything else becomes `WorldState`, seeded by today's `Default` body — keep
that constructor and its whole comment block, because it is the only place the
station's four verified ids are written down. (Note `station_name: "Home
Station"` is a per-boot invention the client's own `npcStations` disagrees
with: a static field hiding in a mutable struct.)

The ~15 `self.config.characters.first()` sites all become `self.character()` —
*the character this session selected* — which is the right reading at every
one of them.

**Five locking rules, with the reason attached so nobody re-derives them:**

1. **One world-wide `RwLock`**, not per-entity locks. At 1–10 players a write
   lock held for microseconds is free, and one lock removes every
   lock-ordering question — the class of bug hardest to test for.
2. **Exactly one acquisition per dispatch arm, taken at the top.**
   `std::sync::RwLock` is not reentrant, so a nested write lock is an instant
   self-deadlock on the connection's own thread — and the client would show it
   as §1 of `docs/guides/debugging-the-client.md`: a black screen with nothing in either
   log. Enforce it structurally with `read(|w| ...)` / `write(|w| ...)`
   helpers so an arm cannot nest by accident.
3. **RAM leads disk by at most `SAVE_EVERY`, and the disk is always one
   whole world.** *Revised by owner decision O1 (CP52, wave E); this rule used
   to read "RAM mutation and store write happen together, under the write
   lock".* A mutation only marks the world dirty. Every `SAVE_EVERY` (5 s
   **[A]**, `push.rs`) the world ticker renders the snapshot to text *under*
   the write lock (CPU only) and hands it to the `world-saver` thread
   (`store::Saver`), which writes, fsyncs and renames with no lock held.
   - Why it changed: at 50 bots almost every turn was dirty, so the server
     rewrote the whole world with an fsync inside the lock 1-4 times a
     second, and the file grows with the bot economy (`R-ECON` §1.3).
   - Why it is safe: a crash loses at most `SAVE_EVERY` of play, never half
     a world (tmp + fsync + rename), and the allocator leap on load (C7 §3.4)
     means no id handed out in the lost window is reissued. RAM and disk
     still never *disagree* about a past state: the disk is always some
     whole earlier RAM state.
   - A clean shutdown (`Server` drop) renders the last dirty world and
     closes the saver, which writes it before releasing the `--world` lock;
     the saver owns that lock, so no write ever lands unlocked.
4. **Never hold a lock across a socket write, and never do store I/O on the
   synchronous draw path** (`GetSelfInvItem`, `ship.GetDirtTimestamp`,
   `dogmaIM.GetAllInfo`). An fsync there is a hang, not a slowdown. Reads take
   the read lock and never touch the store.
5. **Decide the poisoning policy explicitly.** With a shared world, a panic
   mid-mutation poisons the lock and kills the world for everyone — one
   cosmetic bug becomes a dead server. Validate before mutating so the
   critical section has no fallible work in it, and recover from poisoning
   deliberately, with a comment naming which risk was chosen over which.

## 5. Migration path

**Step 1 — share the world.** No new dependency, no behaviour change.
`Arc<World>`, `Registry` holds a handle, `World::seed()` carries today's
`Default` verbatim. **Shippable on its own, and that is the point: the risky
refactor lands while the client still draws.**

The exact change, verified against the tree by building it in a scratch copy:

- `Server` grows `world: Arc<Registry>`, built once in `bind` beside the
  other once-per-process setup.
- `serve_one` takes `&Arc<Registry>` instead of cloning `config.sandbox`.
- `GpsSession.registry` becomes `Arc<Registry>`. **No call site inside
  `dispatch.rs` changes** — all five uses go through `Deref`.
- `Registry::dispatch` is the keystone: an `Arc<Registry>` cannot call a
  `&mut self` method at all.

Three findings from that exercise are worth keeping:

- **There are two full copies of the world per connection today**, not one:
  `Server::run` clones the whole `Config` into each thread, and `serve_one`
  then clones `config.sandbox` again. After Step 1 the second disappears and
  the first becomes dead weight that the next reader will trust. Document or
  narrow it in the same commit.
- **`Registry::config(&self) -> &SandboxConfig` has zero callers**, and it is
  exactly the signature that becomes *impossible* under a lock — you cannot
  return a reference out of a guard. Deleting it in Step 1 removes the only
  way to get the locking wrong later, for free.
- **Dropping `&mut self` makes ~16 test-local `let mut r` bindings fire
  `unused_mut`**, which `clippy -D warnings` turns into a gate failure. Take
  that list from clippy, not from a plan; it moves as tests are added.

**Correction, and the reason the obvious shortcut is gone.** This step was
planned when `Registry` had *no mutable state at all* — every one of its
`self.config` reads was a read, `dispatch`'s `&mut self` was vestigial, and
`Arc<Registry>` with no lock whatsoever would have been a complete Step 1.
That window closed when `/giveisk` landed: it does
`self.config.characters.first_mut()`, which is the first real mutation in the
file. So Step 1 now needs the `RwLock` from the start, and §4's five rules
apply to it immediately rather than from Step 3.

That is the right trade and not a mistake to undo — `/giveisk` is something a
player can see and Step 1 is something nobody can — but it is worth naming,
because "the lock-free version was available and we spent it" is the kind of
fact that is invisible six weeks later.

**One hazard to design around now, not later.** If the lock is taken *inside*
`Registry::dispatch`, rule 2 ("exactly one acquisition per dispatch arm") is
already violated on the undock path: `handle_bind` calls `dispatch` and then
`with_session_change` → `undock_change`, two sequential acquisitions. Not a
deadlock — `std::sync::RwLock` is not reentrant but these do not nest — but
once there is a writer, the answer and the session change that describes it
are computed against two different world states and the client applies both.
The fix is to hoist the acquisition into `GpsSession`, take it once per
payload, and pass `&WorldState` down. **Do not put the lock inside `dispatch`
and plan to fix it later.**

*Testable:* the honest test for Step 1 is structural, not behavioural — assert
that two `GpsSession`s hold the *same* world (`std::ptr::eq`), which fails by
construction today and passes after. A behavioural test needs Step 3.

**Step 2 — the `Store` seam and a snapshot file. — LANDED, 2026-10-20.**
`tmp + fsync + rename`, modelled on `capture.rs`, with its `create_new` policy
deliberately inverted -- a capture is evidence and must not be overwritten, a
world file is the opposite.

`crate::store` is the seam: a `Store` trait, a `Snapshot` of the durable half,
a `NoStore` that keeps nothing (the default, so a test gets a fresh universe
without arranging one) and a `FileStore`. `--world FILE` turns it on.

Three decisions worth keeping:

- **The format is line-oriented text, not a binary blob.** §3's best argument
  for SQLite was inspectability -- `sqlite3 world.db 'select * from items'`
  answering "where did my item go" in one command -- and `grep item world.txt`
  answers it with no tool of ours at all. `.evecap` already showed what a
  bespoke binary format costs: it needed a reader binary before anyone could
  look at it. SQLite is still right eventually, and still needs a dependency
  decision that is the owner's.
- **A load applies *over* the seed rather than instead of it.** `Registry::new`
  rebuilds the station, the capsule and the starter skills from
  `SandboxConfig`, and those should be identical every boot; a snapshot carries
  what the *player* changed. So a snapshot with no skills leaves the starter
  set alone instead of emptying it -- loading a world is never destructive.
- **An unreadable world file is fatal at `bind`, and that is the one place in
  this server where something unreadable is not merely logged.** Continuing
  would hand the player a fresh character and silently orphan everything they
  had; they would find out by looking at their empty hangar. A refusal to
  start is the kind failure. A newer schema version is refused by name for the
  same reason.

The save is gated on a dirty flag, so the only payloads that touch the disk
are the ones that changed something -- which keeps it off the draw path (§4
rule 4), where an fsync is a hang rather than a slowdown. It happens inside the
write lock, per §4 rule 3, and the cost is named rather than discovered: one
connection's save blocks the others for an fsync. A failed save is logged and
does **not** fail the call, because the player's action already happened in
RAM and "your command failed" would be a second, worse lie.

*The first test in this project that can fail if persistence breaks* is
`the_world_survives_the_server_being_dropped`, and it is deliberately harsher
than a file round-trip: it grants ISK, a ship and a skill, **drops the entire
`Server`** -- listener, world, ticker, every `Arc` -- and builds a second one
from nothing but the path on disk.

Still **not** persisted, each for a reason in §2: the session and its derived
attributes, bound objects and `next_object_id`, captures. Item and transaction
id allocators *are*, because reusing an id across a restart hands a cached
client a stale object.

**Step 3 — the first mutator end to end: a GM ISK grant.** Balance plus a
journal row with `refGMCashTransfer`, which `GetEntryTypes` already ships.
**This is the step that satisfies the hard requirement.** Two documented
gotchas: `GetEntryTypes` latches, and `GetCashBalance` caches on `is None`, so
the grant shows up after a relog until step 6 pushes a notification.

**The RAM half of this already shipped, ahead of Steps 1 and 2.** `/giveisk`
moves `characters[0].balance`, which is what `account.GetCashBalance` serves,
and the console line tells the player both of the caveats above. What is left
for Step 3 proper is the journal row and the store write — i.e. making the
grant survive a restart, which is the part that was ever actually blocked.

**Step 4 — SQLite replaces the snapshot behind the unchanged trait.** Verify
the apt package first. The step-2 tests run unchanged, which is why the seam
went in first.

**Step 5 — items and the id allocator.** Then `/createitem`.

**Step 6 — the session table and the first push.** `GetGuests`,
`multiLoginBlocker` and `onlineStatus` start telling the truth; `logoff_date`
is written on disconnect; the first server-initiated `Notification` goes out.
**Two clients see each other.**

**And the first half of this is much cheaper than it looks, because the
station lobby pulls rather than listens.** `station.GetGuests` answers a
hardcoded empty list today, and the Guests tab calls it *when the player
opens it* — see `docs/archive/entering-the-game.md`. So a live session table alone,
with no push path, no `TcpStream::try_clone`, no notification id allocator
and no store, is enough for client A to open the lobby and **see client B**.
`multiLoginBlocker.Login` and `onlineStatus.GetOnlineStatus`, both currently
hardcoded, become honest off the same table.

That is the cheapest thing two players can actually see each other through,
and it is worth pulling forward. It has one prerequisite, which is §7's
identity split: both connections currently read `characters.first()`, so
without it the lobby lists the same character twice.

Steps 1–3 are a day or two each and independently shippable.

## 6. What not to do yet

- **No async runtime.** Blocking thread-per-connection is argued for in
  `lib.rs` and N=10 does not change the arithmetic. Server-initiated push
  needs a cloned socket handle, not a scheduler.
- **No event-sourced log with replay.** It means writing every mutation twice
  and maintaining replay as a second implementation of every rule.
- **No background tick loop.** Skill training and market history want *time*,
  but both compute lazily from stored timestamps. A ticker is only needed when
  something must happen while nobody is watching *and* be seen by someone
  else.
- **No lock sharding, actor model, or migration framework.** One `RwLock`;
  `PRAGMA user_version` and a `match`.
- **Do not make the service arms read the store.** They read RAM. Several arms
  are documented as repeating and *have to stay cheap* — see the live-lock
  case in `docs/guides/debugging-the-client.md` §4 for what a call the client re-issues at
  UI refresh rate costs when it is not.

### Risks worth naming

1. **Lock poisoning turns a cosmetic bug into a dead world** — the highest-
   severity new failure mode this introduces.
2. **Store I/O on the draw path is a hang, not a slowdown**, with no traceback
   and nothing in either log.
3. **Id reuse across a restart**, against a client that caches aggressively.
4. **The dependency may not exist** — keep the rusqlite surface to one module
   behind `Store` so swapping it costs a day rather than a milestone.
5. **Over-fitting the schema to shapes we only inferred.** `item_descriptor`'s
   provenance note is the model: the column order *was* wrong once and worked
   anyway, because the client takes the descriptor from us. A wrong-but-self-
   consistent world is invisible until something external reads it. Mark
   inferred columns **[I]** in the schema, exactly as the code does.
