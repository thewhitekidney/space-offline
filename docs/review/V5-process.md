# V5: process, docs and knowledge

Review lane V5, 2026-10-25, at `3f2741d`. Read-only: this file is the only
write. Brief: `docs/review/README.md` §2 and §3 (V5). Marks: **[V]** checked
in the tree or the git log, **[I]** inferred, **unknown** where neither.

## 0. The short version

The lane system works. 580 commits in seven days [V: `git log | wc -l`],
71 of them lane landings, and the failures in the brief (wiped scratchpad,
full disk, RAM-backed `/tmp`) each became a rule within hours (rules 9-11).
What it costs us now is **knowledge flow, not code flow**:

1. **The human-check queue is write-only.** 43 numbered items in §8 plus
   sections 2-7. Three have been answered in five days. 23 commits only added
   entries. It is about a day of playing, not a morning. Forty-odd features
   that friends would touch have never been seen by a human on a real client.
2. **No one place says what is current.** The queue moved from
   `docs/archive/plan-subsystems.md` (W0-W9) to `content/plan.md` (W10.5-W18). W10 has no
   plan at all. Six demotion banners still point at `docs/archive/plan-subsystems.md` as
   "current truth", and its own baseline, code column and "read only one
   thing" are stale. That is the mechanism behind the two phantom packets
   already recorded (tracker §60.1, §61.1).
3. **Packet ids collide** across plans: `K2`, `L1` and `S1` each name two
   different things. A brief that says "packet L1" can send a lane to the
   wrong spec.
4. **A fresh agent has no front door.**
   - There is no `CLAUDE.md`, no `docs/README.md` and no `docs/re/README.md`.
   - The root `README.md` says "No live server yet". Five of the eight docs
     it links are demoted.
   - The auto-memory's "next direction" sends the agent to a demoted plan
     and to a job (the stamp allocator) that tracker §36 retired.

None of this needs code. The fixes are six doc packets and one stdlib lint
that the gate already knows how to run.

## 1. The lane system: rules 1-11 and what they miss

### 1.1 What works, and should not change [V]

- **Rule 1 (files, not subsystems) and rule 2 (remove the shared file).**
  These rules raised wave width from 2-4 lanes (W1-W5) to 10 (W8). Twelve
  `Register ...` commits pre-create modules, so no lane touches `lib.rs`
  (`git log --format=%s | grep -c '^Register'` = 12).
- **Rule 5 (park with a note).** The code cites lane notes 295 times
  (`grep -rn 'lanes/' server/crates`), so the notes are load-bearing.
- **Rule 6 (the brief is the whole context), with a template.**
  `content/plan.md` §10 is the best artefact in the process. It turns rules
  1, 4, 5 and 8-11 into the brief text itself.
- **The red-team step.** `content/plan-review.md` §5 caught four false
  claims before they became packets. Examples: `Registry::touch_system`
  "does not exist" (:526), and "the gate re-checks it" was false. Keep red
  teams for every multi-lane plan.

### 1.2 The rules file itself has decayed [V]

`docs/lanes/README.md`:
- **Out of order.** Rules 1-8 are followed by "What a lane is good for" and
  "Calibration". Then a stray bullet about `docs/evejs/review/README.md` (:116, an index
  entry that landed mid-file). Then rules 9-11 (:118-139). A lane skimming
  for "the rules" stops at Calibration and misses 9-11. Those three rules
  are the ones that cost a disk, a scratchpad and RAM.
- **The hot-files register is stale** (:52-58):
  - `service.rs` is described as holding "the original ~2,200-line test
    module". The tests now live in `service/tests.rs`, at 4,480 lines.
  - `conn.rs` is described as "the push path is being built here". It was
    built at tracker §21.
  - Files that are hot in practice are missing: `lib.rs` (the coordinator
    registers modules), `push.rs`, `store.rs` and `notify.rs` (W8's
    shared-file list, `docs/archive/plan-subsystems.md` §W8), and the coordinator-only
    docs (`port-tracker.md`, `docs/guides/human-checks.md`, the plans).
  - Two registers disagree, and that is duplicated truth.

### 1.3 What the rules miss (each is a failure already paid for)

