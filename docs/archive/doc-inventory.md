# Documentation inventory and cleanup (DOCS-CLEAN)

> **Demoted 2026-11-01** — the finished DOCS-CLEAN inventory of 2026-10-28, a snapshot. Current truth: `docs/README.md`.

Base `4c6fd99`, 2026-10-28. Phase 1 is this inventory; phase 2 is the cleanup it led to
(the move map is `moves.sh`, the reference rewrite is `rewrite_refs.py`, both in the handback).
The live index of the result is `docs/README.md`; this file is the evidence behind it.

## Findings

**What was asked:** analyse the docs, find the stale and duplicated ones, sort them, clean up.

1. **The tree had 534 text docs and no index.** 419 of them were lane notes (`docs/lanes/`), 54 loose
   files in `docs/` root, 19 in `docs/re/`, 15 in `docs/content/`, 11 in `docs/review/`, 7 in
   `docs/inventory/`, and a handful of `findings/` and READMEs. `docs/README.md` had been planned
   since IP23 and never written; several docs (the lanes README, review V5, the lane notes ip31, nb-12, grid-research) said so. The doc lint's
   checks (b) and (c), which need the index, were skipped for that reason.
2. **Duplicates: no verbatim copies** (a line-level scan for 100+ character lines shared by two files
   found only three lane-note pairs sharing two lines each). The duplication is by **topic**: a
   later doc supersedes an earlier one and the earlier stays. Those pairs are already marked with
   `> **Demoted` banners and are now grouped in `docs/archive/`:
   `session-flow` -> `login-path`; `wire-format` and `marshal-tags` -> `framing`, `dbrow`,
   `object-wrapper`; `evejs-261018-reference` -> `re/evejs-crosscheck`; `destiny-and-the-ballpark`
   §4.4 -> `re/destiny-ball-crosscheck`; `recon-v24.01` -> `plan-subsystems` §2a; the four overnight
   plans (`OVERNIGHT_PLAN`, `plan-overnight-*` x3) -> `plan-subsystems`. None of them was deleted: each
   holds facts the successor does not restate (they are still cited from code and from other docs), and the
   brief says never to delete a fact that exists in one doc only. **Nothing was deleted** (`deletions.txt` is empty).
3. **Overlap, not merged:** `inventory/*` (7 EvEJS inventories, ~360 KB), `docs/re/evejs-data.md`,
   `lanes/evejs/static-data.md` and `evejs-surprises.md` (old locations under `docs/`) all describe what EvEJS ships, each cut
   differently. They are now together under `docs/evejs/` (inventory, review) and `docs/re/`. A merge would
   mean re-reading ~700 KB of EvEJS facts and `[unverified]` is the honest status of how much they repeat.
