# V3: world state, concurrency and performance

Lane V3 of `docs/review/README.md` §3. Read-only for the shared tree. Every
number here was measured on 2026-10-25 in an exported copy at
`~/code/eve-offline-lanes/V3/` (deleted before this report), from `git
archive HEAD` at `3f2741d` plus `out/content/`. Builds and runs used that
copy's own `tools/dev.sh` with `EVE_MAX_JOBS=4`. Marks: **[V]** verified by
reading or measuring, **[I]** inferred, **unknown**.

## 0. The answer in one screen

- **The world lock is not our bottleneck, at 50 bots or at 100.**
  - 100 bots, release build: about 30 payload turns a second, each holding
    the world lock for 1.2 ms on average. That is about 4% occupancy.
  - The world clock's whole stage table takes at most 0.7 ms per tick in
    release, against a 25 ms budget **[V]**.
  - The mean wait for the lock is below 1 ms.
- **The problem is the tail, and one turn causes it.** Every
  `beyonce.MachoBindObject` (undock, or login in space) holds the world for:
  - **40 ms in release;**
  - **160-200 ms in the debug build we actually ship.**

  The cause is that `Field::in_system` scans **all 2.12M rocks** in
  the universe to find one system's (`asteroid.rs:382-386`, called at
  `destiny.rs:2320` and `scan.rs:303`) **[V]**. Every other player waits behind
  it. Fixing it is a size-S index.
- **We play on the debug build.** `tools/up.sh:92` and `tools/soak.sh` run
  `target/debug`. Compared with release, debug costs:
  - 4x on that tail;
  - 8x on the tick;
  - 3x on boot to `listening` (4.5 s against 1.4 s) **[V]**.
- **Saves.**
  - Rate: 1-4 a second at 50-100 bots.
  - Each is an fsync of 10.5 ms (max 20 ms), done under the world lock.
  - Cost: 1-2.5% of the lock **[V]**, and an 11 ms floor under the max of
    every mutating call.
  - Verdict: tolerable at this scale. Not yet the threat C7 §5.4 predicted
    ("dozens of times a second" **[I]** there; 1-4/s measured here).
- **Memory is flat and small, now that §67 is fixed.**
  - Boot with the full content bundle: 538 MB. About 415 MB of that is the
    eagerly built rocks.
  - Each connection adds about 2.0-2.1 MB.
  - 100 bots plateau at 753 MB. No growth after login **[V]**.
- **The tick's own health line misreports.** `TickHealth`'s "world lock max"
  includes the time spent *waiting* for the lock (`push.rs:665` to `:736`),
  so it shows 40-300 ms where the hold is under 2 ms **[V]**. The per-stage
  timings `tick::run_all` returns are thrown away (`push.rs:705`).

## 1. Method

The shared tree was not touched. All changes were made only in the copy:

1. A timing patch, kept at
   `<scratchpad>/V3/instrument.diff`
   plus `v3.rs`. It records:
   - per payload turn: lock wait, lock hold, and whether the turn saved;
   - per save: snapshot build, render, write+fsync+rename, and bytes;
   - per clock tick: lock wait and every `STAGES` row (the return value
     `push.rs:705` discards);
   - per `service.method`: count, total and max, with time attributed
     between `Registry::dispatch` calls;
   - every turn over 20 ms, with the payload's printable strings.
2. A runner with soak.sh's shape plus a `/proc/<pid>/status` sampler every
   5 s (`scratchpad/V3/run.sh`).
3. The content bundle (`--content out/content`) and `--world` for every run.

The bots used the default `--pace human` and had chat on. Raw lines are in
`scratchpad/V3/logs/<run>/`.

| Run | Build | Bots (miners+escorts) | Secs | Result |
|---|---|---|---|---|
| dbg50 | debug | 40+10 | 600 | bots exit 0, 0 panics, 0 UNANSWERED |
| rel50 | release | 40+10 | 600 | same |
| dbg50m / rel50m | debug / release, + per-method | 40+10 | 480 | same |
| rel100 | release, + per-method | 80+20 | 600 | same |
| relslow | release, + payload dump | 20+5 | 240 | same |
| idle boots | both | 0 | 25 | RSS partition, §3 |

