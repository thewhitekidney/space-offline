# Emulator review: what to improve in the structure, the code and the process

Owner request, 2026-10-25: *"follow this up with a comprehensive review plan
of our current emulator and its structure to see how we can improve this,
also: content data, comprehensive plan to address how this fits into our
emu"*. This was asked for after the content-plan work. W11 has landed
(`docs/content/plan.md` v2), so this is that follow-up.

The process is the same as `docs/content/`:
1. **Lanes V1-V7** run in parallel. Each is a read-only review with its own
   brief in §3.
2. **Synthesis:** one lane writes `docs/review/plan.md`.
3. **Red team:** another lane attacks that plan.
4. **The coordinator** settles what is disputed and files the work.

## 1. The emulator today (the coordinator's snapshot at `5e7880c`+W11)

**Code:**
- About 160k lines of Rust in 9 crates:
  - `eve-server`, the server;
  - `eve-marshal`, `eve-proto`, `eve-rowset`, `eve-inflate`, the wire;
  - `eve-dns`, `eve-xmpp`, the jail's resolver and chat;
  - `eve-bot`, `eve-probe`, the bots and the probe.
- Std-only, with zero external crates.
- The largest files:

  | File | Lines |
  |---|---|
  | `planet.rs` | 7.6k |
  | `destiny.rs` | 6.9k |
  | `service/character.rs` | 5.6k |
  | `industry.rs` | 5.2k |
  | `service/tests.rs` | 4.5k |
  | `service.rs` | 4.3k |
  | `service/inventory.rs` | 4.0k |

**One world:**
- An `Arc<RwLock<Registry>>` holds everything, shared by every connection
  thread and two ticker threads: the wire clock and the world clock.
- The world clock runs `tick::STAGES`.
- The Registry is reached through extension points: `DOMAINS`, `notify`,
  `SLOTS`, `TABLES`, `STAGES`, `RESOLVERS`, `SOURCES`, `HOOKS`, `GUARDS`,
  `COMMANDS`, `ROLES`. They were added at W10.5.

**Persistence:**
- A snapshot text file, rewritten in full under the world lock after a
  payload that changed something.
- Plus the C7 world record and deltas (CP5).

**Content:**
- `tools/content.py` writes `out/content/` with a MANIFEST.
- `--content` loads it.
- `content/` holds authored tables.
- Some client-derived rows are still baked into Rust: hulls, wrecks,
  skills, and `reprocess` and `industry` `tables.rs` (tracker §66).

**Process:**
- Lanes own files (`docs/lanes/README.md`, rules 1-11).
- A gate: fmt, clippy, tests and the Python unit tests.
- `land.sh --head` lands commits.
- About 1,800 tests.
- `tools/soak.sh` runs the server plus bots unattended.
- Human checks are queued in `docs/guides/human-checks.md` (43 items).
- There are 51 files in `docs/`, several demoted, and 98 lane notes.

**Known pain this session:**
- A per-connection copy of the universe ran 50 bots out of memory. It was
  found only by the first soak (tracker §67).
- A shared scratchpad was wiped by one lane's cleanup.
- Lanes reused packet names.
- Plans cited tests that did not exist.
- Hot files made lanes queue behind each other.
- Generated client data was committed (tracker §66).
- No real client run has been possible unattended: `sudo` is needed for the
  jail.

## 2. Shared brief: every V-lane obeys this

- **Read-only.** Write only `docs/review/<lane>.md`, plus optionally
  `findings/review-<lane>*.tsv`. No cargo builds in the shared tree, no
  edits to `server/` or `tools/`, no commits.
- If you need to measure something, such as compile time, test time,
  memory or a soak, do it in an exported copy at
  `~/code/eve-offline-lanes/<lane>/` using that copy's own `tools/dev.sh`
  with `EVE_MAX_JOBS=4`, and delete the copy before you report (lane rules
  9-11).
- Scratch notes go in `scratchpad/<lane>/` only:
  `$SCRATCH/<lane>/`.
- Never install anything on the host.
- **Evidence:** every finding cites `file:line`, a measurement, or a
  tracker entry. Mark claims **[V]** if verified, **[I]** if inferred, or
  **unknown**.