4. **Stale statements found and fixed** (each checked against the tree):
   - `CLAUDE.md` said the last tracker sections are §80 and §81; the tracker runs to §85. Fixed, and it
     now names the index and the two plans (`plan-deep-queue`, `plan-living-world`).
   - `server.md` §2 says the station view is missing; `map.GetStationInfo` and
     `stationSvc.GetStationItemBits` are answered in `service/world.rs`. Status note added at the top.
   - `unattended-root.md` says "Status: proposal, nothing installed"; the helper it argues for exists
     (`tools/install-jail-helper.sh`, `tools/eve-offline-jail.in`, which cite its §3 to §5). Status note added.
   - `roadmap.md`: milestones end at "undock", `M4+` is one line; the server is playable.
     Demoted to the archive with a banner (its own "layer below" section already said the spine is done).
   - `plan-docked-subsystems.md`, `plan-port-from-evejs.md` (its §1 says `Cargo.toml` declares
     `LicenseRef-Proprietary`, fixed on 2026-10-20), `plan-the-churn.md`: finished plans with no banner;
     banners added and moved to the archive.
   - `patcher.md` named `tools/client-pin.sh`; the script is `tools/pin-client.sh`. Fixed.
   - `docs/lanes/README.md`: "this list moves to the docs index once that exists", and a row listing
     `docs/README.md` and `docs/STATUS.md` "once they exist". Fixed; `STATUS.md` was never written (the
     tracker's last sections do that job) and stays in the lint's `PLANNED` list with that reason.
   - Stale in part, **marked in the index, not edited** (they are research records or plans):
     `content/C3` (cites `server/content/*.tsv`, now `content/`), `C5` and `C6` (plan `tools/econ.py`,
     `tools/npc_content.py`, built as `tools/content_econ.py`, `tools/content_npc.py`), `C1` and
     `history-purge` (cite `reprocess/tables.rs`, purged on purpose), `re/warp.md` and `re/static-corpus.md`
     (already carry partial banners), `archive/architecture-sweep.md` (open items last checked 2026-10-19,
     `[unverified]` since), `review/V*.md` (frozen against their tree by design).
   - Could not verify: whether the history purge (owner O2) has been run; whether `architecture-sweep`'s
     open items are still open; how much of the EvEJS inventories overlap.
5. **What the code cites.** 88 doc paths outside `docs/lanes/` and 200 lane notes are cited from Rust, Python
   and shell sources (tables below, from the new tree). The most cited: `docs/guides/jail.md` (74), `docs/archive/plan-client-ci.md` (74), `docs/port-tracker.md` (71), `docs/content/plan.md` (70). All of them were rewritten with the moves.
   `tools/test_docs.py` (a) checks every `docs/...md` path cited anywhere exists; the new check (h) checks
   every relative markdown link in `docs/`, `CLAUDE.md` and `README.md`. Both pass on the new tree.
6. **Orphans.** 104 lane notes had no inbound reference of any kind. They are now all listed in
   `docs/lanes/INDEX.md`. Every non-lane doc was already linked from at least one doc, and now from the index.

## The structure

```
docs/README.md            the index (every doc, role and status; checked by the lint)
docs/port-tracker.md      the history, append-only, in place (127 citations)
docs/guides/              how to run, test, operate: dev-mode, jail, human-checks, client-ci (new),
                          playing-with-friends, patcher, census, build-compat, history-purge, debugging-the-client
docs/reference/           how the protocol and server work now: framing, dbrow, object-wrapper, login-path,
                          push-path, push-events, bound-objects, code-push, session-roles, client-call-surface,
                          client-architecture, destiny-reference-blob, item-spawn, market-browse,
                          skill-system, wallet-ledger, world-state, threads-and-locks, server, unattended-root
docs/plans/               plan-subsystems, plan-deep-queue, plan-living-world, architecture-sweep
docs/content/, review/    unchanged: the two big plans with their research lanes, rulings and schema
docs/re/                  unchanged: client reverse-engineering facts
docs/evejs/               EvEJS facts (AGPL, facts only): run-safely, subsystem-gap, surprises, inventory/, review/
docs/lanes/               README.md (the convention), INDEX.md (generated), one flat note per lane
docs/archive/             demoted and finished docs (roadmap, wire-format, marshal-tags, session-flow, ...)
findings/                 unchanged: generated censuses
```

**Why the lane notes stay flat.** Moving 413 finished notes into `docs/lanes/archive/` would rewrite
about 400 files for no reader gain, and other lanes in flight add `docs/lanes/<name>.md` at the old path
while this runs. Instead `docs/lanes/INDEX.md` (generated by `tools/doc_lane_index.py`) groups them by series
with title, size and a "cited by code" flag, and the doc lint exempts flat lane notes from the per-file row.

Status words: **current**, **stale-in-part** (with the evidence above or in the row), **historical** (a finished
lane or plan, kept), **superseded** (replaced by the doc its banner names), **orphan** (no inbound reference before
the index existed).

## Inventory: documents outside `docs/lanes/`

Columns: new path (old path if moved, relative to `docs/`), size in KB, last commit date, source files that cite it (Rust, Python,
shell, TOML), docs that cite it (before the cleanup), status, what it covers.

| path | was | KB | last commit | code cites | doc cites | status | covers |
|---|---|---|---|---|---|---|---|
| `docs/guides/dev-mode.md` | `dev-mode.md` | 4 | 2026-10-28 | 4 | 3 | current | Dev mode: a debug world where nothing waits for training, building or income. |
| `docs/guides/jail.md` | `jail.md` | 34 | 2026-10-26 | 74 | 24 | current | The jail: the client never gets a route to CCP; what it protects and what it does not. |
| `docs/guides/human-checks.md` | `human-checks.md` | 61 | 2026-10-27 | 7 | 25 | current | The queue of things only a human with the client can verify: the card, the backlog, the commands. |
| `docs/guides/client-ci.md` |  | 3 | new | 1 |  | current | Client CI in one page: run a scenario against the real client, exit codes, where the pieces are. |
| `docs/guides/playing-with-friends.md` | `playing-with-friends.md` | 10 | 2026-10-26 | 5 | 9 | current | How a friend reaches the owner's server (ssh tunnel, kit) and how the owner runs accounts. |
| `docs/guides/patcher.md` | `patcher.md` | 8 | 2026-10-26 | 6 | 6 | current | The phone-home patcher `tools/eve_patch.py` and its per-build manifests. |
| `docs/guides/census.md` | `census.md` | 13 | 2026-10-26 | 22 | 6 | current | The census: proving the patched client phones nobody. |
| `docs/guides/build-compat.md` | `build-compat.md` | 4 | 2026-10-27 | 1 | 1 | current | Adding, checking and retiring a client build (`tools/new-build.sh`). |
| `docs/guides/history-purge.md` | `history-purge.md` | 6 | 2026-10-27 | 1 | 1 | current | Owner decision O2: the history-purge tooling and its dry run (the real run is an owner action). |
| `docs/guides/debugging-the-client.md` | `debugging-the-client.md` | 57 | 2026-10-24 | 16 | 42 | current | The method for debugging the real client against our server: instruments, traps, tooling. |
| `docs/reference/server.md` | `server.md` | 23 | 2026-10-27 | 1 | 12 | stale-in-part | What `eve-server` does; §§1-7 are the 2026-10-19..22 state (status note at the top), §8 lists services, flags and GM commands. |
| `docs/reference/world-state.md` | `world-state.md` | 30 | 2026-10-27 | 15 | 35 | current | Durable world state: STORE_FORMAT, delta kinds, the backup, the upgrade policy (the rest of the file is history). |
| `docs/reference/threads-and-locks.md` | `threads-and-locks.md` | 94 | 2026-10-26 | 2 | 13 | current | Threads, the world lock and the wire lock: the short picture, then the precise rules. |
| `docs/reference/unattended-root.md` | `unattended-root.md` | 15 | 2026-10-26 | 4 | 7 | stale-in-part | The threat model of the passwordless jail helper (status note at the top: the helper is built). |
| `docs/reference/framing.md` | `framing.md` | 10 | 2026-10-19 | 6 | 10 | current | machoNet TCP framing, from disassembly. |
| `docs/reference/dbrow.md` | `dbrow.md` | 15 | 2026-10-28 | 8 | 16 | current | `blue.DBRow` packed-row layout; supersedes the DBRow gap in the archived wire-format doc. |
| `docs/reference/object-wrapper.md` | `object-wrapper.md` | 36 | 2026-10-19 | 10 | 10 | current | The object wrapper codes 0x17, 0x22, 0x23 and the 0x2D terminator. |
| `docs/reference/login-path.md` | `login-path.md` | 82 | 2026-10-22 | 23 | 32 | current | The exact login call sequence and the shape of every value, TCP connect to character select. |
| `docs/reference/push-path.md` | `push-path.md` | 62 | 2026-10-22 | 6 | 20 | current | The server-initiated push path (spine step D). |
| `docs/reference/push-events.md` | `push-events.md` | 33 | 2026-10-19 | 2 | 10 | current | The 3,177 push handlers the client already has, and how `findings/push-events.tsv` was made. |
| `docs/reference/bound-objects.md` | `bound-objects.md` | 58 | 2026-10-22 | 15 | 21 | current | Bound objects: the client's second call style, and how the server mints them. |
| `docs/reference/code-push.md` | `code-push.md` | 8 | 2026-10-19 | 2 | 2 | current | The two answers that hand the client executable code (CPython marshal, not our marshal). |
| `docs/reference/session-roles.md` | `session-roles.md` | 47 | 2026-10-19 | 58 | 10 | current | The session object and the role mask: what unlocks the dev and GM tools. |
| `docs/reference/client-call-surface.md` | `client-call-surface.md` | 30 | 2026-10-19 | 1 | 6 | current | The census of every machoNet call site in the client (`findings/client-calls.tsv`). |
| `docs/reference/client-architecture.md` | `client-architecture.md` | 209 | 2026-10-26 | 1 | 3 | current | The real client mapped from the server's side (208 KB; opens with a build-number provenance note). |
| `docs/reference/destiny-reference-blob.md` | `destiny-reference-blob.md` | 52 | 2026-10-21 | 3 | 13 | current | Getting a reference Destiny blob out of the client (`tools/destiny-ref.sh`). |
| `docs/reference/item-spawn.md` | `item-spawn.md` | 41 | 2026-10-20 | 56 | 3 | current | The item store and the GM `/spawn` spec, from the client's bytecode. |
| `docs/reference/market-browse.md` | `market-browse.md` | 32 | 2026-10-20 | 2 | 6 | current | The market browse tree: why it was empty and what the server can do. |
| `docs/reference/skill-system.md` | `skill-system.md` | 60 | 2026-10-20 | 1 | 5 | current | The skill system, server side: what the client asks and what each answer must look like. |
| `docs/reference/wallet-ledger.md` | `wallet-ledger.md` | 52 | 2026-10-27 | 5 | 6 | current | The wallet ledger spec (account transaction history). |
| `docs/port-tracker.md` |  | 437 | 2026-10-27 | 71 | 59 | current | The numbered history of the port, §1-§85. The last sections are the current state and the owner's latest decisions. Never rewritten, only appended. |
| `docs/archive/plan-subsystems.md` | `plan-subsystems.md` | 46 | 2026-10-25 | 5 | 16 | current | The subsystem plan: the three planning layers, one source of truth per subsystem, the packets. |
| `docs/archive/plan-deep-queue.md` | `plan-deep-queue.md` | 27 | 2026-10-27 | 2 | 31 | current | The coordinator's deep work queue from 2026-10-27, wave by wave. |
| `docs/archive/plan-living-world.md` | `plan-living-world.md` | 7 | 2026-10-28 | 2 | 1 | current | The living world (2026-10-28): doctrines, a fluid economy, difficulty, four-level relations. |
| `docs/archive/architecture-sweep.md` | `architecture-sweep.md` | 7 | 2026-10-19 | 2 | 6 | stale-in-part | A reading of our own code for costs to come; its open items were last checked 2026-10-19 `[unverified]` since. |
| `docs/packets.tsv` |  | 8 | new | 0 |  | current | The packet registry: every issued id, its plan, lane note, landing sha (checked by `tools/test_docs.py`). |
| `docs/content/README.md` |  | 12 | 2026-10-25 | 8 | 21 | current | The content process: analyze, lanes C1-C7, synthesis, red team. |
| `docs/content/plan.md` |  | 114 | 2026-10-25 | 70 | 101 | current | The content plan v3: how the server gets, generates and keeps a live universe; packets CP*. |
| `docs/content/schema.md` |  | 99 | 2026-10-28 | 44 | 50 | current | The content schema: every authored table's header line, frozen. |
| `docs/content/rulings.md` |  | 4 | 2026-10-25 | 1 | 6 | current | Coordinator rulings on the content-plan review. |
| `docs/content/plan-review.md` |  | 29 | 2026-10-25 | 0 | 2 | current | The red-team review of the content plan. |
| `docs/content/C1-corpus-map.md` |  | 30 | 2026-10-25 | 1 | 2 | stale-in-part | Research lane C1: the corpus-to-feature map (cites `reprocess/tables.rs`, which was purged from the tree). |
| `docs/content/C2-live-surface.md` |  | 42 | 2026-10-25 | 4 | 6 | current | Research lane C2: the live-content surface of the client core. |
| `docs/content/C3-population.md` |  | 34 | 2026-10-25 | 3 | 4 | stale-in-part | Research lane C3: the world population generator (cites `server/content/*.tsv`, now `content/`). |
| `docs/content/C4-agents-missions.md` |  | 48 | 2026-10-25 | 0 | 3 | current | Research lane C4: agents, missions, arcs and the Agency. |
| `docs/content/C5-economy.md` |  | 45 | 2026-10-25 | 4 | 2 | stale-in-part | Research lane C5: economy content (names `tools/econ.py`, built as `tools/content_econ.py`). |
| `docs/content/C6-npc-content.md` |  | 43 | 2026-10-25 | 0 | 4 | stale-in-part | Research lane C6: NPCs as content (names `tools/npc_content.py`, built as `tools/content_npc.py`). |
| `docs/content/C7-liveness.md` |  | 38 | 2026-10-25 | 4 | 7 | current | Research lane C7: liveness and operations. |
| `docs/content/R-FW.md` |  | 24 | 2026-10-25 | 3 | 5 | current | Spec: factional warfare. |
| `docs/content/R-TACKLE.md` |  | 28 | 2026-10-25 | 5 | 4 | current | Spec: warp scramblers and disruptors. |
| `docs/content/R-WH.md` |  | 39 | 2026-10-25 | 2 | 4 | current | Spec: wormholes. |
| `docs/review/README.md` |  | 8 | 2026-10-25 | 0 | 8 | current | The emulator review: lanes V1-V7, synthesis, red team. |
| `docs/review/plan.md` |  | 72 | 2026-10-25 | 5 | 45 | current | The improvement plan v2: structure, security, the friends path; packets IP*. |
| `docs/review/rulings.md` |  | 6 | 2026-10-25 | 2 | 14 | current | Coordinator rulings on the review red team. |
| `docs/review/plan-review.md` |  | 31 | 2026-10-25 | 3 | 11 | current | The red-team review of the improvement plan. |
| `docs/review/V1-architecture.md` |  | 23 | 2026-10-25 | 2 | 3 | historical | Review lane V1: architecture and module structure (frozen against its tree). |
| `docs/review/V2-wire.md` |  | 13 | 2026-10-25 | 8 | 6 | historical | Review lane V2: wire, protocol and session robustness (frozen). |
| `docs/review/V3-runtime.md` |  | 19 | 2026-10-25 | 1 | 6 | historical | Review lane V3: world state, concurrency, performance (frozen). |
| `docs/review/V4-testing.md` |  | 30 | 2026-10-25 | 2 | 6 | historical | Review lane V4: testing, tooling and the gate (frozen). |
| `docs/review/V5-process.md` |  | 26 | 2026-10-25 | 0 | 3 | historical | Review lane V5: process, docs and knowledge (frozen; it asked for this index). |
| `docs/review/V6-security-ops.md` |  | 27 | 2026-10-25 | 2 | 7 | historical | Review lane V6: security, the jail and operations (frozen). |
| `docs/review/V7-content-fit.md` |  | 25 | 2026-10-25 | 0 | 4 | historical | Review lane V7: how the content data fits the emulator (frozen). |
| `docs/lanes/README.md` |  | 14 | 2026-10-27 | 5 | 41 | current | The lane convention: rules 1-16, the shape of a round, the lane note header. |
| `docs/archive/doc-inventory.md` |  | 64 | new | 0 |  | current | The inventory behind this index: every doc with size, date, status, citations, the move map and the cleanup findings. |
| `docs/lanes/INDEX.md` |  | 37 | new | 1 |  | current | Every flat lane note (`docs/lanes/<name>.md`) grouped by series, with title and size. Generated by `tools/doc_lane_index.py`. |
| `docs/re/dogma.md` |  | 57 | 2026-10-21 | 8 | 21 | current | Dogma: the attribute and effect system. |
| `docs/re/modules.md` |  | 54 | 2026-10-22 | 4 | 3 | current | Activating a fitted module. |
| `docs/re/targeting.md` |  | 55 | 2026-10-22 | 4 | 6 | current | Target locking, as an implementation spec. |
| `docs/re/damage-death.md` |  | 64 | 2026-10-22 | 7 | 8 | current | Taking damage and dying, as an implementation spec. |
| `docs/re/warp.md` |  | 56 | 2026-10-24 | 5 | 7 | stale-in-part | Warp as an implementation spec; the align-phase section is superseded (banner at the top). |
| `docs/re/jump-gates.md` |  | 70 | 2026-10-22 | 4 | 7 | current | A stargate jump, end to end. |
| `docs/re/npc-behaviors.md` |  | 66 | 2026-10-22 | 3 | 6 | current | NPC behaviours and what it takes for a bot to be a player. |
| `docs/re/agents-missions.md` |  | 65 | 2026-10-20 | 6 | 14 | current | Agents and missions from the client's side. |
| `docs/re/mission-model.md` |  | 46 | 2026-10-20 | 0 | 6 | current | The mission model: what a server implements one real mission from. |
| `docs/re/dungeon-model.md` |  | 46 | 2026-10-20 | 7 | 19 | current | The dungeon model, `dungeons.fsdbinary` read end to end. |
| `docs/re/static-corpus.md` |  | 32 | 2026-10-24 | 3 | 20 | stale-in-part | The static corpus `res:/staticdata/`; §6.1 offsets are superseded by `dungeon-model.md` (banner). |
| `docs/re/native-loaders.md` |  | 35 | 2026-10-20 | 2 | 8 | current | The native cFSD loaders: every table's record layout. |
| `docs/re/localization.md` |  | 26 | 2026-10-20 | 1 | 5 | current | The localisation corpus: format, lookup path, contents. |
| `docs/re/errors.md` |  | 38 | 2026-10-20 | 2 | 7 | current | The client's error vocabulary. |
| `docs/re/client-callable.md` |  | 30 | 2026-10-20 | 0 | 3 | current | The server-to-client call path and the `__exportedcalls__` catalogue. |
| `docs/re/session-and-binding.md` |  | 35 | 2026-10-20 | 2 | 3 | current | Session and binding from the client's side. |
| `docs/re/destiny-ball-crosscheck.md` |  | 11 | 2026-10-20 | 0 | 12 | current | The ball record cross-checked against a second implementation; current truth for the ball table. |
| `docs/re/evejs-crosscheck.md` |  | 52 | 2026-10-20 | 2 | 9 | current | EvEJS claims cross-checked against the client; current truth over the archived EvEJS reference. |
| `docs/re/evejs-data.md` |  | 68 | 2026-10-22 | 0 | 4 | current | What data EvEJS ships and which of it we lack (facts only). |
| `docs/evejs/review/README.md` | `lanes/evejs/README.md` | 16 | 2026-10-24 | 0 | 2 | current | The EvEJS fork review: what it ships, its Destiny model, our four blocked features; the entry to `docs/evejs/review/`. |
| `docs/evejs/review/static-data.md` | `lanes/evejs/static-data.md` | 46 | 2026-10-24 | 1 | 0 | current | The datamined data EvEJS ships, formats and schemas. |
| `docs/evejs/review/destiny.md` | `lanes/evejs/destiny.md` | 60 | 2026-10-24 | 0 | 0 | current | Their space and Destiny model: constants, formulas, visibility. |
| `docs/evejs/review/features.md` | `lanes/evejs/features.md` | 159 | 2026-10-24 | 0 | 4 | current | Stargates, a second station, asteroid fields, the sun (157 KB). |
| `docs/evejs/review/docs-packs.md` | `lanes/evejs/docs-packs.md` | 56 | 2026-10-24 | 0 | 0 | current | Their documentation and the 339 MB of content packs. |
| `docs/evejs/evejs-run-safely.md` | `evejs-run-safely.md` | 12 | 2026-10-20 | 1 | 1 | current | Running EvEJS in a container: go/no-go, four changes, the build delta. |
| `docs/evejs/evejs-subsystem-gap.md` | `evejs-subsystem-gap.md` | 30 | 2026-10-21 | 0 | 7 | current | A scope map of what EvEJS implements that we do not (from the file listing alone). |
| `docs/evejs/evejs-surprises.md` | `evejs-surprises.md` | 45 | 2026-10-22 | 1 | 1 | current | Where EvEJS and the client disagree, re-verified against the client. |
| `docs/evejs/inventory/character-dogma.md` | `inventory/character-dogma.md` | 56 | 2026-10-20 | 0 | 1 | current | Inventory of EvEJS: character, skills, dogma. |
| `docs/evejs/inventory/corp-sov-structures.md` | `inventory/corp-sov-structures.md` | 55 | 2026-10-20 | 0 | 1 | current | Inventory of EvEJS: corporations, sov, structures, fleets. |
| `docs/evejs/inventory/industry-mining.md` | `inventory/industry-mining.md` | 50 | 2026-10-20 | 1 | 2 | current | Inventory of EvEJS: industry, mining, reprocessing, planets. |
| `docs/evejs/inventory/market-economy.md` | `inventory/market-economy.md` | 51 | 2026-10-20 | 0 | 1 | current | Inventory of EvEJS: market, wallet, economy. |
| `docs/evejs/inventory/npc-content.md` | `inventory/npc-content.md` | 51 | 2026-10-20 | 0 | 1 | current | Inventory of EvEJS: NPCs, dungeons, missions, sites. |
| `docs/evejs/inventory/social-infra.md` | `inventory/social-infra.md` | 66 | 2026-10-20 | 0 | 1 | current | Inventory of EvEJS: social, chat, session, infrastructure. |
| `docs/evejs/inventory/space-destiny.md` | `inventory/space-destiny.md` | 38 | 2026-10-20 | 1 | 3 | current | Inventory of EvEJS: space, Destiny, combat. |
| `docs/archive/roadmap.md` | `roadmap.md` | 42 | 2026-10-24 | 5 | 26 | historical | The original roadmap; its founding vision still stands, its milestone list is long past. |
| `docs/archive/OVERNIGHT_PLAN.md` | `OVERNIGHT_PLAN.md` | 41 | 2026-10-24 | 0 | 5 | superseded | First overnight plan; superseded twice. |
| `docs/archive/plan-overnight-8h.md` | `plan-overnight-8h.md` | 17 | 2026-10-24 | 0 | 5 | superseded | The eight-hour unattended run (2026-10-20). |
| `docs/archive/plan-overnight-261021.md` | `plan-overnight-261021.md` | 23 | 2026-10-24 | 0 | 4 | superseded | The overnight run of 2026-10-21. |
| `docs/archive/plan-overnight-2026-10-23.md` | `plan-overnight-2026-10-23.md` | 34 | 2026-10-24 | 0 | 5 | superseded | The overnight run of 2026-10-23. |
| `docs/archive/plan-bots.md` | `plan-bots.md` | 16 | 2026-10-24 | 4 | 4 | historical | The bot design (milestones now built; see the lane notes). |
| `docs/archive/plan-client-ci.md` | `plan-client-ci.md` | 12 | 2026-10-24 | 74 | 11 | historical | The client-CI plan; the harness exists, see `docs/guides/client-ci.md`. |
| `docs/archive/plan-docked-subsystems.md` | `plan-docked-subsystems.md` | 9 | 2026-10-19 | 5 | 16 | historical | The docked-subsystems plan, two rounds (built). |
| `docs/archive/plan-port-from-evejs.md` | `plan-port-from-evejs.md` | 14 | 2026-10-20 | 1 | 3 | historical | The EvEJS porting plan; §1 records the licence decision. |
| `docs/archive/plan-the-churn.md` | `plan-the-churn.md` | 28 | 2026-10-20 | 0 | 11 | historical | The first reverse-engineering sweep plan. |
| `docs/archive/recon-v24.01.md` | `recon-v24.01.md` | 20 | 2026-10-24 | 1 | 15 | historical | The first recon of the V24.01 client. |
| `docs/archive/session-flow.md` | `session-flow.md` | 8 | 2026-10-24 | 2 | 10 | superseded | Session establishment overview; superseded by `docs/reference/login-path.md`. |
| `docs/archive/wire-format.md` | `wire-format.md` | 21 | 2026-10-24 | 5 | 16 | superseded | The first wire-format doc; superseded by `framing.md` and `dbrow.md`. |
| `docs/archive/marshal-tags.md` | `marshal-tags.md` | 24 | 2026-10-24 | 5 | 17 | superseded | Marshal type codes; superseded by `object-wrapper.md` and `dbrow.md`. |
| `docs/archive/entering-the-game.md` | `entering-the-game.md` | 45 | 2026-10-24 | 4 | 7 | historical | From the character click to a station hangar (a 2026-10-19 snapshot). |
| `docs/archive/destiny-and-the-ballpark.md` | `destiny-and-the-ballpark.md` | 55 | 2026-10-24 | 6 | 22 | superseded | Destiny and the ballpark; §4.4 superseded by `docs/re/destiny-ball-crosscheck.md`. |
| `docs/archive/evejs-261018-reference.md` | `evejs-261018-reference.md` | 35 | 2026-10-24 | 1 | 11 | superseded | The first EvEJS briefing; superseded by `docs/re/evejs-crosscheck.md`. |
| `CLAUDE.md` |  | 3 | 2026-10-26 | 0 | 4 | current | Agent entry point: the rules and what to read first (fixed: tracker sections, index). |
| `README.md` |  | 7 | 2026-10-27 | 0 | 79 | current | How to play, the licence statement, the repository layout. |
| `content/README.md` |  | 3 | 2026-10-25 | 0 | 21 | current | What may go in `content/` (authored tables), and the file format. |
| `captures/regression/README.md` |  | 2 | 2026-10-19 | 0 | 0 | current | The few curated captures kept as regression tests. |
| `findings/outbound-endpoints.txt` |  | 2 | 2026-10-19 | 0 | 3 | current | Census of the client's outbound network surface. |
| `findings/remote-services.txt` |  | 21 | 2026-10-19 | 0 | 9 | current | Inventory of `RemoteSvc` call sites (superseded in part by `bound-objects.md` and the call-surface TSV). |
| `findings/proto-messages.txt` |  | 493 | 2026-10-19 | 0 | 4 | current | Protobuf message inventory (492 KB, generated). |
| `findings/destiny-ref/00-REPORT.txt` |  | 78 | 2026-10-21 | 0 | 2 | current | Report of the reference Destiny blob extraction (output of `tools/destiny-ref.sh`). |

Not docs and left alone: `containers/analysis.requirements.txt` (a pip list), `tools/client-ci/fci-block.txt` (the
embedded block every scenario carries), `docs/packets.tsv` (the registry, listed above), `findings/*.tsv`.

## Move map (old -> new)

Old paths are written relative to `docs/` (so that the doc lint does not read them as live citations). `moves.sh` has the same lines as `git mv`. 65 moves; the lane notes and `docs/port-tracker.md` did not move.

| old | new |
|---|---|
| `dev-mode.md` | `docs/guides/dev-mode.md` |
| `jail.md` | `docs/guides/jail.md` |
| `human-checks.md` | `docs/guides/human-checks.md` |
| `playing-with-friends.md` | `docs/guides/playing-with-friends.md` |
| `patcher.md` | `docs/guides/patcher.md` |
| `census.md` | `docs/guides/census.md` |
| `build-compat.md` | `docs/guides/build-compat.md` |
| `history-purge.md` | `docs/guides/history-purge.md` |
| `debugging-the-client.md` | `docs/guides/debugging-the-client.md` |
| `framing.md` | `docs/reference/framing.md` |
| `dbrow.md` | `docs/reference/dbrow.md` |
| `object-wrapper.md` | `docs/reference/object-wrapper.md` |
| `login-path.md` | `docs/reference/login-path.md` |
| `push-path.md` | `docs/reference/push-path.md` |
| `push-events.md` | `docs/reference/push-events.md` |
| `bound-objects.md` | `docs/reference/bound-objects.md` |
| `code-push.md` | `docs/reference/code-push.md` |
| `session-roles.md` | `docs/reference/session-roles.md` |
| `unattended-root.md` | `docs/reference/unattended-root.md` |
| `client-call-surface.md` | `docs/reference/client-call-surface.md` |
| `client-architecture.md` | `docs/reference/client-architecture.md` |
| `destiny-reference-blob.md` | `docs/reference/destiny-reference-blob.md` |
| `item-spawn.md` | `docs/reference/item-spawn.md` |
| `market-browse.md` | `docs/reference/market-browse.md` |
| `skill-system.md` | `docs/reference/skill-system.md` |
| `wallet-ledger.md` | `docs/reference/wallet-ledger.md` |
| `world-state.md` | `docs/reference/world-state.md` |
| `threads-and-locks.md` | `docs/reference/threads-and-locks.md` |
| `server.md` | `docs/reference/server.md` |
| `plan-subsystems.md` | `docs/archive/plan-subsystems.md` |
| `plan-deep-queue.md` | `docs/archive/plan-deep-queue.md` |
| `plan-living-world.md` | `docs/archive/plan-living-world.md` |
| `architecture-sweep.md` | `docs/archive/architecture-sweep.md` |
| `OVERNIGHT_PLAN.md` | `docs/archive/OVERNIGHT_PLAN.md` |
| `plan-overnight-2026-10-23.md` | `docs/archive/plan-overnight-2026-10-23.md` |
| `plan-overnight-261021.md` | `docs/archive/plan-overnight-261021.md` |
| `plan-overnight-8h.md` | `docs/archive/plan-overnight-8h.md` |
| `plan-bots.md` | `docs/archive/plan-bots.md` |
| `plan-client-ci.md` | `docs/archive/plan-client-ci.md` |
| `session-flow.md` | `docs/archive/session-flow.md` |
| `marshal-tags.md` | `docs/archive/marshal-tags.md` |
| `wire-format.md` | `docs/archive/wire-format.md` |
| `entering-the-game.md` | `docs/archive/entering-the-game.md` |
| `destiny-and-the-ballpark.md` | `docs/archive/destiny-and-the-ballpark.md` |
| `recon-v24.01.md` | `docs/archive/recon-v24.01.md` |
| `roadmap.md` | `docs/archive/roadmap.md` |
| `plan-docked-subsystems.md` | `docs/archive/plan-docked-subsystems.md` |
| `plan-port-from-evejs.md` | `docs/archive/plan-port-from-evejs.md` |
| `plan-the-churn.md` | `docs/archive/plan-the-churn.md` |
| `evejs-261018-reference.md` | `docs/archive/evejs-261018-reference.md` |
| `evejs-run-safely.md` | `docs/evejs/evejs-run-safely.md` |
| `evejs-subsystem-gap.md` | `docs/evejs/evejs-subsystem-gap.md` |
| `evejs-surprises.md` | `docs/evejs/evejs-surprises.md` |
| `inventory/character-dogma.md` | `docs/evejs/inventory/character-dogma.md` |
| `inventory/corp-sov-structures.md` | `docs/evejs/inventory/corp-sov-structures.md` |
| `inventory/industry-mining.md` | `docs/evejs/inventory/industry-mining.md` |
| `inventory/market-economy.md` | `docs/evejs/inventory/market-economy.md` |
| `inventory/npc-content.md` | `docs/evejs/inventory/npc-content.md` |
| `inventory/social-infra.md` | `docs/evejs/inventory/social-infra.md` |
| `inventory/space-destiny.md` | `docs/evejs/inventory/space-destiny.md` |
| `lanes/evejs/README.md` | `docs/evejs/review/README.md` |
| `lanes/evejs/destiny.md` | `docs/evejs/review/destiny.md` |
| `lanes/evejs/docs-packs.md` | `docs/evejs/review/docs-packs.md` |
| `lanes/evejs/features.md` | `docs/evejs/review/features.md` |
| `lanes/evejs/static-data.md` | `docs/evejs/review/static-data.md` |

## What the code cites

Doc paths cited by Rust, Python and shell sources, in the new tree (number of citing source files). Section references such as `docs/lanes/<name>.md §3` ride along: the rewrite changes the path and leaves the section.

| doc | citing files |
|---|---|
| `docs/archive/plan-client-ci.md` | 74 |
| `docs/guides/jail.md` | 74 |
| `docs/port-tracker.md` | 71 |
| `docs/content/plan.md` | 70 |
| `docs/reference/session-roles.md` | 58 |
| `docs/reference/item-spawn.md` | 56 |
| `docs/content/schema.md` | 44 |
| `docs/reference/login-path.md` | 23 |
| `docs/guides/census.md` | 22 |
| `docs/guides/debugging-the-client.md` | 16 |
| `docs/reference/bound-objects.md` | 15 |
| `docs/reference/world-state.md` | 15 |
| `docs/reference/object-wrapper.md` | 10 |
| `docs/content/README.md` | 8 |
| `docs/re/dogma.md` | 8 |
| `docs/reference/dbrow.md` | 8 |
| `docs/review/V2-wire.md` | 8 |
| `docs/guides/human-checks.md` | 7 |
| `docs/re/damage-death.md` | 7 |
| `docs/re/dungeon-model.md` | 7 |
| `docs/archive/destiny-and-the-ballpark.md` | 6 |
| `docs/guides/patcher.md` | 6 |
| `docs/re/agents-missions.md` | 6 |
| `docs/reference/framing.md` | 6 |
| `docs/reference/push-path.md` | 6 |
| `docs/archive/marshal-tags.md` | 5 |
| `docs/archive/plan-docked-subsystems.md` | 5 |
| `docs/archive/roadmap.md` | 5 |
| `docs/archive/wire-format.md` | 5 |
| `docs/content/R-TACKLE.md` | 5 |
| `docs/guides/playing-with-friends.md` | 5 |
| `docs/archive/plan-subsystems.md` | 5 |
| `docs/re/warp.md` | 5 |
| `docs/reference/wallet-ledger.md` | 5 |
| `docs/review/plan.md` | 5 |
| `docs/archive/entering-the-game.md` | 4 |
| `docs/archive/plan-bots.md` | 4 |
| `docs/content/C2-live-surface.md` | 4 |
| `docs/content/C5-economy.md` | 4 |
| `docs/content/C7-liveness.md` | 4 |
| `docs/guides/dev-mode.md` | 4 |
| `docs/re/jump-gates.md` | 4 |
| `docs/re/modules.md` | 4 |
| `docs/re/targeting.md` | 4 |
| `docs/reference/unattended-root.md` | 4 |
| `docs/content/C3-population.md` | 3 |
| `docs/content/R-FW.md` | 3 |
| `docs/re/npc-behaviors.md` | 3 |
| `docs/re/static-corpus.md` | 3 |
| `docs/reference/destiny-reference-blob.md` | 3 |
| `docs/review/plan-review.md` | 3 |
| `docs/README.md` | 2 |
| `docs/archive/session-flow.md` | 2 |
| `docs/content/R-WH.md` | 2 |
| `docs/archive/architecture-sweep.md` | 2 |
| `docs/archive/plan-deep-queue.md` | 2 |
| `docs/archive/plan-living-world.md` | 2 |
| `docs/re/errors.md` | 2 |
| `docs/re/evejs-crosscheck.md` | 2 |
| `docs/re/native-loaders.md` | 2 |
| `docs/re/session-and-binding.md` | 2 |
| `docs/reference/code-push.md` | 2 |
| `docs/reference/market-browse.md` | 2 |
| `docs/reference/push-events.md` | 2 |
| `docs/reference/threads-and-locks.md` | 2 |
| `docs/review/V1-architecture.md` | 2 |
| `docs/review/V4-testing.md` | 2 |
| `docs/review/V6-security-ops.md` | 2 |
| `docs/review/rulings.md` | 2 |
| `docs/archive/evejs-261018-reference.md` | 1 |
| `docs/archive/plan-port-from-evejs.md` | 1 |
| `docs/archive/recon-v24.01.md` | 1 |
| `docs/content/C1-corpus-map.md` | 1 |
| `docs/content/rulings.md` | 1 |
| `docs/evejs/evejs-run-safely.md` | 1 |
| `docs/evejs/evejs-surprises.md` | 1 |
| `docs/evejs/inventory/industry-mining.md` | 1 |
| `docs/evejs/inventory/space-destiny.md` | 1 |
| `docs/evejs/review/static-data.md` | 1 |
| `docs/guides/build-compat.md` | 1 |
| `docs/guides/client-ci.md` | 1 |
| `docs/guides/history-purge.md` | 1 |
| `docs/re/localization.md` | 1 |
| `docs/reference/client-architecture.md` | 1 |
| `docs/reference/client-call-surface.md` | 1 |
| `docs/reference/server.md` | 1 |
| `docs/reference/skill-system.md` | 1 |
| `docs/review/V3-runtime.md` | 1 |

Lane notes cited by code: 200 distinct notes, 584 citations. They did not move.
The most cited: `docs/lanes/ci-scenarios.md` (72), `docs/lanes/f-ci.md` (30), `docs/lanes/bots.md` (15), `docs/lanes/docked.md` (12), `docs/lanes/chat.md` (10), `docs/lanes/gaps-2.md` (10), `docs/lanes/account-spec.md` (9), `docs/lanes/npc.md` (9).

## Inventory: lane notes (`docs/lanes/`)

All **historical**: each is a finished lane's hand-off note. `orphan` = no file referenced it before the cleanup; `code` = number of source files that cite it. Titles are in `docs/lanes/INDEX.md`.

| note | KB | last commit | code | doc cites | status |
|---|---|---|---|---|---|
| `docs/lanes/README.md` | 14 | 2026-10-27 | 5 | 41 | current (the convention) |
| `docs/lanes/acc2.md` | 10 | 2026-10-26 | 3 | 2 | historical |
| `docs/lanes/acc2tools.md` | 12 | 2026-10-26 | 3 | 1 | historical |
| `docs/lanes/account-spec.md` | 31 | 2026-10-25 | 9 | 10 | historical |
| `docs/lanes/addballs.md` | 69 | 2026-10-21 | 3 | 8 | historical |
| `docs/lanes/agents-missions.md` | 34 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/asteroids.md` | 35 | 2026-10-25 | 5 | 4 | historical |
| `docs/lanes/ba.md` | 11 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/bookmarks.md` | 14 | 2026-10-24 | 2 | 1 | historical |
| `docs/lanes/bot-accounts.md` | 3 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/bot-corp.md` | 7 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/bot-ctl.md` | 7 | 2026-10-28 | 2 | 0 | historical |
| `docs/lanes/bot-panel.md` | 7 | 2026-10-28 | 1 | 1 | historical |
| `docs/lanes/bot-tune.md` | 6 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/botmark.md` | 5 | 2026-10-26 | 1 | 0 | historical |
| `docs/lanes/bots.md` | 63 | 2026-10-25 | 15 | 5 | historical |
| `docs/lanes/breaker-sweep.md` | 6 | 2026-10-25 | 7 | 1 | historical |
| `docs/lanes/c4-fix.md` | 1 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/c6b.md` | 6 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/c8-a.md` | 13 | 2026-10-27 | 7 | 1 | historical |
| `docs/lanes/call-surface.md` | 8 | 2026-10-19 | 0 | 1 | historical |
| `docs/lanes/cap-b.md` | 6 | 2026-10-27 | 0 | 1 | historical |
| `docs/lanes/cap-c.md` | 7 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/cap-d.md` | 12 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/cap-e.md` | 11 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/cap-f.md` | 4 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/cap-g.md` | 15 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/cap-h.md` | 8 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/cap-k.md` | 4 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/cap-l.md` | 7 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/cap-loot.md` | 6 | 2026-10-27 | 2 | 1 | historical |
| `docs/lanes/cen1.md` | 9 | 2026-10-26 | 1 | 2 | historical |
| `docs/lanes/cen23.md` | 8 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/cen4.md` | 9 | 2026-10-26 | 8 | 1 | historical |
| `docs/lanes/census-blindspots.md` | 32 | 2026-10-21 | 1 | 3 | historical |
| `docs/lanes/character.md` | 20 | 2026-10-19 | 0 | 1 | historical |
| `docs/lanes/chat-petitions.md` | 17 | 2026-10-19 | 1 | 2 | historical |
| `docs/lanes/chat.md` | 39 | 2026-10-25 | 10 | 1 | historical |
| `docs/lanes/chatflake.md` | 1 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/ci-display.md` | 18 | 2026-10-22 | 3 | 1 | historical |
| `docs/lanes/ci-harness.md` | 36 | 2026-10-24 | 3 | 7 | historical |
| `docs/lanes/ci-scenarios.md` | 102 | 2026-10-24 | 72 | 5 | historical |
| `docs/lanes/ci-trigger.md` | 15 | 2026-10-22 | 4 | 5 | historical |
| `docs/lanes/client-callable.md` | 4 | 2026-10-20 | 0 | 1 | historical |
| `docs/lanes/clock.md` | 12 | 2026-10-27 | 3 | 0 | historical |
| `docs/lanes/clones.md` | 16 | 2026-10-24 | 3 | 3 | historical |
| `docs/lanes/co-found.md` | 4 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/concord-1a.md` | 7 | 2026-10-27 | 2 | 1 | historical |
| `docs/lanes/concord-1b.md` | 5 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/concord-gate.md` | 5 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/contacts.md` | 15 | 2026-10-24 | 1 | 1 | historical |
| `docs/lanes/contracts.md` | 30 | 2026-10-25 | 1 | 2 | historical |
| `docs/lanes/corp-buy.md` | 6 | 2026-10-28 | 3 | 0 | historical |
| `docs/lanes/corp-o.md` | 18 | 2026-10-27 | 5 | 1 | historical |
| `docs/lanes/corp-s.md` | 5 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/corp-t.md` | 11 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/corp-w.md` | 20 | 2026-10-27 | 4 | 3 | historical |
| `docs/lanes/corp.md` | 15 | 2026-10-25 | 3 | 3 | historical |
| `docs/lanes/cp1.md` | 8 | 2026-10-25 | 1 | 4 | historical |
| `docs/lanes/cp10.md` | 5 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp11a.md` | 5 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp11b.md` | 7 | 2026-10-25 | 1 | 2 | historical |
| `docs/lanes/cp12.md` | 6 | 2026-10-25 | 2 | 2 | historical |
| `docs/lanes/cp13.md` | 4 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp14p.md` | 6 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp15.md` | 5 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp16.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp17.md` | 6 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp18.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp19.md` | 4 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp2.md` | 3 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp20.md` | 4 | 2026-10-25 | 0 | 4 | historical |
| `docs/lanes/cp21.md` | 6 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp22.md` | 5 | 2026-10-25 | 1 | 3 | historical |
| `docs/lanes/cp23.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp24.md` | 7 | 2026-10-25 | 1 | 3 | historical |
| `docs/lanes/cp25.md` | 5 | 2026-10-25 | 1 | 2 | historical |
| `docs/lanes/cp26a.md` | 6 | 2026-10-25 | 0 | 3 | historical |
| `docs/lanes/cp26b-1.md` | 5 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp26b-2.md` | 5 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp27.md` | 5 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp28a.md` | 7 | 2026-10-25 | 1 | 2 | historical |
| `docs/lanes/cp28b.md` | 6 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp29.md` | 5 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp3.md` | 5 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp30.md` | 4 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp31.md` | 3 | 2026-10-25 | 0 | 4 | historical |
| `docs/lanes/cp32a.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp32b.md` | 5 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp33a.md` | 3 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp33b.md` | 6 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp34.md` | 5 | 2026-10-25 | 1 | 4 | historical |
| `docs/lanes/cp35.md` | 5 | 2026-10-25 | 1 | 2 | historical |
| `docs/lanes/cp36.md` | 4 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp37a.md` | 6 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp37b.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp38.md` | 4 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp39.md` | 6 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp3b.md` | 4 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp3c.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp4.md` | 2 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp40.md` | 4 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp41.md` | 7 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp42.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp43.md` | 5 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp44.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp45.md` | 5 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp46.md` | 4 | 2026-10-25 | 1 | 0 | historical |
| `docs/lanes/cp47.md` | 5 | 2026-10-25 | 1 | 0 | historical |
| `docs/lanes/cp48.md` | 6 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp49.md` | 9 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp5.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp50.md` | 4 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp51.md` | 4 | 2026-10-25 | 1 | 2 | historical |
| `docs/lanes/cp53a.md` | 5 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp53b.md` | 5 | 2026-10-25 | 1 | 0 | historical |
| `docs/lanes/cp54a.md` | 6 | 2026-10-25 | 1 | 2 | historical |
| `docs/lanes/cp54b.md` | 6 | 2026-10-25 | 1 | 0 | historical |
| `docs/lanes/cp55.md` | 6 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp56.md` | 6 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp57.md` | 8 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/cp58a.md` | 5 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp58b.md` | 7 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/cp58c.md` | 3 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp58d.md` | 5 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/cp58e.md` | 6 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp59a.md` | 4 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp59b.md` | 5 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/cp6.md` | 3 | 2026-10-25 | 0 | 3 | historical |
| `docs/lanes/cp60.md` | 7 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp61.md` | 3 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/cp7.md` | 2 | 2026-10-25 | 0 | 3 | historical |
| `docs/lanes/cp8.md` | 3 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/crimewatch.md` | 16 | 2026-10-25 | 1 | 0 | historical |
| `docs/lanes/cw-1.md` | 16 | 2026-10-27 | 3 | 1 | historical |
| `docs/lanes/d-hub.md` | 4 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/d2-fix.md` | 1 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/damage-death.md` | 46 | 2026-10-25 | 5 | 3 | historical |
| `docs/lanes/damage-gap.md` | 1 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/dbrow-singleton.md` | 1 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/dev-mode.md` | 3 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/difficulty.md` | 3 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/doc-0.md` | 5 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/doc-1.md` | 7 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/doc-2.md` | 4 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/doc-3.md` | 8 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/doc-4.md` | 11 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/doc-5.md` | 6 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/doc-6.md` | 8 | 2026-10-28 | 2 | 0 | historical |
| `docs/lanes/doc-9.md` | 8 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/dock-up.md` | 63 | 2026-10-21 | 0 | 5 | historical |
| `docs/lanes/docked.md` | 13 | 2026-10-25 | 12 | 0 | historical |
| `docs/lanes/dogma-corpus.md` | 20 | 2026-10-24 | 0 | 1 | historical |
| `docs/lanes/dogma-engine.md` | 7 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/drones.md` | 21 | 2026-10-25 | 3 | 1 | historical |
| `docs/lanes/dungeon-model.md` | 6 | 2026-10-20 | 0 | 1 | historical |
| `docs/lanes/e1-settle.md` | 13 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/e1.md` | 7 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/e10.md` | 7 | 2026-10-26 | 1 | 0 | historical |
| `docs/lanes/e10b.md` | 7 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/e11.md` | 5 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/e2.md` | 4 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/e3.md` | 4 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/e4.md` | 7 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/e5.md` | 8 | 2026-10-26 | 1 | 0 | historical |
| `docs/lanes/e5b.md` | 6 | 2026-10-26 | 1 | 0 | historical |
| `docs/lanes/e6.md` | 5 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/e7.md` | 5 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/e8.md` | 8 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/e9.md` | 8 | 2026-10-26 | 1 | 0 | historical |
| `docs/lanes/engine-fix.md` | 9 | 2026-10-28 | 2 | 0 | historical |
| `docs/lanes/error-vocabulary.md` | 9 | 2026-10-20 | 0 | 1 | historical |
| `docs/lanes/evejs-crosscheck.md` | 8 | 2026-10-20 | 0 | 1 | historical |
| `docs/lanes/evejs-data-static.md` | 6 | 2026-10-22 | 0 | 1 | historical |
| `docs/lanes/evejs-probe.md` | 24 | 2026-10-22 | 0 | 4 | historical |
| `docs/lanes/f-ci.md` | 7 | 2026-10-27 | 30 | 1 | historical |
| `docs/lanes/f-hist.md` | 1 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/fci-catchup.md` | 7 | 2026-10-27 | 7 | 0 | historical |
| `docs/lanes/fe-build.md` | 6 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/fe-demand.md` | 6 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/fe-measure.md` | 9 | 2026-10-28 | 2 | 0 | historical |
| `docs/lanes/fe-trade.md` | 7 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/fe-value.md` | 6 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/fitting-market.md` | 4 | 2026-10-28 | 2 | 0 | historical |
| `docs/lanes/fitting.md` | 52 | 2026-10-25 | 8 | 2 | historical |
| `docs/lanes/flake-gate.md` | 1 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/fleet.md` | 42 | 2026-10-24 | 8 | 6 | historical |
| `docs/lanes/friends.md` | 15 | 2026-10-26 | 1 | 1 | historical |
| `docs/lanes/fsd-generic.md` | 13 | 2026-10-25 | 3 | 0 | historical |
| `docs/lanes/fsd-reader.md` | 7 | 2026-10-20 | 0 | 0 | historical, orphan |
| `docs/lanes/ft-a.md` | 4 | 2026-10-27 | 3 | 2 | historical |
| `docs/lanes/ft-b.md` | 11 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/ft-e.md` | 16 | 2026-10-27 | 3 | 0 | historical |
| `docs/lanes/ft-gh.md` | 17 | 2026-10-27 | 4 | 0 | historical |
| `docs/lanes/fu-fwlp.md` | 9 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/fu-pi2.md` | 10 | 2026-10-27 | 0 | 1 | historical |
| `docs/lanes/fx1.md` | 5 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/fx10.md` | 6 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/fx11.md` | 3 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/fx2.md` | 3 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/fx3.md` | 4 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/fx5.md` | 5 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/fx6.md` | 13 | 2026-10-25 | 2 | 0 | historical |
| `docs/lanes/fx7.md` | 6 | 2026-10-25 | 1 | 0 | historical |
| `docs/lanes/fx8.md` | 7 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/fx9.md` | 9 | 2026-10-25 | 1 | 0 | historical |
| `docs/lanes/gap-refresh.md` | 9 | 2026-10-21 | 0 | 0 | historical, orphan |
| `docs/lanes/gaps-2.md` | 16 | 2026-10-27 | 10 | 0 | historical |
| `docs/lanes/gaps-b.md` | 9 | 2026-10-27 | 3 | 1 | historical |
| `docs/lanes/gb-fix.md` | 12 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/grid-build.md` | 10 | 2026-10-25 | 0 | 4 | historical |
| `docs/lanes/grid-research.md` | 26 | 2026-10-25 | 9 | 2 | historical |
| `docs/lanes/grid-vis.md` | 2 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/hk1.md` | 7 | 2026-10-26 | 1 | 0 | historical |
| `docs/lanes/hk2.md` | 8 | 2026-10-26 | 2 | 1 | historical |
| `docs/lanes/hk3.md` | 8 | 2026-10-26 | 1 | 1 | historical |
| `docs/lanes/hk4.md` | 9 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/hull-numbers.md` | 20 | 2026-10-21 | 1 | 3 | historical |
| `docs/lanes/identity-spec.md` | 39 | 2026-10-23 | 0 | 2 | historical |
| `docs/lanes/implants.md` | 11 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/industry.md` | 111 | 2026-10-25 | 4 | 7 | historical |
| `docs/lanes/inventory.md` | 37 | 2026-10-24 | 5 | 1 | historical |
| `docs/lanes/ip1.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip10.md` | 5 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip11.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip12.md` | 4 | 2026-10-25 | 2 | 1 | historical |
| `docs/lanes/ip13.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip14.md` | 10 | 2026-10-25 | 0 | 7 | historical |
| `docs/lanes/ip14b.md` | 7 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip15.md` | 6 | 2026-10-25 | 0 | 3 | historical |
| `docs/lanes/ip16.md` | 4 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/ip17.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip18.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip19.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip2.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip20.md` | 2 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip21.md` | 4 | 2026-10-25 | 1 | 1 | historical |
| `docs/lanes/ip27-a.md` | 6 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip27-b.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip28-a.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip29.md` | 7 | 2026-10-25 | 2 | 5 | historical |
| `docs/lanes/ip30.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip31.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip32.md` | 3 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/ip34.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip36-a.md` | 1 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip36-b.md` | 2 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip37-a.md` | 2 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip37-b.md` | 2 | 2026-10-25 | 0 | 2 | historical |
| `docs/lanes/ip41.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip42.md` | 4 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/ip6.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip7.md` | 4 | 2026-10-25 | 1 | 2 | historical |
| `docs/lanes/ip8.md` | 6 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/ip9.md` | 4 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/item-spawn.md` | 6 | 2026-10-20 | 0 | 0 | historical, orphan |
| `docs/lanes/iw5-character.md` | 6 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/iw5-destiny.md` | 8 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/iw6.md` | 7 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/jump-gates.md` | 6 | 2026-10-22 | 0 | 0 | historical, orphan |
| `docs/lanes/k-boost.md` | 14 | 2026-10-27 | 6 | 0 | historical |
| `docs/lanes/k-drone.md` | 11 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/k-ewar.md` | 11 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/k-logi.md` | 13 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/k-mskill.md` | 9 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/k-overheat.md` | 16 | 2026-10-27 | 4 | 0 | historical |
| `docs/lanes/k-pod.md` | 3 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/k-ratai.md` | 4 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/loader-pyd.md` | 10 | 2026-10-20 | 0 | 0 | historical, orphan |
| `docs/lanes/localization-corpus.md` | 9 | 2026-10-20 | 0 | 1 | historical |
| `docs/lanes/lookup.md` | 7 | 2026-10-25 | 2 | 0 | historical |
| `docs/lanes/m1.md` | 3 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/m2.md` | 3 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/m3.md` | 6 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/m4.md` | 3 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/mail-1.md` | 8 | 2026-10-27 | 3 | 0 | historical |
| `docs/lanes/map.md` | 3 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/market-tree.md` | 7 | 2026-10-20 | 0 | 1 | historical |
| `docs/lanes/market.md` | 23 | 2026-10-24 | 2 | 3 | historical |
| `docs/lanes/merge-b.md` | 6 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/merge-d.md` | 3 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/merge-e.md` | 3 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/miner-defense.md` | 4 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/mining.md` | 11 | 2026-10-24 | 2 | 2 | historical |
| `docs/lanes/mission-model.md` | 6 | 2026-10-20 | 0 | 1 | historical |
| `docs/lanes/mn-a.md` | 6 | 2026-10-27 | 0 | 1 | historical |
| `docs/lanes/mn-b.md` | 11 | 2026-10-27 | 2 | 1 | historical |
| `docs/lanes/mn-d.md` | 12 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/mn-fg.md` | 13 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/modules.md` | 28 | 2026-10-24 | 3 | 1 | historical |
| `docs/lanes/movement.md` | 64 | 2026-10-21 | 8 | 13 | historical |
| `docs/lanes/nb-12.md` | 1 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/newcomer-calls.md` | 51 | 2026-10-21 | 0 | 0 | historical, orphan |
| `docs/lanes/notif-1.md` | 13 | 2026-10-27 | 2 | 1 | historical |
| `docs/lanes/notify-spec.md` | 52 | 2026-10-20 | 2 | 2 | historical |
| `docs/lanes/npc-behaviors.md` | 6 | 2026-10-22 | 0 | 1 | historical |
| `docs/lanes/npc-chase.md` | 3 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/npc.md` | 63 | 2026-10-25 | 9 | 4 | historical |
| `docs/lanes/o19.md` | 6 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/orbit.md` | 6 | 2026-10-24 | 0 | 1 | historical |
| `docs/lanes/patch-1.md` | 7 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/perf-fight.md` | 9 | 2026-10-27 | 0 | 1 | historical |
| `docs/lanes/perf-save.md` | 4 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/planet.md` | 51 | 2026-10-25 | 6 | 3 | historical |
| `docs/lanes/plex-vault.md` | 14 | 2026-10-19 | 0 | 1 | historical |
| `docs/lanes/prog-mine.md` | 15 | 2026-10-27 | 5 | 0 | historical |
| `docs/lanes/prog-null.md` | 8 | 2026-10-28 | 3 | 0 | historical |
| `docs/lanes/prog-pve.md` | 16 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/prog-pve2.md` | 16 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/pstart.md` | 8 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/pstartb.md` | 12 | 2026-10-26 | 0 | 2 | historical |
| `docs/lanes/push-events.md` | 4 | 2026-10-19 | 0 | 0 | historical, orphan |
| `docs/lanes/q-backup.md` | 1 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/q-flake.md` | 3 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/q-flake2.md` | 3 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/q-format.md` | 1 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/q-fuzz.md` | 7 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/r-rel-1.md` | 3 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/r-rel-2.md` | 6 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/r-rel-3.md` | 9 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/r-rel-4.md` | 6 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/r-rel-5.md` | 11 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/rel-fix.md` | 5 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/reprocess.md` | 71 | 2026-10-24 | 2 | 7 | historical |
| `docs/lanes/review-fix.md` | 8 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/review-fix2.md` | 4 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/ridx.md` | 4 | 2026-10-25 | 0 | 3 | historical |
| `docs/lanes/roam-gang.md` | 12 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/roster.md` | 63 | 2026-10-21 | 1 | 1 | historical |
| `docs/lanes/round2-spec.md` | 59 | 2026-10-24 | 5 | 11 | historical |
| `docs/lanes/scan.md` | 16 | 2026-10-25 | 4 | 1 | historical |
| `docs/lanes/scrap.md` | 4 | 2026-10-26 | 1 | 0 | historical |
| `docs/lanes/sec-1.md` | 21 | 2026-10-27 | 4 | 3 | historical |
| `docs/lanes/sec-1b.md` | 5 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/sec-fix.md` | 9 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/sensor-suite.md` | 6 | 2026-10-19 | 0 | 2 | historical |
| `docs/lanes/session-and-binding.md` | 53 | 2026-10-23 | 2 | 3 | historical |
| `docs/lanes/session-gm.md` | 5 | 2026-10-28 | 1 | 1 | historical |
| `docs/lanes/settings-blob.md` | 5 | 2026-10-19 | 0 | 0 | historical, orphan |
| `docs/lanes/ship-pool.md` | 5 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/shutdown.md` | 5 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/skill-training.md` | 19 | 2026-10-25 | 2 | 0 | historical |
| `docs/lanes/skills-recon.md` | 9 | 2026-10-20 | 0 | 0 | historical, orphan |
| `docs/lanes/soc-a1.md` | 6 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/soc-a2.md` | 8 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/soc-b1.md` | 8 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/soc-b10.md` | 7 | 2026-10-26 | 1 | 1 | historical |
| `docs/lanes/soc-b2.md` | 7 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/soc-b3.md` | 7 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/soc-b5.md` | 8 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/soc-b6.md` | 9 | 2026-10-26 | 1 | 1 | historical |
| `docs/lanes/soc-b7.md` | 8 | 2026-10-26 | 2 | 0 | historical |
| `docs/lanes/soc-b8.md` | 9 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/soc-b9.md` | 7 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/soc-c1.md` | 14 | 2026-10-27 | 6 | 2 | historical |
| `docs/lanes/soc-c2.md` | 9 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/soc-c3.md` | 12 | 2026-10-27 | 2 | 2 | historical |
| `docs/lanes/soc-c4.md` | 14 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/soc-c5.md` | 18 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/soc-c6.md` | 10 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/soc-c7.md` | 11 | 2026-10-27 | 3 | 1 | historical |
| `docs/lanes/soc-sig.md` | 6 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/sov-d1.md` | 12 | 2026-10-27 | 3 | 1 | historical |
| `docs/lanes/sov-d2.md` | 16 | 2026-10-27 | 2 | 1 | historical |
| `docs/lanes/sov-d3a.md` | 12 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/sov-d3b.md` | 13 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/sov-d4.md` | 14 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/sov-d5.md` | 8 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/srv-gaps.md` | 18 | 2026-10-27 | 2 | 2 | historical |
| `docs/lanes/standings.md` | 17 | 2026-10-24 | 2 | 1 | historical |
| `docs/lanes/stargates.md` | 39 | 2026-10-24 | 8 | 3 | historical |
| `docs/lanes/start-skills.md` | 6 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/t3-a.md` | 5 | 2026-10-27 | 0 | 1 | historical |
| `docs/lanes/t3-d.md` | 5 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/t3-e.md` | 5 | 2026-10-27 | 3 | 0 | historical |
| `docs/lanes/t3-f.md` | 6 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/t3-h.md` | 9 | 2026-10-28 | 1 | 1 | historical |
| `docs/lanes/t3-price.md` | 5 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/t3-salvage.md` | 13 | 2026-10-28 | 4 | 0 | historical |
| `docs/lanes/t3-supply.md` | 11 | 2026-10-28 | 4 | 4 | historical |
| `docs/lanes/targeting.md` | 5 | 2026-10-22 | 0 | 2 | historical |
| `docs/lanes/tick-1.md` | 8 | 2026-10-28 | 1 | 0 | historical |
| `docs/lanes/travel.md` | 7 | 2026-10-26 | 0 | 1 | historical |
| `docs/lanes/two-in-space.md` | 62 | 2026-10-24 | 2 | 8 | historical |
| `docs/lanes/typedata.md` | 5 | 2026-10-25 | 1 | 3 | historical |
| `docs/lanes/unanswered-calls.md` | 69 | 2026-10-21 | 0 | 3 | historical |
| `docs/lanes/undock-points.md` | 26 | 2026-10-21 | 2 | 5 | historical |
| `docs/lanes/w105-a.md` | 5 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/w105-b.md` | 4 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/w105-c.md` | 4 | 2026-10-25 | 0 | 0 | historical, orphan |
| `docs/lanes/w105-d.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/w105-e.md` | 3 | 2026-10-25 | 0 | 1 | historical |
| `docs/lanes/wallet-ledger.md` | 10 | 2026-10-20 | 0 | 0 | historical, orphan |
| `docs/lanes/warp-destiny.md` | 7 | 2026-10-24 | 0 | 2 | historical |
| `docs/lanes/warp-min.md` | 2 | 2026-10-28 | 0 | 0 | historical, orphan |
| `docs/lanes/world-corp.md` | 14 | 2026-10-19 | 1 | 0 | historical |
| `docs/lanes/x-contract.md` | 6 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/x-econreport.md` | 3 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/x-ice.md` | 7 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/x-insure.md` | 5 | 2026-10-27 | 1 | 1 | historical |
| `docs/lanes/x-lpstore.md` | 7 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/x-pi3.md` | 7 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/x-rig.md` | 11 | 2026-10-27 | 4 | 0 | historical |
| `docs/lanes/x-salv.md` | 16 | 2026-10-27 | 5 | 1 | historical |
| `docs/lanes/x-skill.md` | 2 | 2026-10-27 | 0 | 0 | historical, orphan |
| `docs/lanes/x-trade.md` | 9 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/x2a-items.md` | 6 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/x2a-space.md` | 5 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/x2a-world.md` | 6 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/x2b.md` | 10 | 2026-10-26 | 0 | 0 | historical, orphan |
| `docs/lanes/y-combat.md` | 8 | 2026-10-27 | 3 | 0 | historical |
| `docs/lanes/y-escal.md` | 9 | 2026-10-27 | 2 | 0 | historical |
| `docs/lanes/y-roam.md` | 11 | 2026-10-27 | 1 | 0 | historical |
| `docs/lanes/y-wh.md` | 11 | 2026-10-28 | 2 | 0 | historical |

414 lane notes; 104 orphans (listed in the index now).