Two runs went in parallel in separate containers, each with 4 CPUs, on a
32-thread host. The coordinator's soak was also running. Absolute timings
carry that noise. The ratios and the tails were consistent across runs.

## 2. The world lock

### 2.1 Measured (steady state, 10 s windows)

| | rel50 | dbg50 | rel100 |
|---|---|---|---|
| payload turns/s | 9-29 | 9-24 | 26-30 |
| hold mean | 0.5-1.8 ms | 1.1-5.3 ms | 0.6-2.1 ms |
| hold max, non-saving turn | **39-46 ms** | **157-196 ms** | **40-47 ms** |
| wait mean / max | 0.0-0.6 / 9-40 ms | 0.0-5.2 / 10-208 ms | 0.0-6.0 / 0.5-41 ms |
| saves/s | 0.7-3.6 | 0.7-3.0 | similar |
| save write+fsync max | 10.5-19.5 ms | 10.4-20.4 ms | 10.5-13 ms |
| snapshot render max | 0.3 ms | 1.0 ms | 0.3 ms |
| world file | 18 KB to 34 KB in 10 min | same | 61 KB |
| clock: all stages mean/max | 0.1/0.3 ms | 1.2/4.6 ms | 0.45/1.6 ms |
| clock: lock wait max | 0-40 ms | 0-140 ms | 0-39 ms |

**[V]** in every cell.

- `npc` is about 85% of the stage time. It tracks how many NPCs exist: it
  rose over the run and fell back after the bots left (rel100: from 0.03 ms,
  to a 0.45 ms peak, down to 0.06 ms).
- Every other stage stays under 0.03 ms in release.

### 2.2 Where the time goes

**The slow turns are one call.** In relslow, 25 of 25 turns over 20 ms were
`beyonce` `MachoBindObject` **[V]**.

- That is the fused bind that builds the ballpark: `destiny.rs`'s
  system-state build.
- Its rock loop is `for r in w.rocks().in_system(..)` (`destiny.rs:2320`).
- `Field::in_system` is a `.values().filter(..)` over the
  `BTreeMap<i64, Rock>` of every rock loaded (`asteroid.rs:382-386`).
- With the bundle loaded, that is 2,123,854 rocks.
- Scanning (`scan.rs:303`) pays the same cost per scan **[V-code]**. That
  was not timed.

**Calls through `Registry::dispatch` are cheap** (rel100, 600 s):
- The heaviest by total time was `slash.SlashCmd`: n=326, mean 4.0 ms.
  These are the bots' GM setup.
- After it come `ship.Board` (7.4 ms), `inventory.MultiAdd` (6.1 ms) and
  `inventory.Add` (5.1 ms).
- Each of those is a dirty turn, so its mean includes the ~10 ms fsync.
- Every method's max sits at 11-14 ms. That is the fsync, not the method.
- `inventory.List` (n=1,449) has a mean of 0.02 ms **[V]**.

### 2.3 The projection: 50 bots plus 10 humans

rel100 is the measurement for this case. It is a superset of the target
load, with 100 sessions **[V]**.

- **Lock occupancy:** about 30 turns/s x 1.2 ms, plus the clock at 4 x
  0.5 ms. That is under 5% of one core's time **[I from V]**.
- **What a human can feel** is the wait behind the single slowest turn
  ahead of them. Today that is:
  - 40 ms (release) or 160-200 ms (debug) whenever anyone undocks or logs
    in;
  - plus 10-20 ms whenever anyone's turn saves.
- A real client's undock storm is 10 humans undocking together after a
  fleet warp-in. In debug, about 10 x 170 ms queues up: **1.7 s of frozen
  world** for everyone **[I]**. That is the felt cost to friends.

### 2.4 What does and does not scale

Each item with its measurement or citation:

| Scales with | Thing | Verdict |
|---|---|---|
| **rocks in the universe** | `Field::in_system`, per ballpark bind and per scan (`asteroid.rs:382`) | **must not.** Fix: P1 |
| rocks in the universe | boot RSS of about 415 MB, eager `Field::generate` (C7 §1.4) | acceptable for one home server. C7 L4 (lazy rocks) is the fix when it matters |
| characters in the roster (debug only) | `Roster::in_system`'s `debug_assert` rescans every ball on every call (`space_state.rs:1092-1104`) | a debug-build tax on a hot path. P3 removes it from play |
| NPCs alive | the `npc` stage. `tick_system` filters all NPCs once per system (`npc.rs:773-781`), so O(NPCs x systems with NPCs) | fine as measured (0.45 ms peak). Revisit after CP21 adds site spawns **[I]** |
| durable world size | the snapshot is rebuilt and rewritten whole on every dirty turn (`dispatch.rs:1660`, `store.rs:290-308`) | fine at 61 KB (render 0.3 ms). Grows with journal and market, which bots do not touch yet. **unknown** at a week |
| connections | about 2 MB RSS and about 3 threads each (conn, chat, and so on) | fine: 60 sessions is about 120 MB |
| ballpark bind payload | the whole system's rocks go in `AddBalls2`, not one grid (`destiny.rs:2312-2318`, a comment) | a wire-size issue as much as a lock one (V2/V7) |

**Not O(universe) any more, contrary to C7 §1.1:**
- `Roster::in_system` reads a `by_system` index in release
  (`space_state.rs:1086-1091`).
- `liveness::refresh` works on the hot set.

## 3. Memory

Idle boots, 25 s after start, by subtracting one configuration from another
**[V]**:

| Configuration | RSS (release) | RSS (debug) |
|---|---|---|
| no data | 5.6 MB | 10.7 MB |
| `--celestials` only (481,069 rows) | 97 MB | |
| `--typedata` only | 31 MB | |
| full `--content` bundle | **538 MB** | 543 MB |

From those rows:
- the rocks are about 538 - 97 - 31 + 6 = **~415 MB**, or about 195 bytes
  per rock **[I by subtraction]**;
- under load: 50 bots reach 640 MB (+100 MB, **2.0 MB/connection**), and
  100 bots reach 753 MB (+215 MB, **2.1 MB/connection**) **[V]**.

There is no upward drift after the logins finish:
- with bots holding: 631 MB, then 633, 634, 635 and 637 MB over 4 minutes,
  which is ore stacks;
- RSS does not fall when the bots leave, which is allocator retention,
  not a leak.

Tracker §67's re-soak reported about 1.46 GB steady with the loose
extracts. This run's 640 MB used the bundle. The difference is
**unknown**. Both load 2.12M rocks.

## 4. Findings, ranked by cost to the end goal

1. **F1: one undock freezes everyone for 40-200 ms.**
   - Cause: `asteroid.rs:382-386`, a linear scan over every rock in the
     universe under the world write lock **[V: 25/25 slow turns]**.
   - Cost: friends feel a hitch on every undock. In debug, a group undock is
     seconds. It gets worse with any content wave that adds rocks.
2. **F2: the play server is a debug build.**
   - Where: `tools/up.sh:92-93`, and the soak.
   - Cost: 4x the tail of F1, 8x the clock, and the per-call `debug_assert`
     rescan at `space_state.rs:1092`.
   - Two sides: debug catches index bugs in soaks, which is worth keeping;
     friends should play on release.
3. **F3: the health line names the wrong number.**
   - Where: `TickHealth.lock_max` is measured from `now` (`push.rs:665`),
     taken *before* `world.write()` (`push.rs:688`), so it is wait plus hold.
   - Measured: "world lock max 300.0 ms" in dbg50m while the stages took
     4.6 ms **[V]**.
   - Also: stage timings are discarded at `push.rs:705`, and there is no
     per-turn hold, wait or save measure anywhere.
   - Cost: the next stall gets chased in the wrong place, which is exactly
     the EvEJS lesson `push.rs:440-450` cites.
4. **F4: saves fsync under the lock, one to four times a second.**
   - Where: `dispatch.rs:1660`, `store.rs:290-308`.
   - Measured: 10.5 ms per save on this host's disk. 1-2.5% lock occupancy.
     Every mutating call's max is at least 11 ms **[V]**.
   - On a home HDD, or an SD-card box, fsync can be 50-200 ms **[I]**, and
     that would make it F1-sized.
   - Also still true: the clock never saves (`take_dirty` has one caller,
     `dispatch.rs:1660`), so a clock-only change on an empty server is lost
     on a kill (C7 §1.2) **[V]**.
