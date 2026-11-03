# The subsystem implementation plan

> **Demoted 2026-11-01** — waves W0-W9 finished long ago. Current truth: `docs/plans/master-plan.md`. Kept because §2a (the build pin) and §2b (verify before citing) are cited from code and tools.

**Written 2026-10-24**, from four read-only survey passes over this repository:
every file in `docs/lanes/` (62), every file in `docs/re/` (19) and `docs/`
(41), the whole of `server/crates/eve-server/` as built, and every open item
`docs/port-tracker.md` and `docs/guides/human-checks.md` leave behind.

## 0. What this is, and what it is not

There are now three planning layers and they answer different questions.
Confusing them has cost this project rounds before, so they are named here
once:

| document | question it answers | unit |
|---|---|---|
| `docs/archive/roadmap.md` | where are we going, and in what order | milestone |
| `docs/evejs/evejs-subsystem-gap.md` | how much game exists to build | subsystem |
| **this file** | what is the next thing a lane can pick up cold | **packet** |

This file does **not** supersede either of the other two. It sits under them:
the roadmap's spine (steps A–E) is complete, and this is the layer the spine
deliberately deferred.

**It does supersede the queues** of `docs/archive/plan-overnight-8h.md`,
`docs/archive/plan-overnight-261021.md`, `docs/archive/plan-overnight-2026-10-23.md` and
`docs/archive/OVERNIGHT_PLAN.md`. Those four were written as run queues for specific
nights; three already declare each other superseded, and none describes the
tree as it stands. Their *findings* remain good. Their *queues* are dead.

**The roadmap's "why not system-by-system" argument has expired, and that is
why this file can exist.** It rejected subsystem-by-subsystem work on two
premises: that every window opens another layer of calls (an unbounded tree),
and that answers had to be invented. Both moved. 139 of 149 native tables now
decode with CCP's own field names, all five dogma tables including 5,178
modifier rules, 2,892 missions joining into 5,516 dungeons with room layouts.
A large share of what used to be invention is now **lookup**, and a lookup is
plannable where an invention is not.

## 1. The measured baseline

Everything below is counted, not estimated.

```
server/crates/eve-server/src/        69,066 lines     700 unit tests
workspace                                           1,056 tests
machoNet services advertised             94
  REAL   (reads or mutates state)        26
  STUB   (constant or empty)             63
  ERROR  (deliberate UserError)           1  + 4 mixed
celestials extracted             477,082 in 8,490 systems
```

**63 stubs is not a backlog of 63 items.** Many are argued in their own doc
comments as the *truthful* answer — `fleetProxy.GetAvailableFleetAds`
returning `{}` is correct, because there are no fleet ads. §5 of this file
gives the filter that separates a stub-by-design from a stub that blocks a
feature. Applying it is the first thing to do before quoting 63 at anybody.

## 2. Before citing any document: the demotion list

A survey pass found **16 documents that are stale, superseded, or contradict
themselves.** This matters more than it sounds: a packet brief is the whole
context a lane will ever have (lane rule 6), so a brief that cites a demoted
document sends a lane to do the wrong work. That has already happened at
least three times in this project — two lanes briefed to build something that
was already built, and one pointer to `docs/reference/world-state.md` §7, which does not
exist.

**Do not cite these as the state of anything.** Their findings may still be
good; their status claims are not.

| document | what is wrong | use instead |
|---|---|---|
| `docs/archive/session-flow.md` | self-declared superseded; 3 own claims wrong, 2 still wrong | `docs/reference/login-path.md` |
| `docs/archive/OVERNIGHT_PLAN.md` | queue superseded twice | this file |
| `docs/archive/plan-overnight-8h.md` | §3 queue superseded | this file |
| `docs/archive/plan-overnight-261021.md` | queue superseded and self-reported exhausted | this file |
| `docs/archive/plan-overnight-2026-10-23.md` | newest plan, but §(line 70) self-marked obsolete; says the roadmap is stale | this file for the queue |
| `docs/archive/marshal-tags.md` | gap 1 closed elsewhere; DBRow tag `0x2A` not `0x2B` | `docs/reference/object-wrapper.md`, `docs/reference/dbrow.md` |
| `docs/archive/wire-format.md` | gap 3 closed; DBRow gap superseded | `docs/reference/framing.md`, `docs/reference/dbrow.md` |
| `docs/archive/destiny-and-the-ballpark.md` §4.4 | 3 rows of the ball table wrong | `re/destiny-ball-crosscheck.md` §2 |
| `docs/archive/evejs-261018-reference.md` | 8 of its claims did not survive cross-check | `re/evejs-crosscheck.md` |
| `docs/archive/recon-v24.01.md` | pins a build the live Steam copy has left | §2a below |
| `docs/reference/world-state.md` | "the server persists nothing" — it does | `store.rs`, `docs/reference/server.md` |
| `docs/archive/entering-the-game.md` header | 2026-10-19 status snapshot | the code |
| `re/static-corpus.md` §6.1 | room/object offsets superseded | `re/dungeon-model.md` |
| `re/warp.md` :153 | carries its own SUPERSEDED banner | `port-tracker.md` §§47–56 |
| `docs/archive/plan-bots.md` :3 | "nothing here is built yet" — milestones 2–4 are, 1 is half built (character selection), 5 has no status | its own milestone notes |
| `docs/archive/plan-client-ci.md` :10 | "a plan, not a built thing" — `tools/client-ci.sh` exists | `lanes/ci-harness.md` |

**W7 of this plan is to put a banner on each of these**, because a demotion
list in one file is weaker than a banner in sixteen. Until then, this table is
the authority.

### 2b. The rule that cost two lane rounds on day one

§2's demotion list guards against stale *documents*. It did not stop two
phantom packets in the first parallel round, because `port-tracker.md` is not
stale — it is **append-only by design**, and that is what makes it worth
having. A section is a true record of what was open *the day it was written*.