- **Rank by cost to the end goal:** co-op and PvP with 2-10 friends plus
  about 50 player-like bots, on a server one person runs at home. Say what
  each finding costs us if left alone: bugs, lost lanes, slowed waves,
  friends unable to play.
- **Deliver changes, not opinions.** Each finding ends in a concrete
  change: the files, the shape of the refactor or rule, the test or check
  that pins it, a size (S/M/L), and whether it is behaviour-neutral.
  Respect the owner's rules:
  - std-only;
  - no client data in the repo;
  - Docker only;
  - [A]s are fine but never in a test;
  - lanes own files.
- **Your doc ends with:**
  - **Proposals** (numbered);
  - **Packets**, each with its files, tests and size;
  - **Do not do**: things that look like improvements but are not worth it
    here, and why;
  - **unknown**.
- Report in 15 lines or fewer.

## 3. Lanes

**V1: architecture and module structure.**
- `Registry` as a god object, and whether W10.5's extension points are
  enough.
- `service.rs` and the large files: where to split, and where a split
  would only add churn.
- Crate boundaries: should `eve-server` become several crates? Measure
  compile times first.
- Dependency direction between modules, and the `pub` surface.
- Output: `docs/review/V1-architecture.md`.

**V2: wire, protocol and session robustness.**
- Marshal, dispatch, the session and bound objects.
- What a malformed or hostile packet does: panics, unbounded allocation,
  a held lock. The server will face friends' clients over a LAN or the
  internet, so this matters.
- Error paths that hang the client, which has a 30 s frame cliff.
- Output: `docs/review/V2-wire.md`.

**V3: world state, concurrency and performance.**
- The single `RwLock`: hold times, writers per tick, saves under the lock
  (owner decision O1).
- Memory per connection and per system. Use `tools/soak.sh` and the
  tracker §67 method.
- Tick budget at 50 bots plus 10 humans.
- What scales with systems or players, and what should not.
- Measure; don't guess.
- Output: `docs/review/V3-runtime.md`.

**V4: testing, tooling and the gate.**
- The test pyramid: unit vs wire vs bot harness vs soak vs client CI.
- Flaky or slow tests, the gate's runtime and duplicated fixtures.
- What a real-client CI run needs to work unattended. Today it is blocked
  on `sudo`; propose the narrowest sudoers rule or a rootless jail
  alternative, and keep the no-phoning-home guarantee.
- Output: `docs/review/V4-testing.md`.

**V5: process, docs and knowledge.**
- The lane system: rules 1-11 and what they still miss.
- Briefs, the coordinator's bottlenecks, `docs/` sprawl (51 files, demoted
  docs, 98 lane notes), duplicated truth, and how a fresh agent catches
  up.
- The human-check queue: are the 43 items still the right size and shape
  for one morning?
- Propose a doc structure and an index, and say what to archive.
- Output: `docs/review/V5-process.md`.

**V6: security, the jail and operations.**
- The jail and the no-phone-home guarantee: re-verify them.
- Supply chain: std-only, the Docker images and their pinning.
- Exposing the server to friends: authentication (the account layer is an
  open owner decision), rate limits, GM command authorisation, and
  secrets.
- Running it: start, stop, back up the world file, upgrade with world
  migration, logs.
- Output: `docs/review/V6-security-ops.md`.

**V7: how the content data fits the emulator.**
- Take `docs/content/plan.md` v2 and the W11 code as landed: `content.rs`,
  `content_tables.rs`, `authored.rs`, `seed.rs`, `delta.rs`, `store.rs`,
  `liveness.rs` and `mission_pool.rs`.
- Check that the plan's W12-W18 packets fit the actual structure:
  - which subsystems must change to consume content;
  - which still read baked rows;
  - the order that gets real content in front of a player soonest;
  - where the plan's assumptions no longer match the code.
- This is the owner's "content data, comprehensive plan to address how
  this fits into our emu".
- Output: `docs/review/V7-content-fit.md`.

## 4. Synthesis

**SR1, after V1-V7 report.** Write `docs/review/plan.md`:
- a ranked improvement plan;
- which items run before, beside or after content waves W12+;
- packets with non-colliding file ownership;
- owner decisions;
- a short "state of the emulator" summary for the owner.

**RR1, red team.** Attack it in `docs/review/plan-review.md`.