5. **F5: per-client outboxes are unbounded.**
   - Where: `Outbox` push at `push.rs:93`, posted from `Writers::post`
     (`push.rs:334`) with no cap.
   - A peer that stops reading keeps its frames queued until its conn thread
     ends. That can take up to the 600 s idle timeout if the reads still
     block.
   - **[I]**, not measured: the bots always read.
   - Cost: memory growth tied to one bad client. Low at 2-10 friends.
6. **F6: lock docs have gone stale, and every lane reads them.**
   - `docs/reference/threads-and-locks.md` F9 says "nothing anywhere takes the read
     lock". `chat.rs:943`, `:1255`, `:1295` and `:1443` now take
     `world.read()` **[V]**.
   - Its line numbers are off (`dispatch.rs:1493` is now `:1623`).
   - Its F2 and F3 predate the outbox split that `push.rs:752-760` cites
     (tracker §57).
   - C7 §1.1 calls `Roster::in_system` a linear scan, which is false in
     release.
   - Cost: a lane relies on a stale locking claim and adds a read inside a
     write.

## Proposals

1. **Index rocks by system.**
   - The change: `asteroid::Field` gains `by_system: BTreeMap<i64,
     Vec<i64>>`, maintained by `generate`, `from_rocks` and `mine`
     (removal). `in_system` reads it. Same iteration order: ids ascending.
   - It kills F1 and the scan cost.
   - It can land independently of C7 L4 (lazy rocks), which would replace
     it later.
2. **Play on release.**
   - `tools/up.sh` starts `target/release` by default. `--debug` is opt-in.
   - `soak.sh` gains `--release`. The default soak stays debug, so the
     index `debug_assert`s keep firing unattended.
   - The release profile already keeps debuginfo (`server/Cargo.toml:19`).
     Measured: an incremental release build took 24 s.
3. **Make the health line truthful, and complete it.**
   - Split `TickHealth` into `lock_wait` and `lock_held`, with `held`
     measured from after acquisition.
   - Keep `run_all`'s stage timings (max and mean per stage). That is C7 L6
     and plan CP15, done now.
   - Add a `[turns]` line: payload turns/s, hold mean and max, wait mean and
     max, saves/s, and save write max.
   - Name the slowest `service.method` per window. The shape is the one in
     `scratchpad/V3/v3.rs`, without the global mutex: use per-thread
     counters, or record under the world lock that is already held.
4. **A soak verdict.**
   - `tools/soak.sh` samples `/proc/<pid>/status` and fails on:
     - RSS growth over 100 MB after the last login **[A]**;
     - a world-turn hold over 100 ms in release **[A]**;
     - the `[turns]` line missing.
   - A memory or tail regression then fails unattended, as §67's lesson asks.
5. **Coalesced saver (C7 L7) stays queued.** The trigger to move it up:
   - the world file passes 1 MB;
   - or save write max passes 25 ms on the owner's machine (P3's line shows
     both);
   - or the first wave that makes bots write the journal or the market.
   - Until then, O1 (save under the lock) holds. The measured cost is 1-2.5%.
   - The one cheap part to take now: the clock calls `take_dirty` and saves
     every 30 s **[A]**. That closes the "clock-only change never saved"
     gap.
6. **Cap the outbox.**
   - `Outbox::push` refuses past 4,096 frames or 8 MB **[A]** and marks the
     wire dead.
   - The conn thread then ends on its next loop, and `Wire::close` frees
     the queue.
7. **Refresh the docs.**
   - In `docs/reference/threads-and-locks.md`: F9 (the chat read locks), the line
     numbers, and F2 and F3 against the §57 outbox split.
   - In C7 §1.1: the `in_system` claim.
   - Add a "measured" table from §2.1 so the next lane does not guess.
   - Doc-only.

## Packets