| # | Gap | Evidence | Cost if left |
|---|---|---|---|
| G1 | **Packet ids are not unique.** | **[V]** `K2` is both C1's `content.py` phase 1 (`C1-corpus-map.md:428`, built as CP1) and C2's `new_style` helper (`C2-live-surface.md:650`, built as CP7). `L1` is both W9's combat loose ends (`docs/archive/plan-subsystems.md:763`) and C7's seed module (`C7-liveness.md:616`, built as CP4). `S1` is both the synthesis lane (`f293520`) and seam S1 (`content/plan.md:614`). The brief's "lanes reused packet names". | A brief or a commit message says "L1" and a lane reads the wrong spec. That is one lost lane round per collision. |
| G2 | **"Verify before cite" lives in a plan, not in the rules.** | **[V]** `docs/archive/plan-subsystems.md` §2b states it after two phantom packets (tracker §60.1, §61.1). It is not in `lanes/README.md`, and that file is the only one a brief tells a lane to read. | It recurs. "Plans cited tests that did not exist" (brief §1) is the same failure, one level down. |
| G3 | **A plan's test citations are not checked.** | **[V]** `plan-review.md:526-528`: three claimed facts about code were false. Plan rows mix "existing test passes" with "new test" in one column (`content/plan.md:614`). | A lane is told to keep a test green that does not exist. It either invents one or stops. |
| G4 | **Session-specific paths are committed as process text.** | **[V]** `content/plan.md:945,953` and `review/README.md:88` hard-code `/tmp/claude-1000/.../bad11cfc-.../scratchpad`. 21 files carry the literal. | The next session pastes a template whose rule 10 path is wrong. Lanes then write into a dead directory, or create one that nothing cleans. |
| G5 | **Lane notes have no fixed header.** | **[I]**, from a grep heuristic: 19 of 98 notes have no "read first / next" pointer, and 30 cite no commit sha. W11 notes do this well (`cp1.md:3-4`: plan row, spec, starting sha); older ones vary. | A fresh lane cannot tell what state a note describes. The note becomes an anchor-less claim (G2 again). |
| G6 | **A demotion does not follow its successor.** | **[V]** Six banners name `docs/archive/plan-subsystems.md` as current truth (`docs/archive/OVERNIGHT_PLAN.md:3`, `docs/archive/plan-overnight-8h.md:3`, `docs/archive/plan-overnight-261021.md:3`, `docs/archive/plan-overnight-2026-10-23.md:3`, `docs/archive/recon-v24.01.md:3`, and the queue column of `docs/archive/plan-subsystems.md` §2). The queue has since left that file. | Each banner adds one hop to a stale page. |

### 1.4 The coordinator's bottlenecks [V]/[I]

What only the coordinator does, from the log:
- registering modules (12 commits);
- applying coordinator diffs (`8cb1fb8` and others);
- landing each lane through `land.sh --head`;
- copying human checks out of lane notes into `docs/guides/human-checks.md` (23
  `human-checks:` commits);
- appending to `port-tracker.md` (41 commits since 10-24) and keeping
  `docs/archive/plan-subsystems.md` current (33).

- **Registration and hunks: fine as they are.** They are minutes each, and
  they buy the wave width [I].
- **Transcribing human checks: avoidable.** The lane already wrote the check
  in its note. Proposal P6 has the lane write it in final form, so the
  coordinator pastes a block instead of re-authoring it.
- **Keeping three planning layers coherent: the real bottleneck.** `roadmap`
  is the milestone layer, `plan-subsystems` the packet layer, and
  `content/plan` the next wave plan. Only one of them is kept current at a
  time, and the other two drift (§2). Proposal P2 collapses "which plan is
  live" into one table that is cheap to keep up.
- **unknown:** how much coordinator wall time goes to hunks compared with
  waiting for lanes. Commit timestamps cluster at landing and do not show it.

## 2. Docs: sprawl, duplicated truth, and the catch-up path

### 2.1 Size [V]

| Area | Files | Lines |
|---|---|---|
| `docs/*.md` | 45 | 37,785 |
| `docs/lanes/` (incl. `evejs/`) | 103 | 44,086 |
| `docs/re/` | 19 | 16,373 |
| `docs/content/` | 12 | 8,656 |
| `docs/evejs/inventory/` | 7 | 3,944 |
| **total** | **188 tracked** | **~111k**, against ~160k lines of Rust |

### 2.2 Paths are load-bearing: code cites docs 1,370 times [V]

`grep -rhoE 'docs/[A-Za-z0-9_/.-]+\.md' server tools content | wc -l` =
1,370. The top targets:

| Doc | Citations from code |
|---|---|
| `port-tracker.md` | 200 (132 of them by section number) |
| `docs/reference/world-state.md` (demoted) | 100 |
| `lanes/fleet.md` | 50 |
| `docs/reference/login-path.md` | 46 |