Both phantoms came from one section, §15.6, a round-end summary whose five
bullets included two that had been closed days later without the bullet being
struck. P1.7 had shipped in `95e2e41`; P1.8's skills in `07a58d9`.

**So: a port-tracker section is evidence of what was once true. Before a
section becomes a packet, verify the claim against the code.** One grep closed
each of these — `take_session_changes`, and the skill names in `skill.rs`.

And the half that prevents it rather than detecting it: **a list of open items
is a liability proportional to its age.** Strike each line as it closes, in the
round that closes it, by whoever closes it. Both bullets are struck now.

### 2a. The build pin, and a liability nobody is guarding

`docs/reference/client-architecture.md` warns that `start.ini` reads `build = 3628513`, which
would invalidate all 19 `docs/re/` files. **Measured 2026-10-24 — it does not
apply to what we run:**

| install | build |
|---|---|
| `tq-offline` (what the jail launches) | **3619879** |
| `tq-offline-2` (second client) | **3619879** |
| `tq` (live Steam) | 3628513 |
| `sisi` | 3440966 |

Steam updated `tq`; our two copies are untouched, which is what jailing them
was for. **But nothing protects them.** A Steam verify, a reinstall, or a
disk-space sweep takes the pinned build with it, and every `[V]` mark in this
repository is measured against it. Checksumming or copying `tq-offline` out of
`compatdata` is a packet in W0 and it is cheap insurance.

## 3. Source of truth per subsystem

One canonical document per subsystem, so a brief has exactly one thing to
cite. `IMPL-SPEC` means implementable as written; the survey graded 29 of 62
lane docs that way.

| subsystem | canonical doc | grade | code today |
|---|---|---|---|
| framing / marshal | `docs/reference/framing.md`, `docs/reference/dbrow.md`, `docs/reference/object-wrapper.md` | IMPL-SPEC | `eve-marshal`, `wire.rs` |
| login / handshake | `docs/reference/login-path.md` | IMPL-SPEC | `handshake.rs`, `dispatch.rs` |
| sessions / binding | `re/session-and-binding.md` | IMPL-SPEC | `session*.rs` |
| push path | `docs/reference/push-path.md` | IMPL-SPEC | `push.rs`, `notify.rs` |
| identity / accounts | `lanes/account-spec.md` | IMPL-SPEC | **decision pending** |
| Destiny ball blob | `re/destiny-ball-crosscheck.md`, `lanes/addballs.md` | IMPL-SPEC | `destiny.rs` |
| motion / warp / orbit | `port-tracker.md` §§46–56, `re/warp.md` | IMPL-SPEC | `space_state.rs`, `orbit.rs`, `warp_in.rs` |
| hull constants | `lanes/hull-numbers.md` | IMPL-SPEC | `destiny.rs::hull` |
| undock geometry | `lanes/undock-points.md` | IMPL-SPEC | `docking.rs` |
| targeting | `re/targeting.md` | IMPL-SPEC | `targeting.rs` |
| **modules / activation** | `re/modules.md` | IMPL-SPEC | **none** |
| **damage / death** | `re/damage-death.md` | IMPL-SPEC | **none** |
| **NPC behaviour** | `re/npc-behaviors.md` | IMPL-SPEC | **none** |
| **jump gates** | `re/jump-gates.md` | IMPL-SPEC | `CmdStargateJump` unserved |
| **asteroids / mining** | `docs/evejs/review/features.md`, §35.6 | IMPL-SPEC | **none** |
| missions / agents | `re/mission-model.md`, `re/agents-missions.md` | IMPL-SPEC | `mission.rs` (one mission) |
| dungeons | `re/dungeon-model.md` | IMPL-SPEC | **none** |
| dogma | `re/dogma.md` | REFERENCE | tables only |
| industry | `lanes/industry.md` | IMPL-SPEC | `industry.rs` + service |
| reprocessing | `lanes/reprocess.md` | IMPL-SPEC | `reprocess.rs` + service |
| planetary interaction | `lanes/planet.md` | IMPL-SPEC | `planet.rs`, **read-only served** |
| market | `lanes/market.md`, `docs/reference/market-browse.md` | DESCRIPTIVE | `service/market.rs`, mutators refused |
| wallet / ledger | `wallet-ledger.md` | IMPL-SPEC | `ledger.rs` |
| skills | `docs/reference/skill-system.md` | IMPL-SPEC | `skill.rs` |
| fleet | `lanes/fleet.md` | IMPL-SPEC | `fleet.rs` + service |
| items / spawn | `item-spawn.md` | IMPL-SPEC | `item.rs` |
| localisation | `re/localization.md` | REFERENCE | **none** |
| errors | `re/errors.md` | REFERENCE | ad hoc |
| static data (cFSD) | `re/native-loaders.md`, `re/static-corpus.md` | REFERENCE | `tools/fsd.py` |
| client CI | `lanes/ci-harness.md`, `lanes/ci-scenarios.md` | IMPL-SPEC | `tools/client-ci.sh` |
| jail | `docs/guides/jail.md` | IMPL-SPEC | `tools/jail-*.sh` |
| gRPC gateway | §10.4, §41.14 | **none** | **none** |
| chat | `lanes/chat-petitions.md` | DESCRIPTIVE | stub |

## 4. The parallelism ceiling, measured — and why W0 exists

`docs/lanes/README.md` rule 1 is that **a lane owns files, not subsystems**,
and rule 2 is that a shared file is removed rather than scheduled, because
"the shared file is the finding". Both rules are right and both are
load-bearing. So the question "how many packets can run at once" is really
"how many disjoint file sets are there", and that is measurable.

Churn over the last 120 commits touching `server/`:

| file | lines | commits | share |
|---|---|---|---|
| `service.rs` | 8,942 | **54** | **45%** |
| `destiny.rs` | 4,850 | 24 | 20% |
| `dispatch.rs` | 1,713 | 22 | 18% |
| `tests/loopback.rs` | 4,124 | 21 | 18% |
| `eve-bot/tests/handshake.rs` | 1,641 | 19 | 16% |
| `lib.rs` | 99 | 18 | 15% |
| `space_state.rs` | 1,616 | 17 | 14% |
| `service/space.rs` | 2,388 | 16 | 13% |

Two conclusions, and they shape every wave below.

**`service.rs` is in nearly half of all commits, and 48% of it is the shared
test module.** `#[cfg(test)] mod tests` starts at line 4,669 of 8,942, so
4,274 lines of tests sit in a file every lane needs. The lane convention
already flags this ("the original ~2,200-line test module, which is still
shared" — it has since grown to 4,274). **This is the parallelism ceiling.**
Splitting it is worth more than any feature in this plan, by rule 2's own
logic: the three previous shared-file removals were each "worth more than the
round they unblocked".

**The space cluster is one ownership unit, not five.** `destiny.rs`,
`space_state.rs`, `service/space.rs`, `orbit.rs`, `warp_in.rs` and both
integration suites move together — every motion change in §§46–56 touched
three or more of them. So **all warp, orbit and motion work is a single
serial lane.** Do not plan the y-damping, the `WarpTo` flip and the orbital
plane as three parallel packets; they are three steps of one.

`tests/loopback.rs` and `eve-bot/tests/handshake.rs` are the third
contention zone, and they are shared by construction — any packet that
changes observable behaviour touches one of them. A file-list gate (P0.2)
reduces the pain; nothing removes it.

### 4a. What `Registry` is, and why it is not next

P0.5 cut the upward coupling from 44 references to 16. The sixteen that remain
are **not** the same kind of mistake:

```
crate::service::Registry            6
crate::service::ServiceError        5
crate::service::world_write         2
crate::service::max_locked_targets_of 1
```

`Registry` is a god object by the usual measure — 15 fields, 54 public
methods, and the process's only `Arc<RwLock<_>>` — and splitting it is the
obvious next refactor. **Do not take it.** The roadmap's step A argues at
length that one lock, acquired exactly once per payload in `on_payload` and
passed down as `&mut Registry`, is what closes the session-ordering hazard
`docs/reference/world-state.md` §5 warned about: the client gives no ordering guarantee
between a session change and the answer to the call that caused it, so
computing them against different worlds is a defect the topology now prevents
**by construction**. A finer-grained lock reopens it.

So `Registry`'s breadth is a consequence of a decision, not an accident. The
thing to watch is different: every subsystem reaches the world through one
door, and W2's combat modules will crowd it. If that becomes painful, the
move is to narrow what passes *through* the door — more `&mut` sub-borrows
like `roster_mut()` and `locks_mut()` — not to split the lock.

`ServiceError` in a domain module is mild and arguably correct: a domain
operation that can be refused has to say so in a vocabulary the dispatcher
can turn into a `UserError`.

## 5. The packet contract

A packet is the unit a lane executes cold. It is a lane brief with the
scheduling fields filled in. **If a packet cannot be written in this form,
the work is not understood well enough to hand off** — lane rule 6.

```
P<wave>.<n>  <one-line goal>
  truth    the ONE document to implement from (§3), plus section numbers
  owns     exact file paths this packet alone may write
  reads    docs to read first, in order
  verify   the assertion that fails before and passes after
  size     S < 100 lines | M < 400 | L < 1000 | XL  [est]
  after    packets that must land first
  not      files and questions explicitly out of scope
```

Three house rules every brief must restate, because a lane that has not been
told them will break them: **no dependencies ever** (`server/deps.allow` is
empty and stays empty); **tests live in the module you own**, never in
`service.rs`'s shared block; **ignore compile errors outside your own files**
(lane rule 4).

And one filter, which is this plan's own contribution. The baseline counts 63
stubs. A stub is work only if it fails this test:

> **Is the empty answer a lie?** If the client would behave correctly given
> that there genuinely is no such thing, the stub is the truth and closing it
> is not work. If the client draws a window the player cannot use, or caches
> an answer that contradicts what we will later say, it is a defect.

`fleetProxy.GetAvailableFleetAds → {}` passes (there are no ads).
`dogmaIM.SetModuleOnline → raise` fails (the player has guns they cannot
turn on). Apply the filter before planning any stub.

## 6. The waves

Ordered by what unblocks the most, not by cost. Each wave ends with the
iteration step of §7.

---

### W0 — Raise the parallelism ceiling (coordinator only, strictly serial) — **COMPLETE 2026-10-24**

Nothing else in this plan parallelises until W0 lands. One commit per packet.

**All five landed.** What W0 actually bought, measured:

| | before | after |
|---|---|---|
| `service.rs` | 8,942 lines | **4,378** |
| upward `crate::service::` refs | 44 | **16** |
| `destiny.rs` → service | 22 | **1** |
| tests | 1,062 | 1,062 |
| can a lane gate mid-round? | no | yes (`--only`, `--without`) |
| can a red gate reach `origin`? | yes, twice | no (`land.sh`) |
| is the pinned build guarded? | no | yes (232 files) |

Two of the five briefs were wrong in ways the work exposed, and both
corrections are recorded in place rather than edited away: P0.1's plan to
distribute the tests could not survive the fact that they test `Registry`
rather than services, and P0.5 turned out to be the cheaper and deeper of the
two.

**P0.1 — Split `service.rs`'s test module.** `L` — **LANDED 2026-10-24, and
the brief was half wrong**
- truth: `lanes/README.md` rule 2; the churn table of §4
- owns: `service.rs`, `service/tests.rs`
- done: `service.rs` **8,942 → 4,378 lines**; 1,062 tests unchanged; the module
  moved whole to `service/tests.rs`
- **the brief said "each moved test lives in the module that owns its
  service", and most of them cannot.** They call `Registry` methods directly
  — `r.owned_item_rows(Some(ALICE))`, `r.hangar_item_rows(..)`,
  `advance_character` — rather than dispatching a service. They are tests of
  the *world*, not of a service surface, so filing them under
  `service/inventory.rs` would put them beside code they do not exercise.
- **the finding, which is the useful part:** the test module is big because
  `Registry` is big. The tests follow the god object. And §4a argues that
  `Registry`'s breadth is a deliberate decision — one lock, acquired once per
  payload, closing the session-ordering hazard by construction — so the deeper
  split is blocked by something we chose and should keep.
- what the move still bought: the hottest file in the crate is halved, and a
  lane editing dispatch logic no longer collides with a lane adding a test.
  That was the parallelism goal; the distribution was the means, and the means
  did not survive contact.
- rule for new tests, now in `service/tests.rs`'s own header: if it dispatches
  a service it belongs in that service's module; only `Registry`, the clock,
  the snapshot and the slash console belong here.

**P0.2 — A gate that runs against an explicit file list.** `M` — **LANDED
2026-10-24**
- truth: `port-tracker.md` §29.6
- owns: `tools/gate.sh`
- three forms, all of which skip the clean-tree check: `--dirty` gates what is
  on disk, `--only CRATE` scopes clippy and the tests to one package, and
  `--without C1,C2` copies the tree, removes those crates and gates the copy —
  which is §29.6's hand-rolled workaround, automated
- verified by reproducing the complaint: a crate with a `Cargo.toml` and no
  `src/` makes cargo fail with `failed to load manifest for workspace member`
  for *every* package, and `--without` gates green around it
- **Limitation, measured on its first real use 2026-10-24 and not anticipated
  in the brief:** `--only` scopes to a **crate**, and the lanes that matter
  here all live in `eve-server`. The first mid-round attempt failed on
  `service/reprocess.rs` — another lane's half-written file — which is lane
  rule 4 working exactly as documented and `--only` not helping at all.
  §29.6 asked for "a gate that can run against an explicit **file list**";
  crate scoping is a coarser approximation of that and it is only useful when
  the lanes are in different crates. Within one crate, rule 8 still stands:
  gate after every lane has reported. A real file-list gate would need
  per-file type checking, which cargo does not offer — so the honest next step
  is not a better flag, it is accepting that intra-crate lanes serialise at
  the gate even when they parallelise at the keyboard.
- **and a partial run cannot authorise a push, by construction**: only the
  full gate prints `== gate passed`, the exact line `tools/land.sh` greps for,
  while every scoped form prints `== partial gate passed`. There is no flag
  combination that makes `land.sh` push on an incomplete check. That is §54.6's
  lesson applied before the fact instead of after it.

**P0.3 — `tools/land.sh`: gate then push, exit code checked.** `S` — **LANDED 2026-10-24**
- truth: `port-tracker.md` §48.6 and §54.6
- owns: new `tools/land.sh`
- verify: with a deliberately failing test, the script must not push
- why: red has reached `origin/master` twice, both times because the gate's
  verdict was read through a pipe or a `;`. The rule is written down and was
  obeyed, and the defect recurred in a new disguise. A script is the fix that
  does not depend on remembering.

**P0.5 — Extract the domain model out of the dispatcher.** `M` — **LANDED 2026-10-24**: upward references fell from 44 to 16, `destiny.rs` 22 → 1, `targeting.rs` 13 → 7. What is left is honest (`Registry` 6, `ServiceError` 5) and is a separate question — see §4a.
- truth: the coupling measurement below
- owns: new `model.rs`; `service.rs`; every module that references the moved
  types
- verify: `grep -c 'crate::service::' destiny.rs` falls from 22 toward 0, and
  `targeting.rs` from 13; no behaviour change, no test rewritten
- measured 2026-10-24, and this is the finding rather than the file size:

  ```
  upward references into crate::service::
    destiny.rs    22          Character       14
    targeting.rs  13          SandboxConfig   13
    fleet.rs       3          ServiceError     5
    item.rs        2          Registry         2
    docking.rs     2
  ```

  Twenty-seven of the forty-four are `Character` and `SandboxConfig` — **world
  model types declared in the dispatcher's file** (`service.rs:313` and
  `:227`). So `service.rs` is not a dispatcher that grew; it is a dispatcher
  *plus the domain model*, and that is why fifteen modules point up at it.
  Moving three type declarations cuts most of it.

  Two related smells, same cause: `Registry` is a god object (15 fields, 54
  public methods, and the process's single `Arc<RwLock<_>>`), and the slash
  console is ~380 lines inside `service.rs` while `service/gm.rs` exists at
  167 lines and delegates back up.

  **What is already right, and should not be disturbed:** the `service/` split
  holds — sixteen submodules with a maximum of **one** cross-reference between
  any two. That seam is real. This packet is about the layer below it, not
  about splitting further.

**P0.4 — Protect the pinned client build.** `S` — **LANDED 2026-10-24**
- truth: §2a of this file
- owns: new `tools/pin-client.sh`, `docs/guides/jail.md`
- verify: a checksum manifest of `tq-offline` that fails if Steam replaces it
- why: every `[V]` mark in the repo is measured against 3619879 and nothing
  guards it

---

### W1 — Close the known defects (space cluster is ONE lane)

Everything here has a measured answer already. **P1.1 is first and is not in
the space cluster; P1.2–P1.6 are one serial lane in file-order.**

**P1.1 — Move the outbox out from behind the socket mutex.** `M` — **LANDED 2026-10-24**, `port-tracker.md` §57. ~90 lines in `push.rs` and `dispatch.rs` only; `conn.rs` and `server.rs` needed nothing. Measured against the old form with a standalone probe: old blocked for the full 500 ms while the wire was held, new got through.
- truth: `port-tracker.md` §26.2, **and its §26.3 correction**
- owns: `push.rs`, `conn.rs`, `dispatch.rs`, `server.rs` (all hot — coordinator)
- verify: one peer with a closed TCP receive window must not stall another
  peer's session change
- size note: §26.2 estimated 80–120 lines; §26.3 measured it at **two lines in
  two functions** and found a second inversion at `dispatch.rs:798`
- why first in W1: stated consequence is "one peer with a closed TCP receive
  window freezes the world for everybody for up to 30 s". This is the highest
  stakes item in the whole plan and the cheapest of the top five.

**P1.2 — Flip `Action::WarpTo` from `Mode::Stop` to `Mode::Warp`.** `M` — **LANDED 2026-10-24** on owner sign-off, `port-tracker.md` §59. Activation is predicted from `align_seconds`, not detected from Carbon's 0.75 gate, because this server integrates lazily — §59.1 has the argument.
- truth: `port-tracker.md` §53.3
- owns: `service/space.rs`, `space_state.rs`, `destiny.rs`, both test suites
- verify: a watcher reads a warping ship as warping, not parked; §44.6's
  arrival reconciliation still lands; `a_warp_schedules_its_own_arrival` is
  rewritten to assert the curve rather than the teleport
- after: owner sign-off — the tracker says this one "is genuinely better done
  with the owner watching", and a client run should back it
- why: the server currently *teleports* in its own records. This is the
  standing `[A]` through §30.3 → §36.8 → §37.3 → §38.2 → §53.3.

**P1.3 — Implement the y-only STOP damping.** `S` — **LANDED 2026-10-24**, `port-tracker.md` §58
- truth: `port-tracker.md` §55.1 (condition, measured off the client) and
  §55.2 (the step — **1 s, not 100 ms**)
- owns: `space_state.rs`
- verify: a stopping ship's y velocity decays with the extra 7.13 s constant;
  x and z unchanged
- stakes: 232 m (frigate) to 888 m (cruiser) of y divergence at 10 s
- not: do not use §51.5's table — it is an order of magnitude out and §55.2
  replaces it

**P1.4 — Wire in `orbit::orbital_normal`.** `M` — **LANDED 2026-10-24**, `lanes/orbit.md`
- truth: `port-tracker.md` §56.3
- owns: `orbit.rs`, `space_state.rs`, `service.rs` (tether site)
- verify: an orbiting ship reconciles onto the derived plane, not merely onto
  the range sphere
- not: **the swept angle stays `[A]`** — it needs an orbit start time nothing
  records. And the projection onto the plane is **our construction, not
  Carbon's**, with maximum error right after an orbit order; say so in the
  doc comment or do not ship it.

**P1.5 — The conditional `total_distance` encoder write.** `S`
- truth: `port-tracker.md` §46
- owns: `destiny.rs`
- verify: `-1.0` is written only while the stamp is negative

**P1.6 — The missing upward clamp during warp acceleration.** `S`
- truth: `port-tracker.md` §48.3
- owns: `destiny.rs`
- verify: a ship entering warp above the curve decelerates to meet it

**P1.7 — Send the fleet session keys.** `S` — **ALREADY DONE** in `95e2e41`,
2026-10-21. A phantom packet: `port-tracker.md` §15.6's bullet was never
struck, and this plan cited the section as truth. See §60.1 — and the rule it
produced, which belongs in §2 of this file:

> **A port-tracker section is evidence of what was once true.** That file is
> append-only by design, so citing a section as a packet's truth cites a
> snapshot. Verify the claim against the **code** before writing the brief.
> One grep would have saved a lane round here.

**P1.10 — The fleet member who never leaves.** `M` — **LANDED 2026-10-24**.
Called from `Registry::session_closed`, inside its `is_online` guard (a second
client keeps you in), with the pushes on the clock queue — `fleet_apply`'s
`self.push` waits for a payload turn a disconnect never has, the same trap
that function's three earlier fixes record. **Left:** the fleet's session
changes (a new boss's `fleetrole`) still drain only on somebody's turn; the
world ticker does not drain `Fleets::take_session_changes`. Found by the
P1.7 lane while reading code the packet did not need
- truth: `port-tracker.md` §60.3
- owns: `conn.rs`, `server.rs` (both hot — coordinator), `fleet.rs`
- `Fleets::character_disconnected` is written, documented and covered by four
  unit tests, and **never called**. A pilot who closes their client stays in
  the fleet forever.
- verify: a disconnect removes the member, and the remaining members are told
- note: a tested function with no caller is invisible to every tool we have —
  coverage passes, tests pass, behaviour absent — and `service/fleet.rs:801`
  carries a comment asserting the behaviour, so a reader checking this exact
  question gets the right answer from the wrong source

**P1.7-superseded — (slot kept so the numbering in commit messages still
resolves).** `S`
- truth: `port-tracker.md` §15.6
- owns: `dispatch.rs` (the existing `with_session_change` drain point)
- size: two lines; non-blocking, cosmetic

**P1.8 — Charge the reprocessing tax, and add the reprocessing skill.** `S` —
**LANDED 2026-10-24**, `port-tracker.md` §61. Half the brief was a phantom (the
eighteen skills shipped in `07a58d9`) and the defect that mattered was not in
it: `GetQuotes` used the character's skills and `Reprocess` did not, so a
trained pilot was **shown** 2,783 Tritanium and **paid** 2,000, silently, on
every refine. Tax measured at 200.00 ISK bare / 278.30 ISK at level 5.
- truth: `port-tracker.md` §15.6, §18.6; `lanes/reprocess.md`
- owns: `service/reprocess.rs`, `reprocess.rs`, `skill.rs`
- verify: the quoted tax appears as a journal row; yield is no longer the bare
  station 0.5
- note: the lane already supplies 18 skill rows from `typedogma` attribute 275

**P1.9 — `tools/dogma.py`'s flag-placement bug.** `S` — **LANDED 2026-10-24**,
`port-tracker.md` §62. `parents=[common]` on both parser levels, so every
subparser's defaults overwrote the flags that had just been parsed — bpo-9351.
§19.8 was understated (`-n` and `--loc` were clobbered too: 4 rows for a
30-row request) and overstated (203/203 staticdata rows are identical across
the builds, so the hazard was latent for *this* tool). The build check now
reads its expected number from P0.4's `client-pin.sha256`.

**P1.11 — the same argparse bug in `tools/fsd_dump.py`.** `S` — **LANDED
2026-10-24**, `SUPPRESS` defaults on the subparser side as in `dogma.py`;
both flag placements checked. Not ported: `dogma.py`'s build refusal —
`fsd_dump.py` still falls back to `fsd.Client`'s default install
- truth: `port-tracker.md` §62.5
- owns: `tools/fsd_dump.py`
- `parents=[common]` at `:447` and `:461`, verified. It is a copy of a fix
  that now exists in `tools/dogma.py`; no other `tools/*.py` uses `parents=`.
- truth: `port-tracker.md` §19.8
- owns: `tools/dogma.py`
- why: today it "fails loudly only by luck" — a second client at the default
  path makes it return a plausible answer from the wrong build

---

### W2 — Make combat exist

**This is the largest capability gap between here and the stated goal**
(co-op and PvP with player-like bots), and nothing in it is started. Targeting
is done; the things a lock is *for* are not. Both packets have IMPL-SPEC docs
over 1,000 lines each.

**P2.0 — Board a ship, and fit a module into it.** `L` — **LANDED 2026-10-24** (`lanes/fitting.md`), first found
found by the P2.1 lane: **every packet in W2 is unreachable from the client
without this, and the plan never listed it.** Everyone is in a capsule;
`/createitem` lands a module in the hangar; the fitting window's drag gets no
answer and `item::Edit` has no "assemble". So modules, damage and death are
all tested only from the inside.
- truth: none yet — research step first, `docs/lanes/fitting.md`
- owns: TBD by the research step (likely `item.rs`, `service/inventory.rs`,
  a new `fitting.rs`, and the `shipid` session change in hot files)
- verify: a player boards a Rifter in the station, fits a 125mm autocannon
  from the hangar, undocks, and the HUD shows the module

**P2.1 — Module activation.** `L` — **LANDED 2026-10-24** (no fitting yet, so not reachable from the client; `lanes/modules.md`)
- truth: `re/modules.md` (1,074 lines, IMPL-SPEC)
- owns: new `modules.rs`; `service/inventory.rs` (the `dogmaIM` arms)
- verify: `SetModuleOnline` stops raising; a fitted gun cycles and the client
  draws the cycle
- after: P0.1 (it needs `service/inventory.rs` tests out of the shared block)
- not: drones, fighters, ewar, weapon grouping — `lanes/damage-death.md:78`
  names them as a separate lane

**P2.2 — Damage, death and wrecks.** `L` — split in two on 2026-10-24:
**P2.2a** (the pure model, `damage.rs`) **LANDED**; **P2.2b** (wiring: real
shield tau on the wire, cycles → shots, pushes, ball removal + wreck)
**LANDED**; **P2.2c** (the capsule, the dead ship destroyed in the store,
wrecks as real items with a despawn, capsule death → revive at home)
**LANDED 2026-10-24**. Left: loot, ammo, tracking, resistances
(`lanes/damage-death.md`)
- truth: `re/damage-death.md` (1,188 lines, IMPL-SPEC)
- owns: new `damage.rs`; `destiny.rs` (the damage state already on the wire)
- verify: a gun that cycles removes shield, then armour, then hull; death
  produces a capsule and a wreck
- after: P2.1
- unblocks: `fleetMgr.RegisterForDamageUpdates`, which is a stub today for the
  stated reason that "there is no damage"

---

### W3 — Make space worth visiting

Parallel with each other **except** where they touch the space cluster.

**P3.1 — Rocks in the asteroid belt.** `M` — **LANDED 2026-10-24** (`lanes/asteroids.md`; mining itself not wired)
- truth: `port-tracker.md` §35.6; `docs/evejs/review/features.md`
- owns: new `asteroid.rs`; `celestial.rs`
- verify: the client draws a bracket on a rock and offers a mining action
- **first, a research packet**: which slim-item fields must an asteroid carry
  before the client will draw a bracket? §33.3's lesson is to find that out
  *before* serving one — a gate's slim without `jumps` raises `IndexError`
  and hands the player a traceback
- why: §35.6 calls it "the next cheap visible thing in space"; all three data
  inputs are `[V]` and in hand

**P3.4 — Mining.** `M` — **LANDED 2026-10-24** (`lanes/mining.md`): a mining laser cycling on a
locked rock (P3.1) puts ore in the hold, and the rock shrinks and depletes.
Builds on P2.0 fitting and P2.1 cycles. Owns new `mining.rs`, `modules.rs`,
`asteroid.rs`, `item.rs`; `lanes/mining.md`.

**P3.2 — Give this sandbox's station a real position.** `L`
- truth: `port-tracker.md` §33.3
- owns: `service.rs`, `destiny.rs`, `docking.rs`, `celestial.rs`
- verify: the station is where the catalogue says, and `/tp` means what it says
- cost: the id is threaded through **37 places**; this is why it has waited
- unblocks: the star, and the real station, which §33.3 calls "the same
  underlying debt"

**P3.3 — Stargates and a second solar system.** `XL` — **LANDED 2026-10-24** (`lanes/stargates.md`): Tanoo ↔ Akpivem, on the clock
- truth: `re/jump-gates.md` (1,261 lines, IMPL-SPEC)
- owns: new `jump.rs`; `space_state.rs` (`CmdStargateJump`); `service.rs`
  (`SandboxConfig` holds one system today)
- verify: a ship jumps and arrives in a different system's park
- **partly blocked, and this is not obvious:** §41.14 measured
  `stargate.lock…GetRestrictedSystemsRequest` on the unserved gRPC gateway, so
  stargates are "not purely a macho-side job". Scope the macho half first and
  measure what the client does without the gateway answer.
- data: 477,082 celestials in 8,490 systems are already extracted

---

### W4 — Make NPCs and bots act

**P4.1 — NPC behaviour.** `XL` — **first NPC LANDED 2026-10-24** (`lanes/npc.md`): a server-side Angel Hijacker spawned with `/entity deploy`, aggro → lock → approach → orbit → fire, killable, wreck; an eve-bot test proves it acts with no payload from anyone
- truth: `re/npc-behaviors.md` (1,234 lines, IMPL-SPEC)
- owns: new `npc.rs`
- after: P2.1, P2.2 — an NPC with no weapons is a decoration
- why it is one piece of work and not two: the roadmap's end goal is
  player-like bots, and `re/npc-behaviors.md` is explicitly about a server
  entity indistinguishable from a player. The same engine serves both.

**P4.2 — Missions and dungeons in space.** `L`
- truth: `re/mission-model.md`, `re/dungeon-model.md`
- owns: `mission.rs`; new `dungeon.rs`
- after: P4.1
- data: 2,892 missions × 5,516 dungeons with room layouts, already decoded

---

### W5 — Docked breadth (parallel, one service module each)

Every packet here owns exactly one `service/*.rs` and is therefore safe to run
concurrently **after P0.1**. Apply §5's filter first: several of these stubs
are the truth.

| packet | owns | truth | size |
|---|---|---|---|
| P5.1 market mutators + `OnOwnOrdersChanged` — **LANDED 2026-10-24** (`lanes/market.md`) | `service/market.rs` | `lanes/market.md` | M |
| P5.2 item ops: stack, repackage, unload — **stack + repackage LANDED 2026-10-24** (`b735c90`); unload waits on P2.1 | `service/inventory.rs`, `item.rs` | `item-spawn.md` | M |
| P5.3 PI mutators (`UserUpdateNetwork` et al.) | `service/planet.rs` | `lanes/planet.md` | L |
| P5.4 industry round 2: science, invention, reactions | `service/industry.rs` | `lanes/industry.md` | L |
| P5.5 corp office divisions → corp jobs | `roles.rs`, `service/corp.rs` | §18.6 | M |
| P5.6 bookmarks — **LANDED 2026-10-24** (`lanes/bookmarks.md`) | `service/world.rs` | — | M |
| P5.7 standings, and the `setstanding` slash arm — **LANDED 2026-10-24** (`lanes/standings.md`) | `service/character.rs` | — | M |
| P5.12 cargo moves and `shipState[1]` — **LANDED 2026-10-24** (`lanes/inventory.md`): hangar ↔ cargo with the client's own capacity re-prompt; loaded charges in the fitting window after a board or relog | `service/inventory.rs`, `fitting.rs`, `modules.rs` | — | M |
| P5.11 player contacts and contact standings — **LANDED 2026-10-24** (`lanes/contacts.md`), found by P5.7: no arm for `AddContact` & co., and they colour the overview between players | `service/character.rs` | `lanes/standings.md` | M |
| P5.8 contracts — **item exchange LANDED 2026-10-25** (`lanes/contracts.md`) | `service/wallet.rs` | — | L |
| P5.9 clones and jump clones — **LANDED 2026-10-24** (`lanes/clones.md`; `revive_in_clone` waits on P2.2c) | `service/character.rs` | `docs/reference/skill-system.md` | M |
| P5.10 scanning and the probe window | `service/space.rs` | `lanes/sensor-suite.md` | L |

P5.7 and P5.9 both own `service/character.rs` — run them in series or split
the file first.

---

### W6 — Infrastructure, and the one owner decision

**P6.1 — The account layer.** `L`, **blocked on a decision, not on hands**
- truth: `lanes/account-spec.md` §§1–3
- the question, and it is ten minutes of reading for the owner: **is one SSO
  token one account, or is the token a character selector?**
- today `character_selection_data` hands every character in the world to every
  connection with no filter, and `SessionEntry.user_id` is written and read
  nowhere — which makes the account layer look present when it is not
- §13.6 calls this "the one invariant here with a recorded history of being
  silently broken while its own test passed"

**P6.2 — The gRPC / protobuf public gateway.** `XL`
- truth: `port-tracker.md` §10.4, §41.14
- gates: the entire Equinox sov UI, part of W3's stargates, and live chat
- §10.4 names schema drift as "the single largest silent-failure risk" —
  protobuf decodes a changed message without complaining
- `EVE_PUBLIC_GATEWAY_ADDRESS` is already the patch-free way in

**P6.3 — Newtypes: `AccountId`, `CharacterId`, `ClientId`.** `M`
- truth: §13.6 — "right answer, wrong round"
- after: P6.1, because doing it first means a 32-site mechanical change on top
  of a 32-site mechanical change

---

### W7 — Documentation hygiene (anytime, owns only `docs/`)

**P7.1 — Banner the 16 demoted documents.** `S` — **LANDED 2026-10-24**
- truth: §2 of this file
- owns: the 16 files listed
- verify: each carries a dated banner naming what supersedes it
- why: a demotion list in one file is weaker than a banner in sixteen, and the
  recorded cost of a stale pointer is a wasted lane round

### W8 — The wide wave (2026-10-25): ten lanes at once

**Why the earlier waves ran two to four lanes**: they were planned around one
space-and-combat cluster (`destiny.rs`, `combat.rs`, `modules.rs`,
`service.rs`) that nearly every packet touched, and the coordinator held each
packet until the last one reported. Three things changed that: P0.1 split
`service.rs`; the coordinator registers every new module in `lib.rs` *before*
a wave, so no lane edits it; and `tools/land.sh --head` lands each report
from a clean worktree while the others keep writing. What is left is mostly
**one subsystem per file**, so the ceiling is the file graph, not the plan.

**The rule this wave runs on**: every lane owns a disjoint set of files;
anything in a shared file (`service.rs`, `push.rs`, `dispatch.rs`,
`store.rs`, `notify.rs`) comes back as a diff the coordinator applies at
landing. Each lane lands as soon as it reports. Lane rule 9: one scratch
copy, deleted before the report.

| lane | packet | owns | kind |
|---|---|---|---|
| A | P8b bots: brain, driver, `tools/bots.sh` | `eve-bot/`, `tools/bots.sh`, `tools/up.sh` | code (running) |
| B | chat: local, private, fleet — XMPP | `docs/lanes/chat.md` | research → spec |
| C | P5.8 contracts | `contracts.rs`, `service/wallet.rs` | code |
| D | P5.5+ player corporations | `service/corp.rs`, `roles.rs` | code |
| E | skill training queue | `skill.rs`, `service/character.rs` | code |
| F | crimewatch: suspect / criminal / weapons timers | `crimewatch.rs`, `combat.rs` | code |
| G | P5.10 probe scanning | `scan.rs`, `service/space.rs` | code |
| H | P4.2 agents and missions in space | `mission.rs`, `dungeon.rs` | code |
| I | drones | `drones.rs`, `modules.rs`, `destiny.rs`, `item.rs` | code |
| J | P5.3 planetary interaction | `planet.rs`, `service/planet.rs` | code |
| K | P5.4 industry round 2 | `industry.rs`, `service/industry.rs` | code |

**Collisions accepted on purpose**: H needs agent arms in
`service/character.rs` (E's) and G needs probe balls in `destiny.rs` (I's) —
both come back as diffs. If two diffs touch the same hunk of a shared file,
the second lander rebases it; that costs minutes, a held lane costs hours.

**Disk**: ten scratch copies at 1-3 GB each fit in the temp volume's ~24 GB
only if rule 9 holds. The coordinator checks `df /tmp` at every landing.

---

### W9 — Breadth and the loose ends W8 left (2026-10-25)

W8 landed nine of its ten lanes in about three hours (chat is still
building). What it exposed: the server knows a handful of hand-copied
hulls, ~20 modules and 44 skills, so most of what a player might fly or
fit does not exist. W9 makes the client's own tables the source.

| lane | packet | owns | kind |
|---|---|---|---|
| B2 | chat (XMPP), from `lanes/chat.md` | `eve-xmpp/`, `chat.rs`, `service/base.rs`, `tools/run-client.sh`, `tools/jail-setup.sh` | code (running) |
| L1 | combat loose ends: drones take damage and die, NPCs target drones, a mission completes when its last NPC dies | `combat.rs`, `npc.rs`, `drones.rs`, `mission.rs`, `dungeon.rs` | code |
| L2 | **data breadth**: every ship, module and charge from the client's tables, loaded at startup | `typedata.rs`, `item.rs`, `modules.rs`, `fitting.rs`, `destiny.rs`, new `tools/typedata.py` | code |
| L3 | skills breadth: every skill, skillbook injection, character attributes | `skill.rs`, `service/character.rs`, `service/inventory.rs` | code |
| L4 | people search: the lookup service every "find a pilot/corp" box uses | `service/social.rs` | code |
| L5 | PI customs offices: import/export | `planet.rs`, `service/planet.rs` | code |
| L6 | courier contracts | `contracts.rs`, `service/wallet.rs` | code |
| L7 | crimewatch: the log-off timer holds a ship in space | `crimewatch.rs`, `service/space.rs`, `docking.rs`, `jump.rs` | code |

---

## 7. The iteration loop

The plan is iterative in a specific sense: **the measurements go stale, so
each wave ends by re-taking them.** This is four commands and it is what keeps
this document from becoming the seventeenth entry in §2.

1. `tools/land.sh` (after P0.3) — gate, then push only on green
2. `tools/client-ci.sh run <scenario>` for whatever the wave touched. The
   verdict lands in the Wine drive at
   `…/compatdata/8500/pfx/drive_c/eve-logs/client-ci/<scenario>.json`, **not**
   under `out/client-ci/`, and a 30-second run is a success, not a crash
3. re-run §4's churn measurement: `git log -120 --name-only --pretty=format:
   -- server/ | sort | uniq -c | sort -rn | head`. If a new file has entered
   the top three, it is the next shared-file removal
4. append what moved to `port-tracker.md`, and park a lane note per rule 5

**Batch the human.** `docs/guides/human-checks.md` §8 is the queue; nothing in it blocks
anything. Add to it rather than interrupting a run.

## 8. Explicitly not planned

Stating these keeps them from being rediscovered as gaps.

- **Avatars and the paperdoll.** `character.rs:1027` — a paperdoll DNA is a
  real structure we do not model, and nothing needs it.
- **The acceleration turn cap.** Dead code in CCP's own source (§51.3a);
  our server agrees with the client by both doing nothing.
- **`GotoThrust`'s quartic homing taper.** `[A]`, accepted (§54.2): defined in
  terms of a tick, bounded by one tick of travel, and erased by §44.6's
  arrival reconciliation.
- **The missile 800 ms window.** §56.4 — at a 1,000 ms step it is a
  first-tick special case, and missiles are not implemented.
- **Every item in §30.4, §38.2, §49's blockers, §47.5, §41's
  `InitClientGlobalConfig`.** All retired in-document; the port tracker's own
  "NOT live" list is authoritative.
- **Downloading the SDE.** Withdrawn by §45.2 — we extract from the client.
- **EvEJS as a source of code.** AGPL, and a different build (3396210). Facts
  only, `[V2]` only.

## 9. If you read only one thing

**P0.1, then P1.1.** The first raises the ceiling on everything else; the
second is two lines against a bug that can freeze the world for every player
for thirty seconds. Neither is a feature, and both are worth more than the
features waiting behind them.