| # | Packet | Files | Test or check that pins it | Size | Behaviour-neutral |
|---|---|---|---|---|---|
| R1 | Rock index | `asteroid.rs` only | `in_system` equals the old filter, for every system, after generate, mine-out and `from_rocks` (property over a small fixture). The existing `a_rock_is_found_by_id_and_by_system` stays. A `debug_assert` cross-check like `space_state.rs:1092` | S | yes (same rows, same order) |
| R2 | Release for play | `tools/up.sh`, `tools/soak.sh` (coordinator: tools) | `up.sh --help` names the flag. A soak run in each mode | S | yes for the world. Faster |
| R3 | Truthful tick and turn health | `push.rs` (`TickHealth`, ticker), a `dispatch.rs` hunk at `:1621-1665` (timing only), `tick.rs` untouched | extend `the_health_line_names_every_number_and_its_unit` to the new fields. A unit test where `observe` gets wait and hold separately and reports both. No timing thresholds in tests | S-M | yes (log lines only) |
| R4 | Soak verdict | `tools/soak.sh` | a run with `--secs 120 --miners 5` prints the RSS and `[turns]` summary. Exit 1 on a synthetic threshold | S | yes |
| R5 | Clock-side periodic save | `push.rs` ticker hunk, `server.rs` (pass the `Store` into `spawn_world_ticker`) | a clock-only mutation (skill-training completion) reaches the world file with no payload, in a test with a short interval. `the_world_survives_the_server_being_dropped` still passes | S | no (adds saves). Under O1 as written |
| R6 | Outbox cap | `push.rs` (`Outbox`) | posting past the cap returns false and marks the wire closed. A normal flush below the cap is unchanged | S | yes below the cap |
| R7 | Lock-doc refresh | `docs/reference/threads-and-locks.md`, `docs/content/C7-liveness.md` §1.1 | grep: every `file:line` in the doc resolves at the landing commit | S | doc |

Order: R1 and R2 first, since they decide what friends feel. Then R3, which
makes the next problem visible. Then R4, R7, R5 and R6. None of them
collides with the C7 packets except R3 against L6 (the same `TickHealth`).
R3 should *be* L6's first half, and the brief should say so.

## Do not do

- **Shard the lock, use per-entity locks, or move to read locks for
  queries.** Measured occupancy is under 5% at 100 sessions. The pain is one
  O(universe) loop, not contention. Sharding brings back every lock-order
  question `docs/reference/world-state.md` §4 rule 1 removed.
- **A fair or ticket lock in front of the world** (C7 §6.4). The waits seen
  are the length of the turn ahead (the F1 tail), not starvation: wait mean
  is under 1 ms. Fix the turn instead.
- **An async runtime or a thread pool for connections.** About 3 threads
  and 2 MB per session is nothing at 60.
- **SQLite or an incremental store now.** The snapshot is 61 KB and renders
  in 0.3 ms. The fsync dominates, not the format. P5's triggers say when to
  look again.
- **Lazy celestials.** 91 MB, read-only, and never iterated per turn.
- **Rush lazy rocks (C7 L4) for memory.** 415 MB is affordable on a home
  server. R1 removes the CPU cost now. Let L4 land with its rollover work.
- **Timing thresholds in unit tests.** They are flaky across hosts. Keep
  thresholds in the soak verdict (R4), where the box is known.

## unknown

- **fsync latency on the owner's real disk.** Every F4 and P5 judgement
  scales with it. P3's save line answers it in one session.
- **A real client's turn mix.** The bots make about 0.3-0.6 turns/s each.
  A real client at login and undock issues many more calls (`GetAllInfo`,
  inventory and dogma bursts). Measure with P3 on the next client run.
- **The world-file size after a week** of journal and market activity.
  Bots do not write either today.
- **Why §67's re-soak steadied at about 1.46 GB** with the loose extracts,
  against 640 MB here with the bundle.
- **Whether F5 (an unbounded outbox) is reachable from a real client**
  that stops reading but keeps its socket open past `WRITE_TIMEOUT`.
- **The `npc` stage once CP21 site populations exist.** Today it is NPCs
  x NPC systems.
- **Scan cost per call** with the full rocks loaded (`scan.rs:303`). The
  code is the same loop as F1, but the bots do not scan, so it was not
  timed.