**This decides the archive question.** Nothing may move or be renumbered.
Archiving means a banner plus a status in an index, never `git mv`.

### 2.3 Duplicated or contradicting truth [V]

| Claim | Where it is stale | Where it is true |
|---|---|---|
| "where are we" | `port-tracker.md:20-43`: "Round 1: three landed", "human-checks has four entries" | the log. Waves W8-W11 have landed, and human-checks has 43 entries |
| live queue | `docs/archive/plan-subsystems.md` §6 stops at W9. `content/README.md:20` says the packets are filed there, and they were not | `content/plan.md` §6 (W10.5-W18) |
| baseline | `docs/archive/plan-subsystems.md:44-50`: 69,066 lines, 1,056 tests | `review/README.md` §1: about 160k lines, about 1,800 tests |
| per-subsystem code | `docs/archive/plan-subsystems.md` §3 lists modules, damage, NPC, dungeons and mining as "**none**" and jump gates as "unserved" | built in W2-W4 and W8 (`modules.rs`, `combat.rs`, `npc.rs`, `asteroid.rs`, `jump.rs` exist) |
| "if you read only one thing" | `docs/archive/plan-subsystems.md:812-817`: "P0.1, then P1.1" | both landed (W0 is "COMPLETE 2026-10-24", :293) |
| client CI | `docs/archive/roadmap.md:248`: 7 scenarios | 8 scenario files in `tools/client-ci/scenarios/` |
| project status | root `README.md:19-23`: "No live server yet". `:71-90` links `entering-the-game`, `recon-v24.01`, `wire-format`, `session-flow` and `evejs-261018-reference`, all demoted | the code |
| next job (memory) | `memory/eve-offline-next-direction.md`: "read docs/archive/plan-overnight-261021.md", "next job is the stamp allocator" | tracker §36: the allocator was unnecessary. That plan is demoted |
| tracker numbering | §19.9 and §19.10 sit after §20 (`port-tracker.md:2184,2232`) | cosmetic. Do not renumber (§2.2) |

### 2.4 Orphans [V]

31 tracked docs have no inbound reference from any other doc. The script
greps each basename across `docs`, `README.md` and `content`. The orphans
include:
- all ten W11 notes (`lanes/cp*.md`);
- all five W10.5 notes (`lanes/w105-*.md`);
- `dogma-engine.md`, `crimewatch.md`, `skill-training.md` and
  `breaker-sweep.md`;
- `docs/reference/unattended-root.md`.

They are reachable only by `ls` or from code comments. An index fixes this
without touching any of them.

### 2.5 More demotion candidates (not yet bannered)

- **[I]** `docs/archive/plan-docked-subsystems.md` (10-19, round 2 is done),
  `docs/archive/plan-the-churn.md` (10-19, waves done) and `docs/evejs/inventory/*` (the EvEJS
  inventory pass, consolidated into tracker §§8-11). These are history:
  banner them and point them at the index.
- **[I]** `docs/archive/roadmap.md` "Where the testing layer stands" and
  `docs/evejs/evejs-subsystem-gap.md` status column (it still reads "one hand-built
  mission", :153). Refresh or banner them.
- **unknown:** `docs/archive/architecture-sweep.md` (10-19). It says items "move out by
  being done or dropped", but nobody has checked which have. V1 may settle
  it.

### 2.6 How a fresh agent catches up today [V]

There is no `CLAUDE.md`, so nothing loads automatically. The paths in are:
- the root `README.md`, which is stale (§2.3);
- the auto-memory, which is stale (§2.3);
- whatever the brief names.

A fresh **lane** is fine, because the §10 template carries everything. A
fresh **coordinator** has to assemble current truth from:
- `lanes/README.md` (139 lines);
- `docs/archive/plan-subsystems.md` (817);
- `content/plan.md` (988);
- the tail of `port-tracker.md` (7,490);
- `docs/guides/human-checks.md` (1,204).

That is about 4k lines minimum, and two of those files contradict each
other (§2.3). A coordinator who skips the content plan briefs from W9's
world.

## 3. The human-check queue: not one morning's shape

### 3.1 Size and flow [V]

- **Sections:** §1-§7 are per-event sections from 10-21. §7 alone is a
  14-step script. §8 is the batch, items 0-43.
- **Inflow:** 23 commits whose subject starts `human-checks:`, plus entries
  added inside lane commits. Items 24-43 were added on 2026-10-25 alone.
- **Outflow:** three answered (§1 DONE, §8.1 and §8.3a ANSWERED,
  :29,704,739). Nothing in 4-43 is marked.
- **Shape:**
  - 31 lines in §8 call for two clients or Bob.
  - 18 call for a relaunch, a regenerated extract, or a first run.
  - The summary table at :19-25 still lists only items 1-6.
- **Time [I]:** at about 10 minutes per item, which is generous for items
  like 36 and 37 with their many sub-steps, §8 alone is about 7 hours. The
  owner's sitting is "the morning" (`memory/eve-offline-overnight-autonomy`).

### 3.2 Items are superseded by later items [V]/[I]

- #7 "board a ship and fit a gun (P2.0)" checks code that #32 (every module)
  and #40 (dogma engine) have since replaced.
- #12, #17 and #20 (first blood, ammo, turrets) are one two-client fight.
- #19, #34 and #42 are all loot.
- #6 "rocks in the belt" is inside #39 "belt rats".

A queue that only grows also accumulates checks of code that no longer
exists in that form.

### 3.3 What a machine could take [I]

A check that asks "the window opens, no traceback, the number equals X" is a
`client-ci` scenario, not a human question. Today there are 8 scenarios.
Candidates by item:
- 5, 8, 9 and 10 (docked UI);
- 13 and 23;
- 25, 29 and 33 (skills: count, inject, refuse);
- 35, 38 (the quote equals the bill: 2,040 / 1,700) and 43.

That is about 12 items. Only these need a human:
- the look (#2, real GPU);
- the feel (#3, "aligning forever");
- the bot's "wingman feel" (#41);
- multi-client PvP judgement.

This overlaps V4's real-client CI brief. V5 sizes the queue, and V4 owns the
scenarios.

## 4. Proposed doc structure and index

**Shape:** keep every path. Add three small files, give every doc one role,
and let the index say which docs are history.

```
CLAUDE.md                  NEW, at most 60 lines. Auto-loaded by Claude Code.
                           House rules (std-only, Docker only, no client
                           data, [A] never in a test, lanes own files), the
                           three commands (dev.sh, gate.sh, land.sh --head),
                           and "read docs/README.md, then docs/STATUS.md".
docs/README.md             NEW: the index. One table row per tracked doc:
                           path | role | status | supersedes/superseded-by
                           | owner. Roles: STATUS, PLAN, SPEC (re/, wire,
                           login, push-path, ...), JOURNAL (port-tracker,
                           lanes/*), RUNBOOK (debugging-the-client, jail,
                           human-checks, evejs-run-safely), HISTORY (the 16
                           demoted docs plus §2.5). It starts with "Start
                           here: 5 docs". It also holds the series-letter
                           registry (P5).
docs/STATUS.md             NEW, at most 150 lines, rewritten in place at each
                           wave end (plan-subsystems §7, step 4). It holds:
                           - the baseline numbers;
                           - the wave table: wave -> plan doc -> state ->
                             landed shas;
                           - per subsystem: canonical spec, code, and state
                             (it absorbs plan-subsystems §3, which goes
                             stale there);
                           - open owner decisions;
                           - a pointer to the human-check card.
docs/packets.tsv           NEW: id, series, plan doc and section, lane note,
                           landing sha, state. One row per packet ever
                           issued.
port-tracker.md            Unchanged as a JOURNAL: append-only, numbering
                           frozen. Its header table (:20-43) is replaced by
                           "status: STATUS.md". New rule: when a section's
                           claim closes, put a one-line "-> closed in §N /
                           <sha>" at the head of that section (the §2b
                           "strike as it closes" rule, done at the source).
docs/archive/plan-subsystems.md         Stays a PLAN for W0-W9. §1, §3 and §9 get a
                           banner pointing at STATUS.md. §6 ends with
                           "W10.5+ -> content/plan.md, review/plan.md".
```

**Archive list:** banner only, no moves.
- The 16 docs already demoted.
- Plus `docs/archive/plan-docked-subsystems.md`, `docs/archive/plan-the-churn.md` and
  `docs/evejs/inventory/*.md` (7 files).
- `docs/archive/roadmap.md` stays a PLAN, but its testing-layer section gets a dated
  banner.

## Proposals

1. **Add a front door.**
   - `CLAUDE.md`, `docs/README.md` (the index) and `docs/STATUS.md`.
   - Fix root `README.md:19-23` and `:69-90`: point it at the index, and
     drop the demoted links.
   - Effect: a fresh agent reaches current truth in under 300 lines, not
     about 4k, and the 31 orphans get an inbound link.
   - Size S. Behaviour-neutral.
2. **One "what is live" table.** The wave table in `STATUS.md`:
   - repoint the six "current truth: docs/archive/plan-subsystems.md" banners at
     `STATUS.md`;
   - fix `content/README.md:20`;
   - banner `docs/archive/plan-subsystems.md` §1, §3 and §9.
   - Size S. Behaviour-neutral.
3. **Rewrite `lanes/README.md` in order.**
   - Rules 1-11 contiguous, then "good for / bad for", then calibration.
   - The evejs bullet moves to the index.
   - The hot-files register is refreshed, with `lib.rs`, `push.rs`,
     `store.rs`, `notify.rs` and the coordinator docs added. The W8 list in
     `docs/archive/plan-subsystems.md` then points to it.
   - Size S. Behaviour-neutral.
4. **New lane rules 12-16.**
   - **12: packet ids are unique and always written with their series**
     (`W9-L1`, `C7-L1`, `C1-K2`). New series letters are claimed in
     `docs/README.md` before a plan uses them. Every issued id gets a row in
     `packets.tsv`. (G1)
   - **13: verify before cite.** A plan, brief or note that states a fact
     about the code cites `file:line` checked at writing time, or marks it
     [I]. A tracker section is a dated record, not a current claim. This
     promotes `docs/archive/plan-subsystems.md` §2b. (G2)
   - **14: a test citation says whether it is new.** It reads either
     `existing: path::to::tests::name` or `new: name`. The lint checks
     every `existing:` against the tree. (G3)
   - **15: no session paths in committed process text.** Briefs and plans
     write `$SCRATCH/<lane>`. The coordinator defines `SCRATCH` once in the
     brief preamble. (G4)
   - **16: a lane note opens with a fixed header.** Packet id, starting
     sha, owned files, tests added, "read first", "not done", and, when the
     work is player-visible, a **Human check** block written in the
     card's format (P6). (G5)
   - Size S. Behaviour-neutral.
5. **A doc lint the gate already runs.** Add `tools/test_docs.py`, stdlib
   `unittest`. `gate.sh:84-91` already runs every `tools/test_*.py` with
   `python3 -I`, so there is no gate edit. It asserts:
   - (a) every `docs/...md` path cited in `server/`, `tools/` and `docs/`
     exists (1,370 citations, pinning §2.2);
   - (b) every tracked `docs/**/*.md` has a row in `docs/README.md`;
   - (c) every row with role HISTORY has a `> **Demoted` banner in its
     first 5 lines;
   - (d) `packets.tsv` ids are unique;
   - (e) no file outside `docs/lanes/` contains `/tmp/claude-` (rule 15).
     Old lane notes are left alone as history;
   - (f) every `existing:` test citation in `docs/` names a `fn` that exists
     (rule 14);
   - (g) the human-check card's minutes sum to at most 60.
   - Size S-M. Behaviour-neutral; no Rust.
6. **Reshape `docs/guides/human-checks.md`.** It keeps one path and gets three parts:
   - **Card**: at most 6 items, at most 60 minutes, one launch config. The
     first card should be the end-goal path in one two-client-plus-bots
     session at a belt: #32 fit, #40 afterburner, #39 belt rats, #42 and
     #34 loot, #12/#20 PvP, #41 escort bot, #37 log-off timer.
   - **Backlog table**: id | feature | sha | needs (1c/2c/bots/regen) | min
     | automatable? | state. Superseded items are merged (§3.2). About 12
     automatable items are marked for V4's scenarios (§3.3).
   - **Results log**: date | id | verdict | follow-up sha. Answered items
     leave the backlog. Sections §1-§7 become one line each in the log, or
     go to HISTORY.

   The coordinator refills the card from the backlog after each sitting.
   Size M (one file, 1,204 lines to triage). Behaviour-neutral.
7. **Refresh the auto-memory.** This one is the coordinator's job, because
   the memory is outside the repo.
   - Replace `eve-offline-next-direction.md` with a three-line pointer to
     `docs/STATUS.md`.
   - Make `MEMORY.md` say "current state: docs/STATUS.md" and nothing more
     specific. A memory that restates state goes stale exactly as fast as
     the tracker header did.
   - Size S.

## Packets

Owned files do not collide. Order: PK1, then PK2-PK5 in parallel, then PK6.
No Rust and no gate change. Every packet is behaviour-neutral for the
server.

| Packet | Owns | Tests and checks | Size |
|---|---|---|---|
| **PK1** front door (P1, P2) | `CLAUDE.md` (new), `docs/README.md` (new), `docs/STATUS.md` (new), root `README.md`, `docs/port-tracker.md` lines 1-45 only, `docs/content/README.md:20` | `docs/README.md` lists all 188 tracked files. The `STATUS.md` baseline is re-measured (line count, `cargo test` count from a lane copy per rules 9-11). Manual: a fresh agent given only `CLAUDE.md` names the live wave plan | S |
| **PK2** rules (P3, P4) | `docs/lanes/README.md`, `docs/content/plan.md` §10 only (`$SCRATCH`, the rule 14/16 wording) | Rules numbered 1-16 with nothing interleaved. The template has no literal session path (lint e, once PK4 lands) | S |
| **PK3** banners (P2, §2.5) | `docs/archive/plan-subsystems.md`, the six banner lines (`docs/archive/OVERNIGHT_PLAN.md`, `docs/archive/plan-overnight-8h.md`, `docs/archive/plan-overnight-261021.md`, `docs/archive/plan-overnight-2026-10-23.md`, `docs/archive/recon-v24.01.md`, `docs/reference/world-state.md`), `docs/archive/plan-docked-subsystems.md`, `docs/archive/plan-the-churn.md`, `docs/evejs/inventory/*.md`, `docs/archive/roadmap.md` testing section | lint (c) passes | S |
| **PK4** lint (P5) | `tools/test_docs.py` (new), `docs/packets.tsv` (new, seeded from the plans' packet tables with series prefixes) | `python3 -I tools/test_docs.py` is green on the PK1-PK3 tree. One synthetic negative per assertion (a temp tree with a missing path, a duplicate id, a stray `/tmp/claude-`) | S-M |
| **PK5** human-check reshape (P6) | `docs/guides/human-checks.md` | lint (g). Every pre-reshape item number is either in the backlog or in the results log, so none is lost (a count check in the lane note) | M |
| **PK6** memory (P7) | the coordinator, outside the repo | none: owner's files | S |

## Do not do

- **Do not move docs into `docs/archive/`, rename them, or renumber the
  tracker.** 1,370 code-to-doc citations, 132 of them by tracker section,
  would break or silently mispoint. A banner plus the index's HISTORY role
  does the same job at no cost.
- **Do not delete the demoted docs.** Code cites them (`docs/reference/world-state.md`
  100 times), and their findings are often still the only `[V]` source.
  Only their status lines are wrong.
- **Do not merge `docs/re/X.md` with `docs/lanes/X.md`.** There are 11 such
  pairs. A spec and a lane journal are different roles on purpose, so the
  index says which is which.
- **Do not split `port-tracker.md` into per-topic files.** Its value is the
  append-only, dated sequence (§2b). Splitting it breaks 200 citations and
  the sequence together. `STATUS.md` takes the "where are we" job off it
  instead.
- **Do not add a docs site, mdBook or a link checker crate.** That adds
  dependencies for a std-only project. The stdlib lint covers what matters.
- **Do not auto-generate `docs/guides/human-checks.md` from lane notes yet.** A parser
  over 98 free-form notes is M-L and brittle. Rule 16's fixed block makes
  the coordinator's transcription a paste. Revisit if transcription still
  shows up as a cost after a wave.
- **Do not add a review gate for every lane.** The red team pays for
  multi-lane plans, not single-file packets. The gate plus spot-checks
  (lanes/README "Calibration") is the right level there.
- **Do not grow the rules past 16.** Prefer making a rule a lint (P5) or
  brief text (the §10 template) over adding prose.

## unknown

- The real minutes per human check. Nothing records how long the owner's
  sittings take. Time the first card and correct P6's 60-minute bound.
- How much coordinator wall time goes to hunks, registration and
  transcription, compared with waiting on lanes (§1.4).
- Whether the owner reads `port-tracker.md` or only `docs/guides/human-checks.md` and
  chat. If it is only the latter, `STATUS.md` matters more than estimated.
- Which items in `docs/archive/architecture-sweep.md` are still open. V1 may settle
  this.
- Whether a root `CLAUDE.md` conflicts with any owner preference. Nothing
  in memory says so. It is an owner nod, not a lane call.
